import * as ldapjs from 'ldapjs'
import _ from "lodash"

import * as ldp from "./ldap_read_search"
import * as ldpSgroup from './ldap_sgroup_read_search_modify'
import * as ldpSubject from './ldap_subject'
import * as api_log from './api_log'
import conf from "./conf"
import ldap_filter from "./ldap_filter"
import { dn_to_sgroup_id, dn_to_url, people_id_to_dn, sgroup_id_to_dn } from "./dn"
import { mono_attrs, mono_attrs_, multi_attrs, sgroup_filter, to_allowed_flattened_attrs, to_flattened_attr, user_has_direct_right_on_group_filter } from "./ldap_helpers"
import { Dn, hLdapConfig, hMright, hMyMap, hRight, toRqSql, LoggedUser, LoggedUserDn, MonoAttrs, Mright, MultiAttrs, MyMap, MySet, Option, RemoteQuery, Right, SgroupAndMoreOut, SgroupOutAndRight, SgroupOutMore, SgroupsWithAttrs, Subjects, SubjectsAndCount, toDn, isRqSql, TestRemoteQuery, SubjectSourceConfig, FlavorDn, SubjectsOrNull } from "./my_types"
import { is_grandchild, is_stem, parent_stems, validate_sgroup_id } from "./stem_helpers"
import { check_right_on_self_or_any_parents, user_has_right_on_sgroup_filter } from "./ldap_check_rights"
import { hSubjectSourceConfig } from "./ldap_subject"
import { guess_subject_source, parse_sql_url, sql_query } from './remote_query'
import { ldap_query, parse_ldap_url } from './remote_ldap_query'
import { internal_error, mapAsync, throw_ } from './helpers'

const user_dn = (logged_user: LoggedUser): LoggedUserDn => (
    'TrustedAdmin' in logged_user ?
        { TrustedAdmin: true } :
        { User: people_id_to_dn(logged_user.User) }
)

function user_highest_right(sgroup_attrs: MultiAttrs, user_dn: Dn): Option<Right> {
    for (const right of hRight.to_allowed_rights('reader')) {
        const dns = sgroup_attrs[to_flattened_attr(right)]
        if (dns?.includes(user_dn)) {
            return right
        }
    }
    return undefined
}

/*
export function to_rel_ou(parent_attrs: MonoAttrs, attrs: MonoAttrs): MonoAttrs {
    // if inside stem "Applications", transform "Applications:Filex" into "Filex" 
    // TODO keep it only if "grouper" migration flag activated?
    if const ((parent_ou), (child_ou)) = (parent_attrs.get("ou"), attrs.get_mut("ou")) {
        if const (child_inner_ou) = child_ou.strip_prefix(parent_ou) {
            *child_ou = child_inner_ou.trim_start_matches(":");
        }
    }
    attrs
}
*/

/** Get the stem direct children */
export async function get_children(id: string): Promise<SgroupsWithAttrs> {
    console.log("  get_children(%s)", id);
    const wanted_attrs = hMyMap.keys(conf.ldap.sgroup_attrs)
    const filter_ = ldap_filter.sgroup_mostly_direct_children(id);
    const filter = ldap_filter.and2_if_some(filter_, conf.ldap.sgroup_filter);
    const children = hMyMap.fromOptionPairs((await ldpSgroup.search_sgroups(filter, wanted_attrs, undefined)).map(e => {
        const child_id = dn_to_sgroup_id(e.dn)
        // ignore grandchildren
        if (!child_id || is_grandchild(id, child_id)) { return undefined }
        const attrs = simplify_hierachical_ou(mono_attrs(e));
        return [child_id, attrs]
    }))
    return children
}

/** NB: it computes direct right, without taking into account right inheritance (inheritance is handled in "get_parents()") */
async function get_parents_raw(filter: string, user_dn: LoggedUserDn, sizeLimit: Option<number>): Promise<MyMap<string, SgroupOutAndRight>> {
    const display_attrs = hMyMap.keys(conf.ldap.sgroup_attrs)
    const wanted_attrs = [ ...display_attrs, ...to_allowed_flattened_attrs('reader') ]
    const groups = hMyMap.fromOptionPairs((await ldpSgroup.search_sgroups(filter, wanted_attrs, sizeLimit)).map(e => {
        const right = 'TrustedAdmin' in user_dn ? 'admin' : user_highest_right(multi_attrs(e), user_dn.User)
        return dn_to_sgroup_id(e.dn)?.oMap(id => {
            const attrs = to_sgroup_attrs(id, e);
            return [ id, { attrs, right, sgroup_id: id } ]
        })
    }))
    return (groups)
}
async function get_parents(id: string, user_dn: LoggedUserDn): Promise<SgroupOutAndRight[]> {
    const parents_id = parent_stems(id);
    const filter = ldap_filter.or(parents_id.map(sgroup_filter))
    const parents = await get_parents_raw(filter, user_dn, undefined)

    // convert to Array using the order of parents_id + compute right (inheriting from parent)
    parents_id.reverse();

    const ordered_parents = _.compact(parents_id.map(id => parents[id]))

    // add inherited rights
    let best: Option<Right> = undefined;
    for (const parent of ordered_parents) {
        parent.right = best = hRight.max(best, parent.right)
    }
    return ordered_parents
}

async function get_right_and_parents(logged_user: LoggedUser, id: string, self_attrs: MultiAttrs): Promise<[Right, SgroupOutAndRight[]]> {
    const user_dn_ = user_dn(logged_user)

    const self_right = 'TrustedAdmin' in user_dn_ ? 'admin' : user_highest_right(self_attrs, user_dn_.User)
    //console.log('  self_right', self_right)

    const parents = await get_parents(id, user_dn_)

    console.log('  best_right_on_self_or_any_parents("%s") with user %s', id, logged_user);
    let best = self_right;
    for (const parent of parents) {
        best = hRight.max(parent.right, best)
    }
    console.log('  best_right_on_self_or_any_parents("%s") with user %s => %s', id, logged_user, best);
    if (!best) { throw `no right to read sgroup "${id}"` }
    return [best, parents]
}

/** Get group/stem information for Vue.js UI */
export async function get_sgroup(logged_user: LoggedUser, id: string): Promise<SgroupAndMoreOut> {
    console.log(`get_sgroup("${id}")`);
    validate_sgroup_id(id)

    // we query all the attrs we need: attrs for direct_members + attrs to compute rights + attrs to return
    const wanted_attrs = [ 
        hMright.to_attr('member'),
        hMright.attr_synchronized,
        ...to_allowed_flattened_attrs('reader'),
        ...hMyMap.keys(conf.ldap.sgroup_attrs),
        conf.remote_forced_periodicity_attr,
    ]
    const entry = await ldpSgroup.read_sgroup(id, wanted_attrs)
    if (!entry) { throw `sgroup ${id} does not exist` }

    //console.log("      read sgroup {} => %s", id, entry);
    const is_stem_ = is_stem(id);

    // use the 3 attrs kinds:
    const mattrs = multi_attrs(entry);
    // #1 direct members
    const direct_members_ = mattrs[hMright.to_attr('member')] || []
    const remote_query_s = mattrs[hMright.attr_synchronized]?.at(0)
    // #2 compute rights (also computing parents because both require user_dn) + check user is allowed
    const [right, parents] = await get_right_and_parents(logged_user, id, mattrs)
    // #3 pack sgroup attrs:
    const attrs = to_sgroup_attrs(id, entry);

    let more : SgroupOutMore
    if (is_stem_) { 
        const children = await get_children(id)
        more = { stem: { children } }
    } else {
        if (remote_query_s) {
            const remote_query = parse_remote_query(remote_query_s)

            const forced_periodicity = mattrs[conf.remote_forced_periodicity_attr]?.at(0)
            if (forced_periodicity) Object.assign(remote_query, { forced_periodicity })

            console.log({ remote_query })
            const { last_log_date } = await api_log.get_sgroup_logs(id, 0, { sync: true })
            more = { synchronizedGroup: { remote_query, last_sync_date: last_log_date } }
        } else { 
            const direct_members = await ldpSubject.get_subjects_from_urls(direct_members_)
            more = { group: { direct_members } }
        }
    }
    return { attrs, right, ...more, parents }
}

/** 
 * Get the direct privileges on the group/stem
 * @param id - the group/stem to query
 */
export async function get_sgroup_direct_rights(_logged_user: LoggedUser, id: string) {
    console.log("get_sgroup_direct_rights(%s)", id);
    validate_sgroup_id(id)

    const group = await ldpSgroup.read_sgroup(id, hRight.to_allowed_attrs('reader'))
    if (!group) { throw `sgroup ${id} does not exist` }

    const attrs = multi_attrs(group);
    const r: MyMap<Right, SubjectsOrNull> = {}
    for (const right of hRight.to_allowed_rights('reader')) {
        const urls = attrs[hRight.to_attr(right)]
        if (urls) {
            const subjects = await ldpSubject.get_subjects_from_urls(urls)
            r[right] = subjects
        }
    }
    return r
}

/**
 * Search the flattened subjects who have the requested mright on this group
 * @param id - the group to query
 * @param sizeLimit - max number of flattened subjects
 */
export async function get_sgroup_flattened_mright(_logged_user: LoggedUser, id: string, mright: Mright, search_token: Option<string>, sizeLimit: Option<number>): Promise<SubjectsAndCount> {
    console.log("get_sgroup_flattened_mright(%s)", id);
    validate_sgroup_id(id)
    
    const flattened_dns = await ldp.read_flattened_mright(sgroup_id_to_dn(id), mright)

    const count = flattened_dns.length
    const subjects = await ldpSubject.get_subjects(flattened_dns.slice(0, sizeLimit), {}, search_token)
    return { count, subjects }
}

async function may_restrict_to_allowed_and_wanted_groups(sscfg: SubjectSourceConfig, logged_user: LoggedUser, filter: string, group_to_avoid: string | undefined) {
    if (sscfg.dn === conf.ldap.groups_dn && !('TrustedAdmin' in logged_user)) {
        return ldap_filter.and([
            filter,
            await user_right_filter(logged_user, 'reader'),
            ...(group_to_avoid ? avoid_group_and_groups_including_it__filter(group_to_avoid).ands : []),
        ])
    } else {
        return filter
    }
}

/**
 * Search subjects
 * @param sizeLimit - is applied for each subject source, so the max number of results is sizeLimit * nb_subject_sources
 * @param source_dn - restrict the search to this specific subject source
 */
export async function search_subjects(logged_user: LoggedUser, search_token: string, sizeLimit: number, source_dn: Option<Dn>, group_to_avoid?: string) {
    console.log("search_subjects(%s, %s)", search_token, source_dn);
    const r: MyMap<Dn, Subjects> = {}
    for (const sscfg of conf.ldap.subject_sources) {
        if (!source_dn || source_dn === sscfg.dn) {
            let filter = ldpSubject.hSubjectSourceConfig.search_filter_(sscfg, search_token);
            filter = await may_restrict_to_allowed_and_wanted_groups(sscfg, logged_user, filter, group_to_avoid)
            r[toDn(sscfg.dn)] = await ldpSubject.search_subjects(toDn(sscfg.dn), sscfg.display_attrs, filter, {}, sizeLimit)
        }
    }
    return r
}

type SubjectOut = { dn: Dn, attrs: MonoAttrs, ssdn: FlavorDn } | { error: "multiple_match" | "no_match" }
export async function subject_id_to_dn(logged_user: LoggedUser, id: string, source_dn: Option<Dn>): Promise<SubjectOut> {
    let r: Option<SubjectOut>
    for (const sscfg of conf.ldap.subject_sources) {
        if (!source_dn || source_dn === sscfg.dn) {
            if (!sscfg.id_attrs) continue;
            let filter = ldap_filter.or(sscfg.id_attrs.map(id_attr => ldap_filter.eq(id_attr, id)))
            filter = await may_restrict_to_allowed_and_wanted_groups(sscfg, logged_user, filter, undefined)
            const subjects = await ldpSubject.search_subjects(toDn(sscfg.dn), sscfg.display_attrs, filter, {}, 2)
            const nb_matches = hMyMap.keys(subjects).length
            if (nb_matches < 1) {
                // ignore
            } else if (nb_matches > 1 || r) {
                return { error: "multiple_match" }
            } else {
                const [dn, {attrs}] = hMyMap.firstEntry(subjects) ?? internal_error()
                r = { dn, attrs, ssdn: sscfg.dn }
            }
        }
    }
    return r ?? { error: "no_match" }
}

/**
 * Get subjects by ids
 * @param source_dn - restrict the search to this specific subject source
 */
export const subject_ids_to_dns = (logged_user: LoggedUser, ids: string[], source_dn: Option<Dn>) => (
    mapAsync(ids, async id => {
        const subjectOut = await subject_id_to_dn(logged_user, id, source_dn)
        // NB: adding "id" to each array element to ease matching searched id to the result
        return { id, ...subjectOut }
    })
)

async function search_sgroups_with_attrs(filters: string[], sizeLimit: Option<number>): Promise<SgroupsWithAttrs> {
    const wanted_attrs = hMyMap.keys(conf.ldap.sgroup_attrs);
    let id2attrs: SgroupsWithAttrs = {}
    for (const filter of filters) {
        for (const e of await ldpSgroup.search_sgroups(filter, wanted_attrs, sizeLimit)) {
            const id = dn_to_sgroup_id(e.dn)
            if (id) id2attrs[id] = mono_attrs(e)
        }
    }
    return id2attrs
}

function simplify_hierachical_ou(attrs: MonoAttrs): MonoAttrs {
    const ou = attrs.ou?.replace(/.*:/, '')
    if (ou) attrs.ou = ou
    return attrs
}

function to_sgroup_attrs(id: string, attrs: ldapjs.SearchEntryObject): MonoAttrs {
    let attrs_ = mono_attrs_(attrs, hMyMap.keys(conf.ldap.sgroup_attrs))
    if (id === "") {
        // TODO, move this in conf?
        attrs_["ou"] = "Racine"
    } else {
        attrs_ = simplify_hierachical_ou(attrs_)
    }
    return attrs_
}

/**
 * groups for which the {@link logged_user} has DIRECT update|admin right
 * (inherited rights via stems are not taken into account)
 */
export async function mygroups(logged_user: LoggedUser) {
    console.log("mygroups()");
    if ('TrustedAdmin' in logged_user) {
        throw "mygroups need a real user"
    } else {        
        const filters = user_has_direct_right_on_group_filter(people_id_to_dn(logged_user.User), 'updater')
        const filter = ldap_filter.and2(ldap_filter.or(filters), conf.ldap.group_filter)
        return await search_sgroups_with_attrs([filter], undefined)
    }
}

// example of filter used: (| (owner=uid=prigaux,...) (supannGroupeAdminDn=uid=prigaux,...) )
async function get_all_stems_id_with_user_right(user_dn: Dn, right: Right): Promise<MySet<string>> {
    const stems_with_right_filter = ldap_filter.and2(
        conf.ldap.stem.filter,
        user_has_right_on_sgroup_filter(user_dn, right),
    );
    const stems_id = await ldpSgroup.search_sgroups_id(stems_with_right_filter)
    return stems_id
}

function terms_search_filters(sscfg: SubjectSourceConfig, search_token: string) {
    const search_tokens = search_token.split(/\s+/)
    let term_filters = [
        hSubjectSourceConfig.search_filter_(sscfg, search_token)
    ]
    if (search_tokens.length > 1) {
        term_filters.push(
            ldap_filter.and(search_tokens.map(token => 
                hSubjectSourceConfig.search_filter_(sscfg, token)
            ))
        )
    }
    return term_filters
}

/**
 * search groups matching search_token, for which the {@link logged_user} has the requested {@link right}
 * if search_token contains multiple words, first search the whole term, then search on each words in various group 
fields
 */
export async function search_sgroups(logged_user: LoggedUser, right: Right, search_token: string, sizeLimit: number): Promise<SgroupsWithAttrs> {
    console.log("search_sgroups(%s, %s)", search_token, right);

    let term_filters = terms_search_filters(hLdapConfig.sgroup_sscfg(conf.ldap), search_token)

    const right_filter = 'TrustedAdmin' in logged_user ? undefined : await user_right_filter(logged_user, right)
    const group_filters = term_filters.map(term_filter => 
            ldap_filter.and(_.compact([
                right_filter, 
                term_filter,
                conf.ldap.sgroup_filter,
            ]))
    )
    console.log(group_filters)
    return await search_sgroups_with_attrs(group_filters, (sizeLimit))
}

/**
 * Search raw groups/stems having a direct member/right on ''subject_dn''
 * (aka subject memberships)
 * @param subject_dn - Subject DN to search
 * @param mright - member/right to search
 */
export async function search_raw_sgroups_using_a_subject(logged_user: LoggedUser, subject_dn: Dn, mright: Mright) {
    console.log("search_sgroups_using_a_subject(%s, %s)", subject_dn, mright);
    const right_filter = 'TrustedAdmin' in logged_user ? undefined : await user_right_filter(logged_user, mright === 'member' ? 'reader' : mright)
    const filter = ldap_filter.and(_.compact([
        right_filter,
        ldap_filter.eq(hMright.to_attr(mright), dn_to_url(subject_dn)),
        conf.ldap.sgroup_filter,
    ]))
    const l = await ldpSgroup.search_sgroups(filter, [], undefined)
    return l.map(e => dn_to_sgroup_id(e.dn))
}

/** if group "A" has group member "B", searching for group subjects to add as member of "B" must exclude "B" and "A"
 * The result is something like: `(& (!(cn=B)) (!(member=cn=B,ou=groups,dc=nodomain)) )`
*/
export const avoid_group_and_groups_including_it__filter = (id: string) => ({ ands: [
    ldap_filter.not(ldap_filter.eq('cn', id)),
    ldap_filter.not(ldap_filter.member(sgroup_id_to_dn(id))),
] })

/** return `(|(cn=a.*)(cn=b.bb.*))` if logged_user has right on `a.` and `b.bb.` */
export async function user_right_filter(logged_user: { User: string }, right: Right) {
    const user_dn = people_id_to_dn(logged_user.User)
    // from direct rights
    // example: (|(supannGroupeLecteurDN=uid=prigaux,...)(supannGroupeLecteurDN=uid=owner,...))
    const user_direct_allowed_groups_filter = user_has_direct_right_on_group_filter(user_dn, right)

    // from direct rights
    // example: (|(cn=a.*)(cn=b.bb.*)) if user has right on stems "a."" and "b.bb." 
    // TODO: cache !?
    const stems_id_with_right = await get_all_stems_id_with_user_right(user_dn, right)
    const children_of_allowed_stems_filter = (
        // TODO: simplify: no need to keep "a." and "a.b."
        stems_id_with_right.map(stem_id => ldap_filter.sgroup_self_and_children(stem_id))
    )

    return ldap_filter.or([
        ...user_direct_allowed_groups_filter,
        ...children_of_allowed_stems_filter,
    ])
}

/**
 * Is group/stem already created
 * @param id - sgroup identifier
 */
export async function sgroup_exists(logged_user: LoggedUser, id: string) {
    validate_sgroup_id(id)

    await check_right_on_self_or_any_parents(logged_user, id, 'reader');

    return await ldpSgroup.is_sgroup_existing(id);
}

/**
 * Read log entries
 * @param id - sgroup identifier
 * @param bytes - maximum number of bytes to read
 * @returns log entries
 */
export async function get_sgroup_logs(logged_user: LoggedUser, id: string, bytes: number, opts?: { sync: true }) {
    console.log("get_sgroup_logs(%s, %s, %s)", id, bytes, opts)
    validate_sgroup_id(id)

    await check_right_on_self_or_any_parents(logged_user, id, 'admin');

    return await api_log.get_sgroup_logs(id, bytes, opts)
}

export function validate_remote(remote: RemoteQuery) {
    if (isRqSql(remote) && !remote.remote_cfg_name) {
        throw "remote_cfg_name is mandatory for remote SQL query"
    }
    if (remote.remote_cfg_name && !conf.remotes[remote.remote_cfg_name]) {
        throw `unknown remote_cfg_name ${remote.remote_cfg_name}`
    }
    const to_ss = toRqSql(remote)?.to_subject_source
    if (to_ss) {
        if (!conf.ldap.subject_sources.some(ss => ss.dn === to_ss.ssdn)) {
            throw `unknown to_subject_source.ssdn ${JSON.stringify(to_ss)}`
        }
    }
}

export const parse_remote_query = (rq: string): RemoteQuery => (
    parse_sql_url(rq) || parse_ldap_url(rq) || throw_(`invalid remote query ${rq}`)
)

export async function test_remote_query(logged_user: LoggedUser, id: string, rq: RemoteQuery): Promise<TestRemoteQuery> {
    console.log("test_remote_query(%s, %s)", id, rq);   
    validate_sgroup_id(id)
    validate_remote(rq)

    await check_right_on_self_or_any_parents(logged_user, id, 'admin')

    const all_values = isRqSql(rq) ? await sql_query(rq) : Object.keys(await ldap_query(rq))
    const count = all_values.length;
    const max_values = 10;    
    const values = all_values.slice(0, max_values); // return an extract
    const ss_guess = count && isRqSql(rq) ? await guess_subject_source(values) : undefined
    return {
        count,
        values,
        values_truncated: count > max_values,
        ss_guess,
    }
}

export const export_for_tests = { user_highest_right }
