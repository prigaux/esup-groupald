use std::collections::{HashSet, HashMap};

use ldap3::{LdapResult, SearchEntry, Mod};
use ldap3::result::{Result, LdapError};

use super::my_types::*;
use super::ldap_wrapper::LdapW;
use super::my_ldap;
use super::my_ldap::{dn_to_url, url_to_dn, url_to_dn_};
use super::ldap_filter;

fn is_disjoint(vals: &Vec<String>, set: &HashSet<String>) -> bool {
    !vals.iter().any(|val| set.contains(val))
}

// true if at least one LDAP entry value is in "set"
fn has_value(entry: SearchEntry, set: &HashSet<String>) -> bool {
    for vals in entry.attrs.into_values() {
        if !is_disjoint(&vals, &set) {
            return true
        }
    }
    false
}

async fn user_urls(ldp: &mut LdapW<'_>, user: &str) -> Result<HashSet<String>> {
    let r = Ok(ldp.user_groups_and_user(user).await?.iter().map(|dn| dn_to_url(&dn)).collect());
    eprintln!("    user_urls({}) => {:?}", user, r);
    r
}

async fn user_has_right_on_sgroup(ldp: &mut LdapW<'_>, user_urls: &HashSet<String>, id: &str, right: &Right) -> Result<bool> {
    if let Some(group) = ldp.read_sgroup(id, right.to_allowed_attrs()).await? {
        Ok(has_value(group, user_urls))
    } else if id == ldp.config.stem.root_id {
        Ok(false)
    } else {
        Err(LdapError::AdapterInit(format!("stem {} does not exist", id)))
    }
}

async fn user_highest_right_on_stem(ldp: &mut LdapW<'_>, user_urls: &HashSet<String>, id: &str) -> Result<Option<Right>> {
    if let Some(group) = ldp.read_sgroup(id, Right::READER.to_allowed_attrs()).await? {
        for right in Right::READER.to_allowed_rights() {
            if let Some(vals) = group.attrs.get(&right.to_attr()) {
                if !is_disjoint(vals, &user_urls) {
                    return Ok(Some(right))
                }
            }
        }
        Ok(None)
    } else if id == ldp.config.stem.root_id {
        Ok(None)
    } else {
        Err(LdapError::AdapterInit(format!("stem {} does not exist", id)))
    }
}


/*async fn user_has_right_on_group(ldp: &mut LdapW<'_>, user: &str, id: &str, right: &Right) -> Result<bool> {    
    fn user_has_right_filter(user_dn: &str, right: &Right) -> String {
        ldap_filter::or(right.to_allowed_rights().iter().map(|r| 
            ldap_filter::eq(r.to_indirect_attr(), user_dn)
        ).collect())
    }
    let filter = user_has_right_filter(&ldp.config.people_id_to_dn(user), right);
    ldp.is_sgroup_matching_filter(id, &filter).await
}*/

async fn check_user_right_on_any_parents(ldp: &mut LdapW<'_>, user_urls: &HashSet<String>, id: &str, right: Right) -> Result<()> {
    let parents = ldp.config.stem.parent_stems(id);
    for parent in parents {
        if user_has_right_on_sgroup(ldp, &user_urls, parent, &right).await? {
            return Ok(())
        }
    }
    Err(LdapError::AdapterInit(format!("no right on {} parents", id)))
}

async fn check_right_on_any_parents(ldp: &mut LdapW<'_>, id: &str, right: Right) -> Result<()> {
    match ldp.logged_user {
        LoggedUser::TrustedAdmin => {
            if let Some(parent_stem) = ldp.config.stem.parent_stem(id) {
                if !ldp.is_sgroup_existing(&parent_stem).await? { 
                    return Err(LdapError::AdapterInit(format!("stem {} does not exist", parent_stem)))
                }    
            }
            Ok(())
        },
        LoggedUser::User(user) => {
            eprintln!("  check_right_on_any_parents({}, {:?})", id, right);
            let user_urls = user_urls(ldp, user).await?;
            check_user_right_on_any_parents(ldp, &user_urls, id, right).await
        }
    }
}

async fn check_right_on_self_or_any_parents(ldp: &mut LdapW<'_>, id: &str, right: Right) -> Result<()> {
    match ldp.logged_user {
        LoggedUser::TrustedAdmin => {
            Ok(())
        },
        LoggedUser::User(user) => {
            eprintln!("  check_right_on_self_or_any_parents({}, {:?})", id, right);
            let user_urls = user_urls(ldp, user).await?;
            if user_has_right_on_sgroup(ldp, &user_urls, id, &right).await? {
                return Ok(())
            }
            check_user_right_on_any_parents(ldp, &user_urls, id, right).await
        }
    }
}

async fn best_right_on_self_or_any_parents(ldp: &mut LdapW<'_>, id: &str) -> Result<Option<Right>> {
    match ldp.logged_user {
        LoggedUser::TrustedAdmin => {
            Ok(Some(Right::ADMIN))
        },
        LoggedUser::User(user) => {
            eprintln!("  best_right_on_self_or_any_parents({}, {})", id, user);
            let user_urls = user_urls(ldp, user).await?;
            let self_and_parents = [ ldp.config.stem.parent_stems(id), vec![id] ].concat();
            let mut best = None;
            for id in self_and_parents {
                let right = user_highest_right_on_stem(ldp, &user_urls, id).await?;
                eprintln!("    best_right_on_self_or_any_parents: {} => {:?}", id, right);
                if right > best {
                    best = right;
                }
            }
            eprintln!("  best_right_on_self_or_any_parents({}, {}) => {:?}", id, user, best);
            Ok(best)
        }
    }
}


pub async fn create<'a>(cfg_and_lu: CfgAndLU<'a>, kind: GroupKind, id: &str, attrs: Attrs) -> Result<LdapResult> {
    eprintln!("create({:?}, {}, _)", kind, id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    check_right_on_any_parents(ldp, id, Right::ADMIN).await?;
    my_ldap::create_sgroup(ldp, kind, id, attrs).await
}

pub async fn delete<'a>(cfg_and_lu: CfgAndLU<'a>, id: &str) -> Result<LdapResult> {
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    // are we allowed?
    check_right_on_self_or_any_parents(ldp, id, Right::ADMIN).await?;
    // is it possible?
    if ldp.one_group_matches_filter(&ldap_filter::sgroup_children(id)).await? { 
        return Err(LdapError::AdapterInit("can not remove stem with existing children".to_owned()))
    }
    // ok, do it:
    ldp.delete_sgroup(id).await
}

// which Right is needed for these modifications?
fn my_mods_to_right(my_mods: &MyMods) -> Right {
    for (right, _) in my_mods {
        if right > &Mright::READER {
            return Right::ADMIN
        }
    }
    Right::UPDATER
}

// Check validity of modifications
// - stems do not allow members
// - "sql://xxx?..." URLs are only allowed:
//   - as members (not updaters/admins/...)
//   - only one URL is accepted (for simplicity in web interface + similar restriction as in Internet2 Grouper)
//   - modification must be a REPLACE (checking mods is simpler that way)
fn check_mods(is_stem: bool, my_mods: &MyMods) -> Result<()> {
    for (right, submods) in my_mods {
        if right == &Mright::MEMBER && is_stem {
            return Err(LdapError::AdapterInit(format!("members are not allowed for stems")))
        }
        for (action, list) in submods {
            if action == &MyMod::REPLACE && list.len() == 1 && right == &Mright::MEMBER {
                // only case where non DNs are allowed!
            } else if let Some(url) = list.iter().find(|url| url_to_dn(url).is_none()) {
                return Err(LdapError::AdapterInit(format!("non DN URL {} is now allowed", url)))
            }
        }
    }
    Ok(())
}

// Search for groups having this group DN in their member/supannGroupeLecteurDN/supannAdminDN/owner
async fn search_groups_mrights_depending_on_this_group(ldp: &mut LdapW<'_>, id: &str) -> Result<Vec<(String, Mright)>> {
    let mut r = vec![];
    let group_dn = ldp.config.sgroup_id_to_dn(id);
    for mright in Mright::list() {
        for id in ldp.search_groups(&ldap_filter::eq(mright.to_indirect_attr(), &group_dn)).await? {
            r.push((id, mright));
        }
    }
    Ok(r)
}

enum UpResult { Modified, Unchanged }

async fn may_update_indirect_mrights_(ldp: &mut LdapW<'_>, id: &str, mright: &Mright, to_add: HashSet<&str>, to_remove: HashSet<&str>) -> Result<UpResult> {
    let attr = mright.to_indirect_attr();
    let mods = [
        if to_add.is_empty()    { vec![] } else { vec![ Mod::Add(attr, to_add) ] },
        if to_remove.is_empty() { vec![] } else { vec![ Mod::Delete(attr, to_remove) ] },
    ].concat();
    if mods.is_empty() {
        return Ok(UpResult::Unchanged)
    }
    let res = dbg!(ldp.ldap.modify(&ldp.config.sgroup_id_to_dn(id), dbg!(mods)).await?);
    if res.rc != 0 {
        Err(LdapError::AdapterInit(format!("update_indirect_mright failed on {}: {}", id, res)))
    } else {
        Ok(UpResult::Modified)
    }
}

// read group direct URLs
// diff with group indirect DNs
// if needed, update group indirect DNs
async fn may_update_indirect_mrights(ldp: &mut LdapW<'_>, id: &str, mright: &Mright) -> Result<UpResult> {
    eprintln!("  may_update_indirect_mrights({}, {:?})", id, mright);
    let group_dn = ldp.config.sgroup_id_to_dn(id);
    let direct_urls = ldp.read_one_multi_attr__or_err(&group_dn, &mright.to_attr()).await?;
    if let Some(mut direct_dns) = direct_urls.into_iter().map(|url| url_to_dn_(url)).collect::<Option<HashSet<_>>>() {
        if direct_dns.is_empty() && mright == &Mright::MEMBER {
            direct_dns.insert("".to_owned());
        }
        let indirect_dns = HashSet::from_iter(
            ldp.read_one_multi_attr__or_err(&group_dn, &mright.to_indirect_attr()).await?
        );
        let to_add = direct_dns.difference(&indirect_dns).map(|s| s.as_str()).collect();
        let to_remove = indirect_dns.difference(&direct_dns).map(|s| s.as_str()).collect();
        may_update_indirect_mrights_(ldp, id, mright, dbg!(to_add), dbg!(to_remove)).await
    } else {
        // TODO: non DN URL
        Ok(UpResult::Unchanged)
    }
}

async fn may_update_indirect_mrights_rec(ldp: &mut LdapW<'_>, mut todo: Vec<(String, Mright)>) -> Result<()> {
    while let Some((id, mright)) = todo.pop() {
        let result = may_update_indirect_mrights(ldp, &id, &mright).await?;
        if let (Mright::MEMBER, UpResult::Modified) = (mright, &result) {
            todo.append(&mut search_groups_mrights_depending_on_this_group(ldp, &id).await?);
        }    
    }
    Ok(())
}

pub async fn modify_members_or_rights<'a>(cfg_and_lu: CfgAndLU<'a>, id: &str, my_mods: MyMods) -> Result<LdapResult> {
    eprintln!("modify_members_or_rights({}, _)", id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;
    // is logged user allowed to do the modifications?
    check_right_on_self_or_any_parents(ldp, id, my_mods_to_right(&my_mods)).await?;
    // are the modifications valid?
    let is_stem = ldp.is_stem(id).await?;
    check_mods(is_stem, &my_mods)?;

    let todo_indirect = if is_stem { vec![] } else {
        my_mods.keys().map(|mright| (id.to_owned(), mright.clone())).collect()
    };

    // ok, let's do update direct mrights:
    let res = my_ldap::modify_direct_members_or_rights(ldp, id, my_mods).await?;
    // then update indirect groups mrights
    may_update_indirect_mrights_rec(ldp, todo_indirect).await?;

    Ok(res)
}

fn contains_ref(l: &Vec<String>, s: &str) -> bool {
    l.iter().any(|e| e == s)
}

fn is_stem(entry: &SearchEntry) -> bool {
    if let Some(vals) = entry.attrs.get("objectClass") {
        !contains_ref(vals, "groupOfNames")
    } else {
        false
    }
}

fn get_sgroups_attrs(attrs: HashMap<String, Vec<String>>) -> Attrs {
    attrs.into_iter().filter_map(|(attr, val)| {
        let attr = Attr::from_string(&attr)?;
        let one = val.into_iter().next()?;
        Some((attr, one))
    }).collect()
}

pub async fn get_sgroup<'a>(cfg_and_lu: CfgAndLU<'a>, id: &str) -> Result<SgroupAndRight> {
    eprintln!("get_sgroup({})", id);
    cfg_and_lu.cfg.ldap.stem.validate_sgroup_id(id)?;
    let ldp = &mut LdapW::open_(&cfg_and_lu).await?;

    let wanted_attrs = [ Attr::list_as_string(), vec![ "objectClass" ] ].concat();
    if let Some(entry) = ldp.read_sgroup(id, wanted_attrs).await? {
        let kind = if is_stem(&entry) { GroupKind::STEM } else { GroupKind::GROUP };
        let attrs = get_sgroups_attrs(entry.attrs);
        let sgroup = SgroupOut { attrs, kind };
        let right = best_right_on_self_or_any_parents(ldp, id).await?
                .ok_or_else(|| LdapError::AdapterInit(format!("not right to read {}", id)))?;
        Ok(SgroupAndRight { sgroup, right })
    } else {
        Err(LdapError::AdapterInit(format!("sgroup {} does not exist", id)))
    }
}

/*
pub async fn get_children<'a>(cfg_and_lu: CfgAndLU<'a>, id: &str) -> Result<()> {
    // Vec<Attrs + "kind">
    Ok(())
}
*/