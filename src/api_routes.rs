use std::collections::BTreeMap;
use std::result::Result;

use rocket::request::{self, FromRequest, Request};
use rocket::http::{Status, ContentType};
use rocket::http::{Cookie, CookieJar};
use rocket::response::{self, Responder, Response};
use rocket::serde::json::json;

use rocket::outcome::{Outcome, try_outcome};
use rocket::serde::{json::Json};
use rocket::{Route, State};

use ldap3::result::LdapError;

use super::my_types::{SgroupAttrs, MyMods, Config, CfgAndLU, LoggedUser, SgroupAndMoreOut, RemoteConfig, SubjectSourceConfig};
use super::api;
use super::test_data;
use super::cas_auth;


#[rocket::async_trait]
impl<'r> FromRequest<'r> for CfgAndLU<'r> {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, ()> {
        let cfg = try_outcome!(request.guard::<&State<Config>>().await);
        let bearer = request.headers().get_one("Authorization")
                        .and_then(|auth| auth.strip_prefix("Bearer "));
        if bearer.is_some() && bearer == cfg.trusted_auth_bearer.as_deref() {
            let user = match request.headers().get_one("X-Impersonate-User") {
                Some(u) => LoggedUser::User(u.to_owned()),
                _ => LoggedUser::TrustedAdmin,
            };
            return Outcome::Success(CfgAndLU { cfg, user });
        }
        if let Some(cookie) = request.cookies().get_private("user_id") {
            return Outcome::Success(CfgAndLU { cfg, user: LoggedUser::User(cookie.value().to_owned()) });
        }
        Outcome::Failure((Status::Unauthorized, ()))
    }
}

struct MyJson { status: Status, body: String }

impl MyJson {
    fn new(status: Status, body: String) -> Self {
        Self { status, body }
    }
}

impl<'r, 'o: 'r> Responder<'r, 'o> for MyJson {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'o> {
        Response::build()
            .status(self.status)
            .header(ContentType::JSON)
            .sized_body(self.body.len(), std::io::Cursor::new(self.body))
            .ok()
    }
}

fn ldp_err_to_json(err: LdapError) -> MyJson {
    dbg!(&err); 
    let body = json!({
        "error": true,
        "msg": err.to_string(),
    });
    MyJson::new(Status::InternalServerError, body.to_string())
}

fn to_json<T>(r: Result<T, LdapError>) -> Result<Json<T>, MyJson> {
    r.map(Json).map_err(ldp_err_to_json)
}

fn action_result(r : Result<(), LdapError>) -> MyJson {
    match r {
        Err(err) => ldp_err_to_json(err),
        Ok(_) => {
            let body = json!({ "ok": true });
            MyJson::new(Status::Ok, body.to_string())
        },
    }
}

#[get("/login?<ticket>")]
async fn login(ticket: String, jar: &CookieJar<'_>, config: &State<Config>) -> Result<(), String> {
    let service = "http://localhost"; // TODO
    let user = cas_auth::validate_ticket(&config.cas.prefix_url, service, &ticket).await?;
    jar.add_private(Cookie::new("user_id", user));
    Ok(())
}

#[get("/set_test_data")]
async fn set_test_data<'a>(cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(test_data::set(cfg_and_lu).await)
}
#[get("/clear_test_data")]
async fn clear_test_data<'a>(cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(test_data::clear(&cfg_and_lu).await)
}
#[get("/add_test_data")]
async fn add_test_data<'a>(cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(test_data::add(cfg_and_lu).await)
}

#[post("/create?<id>", data = "<attrs>")]
async fn create<'a>(id: String, attrs: Json<SgroupAttrs>, cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(
        api::create(cfg_and_lu, &id, attrs.into_inner()).await
    )
}

#[post("/delete?<id>")]
async fn delete<'a>(id: String, cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(api::delete(cfg_and_lu, &id).await)
}

// curl 'localhost:8000/modify_members_or_rights/?id=foo.bar' -d '{ "member": { "add": [ "ldap:///uid=prigaux2,..." ] } }'
#[post("/modify_members_or_rights?<id>", data = "<mods>")]
async fn modify_members_or_rights<'a>(id: String, mods: Json<MyMods>, cfg_and_lu : CfgAndLU<'a>) -> MyJson {
    action_result(api::modify_members_or_rights(cfg_and_lu, &id, mods.into_inner()).await)
}

#[get("/sgroup?<id>")]
async fn sgroup<'a>(id: String, cfg_and_lu : CfgAndLU<'a>) -> Result<Json<SgroupAndMoreOut>, MyJson> {
    to_json(api::get_sgroup(cfg_and_lu, &id).await)
}

#[get("/config/subject_sources")]
fn config_subject_sources<'a>(cfg_and_lu : CfgAndLU<'a>) -> Json<&Vec<SubjectSourceConfig>> {
    Json(&cfg_and_lu.cfg.ldap.subject_sources)
}
#[get("/config/remotes")]
fn config_remotes<'a>(cfg_and_lu : CfgAndLU<'a>) -> Json<&BTreeMap<String, RemoteConfig>> {
    Json(&cfg_and_lu.cfg.remotes)
}

pub fn routes() -> Vec<Route> {
    routes![
        login,
        clear_test_data, add_test_data, set_test_data, 
        sgroup,
        config_subject_sources,
        config_remotes,
        create, delete, modify_members_or_rights,
    ]
}