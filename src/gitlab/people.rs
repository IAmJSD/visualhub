//! People, groups and snippets, asked for as GitHub's users,
//! organizations and gists: you and your keys, emails, stars and follows;
//! anyone's profile, projects and activity; a group and its projects.

use super::{
    call, get, get_all, grp, me, missing, parallel, project_path, shape, user_by_login, Ask, HOUR,
};
use crate::api::{self, Client};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn user(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let m = a.method;
    match rest {
        [] if m == "GET" => Ok(shape::user(c, &me(c)?)),
        [] => Err(missing(
            "way to edit a profile through its API; edit it on the site",
        )),
        ["repos"] if m == "GET" => {
            let order = match a.query.get("sort") {
                Some("created") => "created_at&sort=desc",
                Some("full_name") => "path&sort=asc",
                _ => "last_activity_at&sort=desc",
            };
            let mut path = format!("/projects?membership=true&order_by={order}&{}", a.paging());
            if a.query.get("affiliation") == Some("owner") {
                path.push_str("&owned=true");
            }
            if let Some(v @ ("public" | "private" | "internal")) = a.query.get("type") {
                path.push_str(&format!("&visibility={v}"))
            }
            Ok(shape::projects(c, get(c, &path)?.list("")))
        }
        ["repos"] => create_project(a, None),
        ["orgs"] => {
            let list = get(c, &format!("/groups?min_access_level=10&{}", a.paging()))?;
            Ok(Value::Array(
                list.list("").iter().map(|g| shape::group(c, g)).collect(),
            ))
        }
        ["starred"] => {
            let order = if a.query.get("sort") == Some("created") {
                "created_at"
            } else {
                "last_activity_at"
            };
            Ok(shape::projects(
                c,
                get(
                    c,
                    &format!(
                        "/projects?starred=true&order_by={order}&sort=desc&{}",
                        a.paging()
                    ),
                )?
                .list(""),
            ))
        }
        ["starred", repo @ ..] if m == "GET" => match super::repos::starred(c, &repo.join("/"))? {
            true => Ok(Value::Null),
            false => Err(anyhow::Error::new(api::Status {
                code: 404,
                message: "Not starred".into(),
            })),
        },
        ["starred", repo @ ..] => super::repos::star(c, &repo.join("/"), m != "DELETE"),
        ["subscriptions"] | ["repository_invitations"] => Ok(json!([])),
        ["following"] => {
            let id = me(c)?.i("id");
            Ok(shape::users(
                c,
                get(c, &format!("/users/{id}/following?{}", a.paging()))?.list(""),
            ))
        }
        ["following", login] => {
            let id = user_by_login(c, login)?.i("id");
            let reply = c.raw(
                "POST",
                &format!(
                    "/users/{id}/{}",
                    if m == "DELETE" { "unfollow" } else { "follow" }
                ),
                None,
                None,
            )?;
            // 304: already so.
            if reply.status >= 400 {
                return Err(api::failure(&reply));
            }
            Ok(Value::Null)
        }
        ["memberships", "orgs", group @ ..] => {
            let id = me(c)?.i("id");
            let member = get(c, &format!("{}/members/all/{id}", grp(&group.join("/"))))?;
            Ok(
                json!({ "state": "active", "role": if member.i("access_level") >= 50 { "admin" } else { "member" }, "access_level": member.i("access_level") }),
            )
        }
        ["emails"] => emails(a),
        ["keys"] => keys(a, "auth"),
        ["keys", id] | ["ssh_signing_keys", id] => {
            call(c, "DELETE", &format!("/user/keys/{id}"), None)
        }
        ["ssh_signing_keys"] => keys(a, "signing"),
        ["gpg_keys"] if m == "GET" => {
            let list = get(c, "/user/gpg_keys")?;
            Ok(Value::Array(
                list.list("")
                    .iter()
                    .map(|k| json!({ "id": k.i("id"), "name": format!("GPG key {}", k.i("id")), "key_id": "", "emails": [], "created_at": k.s("created_at") }))
                    .collect(),
            ))
        }
        ["gpg_keys"] => call(
            c,
            "POST",
            "/user/gpg_keys",
            Some(&json!({ "key": a.field("armored_public_key") })),
        ),
        ["gpg_keys", id] => call(c, "DELETE", &format!("/user/gpg_keys/{id}"), None),
        _ => Err(missing(&format!("equivalent of your {}", rest.join("/")))),
    }
}

fn emails(a: &Ask) -> Result<Value> {
    let c = a.c;
    let wanted: Vec<String> = a
        .field("emails")
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    match a.method {
        "GET" => {
            let primary = me(c)?.s("email");
            let list = get(c, "/user/emails")?;
            let mut out = vec![
                json!({ "email": primary, "verified": true, "primary": true, "visibility": "private" }),
            ];
            for e in list.list("") {
                if e.s("email") != primary {
                    out.push(json!({ "email": e.s("email"), "verified": e.has("confirmed_at"), "primary": false, "visibility": "" }));
                }
            }
            Ok(Value::Array(out))
        }
        "POST" => {
            for email in &wanted {
                call(c, "POST", "/user/emails", Some(&json!({ "email": email })))?;
            }
            Ok(Value::Null)
        }
        _ => {
            let list = get(c, "/user/emails")?;
            for e in list
                .list("")
                .iter()
                .filter(|e| wanted.contains(&e.s("email")))
            {
                call(c, "DELETE", &format!("/user/emails/{}", e.i("id")), None)?;
            }
            Ok(Value::Null)
        }
    }
}

/// SSH keys, for signing in (`auth`) or for signing commits (`signing`);
/// a key GitLab keeps for both shows under each.
fn keys(a: &Ask, usage: &str) -> Result<Value> {
    let c = a.c;
    if a.method == "POST" {
        let body = json!({ "title": a.field("title"), "key": a.field("key"), "usage_type": if usage == "signing" { "signing" } else { "auth_and_signing" } });
        return call(c, "POST", "/user/keys", Some(&body));
    }
    let list = get(c, "/user/keys")?;
    Ok(Value::Array(
        list.list("")
            .iter()
            .filter(|k| {
                let kind = k.s("usage_type");
                kind.is_empty() || kind == "auth_and_signing" || kind == usage
            })
            .map(|k| json!({ "id": k.i("id"), "title": k.s("title"), "key": k.s("key"), "created_at": k.s("created_at"), "expires_at": k.s("expires_at") }))
            .collect(),
    ))
}

/// A new project, yours or a group's. GitLab makes no .gitignore or
/// licence when it makes a project, so those are committed after.
pub fn create_project(a: &Ask, group: Option<&str>) -> Result<Value> {
    let c = a.c;
    let name = a.field("name").as_str().unwrap_or("").trim().to_string();
    let visibility = match a.field("visibility").as_str() {
        Some(v @ ("public" | "private" | "internal")) => v.to_string(),
        _ if a.field("private").as_bool() == Some(true) => "private".into(),
        _ => "public".into(),
    };
    let mut body = json!({
        "name": name,
        "description": a.field("description"),
        "visibility": visibility,
        "initialize_with_readme": a.field("auto_init").as_bool().unwrap_or(false),
    });
    if let Some(group) = group {
        body["namespace_id"] = json!(get(c, &grp(group))?.i("id"));
    }
    let project = call(c, "POST", "/projects", Some(&body))?;
    let mut actions = Vec::new();
    if let Some(key) = a
        .field("gitignore_template")
        .as_str()
        .filter(|k| !k.is_empty())
    {
        let template = get(c, &format!("/templates/gitignores/{}", enc(key)))?;
        actions.push(json!({ "action": "create", "file_path": ".gitignore", "content": template.s("content") }));
    }
    if let Some(key) = a
        .field("license_template")
        .as_str()
        .filter(|k| !k.is_empty())
    {
        let template = get(
            c,
            &format!(
                "/templates/licenses/{}?project={}&fullname={}",
                enc(key),
                enc(&name),
                enc(&me(c)?.s("name"))
            ),
        )?;
        actions.push(
            json!({ "action": "create", "file_path": "LICENSE", "content": template.s("content") }),
        );
    }
    if !actions.is_empty() {
        let branch = if project.s("default_branch").is_empty() {
            "main".to_string()
        } else {
            project.s("default_branch")
        };
        let commit = json!({ "branch": branch, "commit_message": "Add .gitignore and licence", "actions": actions });
        call(
            c,
            "POST",
            &format!("/projects/{}/repository/commits", project.i("id")),
            Some(&commit),
        )?;
    }
    Ok(shape::project(
        c,
        &get(c, &format!("/projects/{}", project.i("id")))?,
    ))
}

/// What can follow a user's login in a path.
const USER_PARTS: &[&str] = &[
    "repos",
    "starred",
    "followers",
    "following",
    "orgs",
    "events",
    "received_events",
    "gists",
];

pub fn users(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let at = (1..rest.len())
        .find(|&i| USER_PARTS.contains(&rest[i]))
        .unwrap_or(rest.len());
    let login = rest[..at].join("/");
    let tail = &rest[at..];
    if login.contains('/') {
        // A subgroup: only groups nest.
        return group_as_user(a, &login, tail);
    }
    let found = match user_by_login(c, &login) {
        Ok(u) => u,
        Err(e) if api::status_of(&e) == Some(404) => return group_as_user(a, &login, tail),
        Err(e) => return Err(e),
    };
    let id = found.i("id");
    match tail {
        [] => {
            let full = c.memo.get_or(&format!("user-full:{id}"), HOUR, || {
                get(c, &format!("/users/{id}"))
            })?;
            Ok(shape::user(c, &full))
        }
        ["repos"] => Ok(shape::projects(
            c,
            get(
                c,
                &format!(
                    "/users/{id}/projects?order_by=last_activity_at&sort=desc&{}",
                    a.paging()
                ),
            )?
            .list(""),
        )),
        ["starred"] => Ok(shape::projects(
            c,
            get(c, &format!("/users/{id}/starred_projects?{}", a.paging()))?.list(""),
        )),
        ["followers"] | ["following"] => Ok(shape::users(
            c,
            get(c, &format!("/users/{id}/{}?{}", tail[0], a.paging()))?.list(""),
        )),
        ["orgs"] => {
            // GitLab lists only your own groups.
            if me(c)?.i("id") != id {
                return Ok(json!([]));
            }
            let list = get(
                c,
                "/groups?min_access_level=10&top_level_only=true&per_page=100",
            )?;
            Ok(Value::Array(
                list.list("").iter().map(|g| shape::group(c, g)).collect(),
            ))
        }
        ["events", ..] => events(c, &get(c, &format!("/users/{id}/events?{}", a.paging()))?),
        ["received_events"] => events(c, &get(c, &format!("/events?scope=all&{}", a.paging()))?),
        ["gists"] => {
            if me(c)?.i("id") != id {
                return Ok(json!([]));
            }
            gists(a, &[])
        }
        _ => Err(missing("such list for a user")),
    }
}

/// A user's paths asked about a group: GitLab's addresses don't say which
/// a name is.
fn group_as_user(a: &Ask, group: &str, tail: &[&str]) -> Result<Value> {
    let c = a.c;
    match tail {
        [] => Ok(shape::group(c, &get(c, &grp(group))?)),
        ["repos"] => orgs(a, &[group, "repos"]),
        _ => Ok(json!([])),
    }
}

/// Events as GitHub's, with their projects' paths looked up.
fn events(c: &Client, list: &Value) -> Result<Value> {
    let mut ids: Vec<i64> = list
        .list("")
        .iter()
        .map(|e| e.i("project_id"))
        .filter(|id| *id > 0)
        .collect();
    ids.sort();
    ids.dedup();
    let paths: HashMap<i64, String> = ids
        .iter()
        .copied()
        .zip(parallel(&ids, |id| project_path(c, *id)))
        .collect();
    Ok(Value::Array(
        list.list("")
            .iter()
            .filter_map(|e| {
                shape::event(
                    c,
                    e,
                    paths
                        .get(&e.i("project_id"))
                        .map(String::as_str)
                        .unwrap_or(""),
                )
            })
            .collect(),
    ))
}

/// What can follow a group's path.
const GROUP_PARTS: &[&str] = &[
    "repos",
    "members",
    "teams",
    "invitations",
    "hooks",
    "packages",
    "blocks",
    "outside_collaborators",
    "actions",
    "memberships",
    "public_members",
];

pub fn orgs(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let at = (1..rest.len())
        .find(|&i| GROUP_PARTS.contains(&rest[i]))
        .unwrap_or(rest.len());
    let group = rest[..at].join("/");
    let g = grp(&group);
    match &rest[at..] {
        [] if a.get() => Ok(shape::group(c, &get(c, &g)?)),
        [] => {
            let mut body = json!({});
            for key in ["name", "description"] {
                if a.field(key).is_string() {
                    body[key] = a.field(key).clone();
                }
            }
            Ok(shape::group(c, &call(c, "PUT", &g, Some(&body))?))
        }
        ["repos"] if a.get() => {
            let order = if a.query.get("sort") == Some("full_name") {
                "path&sort=asc"
            } else {
                "last_activity_at&sort=desc"
            };
            let nested = a.query.get("include_subgroups").unwrap_or("true");
            Ok(shape::projects(
                c,
                get(
                    c,
                    &format!(
                        "{g}/projects?include_subgroups={nested}&order_by={order}&{}",
                        a.paging()
                    ),
                )?
                .list(""),
            ))
        }
        ["repos"] => create_project(a, Some(&group)),
        ["members"] => Ok(shape::users(
            c,
            get(c, &format!("{g}/members/all?{}", a.paging()))?.list(""),
        )),
        _ => Err(missing("such page for a group (its own pages have it)")),
    }
}

/// The ref a snippet's files live at, from one of their raw URLs
/// (`…/-/snippets/12/raw/main/a.rb`).
fn snippet_ref(raw_url: &str) -> String {
    raw_url
        .split("/raw/")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("main")
        .to_string()
}

pub fn gists(a: &Ask, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let m = a.method;
    let none = HashMap::new();
    match rest {
        [] if m == "GET" => Ok(Value::Array(
            get(c, &format!("/snippets?{}", a.paging()))?
                .list("")
                .iter()
                .map(|s| shape::snippet(c, s, &none))
                .collect(),
        )),
        [] => {
            let mut files = Vec::new();
            let mut first = String::new();
            if let Value::Object(map) = a.field("files") {
                for (name, f) in map {
                    if first.is_empty() {
                        first = name.clone();
                    }
                    files.push(json!({ "file_path": name, "content": f.s("content") }));
                }
            }
            let title = a
                .field("description")
                .as_str()
                .filter(|d| !d.trim().is_empty())
                .map(str::to_string)
                .unwrap_or(first);
            let body = json!({
                "title": title,
                "visibility": if a.field("public").as_bool().unwrap_or(false) { "public" } else { "private" },
                "files": files,
            });
            Ok(shape::snippet(
                c,
                &call(c, "POST", "/snippets", Some(&body))?,
                &none,
            ))
        }
        ["public"] => Ok(Value::Array(
            get(c, &format!("/snippets/public?{}", a.paging()))?
                .list("")
                .iter()
                .map(|s| shape::snippet(c, s, &none))
                .collect(),
        )),
        ["starred"] => Ok(json!([])),
        [id] if m == "GET" => {
            let s = get(c, &format!("/snippets/{id}"))?;
            let files: Vec<(String, String)> = if s.list("files").is_empty() {
                vec![(s.s("file_name"), s.s("raw_url"))]
            } else {
                s.list("files")
                    .iter()
                    .map(|f| (f.s("path"), f.s("raw_url")))
                    .collect()
            };
            let texts = parallel(&files, |(name, raw)| {
                let path = format!(
                    "/snippets/{id}/files/{}/{}/raw",
                    enc(&snippet_ref(raw)),
                    enc(name)
                );
                c.raw_text(&path, "*/*")
                    .or_else(|_| c.raw_text(&format!("/snippets/{id}/raw"), "*/*"))
                    .unwrap_or_default()
            });
            let contents: HashMap<String, String> =
                files.into_iter().map(|(n, _)| n).zip(texts).collect();
            Ok(shape::snippet(c, &s, &contents))
        }
        [id] if m == "DELETE" => call(c, "DELETE", &format!("/snippets/{id}"), None),
        [id] => Ok(shape::snippet(
            c,
            &call(
                c,
                "PUT",
                &format!("/snippets/{id}"),
                Some(&json!({ "title": a.field("description") })),
            )?,
            &none,
        )),
        [_, "comments"] if m == "GET" => Ok(json!([])),
        _ => Err(missing("stars, forks or comments on personal snippets")),
    }
}

/// Whether you follow `login`.
pub fn following(c: &Client, login: &str) -> Result<bool> {
    let id = me(c)?.i("id");
    let list = get_all(c, &format!("/users/{id}/following"), 10)?;
    Ok(list
        .iter()
        .any(|u| u.s("username").eq_ignore_ascii_case(login)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn snippet_refs_come_from_raw_urls() {
        assert_eq!(
            super::snippet_ref("https://gitlab.com/-/snippets/12/raw/main/a.rb"),
            "main"
        );
        assert_eq!(super::snippet_ref("https://x/-/snippets/1/raw"), "main");
    }
}
