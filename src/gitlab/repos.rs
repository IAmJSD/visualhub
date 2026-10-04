//! A project's pages, asked for as GitHub's repository paths: the project
//! itself, its files, branches and tags, commits and comparisons, checks,
//! releases, labels, milestones, members, deploy keys, stars and forks.

use super::{avatars, call, get, get_all, missing, parallel, proj, shape, user_id, Ask, MINUTE};
use crate::api::{self, Client};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn route(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let m = a.method;
    match rest {
        [] => project(a, repo),
        ["issues", ..] | ["pulls", ..] | ["merge_requests", ..] => super::issues::route(a, repo, rest),
        ["assignees"] => Ok(shape::users(c, get(c, &format!("{p}/users?{}", a.paging()))?.list(""))),
        ["labels"] if m == "GET" => {
            let list = get(c, &format!("{p}/labels?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(shape::label).collect()))
        }
        ["labels"] => {
            c.memo.forget(&format!("labels:{repo}"));
            let body = json!({ "name": a.field("name"), "color": format!("#{}", a.field("color").as_str().unwrap_or("ededed")), "description": a.field("description") });
            Ok(shape::label(&call(c, "POST", &format!("{p}/labels"), Some(&body))?))
        }
        ["labels", name] => {
            c.memo.forget(&format!("labels:{repo}"));
            let path = format!("{p}/labels/{}", enc(name));
            if m == "DELETE" {
                return call(c, "DELETE", &path, None);
            }
            let mut body = json!({});
            for (from, to) in [("new_name", "new_name"), ("description", "description")] {
                if a.field(from).is_string() {
                    body[to] = a.field(from).clone();
                }
            }
            if let Some(color) = a.field("color").as_str() {
                body["color"] = json!(format!("#{}", color.trim_start_matches('#')));
            }
            Ok(shape::label(&call(c, "PUT", &path, Some(&body))?))
        }
        ["milestones"] if m == "GET" => milestones(a, repo),
        ["milestones"] => Ok(shape::milestone(&call(c, "POST", &format!("{p}/milestones"), Some(&milestone_body(a)))?)),
        ["milestones", id] => {
            let path = format!("{p}/milestones/{id}");
            if m == "DELETE" {
                return call(c, "DELETE", &path, None);
            }
            Ok(shape::milestone(&call(c, "PUT", &path, Some(&milestone_body(a)))?))
        }
        ["contents", path @ ..] => contents(a, repo, &path.join("/")),
        ["branches"] => {
            let list = get(c, &format!("{p}/repository/branches?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(branch).collect()))
        }
        ["branches", name @ .., "protection"] => protect(a, repo, &name.join("/")),
        ["branches", name @ .., "rename"] => rename_branch(a, repo, &name.join("/")),
        ["branches", name @ ..] => Ok(branch(&get(c, &format!("{p}/repository/branches/{}", enc(&name.join("/"))))?)),
        ["tags"] => {
            let list = get(c, &format!("{p}/repository/tags?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(|t| json!({ "name": t.s("name"), "commit": { "sha": t.s("commit.id") }, "message": t.s("message") })).collect()))
        }
        ["git", "refs"] => {
            let reference = a.field("ref").as_str().unwrap_or("").to_string();
            let sha = a.field("sha").clone();
            if let Some(tag) = reference.strip_prefix("refs/tags/") {
                call(c, "POST", &format!("{p}/repository/tags"), Some(&json!({ "tag_name": tag, "ref": sha })))?;
            } else {
                let name = reference.trim_start_matches("refs/heads/");
                call(c, "POST", &format!("{p}/repository/branches"), Some(&json!({ "branch": name, "ref": sha })))?;
            }
            Ok(json!({ "ref": reference, "object": { "sha": sha } }))
        }
        ["git", "refs", "heads", name @ ..] => call(c, "DELETE", &format!("{p}/repository/branches/{}", enc(&name.join("/"))), None),
        ["git", "refs", "tags", name @ ..] => call(c, "DELETE", &format!("{p}/repository/tags/{}", enc(&name.join("/"))), None),
        ["git", "trees", sha] => {
            let list = get_all(c, &format!("{p}/repository/tree?recursive=true&ref={}", enc(sha)), 50)?;
            let tree: Vec<Value> = list.iter().map(|e| json!({ "path": e.s("path"), "type": e.s("type"), "sha": e.s("id"), "mode": e.s("mode") })).collect();
            Ok(json!({ "sha": sha, "tree": tree, "truncated": list.len() >= 5000 }))
        }
        ["commits"] => commits(a, repo),
        ["commits", sha] => commit(c, repo, sha),
        ["commits", sha, "comments"] => commit_comments(a, repo, sha),
        ["commits", sha, "discussions", discussion, "notes", note] => {
            let path = format!("{p}/repository/commits/{sha}/discussions/{discussion}/notes/{note}");
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => call(c, "PUT", &path, Some(&json!({ "body": a.field("body") }))),
            }
        }
        ["commits", sha, "check-runs"] => check_runs(c, repo, sha),
        ["commits", sha, "status"] => statuses(c, repo, sha),
        ["compare", range] => compare(c, repo, range),
        ["releases", ..] => releases(a, repo, &rest[1..]),
        ["languages"] => {
            // Shares of the code, as whole numbers the way GitHub counts
            // bytes; only the proportions are shown.
            let langs = get(c, &format!("{p}/languages"))?;
            let mut out = serde_json::Map::new();
            if let Value::Object(map) = &langs {
                for (name, share) in map {
                    out.insert(name.clone(), json!((share.as_f64().unwrap_or(0.0) * 100.0).round() as i64));
                }
            }
            Ok(Value::Object(out))
        }
        ["contributors"] => {
            let list = get(c, &format!("{p}/repository/contributors?order_by=commits&sort=desc&{}", a.paging()))?;
            let emails: Vec<String> = list.list("").iter().map(|p| p.s("email")).collect();
            let faces = avatars(c, &emails);
            Ok(Value::Array(
                list.list("")
                    .iter()
                    .map(|p| json!({ "login": p.s("name"), "name": p.s("name"), "avatar_url": faces.get(&p.s("email")).cloned().unwrap_or_default(), "contributions": p.i("commits"), "type": "User" }))
                    .collect(),
            ))
        }
        ["forks"] if m == "GET" => Ok(shape::projects(c, get(c, &format!("{p}/forks?order_by=star_count&{}", a.paging()))?.list(""))),
        ["forks"] => {
            let mut body = json!({});
            if let Some(owner) = a.field("organization").as_str().filter(|o| !o.trim().is_empty()) {
                body["namespace_path"] = json!(owner.trim());
            }
            if let Some(name) = a.field("name").as_str().filter(|n| !n.trim().is_empty()) {
                body["name"] = json!(name.trim());
                body["path"] = json!(name.trim());
            }
            Ok(shape::project(c, &call(c, "POST", &format!("{p}/fork"), Some(&body))?))
        }
        ["stargazers"] => {
            let list = get(c, &format!("{p}/starrers?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(|s| shape::user(c, s.at("user"))).collect()))
        }
        ["subscribers"] => Ok(json!([])),
        ["subscription"] => subscription(a, repo),
        ["topics"] => call(c, "PUT", &p, Some(&json!({ "topics": a.field("names") }))),
        ["transfer"] => Ok(shape::project(c, &call(c, "PUT", &format!("{p}/transfer"), Some(&json!({ "namespace": a.field("new_owner") })))?)),
        ["collaborators"] => {
            let list = get(c, &format!("{p}/members/all?{}", a.paging()))?;
            Ok(Value::Array(
                list.list("")
                    .iter()
                    .map(|u| {
                        let mut shaped = shape::user(c, u);
                        shaped["role_name"] = json!(shape::role_name(u.i("access_level")));
                        shaped
                    })
                    .collect(),
            ))
        }
        ["collaborators", login] => {
            let id = user_id(c, login)?;
            if m == "DELETE" {
                return call(c, "DELETE", &format!("{p}/members/{id}"), None);
            }
            let level = access_level(a.field("permission").as_str().unwrap_or("push"));
            match call(c, "PUT", &format!("{p}/members/{id}"), Some(&json!({ "access_level": level }))) {
                Err(e) if api::status_of(&e) == Some(404) => call(c, "POST", &format!("{p}/members"), Some(&json!({ "user_id": id, "access_level": level }))),
                other => other,
            }
        }
        ["keys"] if m == "GET" => {
            let list = get(c, &format!("{p}/deploy_keys?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(deploy_key).collect()))
        }
        ["keys"] => {
            let body = json!({ "title": a.field("title"), "key": a.field("key"), "can_push": !a.field("read_only").as_bool().unwrap_or(true) });
            Ok(deploy_key(&call(c, "POST", &format!("{p}/deploy_keys"), Some(&body))?))
        }
        ["keys", id] => call(c, "DELETE", &format!("{p}/deploy_keys/{id}"), None),
        ["actions", "jobs", id, "rerun"] => call(c, "POST", &format!("{p}/jobs/{id}/retry"), None),
        ["check-runs", _, "annotations"] => Ok(json!([])),
        _ => Err(missing(&format!("equivalent of a repository's {}", rest.join("/")))),
    }
}

/// GitHub's repository roles as GitLab's access levels.
pub fn access_level(permission: &str) -> i64 {
    match permission {
        "pull" | "read" | "triage" => 20,
        "maintain" | "admin" => 40,
        _ => 30,
    }
}


fn project(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    match a.method {
        "GET" => Ok(shape::project(c, &get(c, &format!("{p}?license=true&statistics=true"))?)),
        "DELETE" => call(c, "DELETE", &p, None),
        _ => {
            // What the shared screens change: the default branch, the
            // visibility, archiving, the name and description.
            if let Some(archived) = a.field("archived").as_bool() {
                return Ok(shape::project(c, &call(c, "POST", &format!("{p}/{}", if archived { "archive" } else { "unarchive" }), None)?));
            }
            let mut body = json!({});
            for key in ["name", "description", "default_branch"] {
                if a.field(key).is_string() {
                    body[key] = a.field(key).clone();
                }
            }
            if let Some(private) = a.field("private").as_bool() {
                body["visibility"] = json!(if private { "private" } else { "public" });
            }
            if let Some(on) = a.field("has_issues").as_bool() {
                body["issues_access_level"] = json!(if on { "enabled" } else { "disabled" });
            }
            if let Some(on) = a.field("has_wiki").as_bool() {
                body["wiki_access_level"] = json!(if on { "enabled" } else { "disabled" });
            }
            if let Some(on) = a.field("delete_branch_on_merge").as_bool() {
                body["remove_source_branch_after_merge"] = json!(on);
            }
            Ok(shape::project(c, &call(c, "PUT", &p, Some(&body))?))
        }
    }
}

fn branch(b: &Value) -> Value {
    json!({
        "name": b.s("name"),
        "commit": { "sha": b.s("commit.id") },
        "protected": b.b("protected"),
        "default": b.b("default"),
        "merged": b.b("merged"),
    })
}

/// GitHub's protection rule as GitLab's protected branch: maintainers
/// push (and force-push if allowed), developers merge.
fn protect(a: &Ask, repo: &str, name: &str) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let path = format!("{p}/protected_branches/{}", enc(name));
    let _ = call(c, "DELETE", &path, None);
    if a.method == "DELETE" {
        return Ok(Value::Null);
    }
    let body = json!({
        "name": name,
        "push_access_level": if a.field("required_pull_request_reviews").is_null() { 30 } else { 40 },
        "merge_access_level": 30,
        "allow_force_push": a.field("allow_force_pushes").as_bool().unwrap_or(false),
        "code_owner_approval_required": a.field("required_pull_request_reviews.require_code_owner_reviews").as_bool().unwrap_or(false),
    });
    call(c, "POST", &format!("{p}/protected_branches"), Some(&body))
}

/// GitLab can't rename a branch: make the new one where the old one is,
/// move the default if it was, then drop the old one.
fn rename_branch(a: &Ask, repo: &str, old: &str) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let new = a.field("new_name").as_str().unwrap_or("").trim().to_string();
    call(c, "POST", &format!("{p}/repository/branches"), Some(&json!({ "branch": new, "ref": old })))?;
    if get(c, &p)?.s("default_branch") == old {
        call(c, "PUT", &p, Some(&json!({ "default_branch": new })))?;
    }
    call(c, "DELETE", &format!("{p}/repository/branches/{}", enc(old)), None)?;
    Ok(json!({ "name": new }))
}

fn milestone_body(a: &Ask) -> Value {
    let mut body = json!({});
    for key in ["title", "description"] {
        if a.field(key).is_string() {
            body[key] = a.field(key).clone();
        }
    }
    if let Some(due) = a.field("due_on").as_str() {
        body["due_date"] = json!(due.get(..10).unwrap_or(due));
    }
    match a.field("state").as_str() {
        Some("closed") => body["state_event"] = json!("close"),
        Some("open") => body["state_event"] = json!("activate"),
        _ => {}
    }
    body
}

fn milestones(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let state = match a.query.get("state") {
        Some("closed") => "&state=closed",
        Some("all") => "",
        _ => "&state=active",
    };
    let list = get(c, &format!("{p}/milestones?{}{state}", a.paging()))?;
    let list = list.list("").to_vec();
    // Progress: how many of each milestone's issues are closed.
    let counts = parallel(&list, |m| {
        get(c, &format!("{p}/issues_statistics?milestone={}", enc(&m.s("title"))))
            .map(|s| (s.i("statistics.counts.opened"), s.i("statistics.counts.closed")))
            .unwrap_or((0, 0))
    });
    Ok(Value::Array(
        list.iter()
            .zip(counts)
            .map(|(m, (open, closed))| {
                let mut shaped = shape::milestone(m);
                shaped["open_issues"] = json!(open);
                shaped["closed_issues"] = json!(closed);
                shaped
            })
            .collect(),
    ))
}

/// A directory's entries, folders and files as GitHub lists them.
fn tree(c: &Client, repo: &str, path: &str, git_ref: &str) -> Result<Vec<Value>> {
    let p = proj(repo);
    let list = get_all(c, &format!("{p}/repository/tree?path={}&ref={}", enc(path), enc(git_ref)), 20)?;
    let web = format!("{}/{repo}/-", c.web);
    Ok(list
        .iter()
        .map(|e| {
            let path = e.s("path");
            let kind = match (e.s("type").as_str(), e.s("mode").as_str()) {
                ("tree", _) => "dir",
                ("commit", _) => "submodule",
                (_, "120000") => "symlink",
                _ => "file",
            };
            let page = if kind == "dir" { "tree" } else { "blob" };
            json!({ "type": kind, "name": e.s("name"), "path": path, "sha": e.s("id"), "size": 0, "html_url": format!("{web}/{page}/{git_ref}/{path}") })
        })
        .collect())
}

/// A directory's listing, or a file with its contents in base64.
fn contents(a: &Ask, repo: &str, path: &str) -> Result<Value> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("HEAD").to_string();
    match tree(c, repo, path, &git_ref) {
        Ok(entries) if !entries.is_empty() || path.is_empty() => return Ok(Value::Array(entries)),
        Err(e) if api::status_of(&e) != Some(404) => return Err(e),
        _ => {}
    }
    let f = get(c, &format!("{}/repository/files/{}?ref={}", proj(repo), enc(path), enc(&git_ref)))?;
    Ok(json!({
        "type": "file",
        "name": f.s("file_name"),
        "path": f.s("file_path"),
        "size": f.i("size"),
        "sha": f.s("blob_id"),
        "content": f.s("content"),
        "encoding": f.s("encoding"),
        "html_url": format!("{}/{repo}/-/blob/{git_ref}/{path}", c.web),
    }))
}

/// A file or README as text.
pub fn text(a: &Ask, repo: &str, rest: &[&str]) -> Result<String> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("HEAD").to_string();
    let path = match rest {
        ["contents", path @ ..] => path.join("/"),
        ["readme", dir @ ..] => {
            let dir = dir.join("/");
            let entries = tree(c, repo, &dir, &git_ref)?;
            let names: Vec<String> = entries.iter().filter(|e| e.s("type") == "file").map(|e| e.s("name")).collect();
            let pick = names
                .iter()
                .find(|n| n.eq_ignore_ascii_case("readme.md"))
                .or_else(|| names.iter().find(|n| n.to_lowercase().starts_with("readme.")))
                .or_else(|| names.iter().find(|n| n.eq_ignore_ascii_case("readme")))
                .ok_or_else(|| missing("README here"))?;
            if dir.is_empty() { pick.clone() } else { format!("{dir}/{pick}") }
        }
        _ => return Err(missing("text at that path")),
    };
    c.raw_text(&format!("{}/repository/files/{}/raw?ref={}", proj(repo), enc(&path), enc(&git_ref)), "*/*")
}

fn commits(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let mut path = format!("{}/repository/commits?{}", proj(repo), a.paging());
    if let Some(r) = a.query.get("sha") {
        path.push_str(&format!("&ref_name={}", enc(r)));
    }
    for key in ["path", "author", "since", "until"] {
        if let Some(v) = a.query.get(key) {
            path.push_str(&format!("&{key}={}", enc(v)));
        }
    }
    let list = get(c, &path)?;
    Ok(shape_commits(c, repo, list.list("")))
}

/// Commits with their authors' faces.
pub fn shape_commits(c: &Client, repo: &str, list: &[Value]) -> Value {
    let emails: Vec<String> = list.iter().map(|v| v.s("author_email")).collect();
    let faces = avatars(c, &emails);
    Value::Array(list.iter().map(|v| shape::commit(repo, v, faces.get(&v.s("author_email")))).collect())
}

/// One commit, its changes, and whether its signature checks out.
pub fn commit(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let p = proj(repo);
    let v = get(c, &format!("{p}/repository/commits/{}?stats=true", enc(sha)))?;
    let full = v.s("id");
    let faces = avatars(c, &[v.s("author_email")]);
    let mut shaped = shape::commit(repo, &v, faces.get(&v.s("author_email")));
    let diffs = get_all(c, &format!("{p}/repository/commits/{full}/diff"), 10)?;
    shaped["files"] = Value::Array(diffs.iter().map(shape::file).collect());
    if let Ok(signature) = get(c, &format!("{p}/repository/commits/{full}/signature")) {
        shaped["commit"]["verification"]["verified"] = json!(signature.s("verification_status") == "verified");
    }
    shaped["last_pipeline"] = v.at("last_pipeline").clone();
    Ok(shaped)
}

/// A commit's comments: its threads' notes, each with the path that
/// edits it.
fn commit_comments(a: &Ask, repo: &str, sha: &str) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    if a.method == "POST" {
        return call(c, "POST", &format!("{p}/repository/commits/{sha}/discussions"), Some(&json!({ "body": a.field("body") })));
    }
    let threads = get_all(c, &format!("{p}/repository/commits/{sha}/discussions"), 5)?;
    let web = format!("{}/{repo}/-/commit/{sha}", c.web);
    let mut out = Vec::new();
    for thread in &threads {
        for n in thread.list("notes") {
            if n.b("system") {
                continue;
            }
            let id = n.i("id");
            out.push(json!({
                "id": id,
                "body": n.s("body"),
                "user": shape::user(c, n.at("author")),
                "created_at": n.s("created_at"),
                "updated_at": n.s("updated_at"),
                "url": format!("/repos/{repo}/commits/{sha}/discussions/{}/notes/{id}", thread.s("id")),
                "html_url": format!("{web}#note_{id}"),
                "author_association": "",
                "reactions": {},
            }));
        }
    }
    Ok(Value::Array(out))
}

/// The newest pipeline that ran for `sha`, if any did.
pub fn pipeline_for(c: &Client, repo: &str, sha: &str) -> Result<Option<Value>> {
    let list = get(c, &format!("{}/pipelines?sha={}&per_page=1&order_by=id&sort=desc", proj(repo), enc(sha)))?;
    Ok(list.list("").first().cloned())
}

/// The jobs of the newest pipeline for a commit, as check runs.
pub fn check_runs(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let Some(pipeline) = pipeline_for(c, repo, sha)? else {
        return Ok(json!({ "total_count": 0, "check_runs": [] }));
    };
    let jobs = get_all(c, &format!("{}/pipelines/{}/jobs?include_retried=false", proj(repo), pipeline.i("id")), 5)?;
    let runs: Vec<Value> = jobs.iter().map(shape::check_run).collect();
    Ok(json!({ "total_count": runs.len(), "check_runs": runs, "pipeline": pipeline }))
}

/// Statuses other services set on a commit (CI's own jobs are its check
/// runs).
fn statuses(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let list = get(c, &format!("{}/repository/commits/{}/statuses?per_page=100&all=false", proj(repo), enc(sha)))?;
    let statuses: Vec<Value> = list
        .list("")
        .iter()
        .filter(|s| !s.s("target_url").contains("/-/jobs/"))
        .map(|s| {
            let state = match s.s("status").as_str() {
                "success" => "success",
                "failed" => "failure",
                "canceled" => "error",
                _ => "pending",
            };
            json!({ "context": s.s("name"), "state": state, "description": s.s("description"), "target_url": s.s("target_url") })
        })
        .collect();
    let state = if statuses.iter().any(|s| s.s("state") == "failure" || s.s("state") == "error") {
        "failure"
    } else if statuses.iter().any(|s| s.s("state") == "pending") {
        "pending"
    } else {
        "success"
    };
    Ok(json!({ "state": state, "statuses": statuses }))
}

fn compare(c: &Client, repo: &str, range: &str) -> Result<Value> {
    let (base, head) = range.split_once("...").unwrap_or((range, "HEAD"));
    let p = proj(repo);
    let ahead = get(c, &format!("{p}/repository/compare?from={}&to={}&straight=false", enc(base), enc(head)))?;
    let behind = get(c, &format!("{p}/repository/compare?from={}&to={}&straight=false", enc(head), enc(base)))
        .map(|v| v.list("commits").len())
        .unwrap_or(0);
    let commits = ahead.list("commits");
    let status = match (commits.len(), behind) {
        (0, 0) => "identical",
        (_, 0) => "ahead",
        (0, _) => "behind",
        _ => "diverged",
    };
    Ok(json!({
        "status": status,
        "ahead_by": commits.len(),
        "behind_by": behind,
        "commits": shape_commits(c, repo, commits),
        "files": ahead.list("diffs").iter().map(shape::file).collect::<Vec<_>>(),
    }))
}

/// The release GitHub would number `id`: the one whose tag hashes to it.
fn find_release(c: &Client, repo: &str, id: &str) -> Result<Value> {
    let id: u64 = id.parse().unwrap_or(0);
    get_all(c, &format!("{}/releases", proj(repo)), 5)?
        .into_iter()
        .find(|r| crate::forge::release_id(&r.s("tag_name")) == id)
        .ok_or_else(|| missing("release by that number"))
}

fn releases(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let m = a.method;
    let release_body = || {
        let mut body = json!({});
        if a.field("name").is_string() {
            body["name"] = a.field("name").clone();
        }
        if a.field("body").is_string() {
            body["description"] = a.field("body").clone();
        }
        body
    };
    match rest {
        [] if m == "GET" => {
            let list = get(c, &format!("{p}/releases?{}", a.paging()))?;
            Ok(Value::Array(list.list("").iter().map(|r| shape::release(c, repo, r)).collect()))
        }
        [] => {
            let mut body = release_body();
            body["tag_name"] = a.field("tag_name").clone();
            if let Some(target) = a.field("target_commitish").as_str().filter(|t| !t.is_empty()) {
                body["ref"] = json!(target);
            } else {
                body["ref"] = json!(get(c, &p)?.s("default_branch"));
            }
            Ok(shape::release(c, repo, &call(c, "POST", &format!("{p}/releases"), Some(&body))?))
        }
        ["latest"] => Ok(shape::release(c, repo, &get(c, &format!("{p}/releases/permalink/latest"))?)),
        ["assets", id] => {
            let id: i64 = id.parse().unwrap_or(0);
            let release = get_all(c, &format!("{p}/releases"), 5)?
                .into_iter()
                .find(|r| r.list("assets.links").iter().any(|l| l.i("id") == id))
                .ok_or_else(|| missing("asset by that number"))?;
            call(c, "DELETE", &format!("{p}/releases/{}/assets/links/{id}", enc(&release.s("tag_name"))), None)
        }
        [id] => {
            let release = find_release(c, repo, id)?;
            let path = format!("{p}/releases/{}", enc(&release.s("tag_name")));
            match m {
                "GET" => Ok(shape::release(c, repo, &release)),
                "DELETE" => call(c, "DELETE", &path, None),
                _ => Ok(shape::release(c, repo, &call(c, "PUT", &path, Some(&release_body()))?)),
            }
        }
        [id, "assets"] => Ok(shape::release(c, repo, &find_release(c, repo, id)?).at("assets").clone()),
        _ => Err(missing("reactions on releases")),
    }
}

/// Watching, as GitLab's notification level for the project.
fn subscription(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let path = format!("{}/notification_settings", proj(repo));
    match a.method {
        "GET" => {
            let level = get(c, &path)?.s("level");
            match level.as_str() {
                "watch" => Ok(json!({ "subscribed": true, "ignored": false })),
                "disabled" => Ok(json!({ "subscribed": false, "ignored": true })),
                _ => Err(anyhow::Error::new(api::Status { code: 404, message: "Not watching".into() })),
            }
        }
        "DELETE" => call(c, "PUT", &format!("{path}?level=participating"), None),
        _ => {
            let level = if a.field("ignored").as_bool() == Some(true) { "disabled" } else { "watch" };
            call(c, "PUT", &format!("{path}?level={level}"), None)
        }
    }
}

fn deploy_key(k: &Value) -> Value {
    json!({ "id": k.i("id"), "title": k.s("title"), "key": k.s("key"), "created_at": k.s("created_at"), "read_only": !k.b("can_push") })
}

/// Whether the signed-in user starred `repo`.
pub fn starred(c: &Client, repo: &str) -> Result<bool> {
    let name = crate::forge::split_repo(repo).1;
    let found = get(c, &format!("/projects?starred=true&simple=true&per_page=100&search={}", enc(name)))?;
    Ok(found.list("").iter().any(|p| p.s("path_with_namespace").eq_ignore_ascii_case(repo)))
}

/// Star or unstar, as GitHub's `PUT`/`DELETE /user/starred/{repo}`.
pub fn star(c: &Client, repo: &str, on: bool) -> Result<Value> {
    let reply = c.raw("POST", &format!("{}/{}", proj(repo), if on { "star" } else { "unstar" }), None, None)?;
    // 304: it already was.
    if reply.status >= 400 {
        return Err(api::failure(&reply));
    }
    Ok(Value::Null)
}

/// Colours of a project's labels by name, for answers that give only
/// names.
pub fn label_colors(c: &Client, repo: &str) -> HashMap<String, String> {
    c.memo
        .get_or(&format!("labels:{repo}"), MINUTE, || get(c, &format!("{}/labels?per_page=100", proj(repo))))
        .map(|list| list.list("").iter().map(|l| (l.s("name"), l.s("color").trim_start_matches('#').to_string())).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn roles_map_both_ways() {
        assert_eq!(super::access_level("push"), 30);
        assert_eq!(super::shape::role_name(40), "Maintainer");
        assert_eq!(super::shape::role_name(super::access_level("pull")), "Reporter");
    }
}
