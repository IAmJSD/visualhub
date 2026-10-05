//! People, workspaces and snippets, asked for as GitHub's users,
//! organizations and gists: you, your keys and emails, your repositories
//! across workspaces; anyone's profile; a workspace and its repositories
//! and members.
//!
//! Bitbucket keeps no follows, stars or activity feed, so those lists are
//! empty. Keys are named by UUID or fingerprint where GitHub numbers them;
//! they are numbered here by hashing that name, and found again by it.

use super::{
    call, get, get_all, me, missing, page_at, parallel, personal_workspace, repo_api, shape,
    user_by_login, workspace_slugs, workspaces, Ask,
};
use crate::api;
use crate::forge::release_id;
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
        ["orgs"] => Ok(Value::Array(
            workspaces(c)?
                .iter()
                .map(|w| shape::workspace(w.at("workspace")))
                .collect(),
        )),
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
        ["memberships", "orgs", ws] => {
            let found = workspaces(c)?
                .into_iter()
                .find(|w| w.s("workspace.slug") == *ws)
                .ok_or_else(|| {
                    anyhow::Error::new(api::Status {
                        code: 404,
                        message: format!("You aren't in the {ws} workspace."),
                    })
                })?;
            Ok(
                json!({ "state": "active", "role": if found.b("administrator") { "admin" } else { "member" } }),
            )
        }
        ["emails"] if m == "GET" => {
            let list = get_all(c, "/user/emails", 4)?;
            Ok(Value::Array(
                list.iter()
                    .map(|e| json!({ "email": e.s("email"), "primary": e.b("is_primary"), "verified": e.b("is_confirmed"), "visibility": "private" }))
                    .collect(),
            ))
        }
        ["emails"] | ["email", ..] => Err(missing(
            "way to change emails through its API; change them in your Atlassian account",
        )),
        ["keys"] => ssh_keys(a),
        ["keys", id] => {
            let base = format!("/users/{}/ssh-keys", enc(&me(c)?.s("uuid")));
            let uuid = get_all(c, &base, 10)?
                .iter()
                .find(|k| release_id(&k.s("uuid")).to_string() == *id)
                .map(|k| k.s("uuid"))
                .ok_or_else(|| missing("such key"))?;
            call(c, "DELETE", &format!("{base}/{}", enc(&uuid)), None)
        }
        ["gpg_keys"] => gpg_keys(a),
        ["gpg_keys", id] => {
            let base = format!("/users/{}/gpg-keys", enc(&me(c)?.s("uuid")));
            let fingerprint = get_all(c, &base, 10)?
                .iter()
                .find(|k| release_id(&k.s("fingerprint")).to_string() == *id)
                .map(|k| k.s("fingerprint"))
                .ok_or_else(|| missing("such key"))?;
            call(c, "DELETE", &format!("{base}/{}", enc(&fingerprint)), None)
        }
        _ => Err(missing(&format!("equivalent of your {}", rest.join("/")))),
    }
}

fn ssh_keys(a: &Ask) -> Result<Value> {
    let c = a.c;
    let base = format!("/users/{}/ssh-keys", enc(&me(c)?.s("uuid")));
    if a.get() {
        let list = get_all(c, &base, 10)?;
        return Ok(Value::Array(
            list.iter()
                .map(|k| json!({ "id": release_id(&k.s("uuid")), "title": k.s("label"), "key": k.s("key"), "created_at": k.s("created_on") }))
                .collect(),
        ));
    }
    call(
        c,
        "POST",
        &base,
        Some(&json!({ "key": a.field("key"), "label": a.field("title") })),
    )
}

fn gpg_keys(a: &Ask) -> Result<Value> {
    let c = a.c;
    let base = format!("/users/{}/gpg-keys", enc(&me(c)?.s("uuid")));
    if a.get() {
        let list = get_all(c, &base, 10)?;
        return Ok(Value::Array(
            list.iter()
                .map(|k| {
                    let name = if k.s("name").is_empty() {
                        format!("GPG key {}", k.s("key_id"))
                    } else {
                        k.s("name")
                    };
                    json!({ "id": release_id(&k.s("fingerprint")), "name": name, "key_id": k.s("key_id"), "emails": [], "created_at": k.s("added_on") })
                })
                .collect(),
        ));
    }
    call(
        c,
        "POST",
        &base,
        Some(&json!({ "key": a.field("armored_public_key"), "name": a.field("name") })),
    )
}

/// The repositories in every workspace you're in, newest activity first.
/// Bitbucket lists them a workspace at a time, so GitHub's page is made
/// from each workspace's first pages.
fn my_repos(a: &Ask) -> Result<Value> {
    let c = a.c;
    let sort = match a.query.get("sort") {
        Some("created") => "-created_on",
        Some("full_name") => "slug",
        _ => "-updated_on",
    };
    let role = if a.query.get("affiliation") == Some("owner") {
        "admin"
    } else {
        "member"
    };
    let (per, page) = (a.query.per_page(), a.query.page());
    let wanted = per * page;
    let slugs = workspace_slugs(c)?;
    let found = parallel(&slugs, |ws| {
        page_at(
            c,
            &format!("/repositories/{}?role={role}&sort={sort}", enc(ws)),
            wanted.min(100),
            1,
        )
        .unwrap_or_default()
    });
    let mut all: Vec<Value> = found.into_iter().flatten().collect();
    match a.query.get("type") {
        Some("private") => all.retain(|r| r.b("is_private")),
        Some("public") => all.retain(|r| !r.b("is_private")),
        _ => {}
    }
    match sort {
        "slug" => all.sort_by_key(|r| r.s("full_name").to_lowercase()),
        "-created_on" => all.sort_by_key(|r| std::cmp::Reverse(r.s("created_on"))),
        _ => all.sort_by_key(|r| std::cmp::Reverse(r.s("updated_on"))),
    }
    let shown: Vec<Value> = all
        .into_iter()
        .skip((per * (page - 1)) as usize)
        .take(per as usize)
        .collect();
    Ok(shape::repos(&shown))
}

/// A slug Bitbucket will take from a repository's name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn create_repo(a: &Ask, workspace: Option<&str>) -> Result<Value> {
    let c = a.c;
    let ws = match workspace {
        Some(ws) => ws.to_string(),
        None => personal_workspace(c)?,
    };
    let name = a.field("name").as_str().unwrap_or("").to_string();
    let private = match a.field("visibility").as_str() {
        Some("public") => false,
        Some(_) => true,
        None => a.field("private").as_bool().unwrap_or(true),
    };
    let body = json!({
        "scm": "git",
        "name": name,
        "description": a.field("description"),
        "is_private": private,
    });
    let made = call(
        c,
        "POST",
        &repo_api(&format!("{ws}/{}", slug(&name))),
        Some(&body),
    )?;
    Ok(shape::repo(&made, "admin"))
}

/// Anyone, by login: a person, or failing that a workspace.
pub fn users(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let Some((login, tail)) = rest.split_first() else {
        return Err(missing("list of everyone"));
    };
    let person = match user_by_login(c, login) {
        Ok(u) => u,
        Err(e) if api::status_of(&e) == Some(404) => return workspace_as_user(a, login, tail),
        Err(e) => return Err(e),
    };
    match tail {
        [] => Ok(shape::user(c, &person)),
        // A person's own repositories live in their personal workspace,
        // whose slug Bitbucket doesn't tell anyone else.
        ["repos"] if person.s("uuid") == me(c)?.s("uuid") => my_repos(a),
        _ => Ok(json!([])),
    }
}

fn workspace_as_user(a: &Ask, ws: &str, tail: &[&str]) -> Result<Value> {
    let c = a.c;
    let found =
        get(c, &format!("/workspaces/{}", enc(ws))).map_err(|e| match api::status_of(&e) {
            Some(404) => anyhow::Error::new(api::Status {
                code: 404,
                message: format!(
                    "Bitbucket has no person or workspace called {ws} that VisualHub can find."
                ),
            }),
            _ => e,
        })?;
    match tail {
        [] => Ok(shape::workspace(&found)),
        ["repos"] => orgs(a, &[ws, "repos"]),
        _ => Ok(json!([])),
    }
}

/// A workspace, as an organization.
pub fn orgs(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let Some((ws, tail)) = rest.split_first() else {
        return Err(missing("list of every workspace"));
    };
    let w = format!("/workspaces/{}", enc(ws));
    match tail {
        [] if a.get() => Ok(shape::workspace(&get(c, &w)?)),
        ["repos"] if a.get() => {
            let sort = match a.query.get("sort") {
                Some("full_name") => "slug",
                Some("created") => "-created_on",
                _ => "-updated_on",
            };
            let path = format!("/repositories/{}?sort={sort}", enc(ws));
            Ok(shape::repos(&super::page(c, &path, &a.query)?))
        }
        ["repos"] => create_repo(a, Some(ws)),
        ["members"] => {
            let members = super::page(c, &format!("{w}/members"), &a.query)?;
            let people: Vec<Value> = members.iter().map(|m| m.at("user").clone()).collect();
            Ok(shape::users(c, &people))
        }
        _ => Err(missing(
            "such page for a workspace (its own page on the site has it)",
        )),
    }
}

/// Snippets as gists. Their ids carry their workspace: `ws/abc12`.
pub fn gists(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let m = a.method;
    match rest {
        [] if m == "GET" => {
            let slugs = workspace_slugs(c)?;
            let found = parallel(&slugs, |ws| {
                get_all(c, &format!("/snippets/{}?role=owner", enc(ws)), 2).unwrap_or_default()
            });
            let mut all: Vec<Value> = found.into_iter().flatten().collect();
            all.sort_by_key(|s| std::cmp::Reverse(s.s("updated_on")));
            let (per, page) = (a.query.per_page() as usize, a.query.page() as usize);
            Ok(Value::Array(
                all.iter()
                    .skip(per * (page - 1))
                    .take(per)
                    .map(|s| shape::snippet(c, s, &[]))
                    .collect(),
            ))
        }
        [] => Err(missing(
            "way to create a snippet with JSON; create it on the site",
        )),
        ["public"] | ["starred"] => Ok(json!([])),
        [ws, id] => {
            let path = format!("/snippets/{}/{}", enc(ws), enc(id));
            match m {
                "GET" => {
                    let s = get(c, &path)?;
                    let names: Vec<String> = match s.at("files") {
                        Value::Object(map) => map.keys().cloned().collect(),
                        _ => Vec::new(),
                    };
                    let texts = parallel(&names, |name| {
                        c.raw_text(&format!("{path}/files/{}", enc(name)), "*/*")
                            .unwrap_or_default()
                    });
                    let contents: Vec<(String, String)> = names.into_iter().zip(texts).collect();
                    Ok(shape::snippet(c, &s, &contents))
                }
                "DELETE" => call(c, "DELETE", &path, None),
                _ => {
                    let updated = call(
                        c,
                        "PUT",
                        &path,
                        Some(&json!({ "title": a.field("description") })),
                    )?;
                    Ok(shape::snippet(c, &updated, &[]))
                }
            }
        }
        [ws, id, "comments"] => {
            let path = format!("/snippets/{}/{}/comments", enc(ws), enc(id));
            if a.get() {
                let list = get_all(c, &path, 5)?;
                return Ok(Value::Array(
                    list.iter()
                        .map(|cm| {
                            let url = format!("/gists/{ws}/{id}/comments/{}", cm.i("id"));
                            shape::comment(c, cm, &url)
                        })
                        .collect(),
                ));
            }
            call(
                c,
                "POST",
                &path,
                Some(&json!({ "content": { "raw": a.field("body") } })),
            )
        }
        [ws, id, "comments", cid] => {
            let path = format!("/snippets/{}/{}/comments/{cid}", enc(ws), enc(id));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => call(
                    c,
                    "PUT",
                    &path,
                    Some(&json!({ "content": { "raw": a.field("body") } })),
                ),
            }
        }
        [_, _, "star"] if m == "GET" => Err(anyhow::Error::new(api::Status {
            code: 404,
            message: "Not starred".into(),
        })),
        _ => Err(missing("stars or forks on snippets")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_become_slugs() {
        assert_eq!(super::slug("My New Repo!"), "my-new-repo");
        assert_eq!(super::slug("api_v2.rs"), "api_v2.rs");
    }
}
