//! GitLab's objects in GitHub's shape: the fields the screens read, named
//! and valued as GitHub would.
//!
//! Some fields have nowhere to come from and read as GitHub's "none" (no
//! watchers, no reactions beyond thumbs). Some fields are added that only
//! the GitLab translation itself reads back: `node_id`s that say which
//! issue or merge request a mutation is about, and `url`s in the shape of
//! GitHub's API paths that the translation knows how to answer.

use crate::api::Client;
use crate::json::Json as _;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// A URL GitLab gave as a path (`/uploads/…`), made whole.
pub fn abs(c: &Client, url: &str) -> String {
    if url.starts_with('/') {
        format!("{}{url}", c.web)
    } else {
        url.to_string()
    }
}

/// `opened` as `open`; everything else is some kind of closed.
pub fn state(s: &str) -> &'static str {
    if s == "opened" || s == "active" {
        "open"
    } else {
        "closed"
    }
}

/// A day (`2024-03-05`) as a GitHub timestamp.
fn day(date: &str) -> Value {
    if date.is_empty() {
        Value::Null
    } else {
        json!(format!("{date}T00:00:00Z"))
    }
}

fn or_null(s: String) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s)
    }
}

pub fn user(c: &Client, u: &Value) -> Value {
    if u.is_null() || !u.has("username") {
        return Value::Null;
    }
    let company = [
        u.s("organization"),
        u.s("work_information"),
        u.s("job_title"),
    ]
    .into_iter()
    .find(|s| !s.is_empty())
    .unwrap_or_default();
    json!({
        "login": u.s("username"),
        "id": u.i("id"),
        "name": u.s("name"),
        "avatar_url": abs(c, &u.s("avatar_url")),
        "html_url": u.s("web_url"),
        "type": "User",
        "bio": u.s("bio"),
        "location": u.s("location"),
        "company": company,
        "blog": u.s("website_url"),
        "email": u.s("public_email"),
        "twitter_username": u.s("twitter"),
        "created_at": u.s("created_at"),
        "followers": u.i("followers"),
        "following": u.i("following"),
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

/// A group as an organization.
pub fn group(c: &Client, g: &Value) -> Value {
    json!({
        "login": g.s("full_path"),
        "id": g.i("id"),
        "name": g.s("full_name"),
        "description": g.s("description"),
        "avatar_url": abs(c, &g.s("avatar_url")),
        "html_url": g.s("web_url"),
        "type": "Organization",
        "public_repos": 0,
        "followers": 0,
        "location": "",
        "blog": "",
        "visibility": g.s("visibility"),
        "parent_id": g.at("parent_id").clone(),
    })
}

/// The highest access the signed-in user has to a project: 10 guest, 20
/// reporter, 30 developer, 40 maintainer, 50 owner.
pub fn access(p: &Value) -> i64 {
    p.i("permissions.project_access.access_level")
        .max(p.i("permissions.group_access.access_level"))
}

pub fn project(c: &Client, p: &Value) -> Value {
    let ns = p.at("namespace");
    let level = access(p);
    let empty = p.b("empty_repo");
    let size = p.i("statistics.repository_size") / 1024;
    json!({
        "id": p.i("id"),
        "name": p.s("name"),
        "path": p.s("path"),
        "full_name": p.s("path_with_namespace"),
        "owner": {
            "login": ns.s("full_path"),
            "avatar_url": abs(c, &ns.s("avatar_url")),
            "type": if ns.s("kind") == "group" { "Organization" } else { "User" },
        },
        "private": p.s("visibility") == "private",
        "visibility": p.s("visibility"),
        "description": p.s("description"),
        "html_url": p.s("web_url"),
        "clone_url": p.s("http_url_to_repo"),
        "ssh_url": p.s("ssh_url_to_repo"),
        "default_branch": p.s("default_branch"),
        "stargazers_count": p.i("star_count"),
        "forks_count": p.i("forks_count"),
        "subscribers_count": 0,
        "open_issues_count": p.i("open_issues_count"),
        "topics": if p.has("topics") { p.at("topics").clone() } else { p.at("tag_list").clone() },
        "archived": p.b("archived"),
        "fork": p.has("forked_from_project"),
        "parent": if p.has("forked_from_project") { json!({ "full_name": p.s("forked_from_project.path_with_namespace") }) } else { Value::Null },
        "license": if p.has("license") { json!({ "name": p.s("license.name"), "spdx_id": p.s("license.key").to_uppercase() }) } else { Value::Null },
        "homepage": "",
        "created_at": p.s("created_at"),
        "pushed_at": if empty { Value::Null } else { json!(p.s("last_activity_at")) },
        "updated_at": p.s("last_activity_at"),
        "size": if empty { 0 } else { size.max(1) },
        // Signed out, GitLab leaves the feature fields out; say yes then.
        "has_issues": if p.has("issues_access_level") { p.s("issues_access_level") != "disabled" } else { !p.has("issues_enabled") || p.b("issues_enabled") },
        "has_wiki": p.b("wiki_enabled"),
        "has_discussions": false,
        "has_projects": false,
        "is_template": false,
        "language": Value::Null,
        "permissions": { "admin": level >= 40, "maintain": level >= 40, "push": level >= 30, "pull": true },
        "access_level": level,
        "avatar_url": abs(c, &p.s("avatar_url")),
        "empty_repo": empty,
        "merge_method": p.s("merge_method"),
        "squash_option": p.s("squash_option"),
    })
}

pub fn projects(c: &Client, list: &[Value]) -> Value {
    Value::Array(list.iter().map(|p| project(c, p)).collect())
}

/// An access level's name.
pub fn role_name(level: i64) -> &'static str {
    match level {
        50.. => "Owner",
        40..=49 => "Maintainer",
        30..=39 => "Developer",
        20..=29 => "Reporter",
        5..=9 => "Minimal access",
        _ => "Guest",
    }
}

pub fn label(l: &Value) -> Value {
    json!({
        "id": l.i("id"),
        "name": l.s("name"),
        "color": l.s("color").trim_start_matches('#'),
        "description": l.s("description"),
    })
}

/// An issue's labels, which GitLab gives as names (or, asked for, with
/// their details); `colors` fills in what names alone lack.
fn labels(list: &[Value], colors: &HashMap<String, String>) -> Value {
    Value::Array(
        list.iter()
            .map(|l| match l {
                Value::String(name) => json!({ "name": name, "color": colors.get(name).cloned().unwrap_or_else(|| "ededed".into()) }),
                other => label(other),
            })
            .collect(),
    )
}

pub fn milestone(m: &Value) -> Value {
    if m.is_null() {
        return Value::Null;
    }
    json!({
        "id": m.i("id"),
        "number": m.i("id"),
        "title": m.s("title"),
        "description": m.s("description"),
        "state": state(&m.s("state")),
        "due_on": day(&m.s("due_date")),
        "open_issues": m.i("open_issues"),
        "closed_issues": m.i("closed_issues"),
        "html_url": m.s("web_url"),
    })
}

/// The project an issue, merge request or note from a list across
/// projects belongs to: `group/project` from `group/project#12`.
pub fn repo_of(item: &Value) -> String {
    let full = item.s("references.full");
    if let Some(at) = full.rfind(['#', '!', '&']) {
        return full[..at].to_string();
    }
    crate::forge::repo_of(&item.s("web_url"))
}

/// What both kinds have: number, title, people, labels, dates.
fn issuelike(
    c: &Client,
    repo: &str,
    v: &Value,
    colors: &HashMap<String, String>,
) -> Map<String, Value> {
    let up = v.i("upvotes");
    let down = v.i("downvotes");
    let value = json!({
        "id": v.i("id"),
        "number": v.i("iid"),
        "title": v.s("title"),
        "body": v.s("description"),
        "state": state(&v.s("state")),
        "state_reason": "",
        "user": user(c, v.at("author")),
        "labels": labels(v.list("labels"), colors),
        "assignees": users(c, v.list("assignees")),
        "milestone": milestone(v.at("milestone")),
        "comments": v.i("user_notes_count"),
        "created_at": v.s("created_at"),
        "updated_at": v.s("updated_at"),
        "closed_at": v.at("closed_at").clone(),
        "closed_by": user(c, v.at("closed_by")),
        "locked": v.b("discussion_locked"),
        "html_url": v.s("web_url"),
        "repository_url": format!("/repos/{repo}"),
        "author_association": "",
        "reactions": { "+1": up, "-1": down, "total_count": up + down },
    });
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

pub fn issue(c: &Client, repo: &str, v: &Value, colors: &HashMap<String, String>) -> Value {
    let mut map = issuelike(c, repo, v, colors);
    let n = v.i("iid");
    map.insert("url".into(), json!(format!("/repos/{repo}/issues/{n}")));
    map.insert("node_id".into(), json!(format!("gl:issue:{repo}#{n}")));
    map.insert("kind".into(), json!("issue"));
    Value::Object(map)
}

/// Why a merge request can't merge yet, in GitHub's few words and in
/// GitLab's own.
fn merge_state(v: &Value) -> (&'static str, &'static str) {
    if v.s("state") != "opened" {
        return ("", "");
    }
    if v.b("draft") || v.b("work_in_progress") {
        return ("draft", "");
    }
    if v.b("has_conflicts") {
        return ("dirty", "");
    }
    match v.s("detailed_merge_status").as_str() {
        "mergeable" => ("clean", ""),
        "conflict" | "broken_status" => ("dirty", ""),
        "need_rebase" => ("behind", ""),
        "draft_status" => ("draft", ""),
        "ci_must_pass" => (
            "blocked",
            "The pipeline must succeed before this can merge.",
        ),
        "ci_still_running" => ("blocked", "The pipeline is still running."),
        "discussions_not_resolved" => ("blocked", "All threads must be resolved."),
        "not_approved" => ("blocked", "It needs the approvals its rules ask for."),
        "requested_changes" => ("blocked", "A reviewer has asked for changes."),
        "blocked_status" | "merge_request_blocked" => {
            ("blocked", "Another merge request must merge first.")
        }
        "jira_association_missing" => (
            "blocked",
            "The title or description must name a Jira issue.",
        ),
        "status_checks_must_pass" | "external_status_checks" => {
            ("blocked", "External status checks must pass.")
        }
        "security_policy_violations" => ("blocked", "A security policy blocks merging."),
        "locked_paths" | "locked_lfs_files" => ("blocked", "It changes locked files."),
        "title_regex" => ("blocked", "The title doesn't match the project's rules."),
        "commits_status" => ("blocked", "The source branch's commits can't be merged."),
        "not_open" => ("", ""),
        // unchecked, checking, preparing, approvals_syncing
        _ => ("unknown", ""),
    }
}

pub fn mr(
    c: &Client,
    repo: &str,
    v: &Value,
    colors: &HashMap<String, String>,
    source: &str,
) -> Value {
    let mut map = issuelike(c, repo, v, colors);
    let n = v.i("iid");
    let merged = v.s("state") == "merged";
    let (mergeable_state, blocker) = merge_state(v);
    let branch = v.s("source_branch");
    let source = if source.is_empty() {
        repo.to_string()
    } else {
        source.to_string()
    };
    let label = if source == repo {
        branch.clone()
    } else {
        format!("{source}:{branch}")
    };
    let extra = json!({
        "url": format!("/repos/{repo}/pulls/{n}"),
        "node_id": format!("gl:mr:{repo}!{n}"),
        "kind": "merge_request",
        "pull_request": { "url": format!("/repos/{repo}/pulls/{n}") },
        "merged": merged,
        "merged_at": if merged { v.at("merged_at").clone() } else { Value::Null },
        "merged_by": user(c, if v.has("merged_by") { v.at("merged_by") } else { v.at("merge_user") }),
        "draft": v.b("draft") || v.b("work_in_progress"),
        "head": { "ref": branch, "label": label, "sha": v.s("sha"), "repo": { "full_name": source } },
        "base": { "ref": v.s("target_branch"), "repo": { "full_name": repo } },
        "requested_reviewers": users(c, v.list("reviewers")),
        "mergeable": match mergeable_state { "unknown" => Value::Null, s => json!(s == "clean") },
        "mergeable_state": mergeable_state,
        "merge_blocker": blocker,
        "auto_merge": if v.b("merge_when_pipeline_succeeds") { json!({ "enabled_by": user(c, v.at("merge_user")) }) } else { Value::Null },
        "commits": 0,
        "additions": 0,
        "deletions": 0,
        "changed_files": v.s("changes_count").trim_end_matches('+').parse::<i64>().unwrap_or(0),
        "review_comments": 0,
        "merge_commit_sha": or_null(if v.has("merge_commit_sha") { v.s("merge_commit_sha") } else { v.s("squash_commit_sha") }),
        "diff_refs": v.at("diff_refs").clone(),
        "squash": v.b("squash"),
    });
    if let Value::Object(extra) = extra {
        map.extend(extra);
    }
    Value::Object(map)
}

/// A note (a comment) on an issue or merge request: `kind` is
/// `issues` or `merge_requests`.
pub fn note(c: &Client, repo: &str, kind: &str, number: i64, n: &Value, item_url: &str) -> Value {
    let id = n.i("id");
    json!({
        "id": id,
        "event": "commented",
        "body": n.s("body"),
        "user": user(c, n.at("author")),
        "created_at": n.s("created_at"),
        "updated_at": n.s("updated_at"),
        "url": format!("/repos/{repo}/{kind}/{number}/notes/{id}"),
        "html_url": format!("{item_url}#note_{id}"),
        "author_association": "",
        "reactions": {},
    })
}

/// A system note ("added ~bug label", "approved this merge request") as
/// a timeline line.
pub fn system_note(c: &Client, n: &Value) -> Value {
    json!({
        "event": "system_note",
        "id": n.i("id"),
        "actor": user(c, n.at("author")),
        "body": n.s("body"),
        "created_at": n.s("created_at"),
    })
}

/// GitHub's reaction names for GitLab's award emoji, and back.
pub const REACTIONS: [(&str, &str); 8] = [
    ("+1", "thumbsup"),
    ("-1", "thumbsdown"),
    ("laugh", "laughing"),
    ("hooray", "tada"),
    ("confused", "confused"),
    ("heart", "heart"),
    ("rocket", "rocket"),
    ("eyes", "eyes"),
];

pub fn reaction_name(github: &str) -> &str {
    REACTIONS
        .iter()
        .find(|(g, _)| *g == github)
        .map(|(_, l)| *l)
        .unwrap_or(github)
}

pub fn reaction_content(gitlab: &str) -> &str {
    REACTIONS
        .iter()
        .find(|(_, l)| *l == gitlab)
        .map(|(g, _)| *g)
        .unwrap_or(gitlab)
}

/// Award emoji as a reaction list and as GitHub's counts.
pub fn reactions(c: &Client, awards: &[Value]) -> (Value, Value) {
    let list: Vec<Value> = awards
        .iter()
        .map(|a| json!({ "id": a.i("id"), "content": reaction_content(&a.s("name")), "user": user(c, a.at("user")) }))
        .collect();
    let mut counts = Map::new();
    for (github, _) in REACTIONS {
        let n = list.iter().filter(|r| r.s("content") == github).count();
        if n > 0 {
            counts.insert(github.to_string(), json!(n));
        }
    }
    counts.insert("total_count".into(), json!(list.len()));
    (Value::Array(list), Value::Object(counts))
}

pub fn commit(repo: &str, v: &Value, avatar: Option<&String>) -> Value {
    let sha = v.s("id");
    let author = json!({ "name": v.s("author_name"), "email": v.s("author_email"), "date": v.s("authored_date") });
    json!({
        "sha": sha,
        "commit": {
            "message": v.s("message"),
            "author": author,
            "committer": { "name": v.s("committer_name"), "email": v.s("committer_email"), "date": v.s("committed_date") },
            "verification": { "verified": false },
        },
        "author": match avatar { Some(url) if !url.is_empty() => json!({ "avatar_url": url }), _ => Value::Null },
        "parents": v.list("parent_ids").iter().map(|p| json!({ "sha": p })).collect::<Vec<_>>(),
        "html_url": v.s("web_url"),
        "url": format!("/repos/{repo}/commits/{sha}"),
        "stats": { "additions": v.i("stats.additions"), "deletions": v.i("stats.deletions"), "total": v.i("stats.total") },
    })
}

/// A changed file from GitLab's diff list.
pub fn file(d: &Value) -> Value {
    let patch = d.s("diff");
    let mut adds = 0;
    let mut dels = 0;
    for line in patch.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            adds += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            dels += 1;
        }
    }
    let status = if d.b("new_file") {
        "added"
    } else if d.b("deleted_file") {
        "removed"
    } else if d.b("renamed_file") {
        "renamed"
    } else {
        "modified"
    };
    json!({
        "filename": d.s("new_path"),
        "previous_filename": d.s("old_path"),
        "status": status,
        "additions": adds,
        "deletions": dels,
        "changes": adds + dels,
        "patch": patch,
    })
}

/// GitLab's CI statuses as GitHub's status and conclusion.
pub fn ci_status(s: &str) -> (&'static str, &'static str) {
    match s {
        "success" => ("completed", "success"),
        "failed" => ("completed", "failure"),
        "canceled" | "canceling" => ("completed", "cancelled"),
        "skipped" => ("completed", "skipped"),
        "manual" => ("completed", "action_required"),
        "running" => ("in_progress", ""),
        _ => ("queued", ""),
    }
}

/// A CI job as a check run.
pub fn check_run(j: &Value) -> Value {
    let (status, conclusion) = ci_status(&j.s("status"));
    json!({
        "id": j.i("id"),
        "name": j.s("name"),
        "status": status,
        "conclusion": or_null(conclusion.to_string()),
        "started_at": j.at("started_at").clone(),
        "completed_at": j.at("finished_at").clone(),
        "details_url": j.s("web_url"),
        "html_url": j.s("web_url"),
        "app": { "name": "GitLab CI", "slug": "gitlab-ci" },
        "output": { "title": format!("{} stage", j.s("stage")) },
        "stage": j.s("stage"),
    })
}

/// A pipeline's status as GraphQL's `statusCheckRollup.state`.
pub fn rollup(status: &str) -> &'static str {
    match ci_status(status) {
        (_, "success") | (_, "skipped") => "SUCCESS",
        (_, "failure") | (_, "cancelled") => "FAILURE",
        ("completed", _) => "SUCCESS",
        _ => "PENDING",
    }
}

pub fn release(c: &Client, repo: &str, r: &Value) -> Value {
    let tag = r.s("tag_name");
    let assets: Vec<Value> = r
        .list("assets.links")
        .iter()
        .map(|l| {
            let url = if l.has("direct_asset_url") { l.s("direct_asset_url") } else { l.s("url") };
            json!({ "id": l.i("id"), "name": l.s("name"), "size": 0, "download_count": 0, "browser_download_url": url, "updated_at": "" })
        })
        .collect();
    json!({
        "id": crate::forge::release_id(&tag),
        "tag_name": tag,
        "name": r.s("name"),
        "body": r.s("description"),
        "draft": false,
        "prerelease": r.b("upcoming_release"),
        "created_at": r.s("created_at"),
        "published_at": r.s("released_at"),
        "author": user(c, r.at("author")),
        "target_commitish": r.s("commit.id"),
        "html_url": r.s("_links.self"),
        "assets": assets,
        "reactions": {},
        "url": format!("/repos/{repo}/releases/{}", crate::forge::release_id(&r.s("tag_name"))),
    })
}

/// A snippet as a gist; `contents` are its files' text, by name, when
/// they have been read.
pub fn snippet(c: &Client, s: &Value, contents: &HashMap<String, String>) -> Value {
    let mut files = Map::new();
    let listed: Vec<(String, String)> = if s.list("files").is_empty() {
        vec![(s.s("file_name"), s.s("raw_url"))]
    } else {
        s.list("files")
            .iter()
            .map(|f| (f.s("path"), f.s("raw_url")))
            .collect()
    };
    for (name, raw) in listed {
        if name.is_empty() {
            continue;
        }
        let content = contents.get(&name).cloned().unwrap_or_default();
        files.insert(
            name.clone(),
            json!({ "filename": name, "content": content, "size": content.len(), "language": "", "raw_url": raw }),
        );
    }
    let description = match (s.s("title"), s.s("description")) {
        (title, d) if d.is_empty() => title,
        (title, d) => format!("{title}\n{d}"),
    };
    json!({
        "id": s.i("id").to_string(),
        "description": description,
        "public": s.s("visibility") == "public",
        "visibility": s.s("visibility"),
        "owner": user(c, s.at("author")),
        "files": files,
        "comments": 0,
        "created_at": s.s("created_at"),
        "updated_at": s.s("updated_at"),
        "git_pull_url": s.s("http_url_to_repo"),
        "html_url": s.s("web_url"),
        "forks": [],
    })
}

/// An activity event as one of GitHub's, or nothing for the kinds the
/// feed has no words for.
pub fn event(c: &Client, e: &Value, repo: &str) -> Option<Value> {
    let action = e.s("action_name");
    let target = e.s("target_type");
    let actor =
        json!({ "login": e.s("author.username"), "avatar_url": abs(c, &e.s("author.avatar_url")) });
    let number = e.i("target_iid");
    let title = e.s("target_title");
    let (kind, payload) = if e.has("push_data") {
        let p = e.at("push_data");
        let reference = p.s("ref");
        match p.s("action").as_str() {
            "created" => (
                "CreateEvent",
                json!({ "ref_type": p.s("ref_type"), "ref": reference }),
            ),
            "removed" => (
                "DeleteEvent",
                json!({ "ref_type": p.s("ref_type"), "ref": reference }),
            ),
            _ => (
                "PushEvent",
                json!({
                    "ref": format!("refs/heads/{reference}"),
                    "size": p.i("commit_count"),
                    "commits": if p.has("commit_title") { json!([{ "message": p.s("commit_title") }]) } else { json!([]) },
                }),
            ),
        }
    } else if target == "Issue" || target == "WorkItem" {
        let action = match action.as_str() {
            "opened" | "closed" | "reopened" => action.clone(),
            _ => return None,
        };
        (
            "IssuesEvent",
            json!({ "action": action, "issue": { "number": number, "title": title } }),
        )
    } else if target == "MergeRequest" {
        match action.as_str() {
            "approved" => (
                "PullRequestReviewEvent",
                json!({ "pull_request": { "number": number, "title": title } }),
            ),
            "accepted" => (
                "PullRequestEvent",
                json!({ "action": "closed", "number": number, "pull_request": { "title": title, "merged": true } }),
            ),
            "opened" | "closed" | "reopened" => (
                "PullRequestEvent",
                json!({ "action": action, "number": number, "pull_request": { "title": title, "merged": false } }),
            ),
            _ => return None,
        }
    } else if e.has("note") {
        let note = e.at("note");
        let on = note.s("noteable_type");
        let n = note.i("noteable_iid");
        match on.as_str() {
            "Issue" => (
                "IssueCommentEvent",
                json!({ "issue": { "number": n, "title": title }, "comment": { "body": note.s("body") } }),
            ),
            "MergeRequest" => (
                "IssueCommentEvent",
                json!({ "issue": { "number": n, "title": title, "pull_request": {} }, "comment": { "body": note.s("body") } }),
            ),
            "Commit" => (
                "CommitCommentEvent",
                json!({ "comment": { "body": note.s("body") } }),
            ),
            _ => return None,
        }
    } else if action == "created" && target.is_empty() {
        (
            "CreateEvent",
            json!({ "ref_type": "repository", "description": "" }),
        )
    } else if action == "joined" {
        (
            "MemberEvent",
            json!({ "action": "joined", "member": { "login": e.s("author.username") } }),
        )
    } else if target == "WikiPage::Meta" || target == "WikiPage" {
        ("GollumEvent", json!({}))
    } else {
        return None;
    };
    Some(json!({
        "id": e.i("id").to_string(),
        "type": kind,
        "actor": actor,
        "repo": { "name": repo },
        "payload": payload,
        "created_at": e.s("created_at"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(&crate::forge::Account::new(
            crate::forge::Forge::GitLab,
            "gitlab.example.com",
            "",
        ))
    }

    #[test]
    fn merge_requests_read_as_pull_requests() {
        let c = client();
        crate::forge::set_current(&crate::forge::Account::new(
            crate::forge::Forge::GitLab,
            "gitlab.example.com",
            "",
        ));
        let v = json!({
            "iid": 7, "title": "Draft: speed", "state": "opened", "draft": true,
            "author": { "username": "ada", "avatar_url": "/uploads/a.png" },
            "labels": ["bug"], "source_branch": "fast", "target_branch": "main", "sha": "abc",
            "detailed_merge_status": "draft_status", "changes_count": "1000+",
        });
        let mut colors = HashMap::new();
        colors.insert("bug".to_string(), "d73a4a".to_string());
        let pr = mr(&c, "g/sub/p", &v, &colors, "");
        assert_eq!(pr.i("number"), 7);
        assert_eq!(pr.s("state"), "open");
        assert_eq!(pr.s("user.login"), "ada");
        assert_eq!(
            pr.s("user.avatar_url"),
            "https://gitlab.example.com/uploads/a.png"
        );
        assert_eq!(pr.s("labels.0.color"), "d73a4a");
        assert_eq!(pr.s("head.label"), "fast");
        assert_eq!(pr.s("mergeable_state"), "draft");
        assert_eq!(pr.i("changed_files"), 1000);
        assert!(pr.has("pull_request"));
        assert!(!pr.has("merged_at"));
        assert_eq!(
            crate::screens::common::issue_route(&pr),
            crate::hub::Route::Pull {
                repo: "g/sub/p".into(),
                number: 7,
                tab: crate::hub::PullTab::Conversation
            }
        );
    }

    #[test]
    fn diffs_count_their_lines() {
        let f = file(
            &json!({ "new_path": "a.rs", "old_path": "a.rs", "diff": "@@ -1,2 +1,2 @@\n-a\n+b\n c\n" }),
        );
        assert_eq!(
            (f.i("additions"), f.i("deletions"), f.s("status").as_str()),
            (1, 1, "modified")
        );
        assert_eq!(rollup("failed"), "FAILURE");
        assert_eq!(ci_status("running"), ("in_progress", ""));
        assert_eq!(
            repo_of(&json!({ "references": { "full": "a/b/c!4" } })),
            "a/b/c"
        );
    }
}
