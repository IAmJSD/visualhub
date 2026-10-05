//! Bitbucket's objects in GitHub's shape: the fields the screens read,
//! named and valued as GitHub would.
//!
//! People are known by their nickname, which is what Bitbucket shows on
//! their work; Bitbucket can't look a nickname up, so each person seen is
//! remembered under it ([`super::user_by_login`]). A repository's owner is
//! its workspace, as an organization. Some fields are added that only the
//! translation itself reads back: `node_id`s that say which pull request a
//! mutation is about, and `url`s in the shape of GitHub's API paths that
//! the translation knows how to answer.

use crate::api::Client;
use crate::json::Json as _;
use serde_json::{json, Map, Value};
use std::time::Duration;

fn or_null(s: String) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s)
    }
}

/// The name people go by: the nickname, or failing that the display name.
pub fn login_of(u: &Value) -> String {
    [u.s("nickname"), u.s("display_name"), u.s("account_id")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

pub fn user(c: &Client, u: &Value) -> Value {
    if u.is_null() || !(u.has("uuid") || u.has("account_id") || u.has("display_name")) {
        return Value::Null;
    }
    let login = login_of(u);
    // Remember who this is, for when a screen names them back.
    if u.has("uuid") {
        let _ = c
            .memo
            .get_or(&format!("login:{login}"), Duration::ZERO, || Ok(u.clone()));
    }
    json!({
        "login": login,
        "id": 0,
        "uuid": u.s("uuid"),
        "account_id": u.s("account_id"),
        "name": u.s("display_name"),
        "avatar_url": u.s("links.avatar.href"),
        "html_url": u.s("links.html.href"),
        "type": if u.s("type") == "team" { "Organization" } else { "User" },
        "bio": "",
        "location": u.s("location"),
        "company": "",
        "blog": u.s("website"),
        "email": "",
        "twitter_username": "",
        "created_at": u.s("created_on"),
        "followers": 0,
        "following": 0,
        "public_repos": 0,
        "public_gists": 0,
    })
}

pub fn users(c: &Client, list: &[Value]) -> Value {
    Value::Array(
        list.iter()
            .map(|u| user(c, u))
            .filter(|u| !u.is_null())
            .collect(),
    )
}

/// A workspace as an organization.
pub fn workspace(w: &Value) -> Value {
    let slug = w.s("slug");
    json!({
        "login": slug,
        "id": 0,
        "uuid": w.s("uuid"),
        "name": w.s("name"),
        "description": "",
        "avatar_url": w.s("links.avatar.href"),
        "html_url": format!("https://{}/{slug}/", crate::forge::BITBUCKET_HOST),
        "type": "Organization",
        "public_repos": 0,
        "followers": 0,
        "location": "",
        "blog": "",
        "is_private": w.b("is_private"),
        "created_at": w.s("created_on"),
    })
}

/// A repository. `permission` is the signed-in user's (`read`, `write`,
/// `admin`) where it is known.
pub fn repo(r: &Value, permission: &str) -> Value {
    let ws = r.at("workspace");
    let workspace = if ws.has("slug") {
        ws.s("slug")
    } else {
        r.s("full_name").split('/').next().unwrap_or("").to_string()
    };
    let clone = |name: &str| {
        r.list("links.clone")
            .iter()
            .find(|l| l.s("name") == name)
            .map(|l| l.s("href"))
            .unwrap_or_default()
    };
    let size = r.i("size") / 1024;
    json!({
        "id": 0,
        "uuid": r.s("uuid"),
        "name": r.s("name"),
        "slug": r.s("slug"),
        "full_name": r.s("full_name"),
        "owner": {
            "login": workspace,
            "avatar_url": if ws.has("links.avatar.href") { ws.s("links.avatar.href") } else { r.s("owner.links.avatar.href") },
            "type": "Organization",
        },
        "private": r.b("is_private"),
        "visibility": if r.b("is_private") { "private" } else { "public" },
        "description": r.s("description"),
        "html_url": r.s("links.html.href"),
        "clone_url": clone("https"),
        "ssh_url": clone("ssh"),
        "default_branch": r.s("mainbranch.name"),
        "stargazers_count": 0,
        "forks_count": 0,
        "subscribers_count": 0,
        "open_issues_count": 0,
        "topics": [],
        "archived": false,
        "fork": r.has("parent"),
        "parent": if r.has("parent") { json!({ "full_name": r.s("parent.full_name") }) } else { Value::Null },
        "license": Value::Null,
        "homepage": r.s("website"),
        "created_at": r.s("created_on"),
        "pushed_at": r.s("updated_on"),
        "updated_at": r.s("updated_on"),
        "size": size.max(if r.i("size") > 0 { 1 } else { 0 }),
        "has_issues": false,
        "has_wiki": false,
        "has_discussions": false,
        "has_projects": false,
        "is_template": false,
        "language": or_null(r.s("language")),
        "permissions": {
            "admin": permission == "admin",
            "maintain": permission == "admin",
            "push": matches!(permission, "admin" | "write"),
            "pull": true,
        },
        "avatar_url": r.s("links.avatar.href"),
        "project": { "key": r.s("project.key"), "name": r.s("project.name") },
        "fork_policy": r.s("fork_policy"),
    })
}

pub fn repos(list: &[Value]) -> Value {
    Value::Array(list.iter().map(|r| repo(r, "")).collect())
}

/// `OPEN` as `open`; merged, declined and superseded are all closed.
pub fn state(s: &str) -> &'static str {
    if s == "OPEN" {
        "open"
    } else {
        "closed"
    }
}

/// A pull request, as both GitHub's issue and pull request.
pub fn pr(c: &Client, repo: &str, v: &Value) -> Value {
    let n = v.i("id");
    let merged = v.s("state") == "MERGED";
    let open = v.s("state") == "OPEN";
    let source = v.s("source.repository.full_name");
    let source = if source.is_empty() {
        repo.to_string()
    } else {
        source
    };
    let branch = v.s("source.branch.name");
    let label = if source == repo {
        branch.clone()
    } else {
        format!("{}:{branch}", source.split('/').next().unwrap_or(""))
    };
    let reviewers: Vec<Value> = v.list("reviewers").to_vec();
    let draft = v.b("draft");
    let closed_at = if open {
        Value::Null
    } else {
        json!(v.s("updated_on"))
    };
    let value = json!({
        "id": n,
        "number": n,
        "title": v.s("title"),
        "body": v.s("description"),
        "state": state(&v.s("state")),
        "state_reason": if v.s("state") == "DECLINED" { "not_planned" } else { "" },
        "user": user(c, v.at("author")),
        "labels": [],
        "assignees": [],
        "milestone": Value::Null,
        "comments": v.i("comment_count"),
        "created_at": v.s("created_on"),
        "updated_at": v.s("updated_on"),
        "closed_at": closed_at,
        "closed_by": user(c, v.at("closed_by")),
        "locked": false,
        "html_url": v.s("links.html.href"),
        "repository_url": format!("/repos/{repo}"),
        "author_association": "",
        "reactions": {},
    });
    // In two, for json!'s sake.
    let extra = json!({
        "url": format!("/repos/{repo}/pulls/{n}"),
        "node_id": format!("bb:pr:{repo}#{n}"),
        "pull_request": { "url": format!("/repos/{repo}/pulls/{n}") },
        "merged": merged,
        "merged_at": if merged { json!(v.s("updated_on")) } else { Value::Null },
        "merged_by": if merged { user(c, v.at("closed_by")) } else { Value::Null },
        "draft": draft,
        "head": {
            "ref": branch,
            "label": label,
            "sha": v.s("source.commit.hash"),
            "repo": { "full_name": source },
        },
        "base": {
            "ref": v.s("destination.branch.name"),
            "sha": v.s("destination.commit.hash"),
            "repo": { "full_name": repo },
        },
        "requested_reviewers": users(c, &reviewers),
        // Known at once: Bitbucket works out conflicts when asked for the
        // diff, which the full pull request does.
        "mergeable": if open { json!(!draft) } else { Value::Null },
        "mergeable_state": if !open { "" } else if draft { "draft" } else { "clean" },
        "merge_blocker": "",
        "auto_merge": Value::Null,
        "commits": 0,
        "additions": 0,
        "deletions": 0,
        "changed_files": 0,
        "review_comments": 0,
        "merge_commit_sha": or_null(v.s("merge_commit.hash")),
        "close_source_branch": v.b("close_source_branch"),
        "task_count": v.i("task_count"),
    });
    let mut value = value;
    if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
        map.extend(extra);
    }
    value
}

/// A comment, anywhere: on a pull request (`url` says where to edit it)
/// or a commit.
pub fn comment(c: &Client, cm: &Value, url: &str) -> Value {
    let deleted = cm.b("deleted");
    json!({
        "id": cm.i("id"),
        "event": "commented",
        "body": if deleted { "*This comment was deleted.*".to_string() } else { cm.s("content.raw") },
        "user": user(c, cm.at("user")),
        "created_at": cm.s("created_on"),
        "updated_at": cm.s("updated_on"),
        "url": url,
        "html_url": cm.s("links.html.href"),
        "author_association": "",
        "reactions": {},
        "path": cm.s("inline.path"),
        "in_reply_to_id": if cm.has("parent.id") { json!(cm.i("parent.id")) } else { Value::Null },
        "resolved": cm.has("resolution"),
    })
}

/// `Ada Lovelace <ada@example.com>` as the name and the email.
pub fn author_parts(raw: &str) -> (String, String) {
    match raw.split_once('<') {
        Some((name, email)) => (
            name.trim().to_string(),
            email.trim_end().trim_end_matches('>').trim().to_string(),
        ),
        None => (raw.trim().to_string(), String::new()),
    }
}

pub fn commit(c: &Client, repo: &str, v: &Value) -> Value {
    let sha = v.s("hash");
    let (name, email) = author_parts(&v.s("author.raw"));
    let who = v.at("author.user");
    let author = json!({ "name": name, "email": email, "date": v.s("date") });
    json!({
        "sha": sha,
        "commit": {
            "message": v.s("message"),
            "author": author,
            "committer": author,
            "verification": { "verified": false },
        },
        "author": if who.is_null() {
            Value::Null
        } else {
            let mut u = user(c, who);
            u["avatar_url"] = json!(who.s("links.avatar.href"));
            u
        },
        "parents": v.list("parents").iter().map(|p| json!({ "sha": p.s("hash") })).collect::<Vec<_>>(),
        "html_url": v.s("links.html.href"),
        "url": format!("/repos/{repo}/commits/{sha}"),
    })
}

/// A file's part of a whole diff: every `diff --git` section, by the path
/// it is about.
pub fn split_diff(diff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(done) = current.take() {
                out.push(done);
            }
            // `a/old b/new`: the new path, which comes after the last ` b/`.
            let path = rest
                .trim_end()
                .rsplit_once(" b/")
                .map(|(_, p)| p.to_string())
                .unwrap_or_default();
            current = Some((path, String::new()));
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
        }
    }
    if let Some(done) = current {
        out.push(done);
    }
    // Only the hunks, as GitHub's `patch` has them.
    out.into_iter()
        .map(|(path, body)| {
            let start = body.find("\n@@").map(|i| i + 1).or_else(|| {
                if body.starts_with("@@") {
                    Some(0)
                } else {
                    None
                }
            });
            let patch = start.map(|i| body[i..].to_string()).unwrap_or_default();
            (path, patch)
        })
        .collect()
}

/// A changed file, from a diffstat entry and the diff's part for it.
pub fn file(d: &Value, patch: &str) -> Value {
    let new = d.s("new.path");
    let old = d.s("old.path");
    let status = match d.s("status").as_str() {
        "added" => "added",
        "removed" => "removed",
        "renamed" => "renamed",
        _ => "modified",
    };
    let adds = d.i("lines_added");
    let dels = d.i("lines_removed");
    json!({
        "filename": if new.is_empty() { old.clone() } else { new },
        "previous_filename": old,
        "status": status,
        "additions": adds,
        "deletions": dels,
        "changes": adds + dels,
        "patch": patch,
        "conflict": d.s("status").contains("conflict"),
    })
}

/// Diffstat entries with their patches from the whole diff.
pub fn files(stats: &[Value], diff: &str) -> Value {
    let parts = split_diff(diff);
    Value::Array(
        stats
            .iter()
            .map(|d| {
                let path = if d.has("new.path") {
                    d.s("new.path")
                } else {
                    d.s("old.path")
                };
                let patch = parts
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, body)| body.as_str())
                    .unwrap_or("");
                file(d, patch)
            })
            .collect(),
    )
}

/// A build status's state as GitHub's status and conclusion.
pub fn ci_status(state: &str) -> (&'static str, &'static str) {
    match state {
        "SUCCESSFUL" => ("completed", "success"),
        "FAILED" => ("completed", "failure"),
        "STOPPED" => ("completed", "cancelled"),
        "INPROGRESS" => ("in_progress", ""),
        _ => ("queued", ""),
    }
}

/// A commit's build status as a check run.
pub fn check_run(s: &Value, i: usize) -> Value {
    let (status, conclusion) = ci_status(&s.s("state"));
    let name = if s.s("name").is_empty() {
        s.s("key")
    } else {
        s.s("name")
    };
    json!({
        "id": i as i64 + 1,
        "name": name,
        "status": status,
        "conclusion": or_null(conclusion.to_string()),
        "started_at": s.s("created_on"),
        "completed_at": if status == "completed" { json!(s.s("updated_on")) } else { Value::Null },
        "details_url": s.s("url"),
        "html_url": s.s("url"),
        "app": { "name": if s.s("url").contains("/pipelines/") { "Bitbucket Pipelines" } else { "Bitbucket" }, "slug": "bitbucket" },
        "output": { "title": s.s("description") },
    })
}

/// Build statuses as GraphQL's `statusCheckRollup`: how they went, and
/// how many passed, as check runs.
pub fn rollup(statuses: &[Value]) -> Value {
    if statuses.is_empty() {
        return Value::Null;
    }
    let passed = statuses
        .iter()
        .filter(|s| s.s("state") == "SUCCESSFUL")
        .count();
    let states: Vec<String> = statuses.iter().map(|s| s.s("state")).collect();
    let state = if states.iter().any(|s| s == "FAILED" || s == "STOPPED") {
        "FAILURE"
    } else if states.iter().all(|s| s == "SUCCESSFUL") {
        "SUCCESS"
    } else {
        "PENDING"
    };
    json!({
        "state": state,
        "contexts": {
            "checkRunCount": statuses.len(),
            "checkRunCountsByState": [{ "state": "SUCCESS", "count": passed }],
            "statusContextCount": 0,
            "statusContextCountsByState": [],
        },
    })
}

/// A snippet as a gist; `contents` are its files' text, by name, when
/// they have been read.
pub fn snippet(c: &Client, s: &Value, contents: &[(String, String)]) -> Value {
    let ws = s.s("workspace.slug");
    let ws = if ws.is_empty() {
        s.s("owner.username")
    } else {
        ws
    };
    let mut files = Map::new();
    let names: Vec<String> = match s.at("files") {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    for name in names {
        let content = contents
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        files.insert(
            name.clone(),
            json!({ "filename": name, "content": content, "size": content.len(), "language": "", "raw_url": "" }),
        );
    }
    json!({
        "id": format!("{ws}/{}", s.s("id")),
        "description": s.s("title"),
        "public": !s.b("is_private"),
        "owner": user(c, if s.has("creator") { s.at("creator") } else { s.at("owner") }),
        "files": files,
        "comments": 0,
        "created_at": s.s("created_on"),
        "updated_at": s.s("updated_on"),
        "git_pull_url": s.list("links.clone").iter().find(|l| l.s("name") == "https").map(|l| l.s("href")).unwrap_or_default(),
        "html_url": s.s("links.html.href"),
        "forks": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(&crate::forge::Account::new(
            crate::forge::Forge::Bitbucket,
            "",
            "",
        ))
    }

    #[test]
    fn pull_requests_read_as_github_does() {
        let c = client();
        let v = json!({
            "id": 7, "title": "Speed", "state": "OPEN", "draft": false,
            "author": { "uuid": "{a}", "nickname": "ada", "display_name": "Ada L", "links": { "avatar": { "href": "https://x/a.png" } } },
            "source": { "branch": { "name": "fast" }, "commit": { "hash": "abc" }, "repository": { "full_name": "fork/r" } },
            "destination": { "branch": { "name": "main" }, "repository": { "full_name": "ws/r" } },
            "links": { "html": { "href": "https://bitbucket.org/ws/r/pull-requests/7" } },
        });
        let pr = pr(&c, "ws/r", &v);
        assert_eq!(pr.i("number"), 7);
        assert_eq!(pr.s("state"), "open");
        assert_eq!(pr.s("user.login"), "ada");
        assert_eq!(pr.s("head.label"), "fork:fast");
        assert_eq!(pr.s("head.repo.full_name"), "fork/r");
        assert_eq!(pr.s("mergeable_state"), "clean");
        assert!(pr.has("pull_request"));
        assert!(!pr.has("merged_at"));
        // The nickname is remembered for when a screen names them back.
        assert_eq!(
            super::super::user_by_login(&c, "ada").unwrap().s("uuid"),
            "{a}"
        );
    }

    #[test]
    fn diffs_split_by_file() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n-a\n+b\n c\ndiff --git a/new b/new\nnew file mode 100644\n--- /dev/null\n+++ b/new\n@@ -0,0 +1 @@\n+x\n";
        let parts = split_diff(diff);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "a.rs");
        assert_eq!(parts[0].1, "@@ -1,2 +1,2 @@\n-a\n+b\n c\n");
        assert_eq!(parts[1].1, "@@ -0,0 +1 @@\n+x\n");
        let f = files(
            &[
                json!({ "status": "modified", "lines_added": 1, "lines_removed": 1, "new": { "path": "a.rs" }, "old": { "path": "a.rs" } }),
            ],
            diff,
        );
        assert_eq!(f.list("")[0].s("patch"), parts[0].1);
        assert_eq!(
            author_parts("Ada L <ada@example.com>"),
            ("Ada L".to_string(), "ada@example.com".to_string())
        );
        let mixed = rollup(&[
            json!({ "state": "SUCCESSFUL" }),
            json!({ "state": "INPROGRESS" }),
        ]);
        assert_eq!(mixed.s("state"), "PENDING");
        let checks = crate::screens::pulls::Checks::from_rollup(&mixed);
        assert_eq!((checks.passed, checks.total), (1, 2));
    }
}
