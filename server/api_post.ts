import _ from "lodash";
import * as ldapjs from 'ldapjs'
import * as ldapP from 'ldapjs-promise-disconnectwhenidle'
import * as my_ldap from './my_ldap'
import * as api_log from './api_log'
import * as remote_query from './remote_query'
import * as cache from './cache'
import { validate_remote } from "./api_get";
import { hashmap_difference, internal_error } from "./helpers";
import ldap_filter from "./ldap_filter";
import { dn_is_sgroup, dn_to_url, sgroup_id_to_dn, to_flattened_attr, urls_to_dns, validate_sgroups_attrs } from "./ldap_helpers";
import { mono_attrs, one_group_matches_filter, read_flattened_mright, read_flattened_mright_raw, read_one_multi_attr__or_err } from "./ldap_wrapper";
import { check_right_on_any_parents, check_right_on_self_or_any_parents } from "./my_ldap_check_rights";
import { Dn, DnsOpts, hMright, hMyMap, LoggedUser, MonoAttrs, Mright, MyMap, MyMod, MyMods, MySet, Option, RemoteSqlQuery, Right, toDn } from "./my_types";
import { direct_members_to_remote_sql_query } from "./remote_query";
import { is_stem, validate_sgroup_id } from "./stem_helpers";
import conf from "./conf";

export async function create(logged_user: LoggedUser, id: string, attrs: MonoAttrs) {
    console.log("create({}, _)", id);
    validate_sgroup_id(id)
    validate_sgroups_attrs(attrs)
    await check_right_on_any_parents(logged_user, id, 'admin')
    await my_ldap.create_sgroup(id, attrs)
    await api_log.log_sgroup_action(logged_user, id, "create", undefined, attrs)
}

async function current_sgroup_attrs(id: string): Promise<MonoAttrs> {
    const attrs = hMyMap.keys(conf.ldap.sgroup_attrs);
    const e = await my_ldap.read_sgroup(id, attrs) ?? internal_error()
    return mono_attrs(e)
}

async function remove_non_modified_attrs(id: string, attrs: MonoAttrs): Promise<MonoAttrs> {
    const current = await current_sgroup_attrs(id)
    return _.pickBy(attrs, (val, attr) => val !== current[attr])
}

export async function modify_sgroup_attrs(logged_user: LoggedUser, id: string, attrs: MonoAttrs) {
    console.log("modify_attrs({}, _)", id);
    validate_sgroup_id(id)
    validate_sgroups_attrs(attrs)
    
    await check_right_on_self_or_any_parents(logged_user, id, 'admin')

    const attrs_ = await remove_non_modified_attrs(id, attrs)

    await my_ldap.modify_sgroup_attrs(id, attrs_)
    await api_log.log_sgroup_action(logged_user, id, "modify_attrs", undefined, attrs_)
}

export async function delete_(logged_user: LoggedUser, id: string) {
    validate_sgroup_id(id)
    // are we allowed?
    await check_right_on_self_or_any_parents(logged_user, id, 'admin')
    // is it possible?
    if (await one_group_matches_filter(ldap_filter.sgroup_children(id))) { 
        throw "can not remove stem with existing children"
    }
    // save last attrs for logging
    const current = await current_sgroup_attrs(id)

    // ok, do it:
    await my_ldap.delete_sgroup(id)
    await api_log.log_sgroup_action(logged_user, id, "delete", undefined, current)
}

// which Right is needed for these modifications?
function my_mods_to_right(my_mods: MyMods): Right {
    for (const right of hMyMap.keys(my_mods)) {
        if (right !== 'reader') {
            return 'admin'
        }
    }
    return 'updater'
}

function to_submods(add: DnsOpts, delete_: DnsOpts, replace: Option<DnsOpts>): MyMap<MyMod, DnsOpts> {
    return hMyMap.compact({
        add: !_.isEmpty(add) ? add : undefined,
        delete: !_.isEmpty(delete_) ? delete_ : undefined,
        replace: replace,
    })
}
function from_submods(submods: MyMap<MyMod, DnsOpts>): [DnsOpts, DnsOpts, Option<DnsOpts>] {
    return [
        submods.add || {},
        submods.delete || {},
        submods.replace,
    ]
}

async function check_and_simplify_mods_(id: string, mright: Mright, submods: MyMap<MyMod, DnsOpts>): Promise<MyMap<MyMod, DnsOpts>> {
    const [add, delete_, replace] = from_submods(submods);

    if (replace && _.size(replace) > 4) {
        const current_dns = await my_ldap.read_direct_mright(sgroup_id_to_dn(id), mright)
        // transform Replace into Add/Delete
        Object.assign(add, hashmap_difference(replace, current_dns));
        Object.assign(delete_, hashmap_difference(current_dns, replace));
        console.log("  replaced long\n    Replace %s with\n    Add %s\n    Replace %s", replace, add, delete_);
        return to_submods(add, delete_, undefined)
    }
    return to_submods(add, delete_, replace)
}

// Check validity of modifications
// - stems do not allow members
// - "sql://xxx?..." URLs are only allowed:
//   - as members (not updaters/admins/...)
//   - only one URL is accepted (for simplicity in web interface + similar restriction as in Internet2 Grouper)
//   - modification must be a Replace (checking mods is simpler that way)
async function check_and_simplify_mods(is_stem: boolean, id: string, my_mods: MyMods): Promise<MyMods> {
    const r: MyMods = {}
    await hMyMap.eachAsync(my_mods, async (submods, mright) => {
        if (mright === 'member' && is_stem) {
            throw "members are not allowed for stems"
        }
        const submods_ = await check_and_simplify_mods_(id, mright, submods)
        if (!_.isEmpty(submods_)) {
            r[mright] = submods_
        }
    })
    return r
}

type IdMright = { id: string, mright: Mright }

// Search for groups having this group DN in their member/supannGroupeLecteurDN/supannAdminDN/owner
async function search_groups_mrights_depending_on_this_group(id: string) {
    const r: IdMright[] = []
    const group_dn = sgroup_id_to_dn(id);
    for (const mright of hMright.list()) {
        for (id of await my_ldap.search_sgroups_id(ldap_filter.eq(to_flattened_attr(mright), group_dn))) {
            r.push({ id, mright });
        }
    }
    return r
}

enum UpResult { Modified, Unchanged }

async function may_update_flattened_mrights__(id: string, mright: Mright, to_add: MySet<Dn>, to_remove: MySet<Dn>): Promise<UpResult> {
    const attr = to_flattened_attr(mright);
    const mods: ldapjs.Change[] = [];
    if (!_.isEmpty(to_add)) mods.push(new ldapjs.Change({ operation: 'add', modification: { [attr]: to_add } }))
    if (!_.isEmpty(to_remove)) mods.push(new ldapjs.Change({ operation: 'delete', modification: { [attr]: to_remove } }))
    if (_.isEmpty(mods)) {
        return UpResult.Unchanged
    }
    try {        
        await ldapP.modify(sgroup_id_to_dn(id), mods)
        return UpResult.Modified
    } catch (e) {
        throw `update_flattened_mright failed on ${id}: ${e}`
    }
}

async function get_flattened_dns(direct_dns: MySet<Dn>): Promise<MySet<Dn>> {
    const r = [...direct_dns]
    for (const dn of direct_dns) {
        if (dn_is_sgroup(dn)) {
            r.push(...await read_flattened_mright(dn, 'member'))
        }
    }
    return r
}

async function remote_sql_query_to_dns(_logged_user: LoggedUser, remote: RemoteSqlQuery): Promise<DnsOpts> {
    const sql_values = await remote_query.query(conf.remotes, remote)
    // TODO: api_log.log_sgroup_action(logged_user, id, "remote_sql_query")
    return await remote_query.sql_values_to_dns(remote, sql_values)
}

async function urls_to_dns_handling_remote(logged_user: LoggedUser, mright: Mright, urls: string[]): Promise<DnsOpts> {
    if (mright === 'member') {
        const remote = direct_members_to_remote_sql_query(urls)
        if (remote) {
            return await remote_sql_query_to_dns(logged_user, remote)
        }
    }
    return urls_to_dns(urls) ?? internal_error()
}

async function may_update_flattened_mrights_(id: string, mright: Mright, group_dn: Dn, direct_dns: MySet<Dn>) {
    const flattened_dns = await get_flattened_dns(direct_dns)
    if (_.isEmpty(flattened_dns) && mright === 'member') {
        flattened_dns.push(toDn(""));
    }
    const current_flattened_dns = await read_flattened_mright_raw(group_dn, mright)
    const to_add = _.difference(flattened_dns, current_flattened_dns);
    const to_remove = _.difference(current_flattened_dns, flattened_dns);
    return await may_update_flattened_mrights__(id, mright, (to_add), (to_remove))
}

// read group direct URLs
// diff with group flattened DNs
// if (needed, update group flattened DNs
async function may_update_flattened_mrights(logged_user: LoggedUser, id: string, mright: Mright): Promise<UpResult> {
    console.log("  may_update_flattened_mrights(%s, %s)", id, mright);
    const group_dn = sgroup_id_to_dn(id);

    const urls = await read_one_multi_attr__or_err(group_dn, hMright.to_attr(mright))
    const direct_dns = await urls_to_dns_handling_remote(logged_user, mright, urls)        
    return await may_update_flattened_mrights_(id, mright, group_dn, hMyMap.keys(direct_dns))
}

export async function may_update_flattened_mrights_rec(logged_user: LoggedUser, todo: IdMright[]) {
    for (;;) {
        const one = todo.shift()
        if (!one) return
        const result = await may_update_flattened_mrights(logged_user, one.id, one.mright)
        if (one.mright === 'member' && result === UpResult.Modified) {
            todo.push(...await search_groups_mrights_depending_on_this_group(one.id))
        } 
    }
}

async function may_check_member_ttl(id: string, my_mods: MyMods) {
    const submods = my_mods.member
    if (submods) {
        const attrs = await current_sgroup_attrs(id)
        const ttl_max = attrs["groupaldOptions;x-member-ttl-max"]
        if (ttl_max) {
            /*
            const max = Utc.now() + Duration.days(ttl_max.parse().map_err(|_| MyErr.Msg("member-ttl-max must be an integer"))?);
            for (action, list) in submods {
                if (*action !== MyMod.Delete {
                    for (dn, opts) in list {
                        const enddate = DateTime.parse_from_rfc3339(
                            opts.enddate.as_ref().ok_or_else(|| MyErr.Msg("enddate mandatory for this sgroup"))?
                        ).map_err(|_| MyErr.Msg(format!("invalid enddate for {:?}", dn)))?;
                        if (enddate > max {
                            throw (format!("enddate > member-ttl-max for {:?}", dn)))
                        }
                    }
                }
            }
            */
        }
    }    
}

export async function modify_members_or_rights(logged_user: LoggedUser, id: string, my_mods: MyMods, msg: Option<string>) {
    console.log("modify_members_or_rights(%s, _)", id);
    validate_sgroup_id(id)
    // is logged user allowed to do the modifications?
    await check_right_on_self_or_any_parents(logged_user, id, my_mods_to_right(my_mods))
    // are the modifications valid?
    const is_stem_ = is_stem(id);

    await may_check_member_ttl(id, my_mods)

    const my_mods_ = await check_and_simplify_mods(is_stem_, id, my_mods)
    if (_.isEmpty(my_mods_)) {
        // it happens when a "Replace" has been simplified into 0 Add/Delete
        return
    }

    const todo_flattened = is_stem_ ? [] : hMyMap.mapToArray(my_mods_, (_, mright) => ({id, mright}))

    // ok, const's do update direct mrights:
    await my_ldap.modify_direct_members_or_rights(id, my_mods_)

    await api_log.log_sgroup_action(logged_user, id, "modify_members_or_rights", msg, my_mods_)

    // then update flattened groups mrights
    await may_update_flattened_mrights_rec(logged_user, todo_flattened)

}

export async function modify_remote_sql_query(logged_user: LoggedUser, id: string, remote: RemoteSqlQuery, msg: Option<string>) {
    console.log("modify_remote_sql_query(%s, %s, %s)", id, remote, msg);
    validate_sgroup_id(id)
    validate_remote(remote)
    
    await ldapP.modify(sgroup_id_to_dn(id),
        new ldapjs.Change({ operation: 'replace', modification: { [hMright.to_attr_synchronized('member')]: remote } }),
    )

    await api_log.log_sgroup_action(logged_user, id, "modify_remote_sql_query", msg, remote)

    const todo: IdMright[] = [{id, mright: 'member'}];
    await may_update_flattened_mrights_rec(logged_user, todo)

    // needed for new sync group or if "remote_cfg_name" was modified  
    cache.clear_all();
}

