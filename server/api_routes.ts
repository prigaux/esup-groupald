import express from 'express';
import * as _ from 'lodash'

import * as cas_auth from './cas_auth'
import * as api_get from './api_get'
import * as api_post from './api_post'
import * as cache from './cache'
import conf from './conf';
import { throw_ } from './helpers';
import { hLdapConfig, hMright, hRemoteConfig, hRight, MonoAttrs, MyMods, RemoteQuery, SimpleMod, toDn } from './my_types';
import { query_params, q, orig_url, logged_user, query_opt_params, handleJsonP, handleVoidP, handleJson } from './express_helpers';
import { is_stem } from './stem_helpers';

const api = express.Router();

// JSON body-parser will return {} on empty body
type AllowEmptyBody<T> = T | {}

api.post("/login", cas_auth.handle_single_logout)

// eslint-disable-next-line @typescript-eslint/no-misused-promises
api.get("/login", async (req, res) => {
    try {
        const { target, ticket } = query_params(req, { target: q.string, ticket: q.string })
        if (!target.startsWith('/') || target.startsWith("//")) {
            throw `invalid target ${target}, it must be a path-absolute url`
        }
        const service = orig_url(req).match(/(.*)ticket=/)?.[1] ?? throw_("weird login url");
        const user = await cas_auth.validate_ticket(conf.cas.prefix_url, service, ticket)
        // @ts-expect-error (req.session is not typed)
        req.session.user = user;
        res.redirect(target)
    } catch (e) {
        console.error(e)
        res.status(500);
        res.send("internal error")
    }
})

api.post("/create", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { strict } = query_opt_params(req, { strict: q.boolean })
    await api_post.create(logged_user(req), id, req.body as MonoAttrs, strict || false)
}))

api.post("/modify_attrs", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    await api_post.modify_sgroup_attrs(logged_user(req), id, req.body as MonoAttrs)
}))

api.post("/delete", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    await api_post.delete_(logged_user(req), id)
}))

api.post("/modify_member_or_right", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { msg, strict } = query_opt_params(req, { msg: q.string, strict: q.boolean })
    await api_post.modify_member_or_right(logged_user(req), id, req.body as SimpleMod, msg, strict || false)
}))
api.post("/modify_members_or_rights", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { msg, strict } = query_opt_params(req, { msg: q.string, strict: q.boolean })
    await api_post.modify_members_or_rights(logged_user(req), id, req.body as MyMods, msg, strict || false)
}))

// NB: also allowing GET to ease calls in browser
api.all("/sync", handleJson(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { mright } = query_opt_params(req, { mright: q.mright })
    return await api_post.sync(logged_user(req), id, 
        mright ? [mright] : is_stem(id) ? hRight.list() : hMright.list(),
    )
}))

api.post("/modify_remote_query", handleVoidP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { msg } = query_opt_params(req, { msg: q.string })
    await api_post.modify_remote_query(logged_user(req), id, req.body as AllowEmptyBody<RemoteQuery>, msg)
}))

api.post("/test_remote_query", handleJsonP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    return await api_get.test_remote_query(logged_user(req), id, req.body as RemoteQuery)
}))

api.get("/get", handleJsonP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    return await api_get.get_sgroup(logged_user(req), id)
}))

api.get("/exists", handleJsonP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    return await api_get.sgroup_exists(logged_user(req), id)
}))

api.get("/direct_rights", handleJsonP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    return await api_get.get_sgroup_direct_rights(logged_user(req), id)
}))

api.get("/flattened_mright", handleJsonP(async (req) => {
    const { id, mright } = query_params(req, { id: q.string, mright: q.mright })
    const { search_token, sizelimit } = query_opt_params(req, { search_token: q.string, sizelimit: q.int })
    return await api_get.get_sgroup_flattened_mright(logged_user(req), id, mright, search_token, sizelimit)
}))

api.get("/logs", handleJsonP(async (req) => {
    const { id } = query_params(req, { id: q.string })
    const { bytes, sync } = query_opt_params(req, { bytes: q.int, sync: q.boolean })
    return await api_get.get_sgroup_logs(logged_user(req), id, bytes || 9999999, sync ? { sync } : undefined)
}))

api.get("/search_sgroups", handleJsonP(async (req) => {
    const { right, search_token, sizelimit } = query_params(req, { right: q.right, search_token: q.string, sizelimit: q.int })
    return await api_get.search_sgroups(logged_user(req), right, search_token, sizelimit)
}))

/**
 * NB: /raw/xxx APIs are not used by Vue.js UI
 *  */
api.get("/raw/search_sgroups_using_a_subject", handleJsonP(async (req) => {
    const { subject_dn, mright } = query_params(req, { subject_dn: q.string, mright: q.mright })
    return await api_get.search_raw_sgroups_using_a_subject(logged_user(req), toDn(subject_dn), mright)
}))

api.get("/mygroups", handleJsonP(async (req) => {
    return await api_get.mygroups(logged_user(req))
}))

api.get("/clear_cache", () => {
    cache.clear_all();
})

api.get("/search_subjects", handleJsonP(async (req) => {
    const { search_token, sizelimit } = query_params(req, { search_token: q.string, sizelimit: q.int })
    const { source_dn, group_to_avoid } = query_opt_params(req, { source_dn: q.string, group_to_avoid: q.string })
    return await api_get.search_subjects(logged_user(req), search_token, sizelimit, source_dn?.oMap(toDn), group_to_avoid)
}))
api.get("/get_subject", handleJsonP(async (req) => {
    const { subject_id } = query_params(req, { subject_id: q.string })
    const { source_dn } = query_opt_params(req, { source_dn: q.string })
    return await api_get.subject_id_to_dn(logged_user(req), subject_id, source_dn?.oMap(toDn))
}))
api.post("/subject_ids_to_dns", handleJsonP(async (req) => {
    const { source_dn } = query_opt_params(req, { source_dn: q.string })
    return await api_get.subject_ids_to_dns(logged_user(req), req.body as string[], source_dn?.oMap(toDn))
}))

api.get("/config/public", handleJson(() => ({ "cas_prefix_url": conf.cas.prefix_url })))
api.get("/config/ldap", handleJson(() => hLdapConfig.to_js_ui(conf.ldap)))
api.get("/config/remotes", handleJson(() => ({ 
    remotes: _.mapValues(conf.remotes, hRemoteConfig.export),
    additional_periodicities: conf.additional_periodicities
})))

export default api
