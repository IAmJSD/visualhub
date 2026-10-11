//! People and projects, asked for as GitHub's users and organizations:
//! you, your keys and emails, the repositories you can see; anyone's
//! profile and own repositories; a project and its repositories and the
//! people with access to it.
//!
//! The instance keeps no follows, stars or activity feed, so those lists
//! are empty.

use super::{call, get, get_all, me, missing, page, shape, slice, user_by_login, Ask, API};
use crate::api;
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};

pub fn user(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let m = a.method;
    match rest {
        [] if m == "GET" => Ok(shape::user(c, &me(c)?)),
        [] => Err(missing(
            "way to edit a profile through its API; edit it on the site",
        )),
        ["repos"] if m == "GET" => my_repos(a),
        ["repos"] => create_repo(a, None),
        ["orgs"] => {
            let list = page(c, &format!("{API}/projects"), &a.query)?;
            Ok(Value::Array(
                list.iter().map(|p| shape::project(c, p)).collect(),
            ))
        }
        ["starred"]
        | ["subscriptions"]
        | ["repository_invitations"]
        | ["following"]
        | ["followers"]
        | ["blocks"]
        | ["social_accounts"] => Ok(json!([])),
        ["starred", ..] if m == "GET" => Err(anyhow::Error::new(api::Status {
            code: 404,
            message: "Not starred".into(),
        })),
        ["starred", ..] => Err(missing("stars")),
        ["following", ..] => Err(missing("following")),
        ["memberships", "orgs", key] => {
            let admin = get(
                c,
                &format!("{API}/projects/{}/permissions/PROJECT_ADMIN/all", enc(key)),
            )
            .map(|v| v.b("permitted"))
            .unwrap_or(false);
            Ok(json!({ "state": "active", "role": if admin { "admin" } else { "member" } }))
        }
        ["emails"] if m == "GET" => {
            let mine = me(c)?;
            Ok(
                json!([{ "email": mine.s("emailAddress"), "primary": true, "verified": true, "visibility": "private" }]),
            )
        }
        ["emails"] | ["email", ..] => Err(missing(
            "way to change emails through its API; an administrator changes them",
        )),
        ["keys"] if m == "GET" => {
            let slug = me(c)?.s("slug");
            let list = get_all(c, &format!("/rest/ssh/latest/keys?user={}", enc(&slug)), 4)?;
            Ok(Value::Array(
                list.iter()
                    .map(|k| json!({ "id": k.i("id"), "title": k.s("label"), "key": k.s("text"), "created_at": shape::when(k, "createdDate") }))
                    .collect(),
            ))
        }
        ["keys"] => call(
            c,
            "POST",
            "/rest/ssh/latest/keys",
            Some(&json!({ "text": a.field("key"), "label": a.field("title") })),
        ),
        ["keys", id] => call(c, "DELETE", &format!("/rest/ssh/latest/keys/{id}"), None),
        ["gpg_keys"] if m == "GET" => {
            let list = get_all(c, "/rest/gpg/latest/keys", 4)?;
            Ok(Value::Array(
                list.iter()
                    .map(|k| {
                        let emails: Vec<Value> = k
                            .at("emailAddress")
                            .as_str()
                            .map(|e| vec![json!({ "email": e })])
                            .unwrap_or_default();
                        json!({ "id": crate::forge::release_id(&k.s("id")), "name": "", "key_id": k.s("id"), "emails": emails, "created_at": "" })
                    })
                    .collect(),
            ))
        }
        ["gpg_keys"] => call(
            c,
            "POST",
            "/rest/gpg/latest/keys",
            Some(&json!({ "text": a.field("armored_public_key") })),
        ),
        ["gpg_keys", id] => {
            let key = get_all(c, "/rest/gpg/latest/keys", 4)?
                .into_iter()
                .find(|k| crate::forge::release_id(&k.s("id")).to_string() == *id)
                .ok_or_else(|| missing("such key"))?;
            call(
                c,
                "DELETE",
                &format!("/rest/gpg/latest/keys/{}", enc(&key.s("id"))),
                None,
            )
        }
        _ => Err(missing(&format!("equivalent of your {}", rest.join("/")))),
    }
}

/// The repositories you can see: the ones you opened lately first, then
/// the rest, as the instance names no order of its own.
fn my_repos(a: &Ask) -> Result<Value> {
    let c = a.c;
    let permission = if a.query.get("affiliation") == Some("owner") {
        "REPO_ADMIN"
    } else {
        "REPO_READ"
    };
    let (per, page_n) = (a.query.per_page(), a.query.page());
    let wanted = per * page_n;
    let mut all: Vec<Value> = Vec::new();
    if a.query.get("sort") != Some("full_name") {
        all = get_all(
            c,
            &format!("{API}/profile/recent/repos?permission={permission}"),
            1,
        )
        .unwrap_or_default();
    }
    let rest = super::values_at(
        c,
        &format!("{API}/repos?permission={permission}"),
        0,
        (wanted + all.len() as u64).min(1000),
    )?;
    for r in rest {
        if !all.iter().any(|x| x.i("id") == r.i("id")) {
            all.push(r);
        }
    }
    match a.query.get("type") {
        Some("private") => all.retain(|r| !r.b("public")),
        Some("public") => all.retain(|r| r.b("public")),
        _ => {}
    }
    Ok(slice(
        all.iter().map(|r| shape::repo(c, r, "", "")).collect(),
        &a.query,
    ))
}

pub fn create_repo(a: &Ask, project: Option<&str>) -> Result<Value> {
    let c = a.c;
    let name = a.field("name").as_str().unwrap_or("").to_string();
    let public = match a.field("visibility").as_str() {
        Some("public") => true,
        Some(_) => false,
        None => a.field("private").as_bool() == Some(false),
    };
    let body = json!({ "name": name, "scmId": "git", "public": public, "description": a.field("description") });
    let path = match project {
        Some(key) => format!("{API}/projects/{}/repos", enc(key)),
        None => format!("{API}/users/{}/repos", enc(&me(c)?.s("slug"))),
    };
    let made = call(c, "POST", &path, Some(&body))?;
    Ok(shape::repo(c, &made, "admin", ""))
}

/// Anyone, by login: a person, or failing that a project.
pub fn users(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let Some((login, tail)) = rest.split_first() else {
        return Err(missing("list of everyone"));
    };
    let person = match user_by_login(c, login) {
        Ok(u) => u,
        Err(e) if api::status_of(&e) == Some(404) => return orgs(a, rest),
        Err(e) => return Err(e),
    };
    match tail {
        [] => Ok(shape::user(c, &person)),
        ["repos"] => {
            let list = page(
                c,
                &format!("{API}/users/{}/repos", enc(&person.s("slug"))),
                &a.query,
            )?;
            Ok(shape::repos(c, &list))
        }
        _ => Ok(json!([])),
    }
}

/// A project, as an organization. A person's own project (`~NAME`) is
/// theirs.
pub fn orgs(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let Some((key, tail)) = rest.split_first() else {
        return Err(missing("list of every project"));
    };
    let p = format!("{API}/projects/{}", enc(key));
    match tail {
        [] if a.get() => {
            let found = get(c, &p).map_err(|e| match api::status_of(&e) {
                Some(404) => anyhow::Error::new(api::Status {
                    code: 404,
                    message: format!("This Bitbucket has no person or project called {key}."),
                }),
                _ => e,
            })?;
            Ok(shape::project(c, &found))
        }
        ["repos"] if a.get() => Ok(shape::repos(c, &page(c, &format!("{p}/repos"), &a.query)?)),
        ["repos"] => create_repo(a, Some(key)),
        ["members"] => {
            // Only a project's administrators may see who has access.
            let list = page(c, &format!("{p}/permissions/users"), &a.query).unwrap_or_default();
            let people: Vec<Value> = list.iter().map(|m| m.at("user").clone()).collect();
            Ok(shape::users(c, &people))
        }
        _ => Err(missing(
            "such page for a project (its own page on the site has it)",
        )),
    }
}
