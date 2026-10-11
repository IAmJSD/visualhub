//! Bitbucket Data Center's objects in GitHub's shape: the fields the
//! screens read, named and valued as GitHub would.
//!
//! Times come as milliseconds since 1970 and go out as GitHub's ISO 8601.
//! People are known by their slug. A repository's owner is its project, as
//! an organization, or for a person's own repository, the person. Some
//! fields are added that only the translation reads back: `node_id`s that
//! say which pull request a mutation is about, and `url`s in the shape of
//! GitHub's API paths that the translation knows how to answer.

use crate::api::Client;
use crate::json::Json as _;
use serde_json::{json, Value};

fn or_null(s: String) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s)
    }
}

/// Milliseconds since 1970 as `2026-10-11T09:30:00Z`; nothing as empty.
pub fn iso(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A field holding milliseconds, as ISO 8601.
pub fn when(v: &Value, field: &str) -> String {
    iso(v.i(field))
}

/// A path the instance gave (`/users/ada/avatar.png`) made whole.
pub fn absolute(c: &Client, url: &str) -> String {
    if url.starts_with('/') {
        format!("{}{url}", c.web)
    } else {
        url.to_string()
    }
}

/// `WID/widget` from a repository object.
pub fn full_name(r: &Value) -> String {
    format!("{}/{}", r.s("project.key"), r.s("slug"))
}

/// The first of a list of links: `links.self[0].href`.
pub fn link(v: &Value, name: &str) -> String {
    v.list(&format!("links.{name}"))
        .first()
        .map(|l| l.s("href"))
        .unwrap_or_default()
}

pub fn user(c: &Client, u: &Value) -> Value {
    if u.is_null() || !(u.has("slug") || u.has("name")) {
        return Value::Null;
    }
    let login = if u.has("slug") {
        u.s("slug")
    } else {
        u.s("name")
    };
    let avatar = u.s("avatarUrl");
    json!({
        "login": login,
        "id": u.i("id"),
        "name": u.s("displayName"),
        "avatar_url": absolute(c, &avatar),
        "html_url": format!("{}/users/{login}", c.web),
        "type": "User",
        "bio": "",
        "location": "",
        "company": "",
        "blog": "",
        "email": u.s("emailAddress"),
        "twitter_username": "",
        "created_at": "",
        "followers": 0,
        "following": 0,
        "public_repos": 0,
        "public_gists": 0,
        "site_admin": u.s("type") == "SERVICE",
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

/// A project as an organization.
pub fn project(c: &Client, p: &Value) -> Value {
    let key = p.s("key");
    json!({
        "login": key,
        "id": p.i("id"),
        "name": p.s("name"),
        "description": p.s("description"),
        "avatar_url": format!("{}/projects/{key}/avatar.png?s=64", c.web),
        "html_url": format!("{}/projects/{key}", c.web),
        "type": "Organization",
        "public_repos": 0,
        "followers": 0,
        "location": "",
        "blog": "",
        "is_private": !p.b("public"),
        "created_at": "",
    })
}

/// A repository. `permission` is the signed-in user's (`read`, `write`,
/// `admin`) where it is known; `default_branch` likewise.
pub fn repo(c: &Client, r: &Value, permission: &str, default_branch: &str) -> Value {
    let name = full_name(r);
    let key = r.s("project.key");
    let personal = key.starts_with('~');
    let clone = |kind: &str| {
        r.list("links.clone")
            .iter()
            .find(|l| l.s("name") == kind)
            .map(|l| l.s("href"))
            .unwrap_or_default()
    };
    let web = crate::forge::datacenter_repo_web(&name, &c.web);
    let private = !r.b("public") && !r.b("project.public");
    json!({
        "id": r.i("id"),
        "name": r.s("name"),
        "slug": r.s("slug"),
        "full_name": name,
        "owner": {
            "login": key,
            "avatar_url": if personal { String::new() } else { format!("{}/projects/{key}/avatar.png?s=64", c.web) },
            "type": if personal { "User" } else { "Organization" },
        },
        "private": private,
        "visibility": if private { "private" } else { "public" },
        "description": r.s("description"),
        "html_url": format!("{web}/browse"),
        "clone_url": clone("http"),
        "ssh_url": clone("ssh"),
        "default_branch": default_branch,
        "stargazers_count": 0,
        "forks_count": 0,
        "subscribers_count": 0,
        "open_issues_count": 0,
        "topics": [],
        "archived": r.b("archived"),
        "fork": r.has("origin"),
        "parent": if r.has("origin") { json!({ "full_name": full_name(r.at("origin")) }) } else { Value::Null },
        "license": Value::Null,
        "homepage": "",
        "created_at": "",
        "pushed_at": "",
        "updated_at": "",
        "size": 0,
        "has_issues": false,
        "has_wiki": false,
        "has_discussions": false,
        "has_projects": false,
        "is_template": false,
        "language": Value::Null,
        "permissions": {
            "admin": permission == "admin",
            "maintain": permission == "admin",
            "push": matches!(permission, "admin" | "write"),
            "pull": true,
        },
        "project": { "key": key, "name": r.s("project.name") },
        "forkable": r.b("forkable"),
    })
}

pub fn repos(c: &Client, list: &[Value]) -> Value {
    Value::Array(list.iter().map(|r| repo(c, r, "", "")).collect())
}

/// `OPEN` as `open`; merged and declined are both closed.
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
    let source = full_name(v.at("fromRef.repository"));
    let source = if source == "/" {
        repo.to_string()
    } else {
        source
    };
    let branch = v.s("fromRef.displayId");
    let label = if source == repo {
        branch.clone()
    } else {
        format!("{}:{branch}", source.split('/').next().unwrap_or(""))
    };
    let reviewers: Vec<Value> = v
        .list("reviewers")
        .iter()
        .map(|r| r.at("user").clone())
        .collect();
    let draft = v.b("draft");
    let closed = when(v, "closedDate");
    let closed_at = if open || closed.is_empty() {
        if open {
            Value::Null
        } else {
            json!(when(v, "updatedDate"))
        }
    } else {
        json!(closed)
    };
    let value = json!({
        "id": n,
        "number": n,
        "title": v.s("title"),
        "body": v.s("description"),
        "state": state(&v.s("state")),
        "state_reason": if v.s("state") == "DECLINED" { "not_planned" } else { "" },
        "user": user(c, v.at("author.user")),
        "labels": [],
        "assignees": [],
        "milestone": Value::Null,
        "comments": v.i("properties.commentCount"),
        "created_at": when(v, "createdDate"),
        "updated_at": when(v, "updatedDate"),
        "closed_at": closed_at,
        "closed_by": Value::Null,
        "locked": v.b("locked"),
        "html_url": link(v, "self"),
        "repository_url": format!("/repos/{repo}"),
        "author_association": "",
        "reactions": {},
    });
    // In two, for json!'s sake.
    let extra = json!({
        "url": format!("/repos/{repo}/pulls/{n}"),
        "node_id": format!("dc:pr:{repo}#{n}"),
        "pull_request": { "url": format!("/repos/{repo}/pulls/{n}") },
        "merged": merged,
        "merged_at": if merged { closed_at.clone() } else { Value::Null },
        "merged_by": Value::Null,
        "draft": draft,
        "head": {
            "ref": branch,
            "label": label,
            "sha": v.s("fromRef.latestCommit"),
            "repo": { "full_name": source },
        },
        "base": {
            "ref": v.s("toRef.displayId"),
            "sha": v.s("toRef.latestCommit"),
            "repo": { "full_name": repo },
        },
        "requested_reviewers": users(c, &reviewers),
        // The full pull request asks whether it can merge, which says.
        "mergeable": if open { json!(!draft) } else { Value::Null },
        "mergeable_state": if !open { "" } else if draft { "draft" } else { "clean" },
        "merge_blocker": "",
        "auto_merge": Value::Null,
        "commits": 0,
        "additions": 0,
        "deletions": 0,
        "changed_files": 0,
        "review_comments": 0,
        "merge_commit_sha": or_null(v.s("properties.mergeCommit.id")),
        "task_count": v.i("properties.openTaskCount"),
        "version": v.i("version"),
    });
    let mut value = value;
    if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
        map.extend(extra);
    }
    value
}

/// A comment, anywhere: on a pull request (`url` says where to edit it)
/// or a commit. A task (a blocker comment) reads as a comment that says
/// so.
pub fn comment(c: &Client, cm: &Value, url: &str) -> Value {
    let task = cm.s("severity") == "BLOCKER";
    let body = if task {
        let done = cm.s("state") == "RESOLVED";
        format!("{} {}", if done { "- [x]" } else { "- [ ]" }, cm.s("text"))
    } else {
        cm.s("text")
    };
    json!({
        "id": cm.i("id"),
        "event": "commented",
        "body": body,
        "user": user(c, cm.at("author")),
        "created_at": when(cm, "createdDate"),
        "updated_at": when(cm, "updatedDate"),
        "url": url,
        "html_url": "",
        "author_association": "",
        "reactions": {},
        "path": cm.s("anchor.path"),
        "in_reply_to_id": Value::Null,
        "resolved": cm.b("threadResolved") || cm.s("state") == "RESOLVED",
    })
}

/// A commit. Its author is a person here when the instance matched their
/// email to an account.
pub fn commit(c: &Client, repo: &str, v: &Value) -> Value {
    let sha = v.s("id");
    let person = |field: &str| {
        let who = v.at(field);
        json!({
            "name": if who.has("displayName") { who.s("displayName") } else { who.s("name") },
            "email": who.s("emailAddress"),
            "date": when(v, &format!("{field}Timestamp")),
        })
    };
    let author = v.at("author");
    json!({
        "sha": sha,
        "commit": {
            "message": v.s("message"),
            "author": person("author"),
            "committer": person("committer"),
            "verification": { "verified": false },
        },
        "author": if author.has("slug") { user(c, author) } else { Value::Null },
        "parents": v.list("parents").iter().map(|p| json!({ "sha": p.s("id") })).collect::<Vec<_>>(),
        "html_url": format!("{}/commits/{sha}", crate::forge::datacenter_repo_web(repo, &c.web)),
        "url": format!("/repos/{repo}/commits/{sha}"),
    })
}

/// A file's part of a whole diff: every `diff --git` section, by the path
/// it is about. Data Center names the sides `src://` and `dst://` where
/// git says `a/` and `b/`.
pub fn split_diff(diff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(done) = current.take() {
                out.push(done);
            }
            let rest = rest.trim_end();
            let path = rest
                .rsplit_once(" dst://")
                .or_else(|| rest.rsplit_once(" b/"))
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
            let start = body
                .find("\n@@")
                .map(|i| i + 1)
                .or_else(|| body.starts_with("@@").then_some(0));
            let patch = start.map(|i| body[i..].to_string()).unwrap_or_default();
            (path, patch)
        })
        .collect()
}

/// How many lines a patch adds and removes.
pub fn counts(patch: &str) -> (i64, i64) {
    patch.lines().fold((0, 0), |(a, d), line| {
        if line.starts_with('+') {
            (a + 1, d)
        } else if line.starts_with('-') {
            (a, d + 1)
        } else {
            (a, d)
        }
    })
}

/// Changed files, from the instance's list of changes and the whole diff.
pub fn files(changes: &[Value], diff: &str) -> Value {
    let parts = split_diff(diff);
    Value::Array(
        changes
            .iter()
            .map(|ch| {
                let path = ch.s("path.toString");
                let old = ch.s("srcPath.toString");
                let patch = parts
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, body)| body.as_str())
                    .unwrap_or("");
                let (adds, dels) = counts(patch);
                let status = match ch.s("type").as_str() {
                    "ADD" | "COPY" => "added",
                    "DELETE" => "removed",
                    "MOVE" => "renamed",
                    _ => "modified",
                };
                json!({
                    "filename": path,
                    "previous_filename": if old.is_empty() { path.clone() } else { old },
                    "status": status,
                    "additions": adds,
                    "deletions": dels,
                    "changes": adds + dels,
                    "patch": patch,
                    "conflict": ch.has("conflict"),
                })
            })
            .collect(),
    )
}

/// A build status's state as GitHub's status and conclusion.
pub fn ci_status(state: &str) -> (&'static str, &'static str) {
    match state {
        "SUCCESSFUL" => ("completed", "success"),
        "FAILED" => ("completed", "failure"),
        "CANCELLED" => ("completed", "cancelled"),
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
    let date = when(s, "dateAdded");
    json!({
        "id": i as i64 + 1,
        "name": name,
        "status": status,
        "conclusion": or_null(conclusion.to_string()),
        "started_at": date,
        "completed_at": if status == "completed" { json!(date) } else { Value::Null },
        "details_url": s.s("url"),
        "html_url": s.s("url"),
        "app": { "name": "Bitbucket", "slug": "bitbucket" },
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
    let state = if states.iter().any(|s| s == "FAILED" || s == "CANCELLED") {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(&crate::forge::Account::new(
            crate::forge::Forge::DataCenter,
            "git.example.com",
            "",
        ))
    }

    #[test]
    fn times_read_as_iso() {
        assert_eq!(iso(1_791_733_912_004), "2026-10-11T15:51:52Z");
        assert_eq!(iso(0), "");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn pull_requests_read_as_github_does() {
        let c = client();
        let repo_json = json!({ "slug": "widget", "project": { "key": "WID" } });
        let fork = json!({ "slug": "widget", "project": { "key": "~ADA" } });
        let v = json!({
            "id": 7, "version": 3, "title": "Speed", "state": "OPEN", "draft": false,
            "createdDate": 1_791_733_912_004_i64, "updatedDate": 1_791_733_912_004_i64,
            "author": { "user": { "slug": "ada", "displayName": "Ada L", "avatarUrl": "/users/ada/avatar.png?s=64" } },
            "fromRef": { "displayId": "fast", "latestCommit": "abc", "repository": fork },
            "toRef": { "displayId": "main", "latestCommit": "def", "repository": repo_json },
            "reviewers": [{ "user": { "slug": "bob", "displayName": "Bob" }, "status": "APPROVED" }],
            "links": { "self": [{ "href": "https://git.example.com/projects/WID/repos/widget/pull-requests/7" }] },
        });
        let pr = pr(&c, "WID/widget", &v);
        assert_eq!(pr.i("number"), 7);
        assert_eq!(pr.s("state"), "open");
        assert_eq!(pr.s("user.login"), "ada");
        assert_eq!(
            pr.s("user.avatar_url"),
            "https://git.example.com/users/ada/avatar.png?s=64"
        );
        assert_eq!(pr.s("head.label"), "~ADA:fast");
        assert_eq!(pr.s("base.ref"), "main");
        assert_eq!(pr.s("requested_reviewers.0.login"), "bob");
        assert_eq!(pr.s("created_at"), "2026-10-11T15:51:52Z");
        assert_eq!(pr.i("version"), 3);
    }

    #[test]
    fn diffs_split_by_file() {
        let diff = "diff --git src://docs/speed.md dst://docs/speed.md\nnew file mode 100644\nindex 0000000..fe1fa84\n--- /dev/null\n+++ dst://docs/speed.md\n@@ -0,0 +1 @@\n+Go faster.\ndiff --git src://src/main.rs dst://src/main.rs\nindex 76a04e9..5ce7849 100644\n--- src://src/main.rs\n+++ dst://src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    a\n+    b\n }\n";
        let parts = split_diff(diff);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "docs/speed.md");
        assert_eq!(
            parts[1].1,
            "@@ -1,3 +1,3 @@\n fn main() {\n-    a\n+    b\n }\n"
        );
        let f = files(
            &[
                json!({ "type": "ADD", "path": { "toString": "docs/speed.md" } }),
                json!({ "type": "MODIFY", "path": { "toString": "src/main.rs" } }),
            ],
            diff,
        );
        assert_eq!(f.s("0.status"), "added");
        assert_eq!((f.i("1.additions"), f.i("1.deletions")), (1, 1));
        let mixed = rollup(&[
            json!({ "state": "SUCCESSFUL" }),
            json!({ "state": "INPROGRESS" }),
        ]);
        assert_eq!(mixed.s("state"), "PENDING");
    }
}
