#![allow(clippy::comparison_chain)]

use std::collections::{HashSet, BTreeMap};
use std::sync::Arc;
use chrono::{Utc, Duration, DateTime};
use tokio::task;

use ldap3::{Mod};


use crate::api_get::validate_remote;
use crate::cache::AllCaches;
use crate::helpers::hashmap_difference;
use crate::remote_query::direct_members_to_remote_sql_query;
use crate::{my_types::*, remote_query, cache};
use crate::my_err::{Result, MyErr};
use crate::ldap_wrapper::{LdapW, mono_attrs};
use crate::my_ldap::{self, urls_to_dns};
use crate::my_ldap_check_rights::{check_right_on_self_or_any_parents, check_right_on_any_parents};
use crate::ldap_filter;
use crate::api_log;

pub async fn create(cfg_and_lu: CfgAndLU<'_>, id: &str, attrs: MonoAttrs) -> Result<()> {
    eprintln!("create({}, _)", id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    cfg_and_lu.cfg.ldap.validate_sgroups_attrs(&attrs)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    check_right_on_any_parents(ldp, id, Right::Admin).await?;
    my_ldap::create_sgroup(ldp, id, &attrs).await?;
    api_log::log_sgroup_action(&cfg_and_lu, id, "create", &None, serde_json::to_value(attrs)?).await?;
    Ok(())
}

async fn current_sgroup_attrs(ldp: &mut LdapW<'_>, id: &str) -> Result<MonoAttrs> {
    let attrs = ldp.config.sgroup_attrs.keys().collect();
    let e = ldp.read_sgroup(id, attrs).await?
        .ok_or_else(|| MyErr::Msg("internal error".to_owned()))?;
    Ok(mono_attrs(e.attrs))
}

async fn remove_non_modified_attrs(ldp: &mut LdapW<'_>, id: &str, attrs: MonoAttrs) -> Result<MonoAttrs> {
    let current = current_sgroup_attrs(ldp, id).await?;
    Ok(attrs.into_iter().filter(|(attr, val)| 
        Some(val) != current.get(attr)
    ).collect())
}

pub async fn modify_sgroup_attrs(cfg_and_lu: CfgAndLU<'_>, id: &str, attrs: MonoAttrs) -> Result<()> {
    eprintln!("modify_attrs({}, _)", id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    cfg_and_lu.cfg.ldap.validate_sgroups_attrs(&attrs)?;
    
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    check_right_on_self_or_any_parents(ldp, id, Right::Admin).await?;

    let attrs = remove_non_modified_attrs(ldp, id, attrs).await?;

    my_ldap::modify_sgroup_attrs(ldp, id, &attrs).await?;
    api_log::log_sgroup_action(&cfg_and_lu, id, "modify_attrs", &None, serde_json::to_value(attrs)?).await?;

    Ok(())
}

pub async fn delete(cfg_and_lu: CfgAndLU<'_>, id: &str) -> Result<()> {
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    // are we allowed?
    check_right_on_self_or_any_parents(ldp, id, Right::Admin).await?;
    // is it possible?
    if ldp.one_group_matches_filter(&ldap_filter::sgroup_children(id)).await? { 
        return Err(MyErr::Msg("can not remove stem with existing children".to_owned()))
    }
    // save last attrs for logging
    let current = current_sgroup_attrs(ldp, id).await?;

    // ok, do it:
    ldp.delete_sgroup(id).await?;
    api_log::log_sgroup_action(&cfg_and_lu, id, "delete", &None, serde_json::to_value(current)?).await?;
    Ok(())
}

// which Right is needed for these modifications?
fn my_mods_to_right(my_mods: &MyMods) -> Right {
    for right in my_mods.keys() {
        if right > &Mright::Reader {
            return Right::Admin
        }
    }
    Right::Updater
}

fn to_submods(add: DnsOpts, delete: DnsOpts, replace: Option<DnsOpts>) -> BTreeMap<MyMod, DnsOpts> {
    let mut r = btreemap! {};
    if !add.is_empty() { r.insert(MyMod::Add, add); }
    if !delete.is_empty() { r.insert(MyMod::Delete, delete); }
    if let Some(replace) = replace { r.insert(MyMod::Replace, replace); }
    r
}
fn from_submods(mut submods: BTreeMap<MyMod, DnsOpts>) -> (DnsOpts, DnsOpts, Option<DnsOpts>) {
    (
        submods.remove(&MyMod::Add).unwrap_or_default(),
        submods.remove(&MyMod::Delete).unwrap_or_default(),
        submods.remove(&MyMod::Replace),
    )
}

async fn check_and_simplify_mods_(ldp: &mut LdapW<'_>, id: &str, mright: Mright, submods: BTreeMap<MyMod, DnsOpts>) -> Result<BTreeMap<MyMod, DnsOpts>> {
    let (mut add, mut delete, replace) = from_submods(submods);

    if let Some(replace) = &replace {
      if replace.len() > 4 {
        if let Some(current_dns) = {
            let group_dn = ldp.config.sgroup_id_to_dn(id);
            ldp.read_direct_mright(&group_dn, mright).await?
        } {
            // transform Replace into Add/Delete
            add.extend(hashmap_difference(replace, &current_dns));
            delete.extend(hashmap_difference(&current_dns, &replace));
            eprintln!("  replaced long\n    Replace {:?} with\n    Add {:?}\n    Replace {:?}", replace, add, delete);
            return Ok(to_submods(add, delete, None))
        }
      }
    }
    Ok(to_submods(add, delete, replace))
}

// Check validity of modifications
// - stems do not allow members
// - "sql://xxx?..." URLs are only allowed:
//   - as members (not updaters/admins/...)
//   - only one URL is accepted (for simplicity in web interface + similar restriction as in Internet2 Grouper)
//   - modification must be a Replace (checking mods is simpler that way)
async fn check_and_simplify_mods(ldp: &mut LdapW<'_>, is_stem: bool, id: &str, my_mods: MyMods) -> Result<MyMods> {
    let mut r: MyMods = btreemap!{};
    for (mright, submods) in my_mods {
        if mright == Mright::Member && is_stem {
            return Err(MyErr::Msg("members are not allowed for stems".to_owned()))
        }
        let submods = check_and_simplify_mods_(ldp, id, mright, submods).await?;
        if !submods.is_empty() {
            r.insert(mright, submods);
        }
    }
    Ok(r)
}

// Search for groups having this group DN in their member/supannGroupeLecteurDN/supannAdminDN/owner
async fn search_groups_mrights_depending_on_this_group(ldp: &mut LdapW<'_>, id: &str) -> Result<Vec<(String, Mright)>> {
    let mut r = vec![];
    let group_dn = ldp.config.sgroup_id_to_dn(id);
    for mright in Mright::list() {
        for id in ldp.search_sgroups_id(&ldap_filter::eq(ldp.config.to_flattened_attr(mright), &group_dn.0)).await? {
            r.push((id, mright));
        }
    }
    Ok(r)
}

enum UpResult { Modified, Unchanged }

fn dns_to_strs(dns: HashSet<&Dn>) -> HashSet<&str> {
    dns.iter().map(|dn| dn.0.as_ref()).collect()
}

async fn may_update_flattened_mrights__(ldp: &mut LdapW<'_>, id: &str, mright: Mright, to_add: HashSet<&Dn>, to_remove: HashSet<&Dn>) -> Result<UpResult> {
    let attr = ldp.config.to_flattened_attr(mright);
    let mods = [
        if to_add.is_empty()    { vec![] } else { vec![ Mod::Add(attr, dns_to_strs(to_add)) ] },
        if to_remove.is_empty() { vec![] } else { vec![ Mod::Delete(attr, dns_to_strs(to_remove)) ] },
    ].concat();
    if mods.is_empty() {
        return Ok(UpResult::Unchanged)
    }
    let res = dbg!(ldp.ldap.modify(dbg!(&ldp.config.sgroup_id_to_dn(id).0), dbg!(mods)).await?);
    if res.rc != 0 {
        Err(MyErr::Msg(format!("update_flattened_mright failed on {}: {}", id, res)))
    } else {
        Ok(UpResult::Modified)
    }
}

async fn get_flattened_dns(ldp: &mut LdapW<'_>, direct_dns: &HashSet<Dn>) -> Result<HashSet<Dn>> {
    let mut r = direct_dns.clone();
    for dn in direct_dns {
        if ldp.config.dn_is_sgroup(dn) {
            r.extend(ldp.read_flattened_mright(dn, Mright::Member).await?);
        }
    }
    Ok(r)
}

async fn remote_sql_query_to_dns(cfg_and_lu: &CfgAndLU<'_>, ldp: &mut LdapW<'_>, remote: RemoteSqlQuery) -> Result<DnsOpts> {
    let remote = Arc::new(remote);
    let sql_values = {
        let remote = remote.clone();
        let remotes = cfg_and_lu.cfg.remotes.clone();
        task::spawn_blocking(move || remote_query::query(&remotes, &remote)).await??
    };
    // TODO: api_log::log_sgroup_action(&cfg_and_lu, id, "remote_sql_query").await?;
    remote_query::sql_values_to_dns(ldp, &remote, sql_values).await
}

async fn urls_to_dns_handling_remote(cfg_and_lu: &CfgAndLU<'_>, ldp: &mut LdapW<'_>, mright: Mright, urls: Vec<String>) -> Result<DnsOpts> {
    if mright == Mright::Member {
        if let Some(remote) = direct_members_to_remote_sql_query(&urls)? {
            return remote_sql_query_to_dns(cfg_and_lu, ldp, remote).await
        }
    }
    urls_to_dns(urls).ok_or(MyErr::Msg("internal error".to_owned()))
}

async fn may_update_flattened_mrights_(ldp: &mut LdapW<'_>, id: &str, mright: Mright, group_dn: Dn, direct_dns: HashSet<Dn>) -> Result<UpResult> {
    let mut flattened_dns = get_flattened_dns(ldp, &direct_dns).await?;
    if flattened_dns.is_empty() && mright == Mright::Member {
        flattened_dns.insert(Dn::from(""));
    }
    let current_flattened_dns = HashSet::from_iter(
        ldp.read_flattened_mright_raw(&group_dn, mright).await?
    );
    let to_add = flattened_dns.difference(&current_flattened_dns).collect();
    let to_remove = current_flattened_dns.difference(&flattened_dns).collect();
    may_update_flattened_mrights__(ldp, id, mright, dbg!(to_add), dbg!(to_remove)).await
}

// read group direct URLs
// diff with group flattened DNs
// if needed, update group flattened DNs
async fn may_update_flattened_mrights(cfg_and_lu: &CfgAndLU<'_>, ldp: &mut LdapW<'_>, id: &str, mright: Mright) -> Result<UpResult> {
    eprintln!("  may_update_flattened_mrights({}, {:?})", id, mright);
    let group_dn = ldp.config.sgroup_id_to_dn(id);

    let urls = ldp.read_one_multi_attr__or_err(&group_dn, &mright.to_attr()).await?;
    let direct_dns = urls_to_dns_handling_remote(cfg_and_lu, ldp, mright, urls).await?;        
    may_update_flattened_mrights_(ldp, id, mright, group_dn, direct_dns.into_keys().collect()).await
}

pub async fn may_update_flattened_mrights_rec(cfg_and_lu: &CfgAndLU<'_>, ldp: &mut LdapW<'_>, mut todo: Vec<(String, Mright)>) -> Result<()> {
    while let Some((id, mright)) = todo.pop() {
        let result = may_update_flattened_mrights(cfg_and_lu, ldp, &id, mright).await?;
        if let (Mright::Member, UpResult::Modified) = (mright, &result) {
            todo.append(&mut search_groups_mrights_depending_on_this_group(ldp, &id).await?);
        }    
    }
    Ok(())
}

async fn may_check_member_ttl(ldp: &mut LdapW<'_>, id: &str, my_mods: &MyMods) -> Result<()> {
    if let Some(submods) = my_mods.get(&Mright::Member) {
        let attrs = current_sgroup_attrs(ldp, id).await?;

        if let Some(ttl_max) = attrs.get("groupaldOptions;x-member-ttl-max") {
            let max = Utc::now() + Duration::days(ttl_max.parse().map_err(|_| MyErr::Msg("member-ttl-max must be an integer".to_owned()))?);
            for (action, list) in submods {
                if *action != MyMod::Delete {
                    for (dn, opts) in list {
                        let enddate = DateTime::parse_from_rfc3339(
                            opts.enddate.as_ref().ok_or_else(|| MyErr::Msg("enddate mandatory for this sgroup".to_owned()))?
                        ).map_err(|_| MyErr::Msg(format!("invalid enddate for {:?}", dn)))?;
                        if enddate > max {
                            return Err(MyErr::Msg(format!("enddate > member-ttl-max for {:?}", dn)))
                        }
                    }
                }
            }
        }
    }    
    Ok(())
}

pub async fn modify_members_or_rights(cfg_and_lu: CfgAndLU<'_>, id: &str, my_mods: MyMods, msg: &Option<String>) -> Result<()> {
    eprintln!("modify_members_or_rights({}, _)", id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    // is logged user allowed to do the modifications?
    check_right_on_self_or_any_parents(ldp, id, my_mods_to_right(&my_mods)).await?;
    // are the modifications valid?
    let is_stem = ldp.config.stem.is_stem(id);

    may_check_member_ttl(ldp, id, &my_mods).await?;

    let my_mods = check_and_simplify_mods(ldp, is_stem, id, my_mods).await?;
    if my_mods.is_empty() {
        // it happens when a "Replace" has been simplified into 0 Add/Delete
        return Ok(())
    }

    let todo_flattened = if is_stem { vec![] } else {
        my_mods.keys().map(|mright| (id.to_owned(), *mright)).collect()
    };

    // TODO transform Replace into Add/Delete

    // ok, let's do update direct mrights:
    my_ldap::modify_direct_members_or_rights(ldp, id, &my_mods).await?;

    api_log::log_sgroup_action(&cfg_and_lu, id, "modify_members_or_rights", msg, serde_json::to_value(my_mods)?).await?;

    // then update flattened groups mrights
    may_update_flattened_mrights_rec(&cfg_and_lu, ldp, todo_flattened).await?;

    Ok(())
}

pub async fn modify_remote_sql_query(all_caches : &AllCaches, cfg_and_lu: CfgAndLU<'_>, id: &str, remote: RemoteSqlQuery, msg: &Option<String>) -> Result<()> {
    eprintln!("modify_remote_sql_query({}, {:?}, {:?})", id, remote, msg);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    validate_remote(&cfg_and_lu, &remote)?;
    
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    ldp.ldap.modify(&ldp.config.sgroup_id_to_dn(id).0, vec![
        Mod::Replace(Mright::Member.to_attr_synchronized(), hashset![ String::from(&remote) ]),
    ]).await?.success()?;

    api_log::log_sgroup_action(&cfg_and_lu, id, "modify_remote_sql_query", msg, serde_json::to_value(remote.clone())?).await?;

    let todo = vec![(id.to_owned(), Mright::Member)];
    may_update_flattened_mrights_rec(&cfg_and_lu, ldp, todo).await?;

    // needed for new sync group or if "remote_cfg_name" was modified  
    cache::clear(&all_caches.remote_to_sgroup_ids);

    Ok(())
}

