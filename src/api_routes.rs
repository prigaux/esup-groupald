use std::collections::BTreeMap;
use std::result::Result;

use rocket::{Route, State};
use rocket::http::{Cookie, CookieJar};
use rocket::response::{Redirect};
use rocket::serde::json::{Json, Value};
use serde_json::json;

use crate::cache::{AllCaches, self};
use crate::helpers::{before};
use crate::my_types::{MonoAttrs, MyMods, Config, CfgAndLU, SgroupAndMoreOut, RemoteConfig, Right, Subjects, Mright, SgroupsWithAttrs, SubjectsAndCount, LdapConfigOut, Dn, RemoteSqlQuery};
use crate::api_get;
use crate::api_post;
use crate::remote_query::TestRemoteQuerySql;
use crate::rocket_helpers::{OrigUrl, MyJson, action_result, to_json, err_to_json};
use crate::test_data;
use crate::cas_auth;


#[get("/login?<target>&<ticket>")]
async fn login(target: String, ticket: String, orig_url: OrigUrl, cookie_jar: &CookieJar<'_>, config: &State<Config>) -> Result<Redirect, String> {
    if !target.starts_with('/') || target.starts_with("//") {
        return Err(format!("invalid target {}, it must be a path-absolute url", target));
    }
    let service = before(&orig_url.0, "&ticket=").ok_or("weird login url")?;
    let user = cas_auth::validate_ticket(&config.cas.prefix_url, service, &ticket).await?;
    cookie_jar.add_private(Cookie::new("user_id", user));
    Ok(Redirect::found(target))
}

#[get("/set_test_data")]
async fn set_test_data(cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(test_data::set(cfg_and_lu).await)
}
#[get("/clear_test_data")]
async fn clear_test_data(cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(test_data::clear(&cfg_and_lu).await)
}
#[get("/add_test_data")]
async fn add_test_data(cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(test_data::add(cfg_and_lu).await)
}

#[post("/create?<id>", data = "<attrs>")]
async fn create(id: String, attrs: Json<MonoAttrs>, cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(
        api_post::create(cfg_and_lu, &id, attrs.into_inner()).await
    )
}

#[post("/modify_sgroup_attrs?<id>", data = "<attrs>")]
async fn modify_sgroup_attrs(id: String, attrs: Json<MonoAttrs>, cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(
        api_post::modify_sgroup_attrs(cfg_and_lu, &id, attrs.into_inner()).await
    )
}

#[post("/delete?<id>")]
async fn delete(id: String, cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(api_post::delete(cfg_and_lu, &id).await)
}

#[post("/modify_members_or_rights?<id>&<msg>", data = "<mods>")]
async fn modify_members_or_rights(id: String, msg: Option<String>, mods: Json<MyMods>, cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(api_post::modify_members_or_rights(cfg_and_lu, &id, mods.into_inner(), &msg).await)
}

#[post("/modify_remote_sql_query?<id>&<msg>", data = "<remote>")]
async fn modify_remote_sql_query(id: String, msg: Option<String>, remote: Json<RemoteSqlQuery>, all_caches : &State<AllCaches>, cfg_and_lu : CfgAndLU<'_>) -> MyJson {
    action_result(api_post::modify_remote_sql_query(all_caches, cfg_and_lu, &id, remote.into_inner(), &msg).await)
}

#[get("/test_remote_query_sql?<id>&<remote_sql_query>")]
async fn test_remote_query_sql(id: String, remote_sql_query: Json<RemoteSqlQuery>, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<TestRemoteQuerySql>, MyJson> {
    to_json(api_get::test_remote_query_sql(cfg_and_lu, &id, remote_sql_query.into_inner()).await)
}

#[get("/sgroup?<id>")]
async fn sgroup(id: String, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<SgroupAndMoreOut>, MyJson> {
    to_json(api_get::get_sgroup(cfg_and_lu, &id).await)
}

#[get("/sgroup_direct_rights?<id>")]
async fn sgroup_direct_rights(id: String, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<BTreeMap<Right, Subjects>>, MyJson> {
    to_json(api_get::get_sgroup_direct_rights(cfg_and_lu, &id).await)
}

#[get("/group_flattened_mright?<id>&<mright>&<search_token>&<sizelimit>")]
async fn group_flattened_mright(id: String, mright: String, search_token: Option<String>, sizelimit: Option<usize>, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<SubjectsAndCount>, MyJson> {
    let mright = Mright::from_string(&mright).map_err(err_to_json)?;
    to_json(api_get::get_group_flattened_mright(cfg_and_lu, &id, mright, search_token, sizelimit).await)
}

#[get("/sgroup_logs?<id>&<bytes>")]
async fn sgroup_logs(id: String, bytes: i64, cfg_and_lu : CfgAndLU<'_>) -> Result<Value, MyJson> {
    api_get::get_sgroup_logs(cfg_and_lu, &id, bytes).await.map_err(err_to_json)
}

#[get("/search_sgroups?<right>&<search_token>&<sizelimit>")]
async fn search_sgroups(right: String, search_token: String, sizelimit: i32, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<SgroupsWithAttrs>, MyJson> {
    let right = Right::from_string(&right).map_err(err_to_json)?;
    to_json(api_get::search_sgroups(cfg_and_lu, right, search_token, sizelimit).await)
}

#[get("/mygroups")]
async fn mygroups(cfg_and_lu : CfgAndLU<'_>) -> Result<Json<SgroupsWithAttrs>, MyJson> {
    to_json(api_get::mygroups(cfg_and_lu).await)
}

#[get("/clear_cache")]
async fn clear_cache(all_caches : &State<AllCaches>) {
    cache::clear_all(&all_caches);
}

#[get("/search_subjects?<search_token>&<sizelimit>&<source_dn>")]
async fn search_subjects(search_token: String, sizelimit: i32, source_dn: Option<String>, cfg_and_lu : CfgAndLU<'_>) -> Result<Json<BTreeMap<&Dn /* ssdn */, Subjects>>, MyJson> {
    let source_dn = source_dn.map(Dn::from);
    to_json(api_get::search_subjects(cfg_and_lu, search_token, sizelimit, source_dn).await)
}

#[get("/config/public")]
fn config_public(cfg : &State<Config>) -> Value {
    json!({
        "cas_prefix_url": cfg.cas.prefix_url,
    })
}
#[get("/config/ldap")]
fn config_ldap(cfg_and_lu : CfgAndLU<'_>) -> Json<LdapConfigOut<'_>> {
    Json(cfg_and_lu.cfg.ldap.to_js_ui())
}
#[get("/config/remotes")]
fn config_remotes(cfg_and_lu : CfgAndLU<'_>) -> Json<&BTreeMap<String, RemoteConfig>> {
    Json(&cfg_and_lu.cfg.remotes)
}

pub fn routes() -> Vec<Route> {
    routes![
        login,
        clear_cache,
        clear_test_data, add_test_data, set_test_data, 
        sgroup, sgroup_direct_rights, group_flattened_mright, sgroup_logs,
        search_sgroups, search_subjects, mygroups, 
        config_public, config_ldap, config_remotes,
        create, modify_sgroup_attrs, delete, 
        modify_members_or_rights, modify_remote_sql_query, test_remote_query_sql,
    ]
}
