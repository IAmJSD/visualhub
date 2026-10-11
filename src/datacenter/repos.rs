//! A repository's pages, asked for as GitHub's repository paths: the
//! repository itself, its files, branches and tags, commits and
//! comparisons, build statuses as checks, forks, people with access,
//! webhooks and deploy keys.

use super::{
    call, get, get_all, me, missing, page, repo_api, repo_api_at, shape, slice, split,
    user_by_login, with_avatars, Ask, API,
};
use crate::api::{self, Client};
use crate::gitlab::MINUTE;
use crate::json::{enc, enc_path, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn route(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let r = repo_api(repo);
    let m = a.method;
    match rest {
        [] => repository(a, repo),
        ["issues", ..] | ["pulls", ..] | ["pullrequests", ..] => {
            super::pulls::route(a, repo, rest)
        }
        ["assignees"] => {
            let people = get_all(c, &format!("{r}/participants"), 2).unwrap_or_default();
            Ok(shape::users(c, &people))
        }
        ["labels", ..] if a.get() => Ok(json!([])),
        ["labels", ..] => Err(missing("labels")),
        ["milestones", ..] if a.get() => Ok(json!([])),
        ["milestones", ..] => Err(missing("milestones")),
        ["releases", ..] if a.get() && rest.len() == 1 => Ok(json!([])),
        ["releases", ..] => Err(missing("releases; tags stand in for them")),
        ["topics"] | ["transfer"] | ["generate"] => Err(missing(&format!(
            "way to change a repository's {} through its API",
            rest[0]
        ))),
        ["contents", path @ ..] => contents(a, repo, &path.join("/")),
        ["branches"] => {
            let list = page(
                c,
                &format!("{r}/branches?orderBy=ALPHABETICAL&details=true"),
                &a.query,
            )?;
            Ok(Value::Array(list.iter().map(|b| branch(repo, b)).collect()))
        }
        ["branches", .., "protection"] => Err(missing(
            "protection rules like GitHub's; its branch permissions are in the repository's settings",
        )),
        ["branches", .., "rename"] => Err(missing("way to rename a branch")),
        ["branches", name @ ..] => Ok(branch(repo, &find_ref(c, repo, "branches", &name.join("/"))?)),
        ["tags"] => {
            let list = page(c, &format!("{r}/tags?orderBy=MODIFICATION"), &a.query)?;
            Ok(Value::Array(
                list.iter()
                    .map(|t| {
                        let name = t.s("displayId");
                        let sha = t.s("latestCommit");
                        json!({
                            "name": name,
                            "commit": { "sha": sha, "url": format!("/repos/{repo}/commits/{sha}") },
                            "zipball_url": archive(c, repo, &name, "zip"),
                            "tarball_url": archive(c, repo, &name, "tar.gz"),
                        })
                    })
                    .collect(),
            ))
        }
        ["git", "refs"] => {
            let full = a.field("ref").as_str().unwrap_or("");
            let sha = a.field("sha").as_str().unwrap_or("");
            if let Some(name) = full.strip_prefix("refs/tags/") {
                let made = call(
                    c,
                    "POST",
                    &format!("{r}/tags"),
                    Some(&json!({ "name": name, "startPoint": sha })),
                )?;
                return Ok(json!({ "ref": full, "object": { "sha": made.s("latestCommit") } }));
            }
            let name = full.trim_start_matches("refs/heads/");
            let made = call(
                c,
                "POST",
                &format!("{}/branches", repo_api_at("branch-utils", repo)),
                Some(&json!({ "name": name, "startPoint": sha })),
            )?;
            Ok(json!({ "ref": full, "object": { "sha": made.s("latestCommit") } }))
        }
        ["git", "refs", "heads", name @ ..] => {
            let name = name.join("/");
            match m {
                "DELETE" => call(
                    c,
                    "DELETE",
                    &format!("{}/branches", repo_api_at("branch-utils", repo)),
                    Some(&json!({ "name": format!("refs/heads/{name}") })),
                ),
                _ => {
                    let b = find_ref(c, repo, "branches", &name)?;
                    Ok(json!({ "ref": format!("refs/heads/{name}"), "object": { "sha": b.s("latestCommit") } }))
                }
            }
        }
        ["git", "refs", "tags", name @ ..] => {
            let name = name.join("/");
            match m {
                "DELETE" => call(
                    c,
                    "DELETE",
                    &format!("{}/tags/{}", repo_api_at("git", repo), enc(&name)),
                    None,
                ),
                _ => {
                    let t = find_ref(c, repo, "tags", &name)?;
                    Ok(json!({ "ref": format!("refs/tags/{name}"), "object": { "sha": t.s("latestCommit") } }))
                }
            }
        }
        ["git", "trees", sha] => tree(c, repo, sha),
        ["commits"] => commits(a, repo),
        ["commits", sha] => commit(c, repo, sha),
        ["commits", sha, "comments"] => commit_comments(a, repo, sha),
        ["commits", sha, "comments", id] => {
            let path = format!("{r}/commits/{}/comments/{id}", enc(sha));
            let version = get(c, &path)?.i("version");
            match m {
                "DELETE" => call(c, "DELETE", &format!("{path}?version={version}"), None),
                _ => call(
                    c,
                    "PUT",
                    &path,
                    Some(&json!({ "text": a.field("body"), "version": version })),
                ),
            }
        }
        ["commits", sha, "check-runs"] => {
            let runs: Vec<Value> = statuses(c, sha)?
                .iter()
                .enumerate()
                .map(|(i, s)| shape::check_run(s, i))
                .collect();
            Ok(json!({ "total_count": runs.len(), "check_runs": runs }))
        }
        ["commits", sha, "status"] => combined_status(c, sha),
        ["commits", sha, "statuses"] => Ok(Value::Array(
            statuses(c, sha)?.iter().map(status).collect(),
        )),
        ["check-runs", _, "annotations"] => Ok(json!([])),
        // Branch names may hold slashes, which split the range.
        ["compare", range @ ..] => compare(c, repo, &range.join("/")),
        ["languages"] => Ok(json!({})),
        ["contributors"] => contributors(a, repo),
        ["forks"] if a.get() => Ok(shape::repos(c, &page(c, &format!("{r}/forks"), &a.query)?)),
        ["forks"] => {
            let key = match a.field("organization").as_str() {
                Some(org) if !org.is_empty() => org.to_string(),
                _ => format!("~{}", me(c)?.s("slug").to_uppercase()),
            };
            let mut body = json!({ "project": { "key": key } });
            if let Some(name) = a.field("name").as_str().filter(|n| !n.is_empty()) {
                body["name"] = json!(name);
            }
            Ok(shape::repo(c, &call(c, "POST", &r, Some(&body))?, "admin", ""))
        }
        ["stargazers"] => Ok(json!([])),
        ["subscribers"] => Ok(json!([])),
        ["subscription"] => match m {
            // Watching is the signed-in user's; it can be set, not read.
            "GET" => Ok(json!({ "subscribed": false, "ignored": false })),
            "DELETE" => call(c, "DELETE", &format!("{r}/watch"), None),
            _ => call(c, "POST", &format!("{r}/watch"), None),
        },
        ["collaborators"] => {
            let list = page(c, &format!("{r}/permissions/users"), &a.query)?;
            Ok(Value::Array(
                list.iter()
                    .map(|p| {
                        let mut u = shape::user(c, p.at("user"));
                        let perm = p.s("permission");
                        let (admin, push) = (perm == "REPO_ADMIN", perm != "REPO_READ");
                        u["permissions"] = json!({ "admin": admin, "maintain": admin, "push": push, "triage": true, "pull": true });
                        u["role_name"] = json!(perm.trim_start_matches("REPO_").to_lowercase());
                        u
                    })
                    .collect(),
            ))
        }
        ["collaborators", login] => {
            let name = user_by_login(c, login)?.s("name");
            let path = format!("{r}/permissions/users?name={}", enc(&name));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                "GET" => {
                    let found = get_all(c, &format!("{r}/permissions/users?filter={}", enc(&name)), 1)?;
                    found
                        .into_iter()
                        .find(|p| p.s("user.name") == name)
                        .ok_or_else(|| missing("such collaborator"))
                }
                _ => {
                    let permission = match a.field("permission").as_str().unwrap_or("push") {
                        "pull" | "read" | "triage" => "REPO_READ",
                        "admin" | "maintain" => "REPO_ADMIN",
                        _ => "REPO_WRITE",
                    };
                    call(c, "PUT", &format!("{path}&permission={permission}"), None)
                }
            }
        }
        ["keys"] if a.get() => {
            let list = get_all(c, &format!("{}/ssh", repo_api_at("keys", repo)), 4)?;
            Ok(Value::Array(list.iter().map(deploy_key).collect()))
        }
        ["keys"] => {
            let permission = if a.field("read_only").as_bool() == Some(false) {
                "REPO_WRITE"
            } else {
                "REPO_READ"
            };
            Ok(deploy_key(&call(
                c,
                "POST",
                &format!("{}/ssh", repo_api_at("keys", repo)),
                Some(&json!({ "key": { "text": a.field("key"), "label": a.field("title") }, "permission": permission })),
            )?))
        }
        ["keys", id] => call(
            c,
            "DELETE",
            &format!("{}/ssh/{id}", repo_api_at("keys", repo)),
            None,
        ),
        ["hooks"] if a.get() => {
            let list = get_all(c, &format!("{r}/webhooks"), 2)?;
            Ok(Value::Array(list.iter().map(hook).collect()))
        }
        ["hooks"] => Ok(hook(&call(
            c,
            "POST",
            &format!("{r}/webhooks"),
            Some(&hook_body(a, None)),
        )?)),
        ["hooks", id] => {
            let path = format!("{r}/webhooks/{id}");
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                "GET" => Ok(hook(&get(c, &path)?)),
                _ => {
                    let old = get(c, &path)?;
                    Ok(hook(&call(c, "PUT", &path, Some(&hook_body(a, Some(&old))))?))
                }
            }
        }
        ["hooks", id, "pings"] | ["hooks", id, "tests"] => {
            let old = get(c, &format!("{r}/webhooks/{id}"))?;
            call(
                c,
                "POST",
                &format!("{r}/webhooks/test?url={}", enc(&old.s("url"))),
                None,
            )
        }
        ["actions", ..] => Err(missing(
            "pipelines of its own; builds report their statuses on commits",
        )),
        _ => Err(missing(&format!(
            "equivalent of a repository's {}",
            rest.join("/")
        ))),
    }
}

/// A source archive's address on the site.
pub fn archive(c: &Client, repo: &str, git_ref: &str, format: &str) -> String {
    let format = if format == "tar.gz" { "tar.gz" } else { "zip" };
    format!(
        "{}{}/archive?at={}&format={format}",
        c.web,
        repo_api(repo),
        enc(git_ref)
    )
}

fn deploy_key(k: &Value) -> Value {
    json!({
        "id": k.i("key.id"),
        "title": k.s("key.label"),
        "key": k.s("key.text"),
        "read_only": k.s("permission") != "REPO_WRITE",
        "created_at": "",
        "last_used": Value::Null,
    })
}

/// A webhook in GitHub's shape: the events it hears, where it sends them.
fn hook(h: &Value) -> Value {
    json!({
        "id": h.i("id"),
        "name": h.s("name"),
        "active": h.b("active"),
        "events": h.at("events").clone(),
        "config": { "url": h.s("url"), "content_type": "json", "insecure_ssl": if h.b("sslVerificationRequired") { "0" } else { "1" } },
        "created_at": shape::when(h, "createdDate"),
        "updated_at": shape::when(h, "updatedDate"),
    })
}

/// The events a new webhook hears unless told otherwise.
const HOOK_EVENTS: [&str; 6] = [
    "repo:refs_changed",
    "pr:opened",
    "pr:modified",
    "pr:merged",
    "pr:declined",
    "pr:comment:added",
];

/// A webhook's body from GitHub's request, keeping what `old` had for
/// what the request leaves out.
fn hook_body(a: &Ask, old: Option<&Value>) -> Value {
    let url = a
        .field("config.url")
        .as_str()
        .map(str::to_string)
        .or_else(|| old.map(|o| o.s("url")))
        .unwrap_or_default();
    let events = match a.field("events") {
        Value::Array(list) if !list.is_empty() => Value::Array(list.clone()),
        _ => old
            .map(|o| o.at("events").clone())
            .unwrap_or_else(|| json!(HOOK_EVENTS)),
    };
    let name = old
        .map(|o| o.s("name"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| url.split('/').nth(2).unwrap_or("Webhook").to_string());
    let mut body = json!({
        "name": name,
        "url": url,
        "events": events,
        "active": a.field("active").as_bool().unwrap_or(true),
    });
    if let Some(secret) = a.field("config.secret").as_str().filter(|s| !s.is_empty()) {
        body["configuration"] = json!({ "secret": secret });
    }
    body
}

/// The repository, with what the signed-in user may do to it and its
/// default branch.
fn repository(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let r = repo_api(repo);
    match a.method {
        "GET" => {
            let info = get(c, &r)?;
            Ok(shape::repo(
                c,
                &info,
                &permission(c, repo),
                &default_branch(c, repo),
            ))
        }
        "DELETE" => call(c, "DELETE", &r, None),
        _ => {
            if let Some(branch) = a.field("default_branch").as_str() {
                call(
                    c,
                    "PUT",
                    &format!("{r}/default-branch"),
                    Some(&json!({ "id": format!("refs/heads/{branch}") })),
                )?;
                c.memo.forget(&format!("default:{repo}"));
            }
            let mut body = json!({});
            for field in ["name", "description"] {
                if a.field(field).is_string() {
                    body[field] = a.field(field).clone();
                }
            }
            if let Some(private) = a.field("private").as_bool() {
                body["public"] = json!(!private);
            }
            match a.field("visibility").as_str() {
                Some("private") => body["public"] = json!(false),
                Some("public") => body["public"] = json!(true),
                _ => {}
            }
            if let Some(archived) = a.field("archived").as_bool() {
                body["archived"] = json!(archived);
            }
            let updated = if body.as_object().is_some_and(|m| !m.is_empty()) {
                call(c, "PUT", &r, Some(&body))?
            } else {
                get(c, &r)?
            };
            Ok(shape::repo(
                c,
                &updated,
                &permission(c, repo),
                &default_branch(c, repo),
            ))
        }
    }
}

/// The repository's default branch's name, or nothing when it has none
/// yet (a repository nothing has been pushed to).
pub fn default_branch(c: &Client, repo: &str) -> String {
    c.memo
        .get_or(&format!("default:{repo}"), MINUTE, || {
            Ok(json!(get(c, &format!("{}/default-branch", repo_api(repo)))
                .map(|b| b.s("displayId"))
                .unwrap_or_default()))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

/// `read`, `write` or `admin`: what the signed-in user may do here.
fn permission(c: &Client, repo: &str) -> String {
    let (key, slug) = split(repo);
    c.memo
        .get_or(&format!("permission:{repo}"), MINUTE, || {
            let has = |level: &str| {
                get(
                    c,
                    &format!(
                        "{API}/repos?projectkey={}&name={}&permission={level}&limit=100",
                        enc(key),
                        enc(slug)
                    ),
                )
                .map(|list| list.list("values").iter().any(|r| r.s("slug") == slug))
                .unwrap_or(false)
            };
            let level = if has("REPO_ADMIN") {
                "admin"
            } else if has("REPO_WRITE") {
                "write"
            } else {
                "read"
            };
            Ok(json!(level))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

/// A branch or tag by its exact name. The lists filter by a part of it.
fn find_ref(c: &Client, repo: &str, kind: &str, name: &str) -> Result<Value> {
    let list = get_all(
        c,
        &format!(
            "{}/{kind}?filterText={}&details=true",
            repo_api(repo),
            enc(name)
        ),
        2,
    )?;
    list.into_iter()
        .find(|b| b.s("displayId") == name)
        .ok_or_else(|| {
            anyhow::Error::new(api::Status {
                code: 404,
                message: format!("No {} called {name}.", kind.trim_end_matches('s')),
            })
        })
}

fn branch(repo: &str, b: &Value) -> Value {
    let sha = b.s("latestCommit");
    let meta = b.at(
        "metadata.com\\.atlassian\\.bitbucket\\.server\\.bitbucket-branch:latest-commit-metadata",
    );
    let meta = if meta.is_null() {
        b.at("metadata")
            .as_object()
            .and_then(|m| {
                m.iter()
                    .find(|(k, _)| k.ends_with(":latest-commit-metadata"))
                    .map(|(_, v)| v)
            })
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        meta.clone()
    };
    json!({
        "name": b.s("displayId"),
        "commit": {
            "sha": sha,
            "url": format!("/repos/{repo}/commits/{sha}"),
            "commit": { "message": meta.s("message"), "author": { "date": shape::when(&meta, "authorTimestamp") } },
        },
        "protected": false,
        "default": b.b("isDefault"),
    })
}

/// A ref the instance takes as `at=`: a branch or tag name as it is (it
/// works names out itself), or the default branch for none.
fn at(git_ref: &str) -> String {
    if git_ref.is_empty() {
        String::new()
    } else {
        format!("at={}", enc(git_ref))
    }
}

fn web_path(c: &Client, repo: &str, git_ref: &str, path: &str) -> String {
    let base = crate::forge::datacenter_repo_web(repo, &c.web);
    let at = at(git_ref);
    format!(
        "{base}/browse/{}{}{at}",
        enc_path(path),
        if at.is_empty() { "" } else { "?" }
    )
}

fn raw_path(repo: &str, git_ref: &str, path: &str) -> String {
    let at = at(git_ref);
    format!(
        "{}/raw/{}{}{at}",
        repo_api(repo),
        enc_path(path.trim_matches('/')),
        if at.is_empty() { "" } else { "?" }
    )
}

fn entry(c: &Client, repo: &str, git_ref: &str, dir: &str, e: &Value) -> Value {
    let name = e.s("path.toString");
    let path = if dir.is_empty() {
        name.clone()
    } else {
        format!("{dir}/{name}")
    };
    let kind = match e.s("type").as_str() {
        "DIRECTORY" => "dir",
        "SUBMODULE" => "submodule",
        _ => "file",
    };
    json!({
        "name": name.rsplit('/').next().unwrap_or(""),
        "path": path,
        "type": kind,
        "size": e.i("size"),
        "sha": e.s("contentId"),
        "html_url": web_path(c, repo, git_ref, &path),
        "download_url": format!("{}{}", c.web, raw_path(repo, git_ref, &path)),
        "url": format!("/repos/{repo}/contents/{path}?ref={}", enc(git_ref)),
    })
}

/// A directory's entries, in GitHub's contents shape.
fn listing(c: &Client, repo: &str, git_ref: &str, dir: &str) -> Result<Vec<Value>> {
    let dir = dir.trim_matches('/');
    let at = at(git_ref);
    let mut start = 0;
    let mut entries = Vec::new();
    for _ in 0..20 {
        let path = format!(
            "{}/browse/{}?start={start}&limit=500{}{at}",
            repo_api(repo),
            enc_path(dir),
            if at.is_empty() { "" } else { "&" }
        );
        let v = get(c, &path)?;
        let children = v.at("children");
        if !children.is_object() {
            // A file, not a directory.
            return Err(anyhow::Error::new(api::Status {
                code: 404,
                message: format!("{dir} isn't a directory."),
            }));
        }
        entries.extend(
            children
                .list("values")
                .iter()
                .map(|e| entry(c, repo, git_ref, dir, e)),
        );
        if children.b("isLastPage") || !children.has("nextPageStart") {
            break;
        }
        start = children.i("nextPageStart") as u64;
    }
    Ok(entries)
}

/// What sits at a path: `FILE`, `DIRECTORY` or `SUBMODULE`.
fn kind_at(c: &Client, repo: &str, git_ref: &str, path: &str) -> Result<String> {
    let at = at(git_ref);
    let v = get(
        c,
        &format!(
            "{}/browse/{}?type=true{}{at}",
            repo_api(repo),
            enc_path(path),
            if at.is_empty() { "" } else { "&" }
        ),
    )?;
    Ok(v.s("type"))
}

fn raw(c: &Client, repo: &str, git_ref: &str, path: &str) -> Result<String> {
    let reply = c.raw("GET", &raw_path(repo, git_ref, path), None, Some("*/*"))?;
    if reply.status >= 400 {
        return Err(api::failure(&reply));
    }
    Ok(reply.body)
}

/// A directory's entries, or a file with its contents.
fn contents(a: &Ask, repo: &str, path: &str) -> Result<Value> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("").to_string();
    let path = path.trim_matches('/');
    if path.is_empty() || kind_at(c, repo, &git_ref, path)? == "DIRECTORY" {
        return Ok(Value::Array(listing(c, repo, &git_ref, path)?));
    }
    let body = raw(c, repo, &git_ref, path)?;
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let mut file = entry(
        c,
        repo,
        &git_ref,
        dir,
        &json!({ "path": { "toString": name }, "type": "FILE", "size": body.len() }),
    );
    use base64::Engine as _;
    file["encoding"] = json!("base64");
    file["content"] = json!(base64::engine::general_purpose::STANDARD.encode(body.as_bytes()));
    Ok(file)
}

/// Raw text: a file, or a directory's README.
pub fn text(a: &Ask, repo: &str, rest: &[&str]) -> Result<String> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("").to_string();
    let path = match rest {
        ["contents", path @ ..] => path.join("/"),
        ["readme", dir @ ..] => {
            let entries = listing(c, repo, &git_ref, &dir.join("/"))?;
            entries
                .iter()
                .filter(|e| e.s("type") == "file")
                .map(|e| e.s("path"))
                .filter(|p| {
                    p.rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_lowercase()
                        .starts_with("readme")
                })
                // README.md before README.txt before README.
                .min_by_key(|p| {
                    let lower = p.to_lowercase();
                    (
                        !(lower.ends_with(".md") || lower.ends_with(".markdown")),
                        lower.len(),
                    )
                })
                .ok_or_else(|| {
                    anyhow::Error::new(api::Status {
                        code: 404,
                        message: "No README here.".into(),
                    })
                })?
        }
        _ => return Err(missing("text at that path")),
    };
    raw(c, repo, &git_ref, &path)
}

/// Every path under a ref, as GitHub's recursive tree: the instance lists
/// files, and their directories are worked out from them.
fn tree(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let at = at(sha);
    let files = get_all(
        c,
        &format!(
            "{}/files{}{at}",
            repo_api(repo),
            if at.is_empty() { "" } else { "?" }
        ),
        20,
    )?;
    let paths: Vec<String> = files
        .iter()
        .filter_map(|f| f.as_str().map(str::to_string))
        .collect();
    let mut dirs = std::collections::BTreeSet::new();
    for path in &paths {
        let mut parts: Vec<&str> = path.split('/').collect();
        parts.pop();
        while !parts.is_empty() {
            dirs.insert(parts.join("/"));
            parts.pop();
        }
    }
    let mut tree: Vec<Value> = dirs
        .into_iter()
        .map(|d| json!({ "path": d, "type": "tree", "size": 0 }))
        .collect();
    tree.extend(
        paths
            .iter()
            .map(|p| json!({ "path": p, "type": "blob", "size": 0 })),
    );
    Ok(json!({ "sha": sha, "tree": tree, "truncated": paths.len() >= 10_000 }))
}

fn commits(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let mut path = format!("{}/commits", repo_api(repo));
    if let Some(git_ref) = a.query.get("sha") {
        path = crate::hub::with_query(&path, &format!("until={}", enc(git_ref)));
    }
    if let Some(file) = a.query.get("path") {
        path = crate::hub::with_query(&path, &format!("path={}", enc(file)));
    }
    let shaped =
        |list: &[Value]| -> Vec<Value> { list.iter().map(|v| shape::commit(c, repo, v)).collect() };
    // The instance can't filter by author; this page can, from enough of
    // the history to fill it.
    if let Some(author) = a.query.get("author") {
        let author = author.to_lowercase();
        let list = get_all(c, &path, 4)?;
        let mine: Vec<Value> = list
            .into_iter()
            .filter(|v| {
                [
                    v.s("author.slug"),
                    v.s("author.name"),
                    v.s("author.emailAddress"),
                ]
                .iter()
                .any(|s| s.to_lowercase() == author)
            })
            .collect();
        return Ok(slice(shaped(&mine), &a.query));
    }
    Ok(Value::Array(shaped(&page(c, &path, &a.query)?)))
}

/// The diff `since..until` (or a commit's own, with no `since`) as text,
/// kept: a commit's never changes.
fn diff_text(c: &Client, repo: &str, since: Option<&str>, until: &str) -> String {
    let key = format!("diff:{repo}:{}:{until}", since.unwrap_or(""));
    c.memo
        .get_or(&key, MINUTE * 10, || {
            let mut path = format!(
                "{}/diff?until={}&contextLines=3",
                repo_api(repo),
                enc(until)
            );
            if let Some(since) = since {
                path.push_str(&format!("&since={}", enc(since)));
            }
            Ok(json!(c.raw_text(&path, "text/plain").unwrap_or_default()))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

pub fn commit(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let r = repo_api(repo);
    let v = get(c, &with_avatars(&format!("{r}/commits/{}", enc(sha))))?;
    let mut shaped = shape::commit(c, repo, &v);
    let id = v.s("id");
    let changes = get_all(c, &format!("{r}/commits/{}/changes", enc(&id)), 4).unwrap_or_default();
    let files = shape::files(&changes, &diff_text(c, repo, None, &id));
    let (adds, dels) = files.list("").iter().fold((0, 0), |(a, d), f| {
        (a + f.i("additions"), d + f.i("deletions"))
    });
    shaped["stats"] = json!({ "additions": adds, "deletions": dels, "total": adds + dels });
    shaped["files"] = files;
    Ok(shaped)
}

fn commit_comments(a: &Ask, repo: &str, sha: &str) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/commits/{}/comments", repo_api(repo), enc(sha));
    if a.get() {
        let list = get_all(c, &base, 4)?;
        return Ok(Value::Array(
            list.iter()
                .map(|cm| {
                    let url = format!("/repos/{repo}/commits/{sha}/comments/{}", cm.i("id"));
                    let mut shaped = shape::comment(c, cm, &url);
                    shaped["line"] = cm.at("anchor.line").clone();
                    shaped
                })
                .collect(),
        ));
    }
    let mut body = json!({ "text": a.field("body") });
    if let Some(path) = a.field("path").as_str() {
        body["anchor"] = json!({ "path": path, "line": a.field("line"), "lineType": "CONTEXT", "fileType": "TO" });
    }
    call(c, "POST", &base, Some(&body))
}

/// The build statuses reported on a commit.
pub fn statuses(c: &Client, sha: &str) -> Result<Vec<Value>> {
    get_all(
        c,
        &format!("/rest/build-status/1.0/commits/{}", enc(sha)),
        2,
    )
}

fn status(s: &Value) -> Value {
    let state = match s.s("state").as_str() {
        "SUCCESSFUL" => "success",
        "FAILED" => "failure",
        "CANCELLED" => "error",
        _ => "pending",
    };
    json!({ "state": state, "context": if s.s("name").is_empty() { s.s("key") } else { s.s("name") }, "description": s.s("description"), "target_url": s.s("url") })
}

fn combined_status(c: &Client, sha: &str) -> Result<Value> {
    let list = statuses(c, sha)?;
    let state = match shape::rollup(&list).s("state").as_str() {
        "SUCCESS" => "success",
        "FAILURE" => "failure",
        _ => "pending",
    };
    Ok(json!({
        "state": state,
        "total_count": list.len(),
        "statuses": list.iter().map(status).collect::<Vec<_>>(),
    }))
}

/// `base...head`: the commits on head that base lacks, and the files they
/// change.
fn compare(c: &Client, repo: &str, range: &str) -> Result<Value> {
    let (base, head) = range
        .split_once("...")
        .or_else(|| range.split_once(".."))
        .ok_or_else(|| missing("comparison without two sides"))?;
    let r = repo_api(repo);
    let between = |from: &str, to: &str, pages: u64| {
        get_all(
            c,
            &format!("{r}/compare/commits?from={}&to={}", enc(from), enc(to)),
            pages,
        )
    };
    let ahead = between(head, base, 2)?;
    let behind = between(base, head, 1)?;
    let files = if ahead.is_empty() {
        json!([])
    } else {
        let changes = get_all(
            c,
            &format!("{r}/compare/changes?from={}&to={}", enc(head), enc(base)),
            4,
        )?;
        shape::files(&changes, &diff_text(c, repo, Some(base), head))
    };
    let status = match (ahead.len(), behind.len()) {
        (0, 0) => "identical",
        (_, 0) => "ahead",
        (0, _) => "behind",
        _ => "diverged",
    };
    // GitHub lists them oldest first.
    let commits: Vec<Value> = ahead
        .iter()
        .rev()
        .map(|v| shape::commit(c, repo, v))
        .collect();
    Ok(json!({
        "status": status,
        "ahead_by": ahead.len(),
        "behind_by": behind.len(),
        "total_commits": ahead.len(),
        "commits": commits,
        "files": files,
        "merge_base_commit": { "sha": "" },
        "html_url": crate::forge::datacenter_url(
            &crate::hub::Route::Compare { repo: repo.into(), base: base.into(), head: head.into() },
            &c.web,
        ),
    }))
}

/// Who committed most lately, from the default branch's last commits: the
/// instance doesn't count contributions.
fn contributors(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    if a.query.page() > 1 {
        return Ok(json!([]));
    }
    let recent = c
        .memo
        .get_or(&format!("contributors:{repo}"), MINUTE * 10, || {
            let list = get_all(c, &format!("{}/commits", repo_api(repo)), 2)?;
            let mut counts: HashMap<String, (i64, Value)> = HashMap::new();
            for v in &list {
                let who = v.at("author");
                let (login, person) = if who.has("slug") {
                    (who.s("slug"), shape::user(c, who))
                } else {
                    let name = who.s("name");
                    (name.clone(), json!({ "login": name, "avatar_url": "" }))
                };
                let slot = counts.entry(login).or_insert((0, person));
                slot.0 += 1;
            }
            let mut people: Vec<(i64, Value)> = counts.into_values().collect();
            people.sort_by_key(|p| std::cmp::Reverse(p.0));
            Ok(Value::Array(
                people
                    .into_iter()
                    .map(|(n, mut person)| {
                        person["contributions"] = json!(n);
                        person
                    })
                    .collect(),
            ))
        })?;
    let per = a.query.per_page() as usize;
    Ok(Value::Array(
        recent.list("").iter().take(per).cloned().collect(),
    ))
}
