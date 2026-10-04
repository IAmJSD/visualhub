//! Issues and merge requests, asked for as GitHub's issues and pull
//! requests.
//!
//! GitHub numbers issues and pull requests together and reaches a pull
//! request's comments, labels and timeline through `/issues/{n}`. GitLab
//! numbers them apart, so on GitLab the screens reach a merge request's
//! as `/merge_requests/{n}/…` instead, answered here alongside
//! `/issues/{n}/…`.

use super::repos::{label_colors, shape_commits};
use super::{call, get, get_all, missing, parallel, proj, shape, user_id, Ask};
use crate::api::{self, Client};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Issue,
    Mr,
}

impl Kind {
    /// The path part, on GitLab and in the paths the screens use.
    fn part(self) -> &'static str {
        match self {
            Kind::Issue => "issues",
            Kind::Mr => "merge_requests",
        }
    }
}

pub fn route(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let m = a.method;
    let number = |n: &str| n.parse::<i64>().map_err(|_| missing(&format!("issue called {n}")));
    match rest {
        ["issues"] if m == "GET" => list(a, repo, Kind::Issue),
        ["issues"] => create_issue(a, repo),
        ["issues", n, tail @ ..] => item(a, repo, Kind::Issue, number(n)?, tail),
        ["merge_requests", n, "discussions", thread, "notes", note] => {
            let path = format!("{p}/merge_requests/{n}/discussions/{thread}/notes/{note}");
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => call(c, "PUT", &path, Some(&json!({ "body": a.field("body") }))),
            }
        }
        ["merge_requests", n, tail @ ..] => item(a, repo, Kind::Mr, number(n)?, tail),
        ["pulls"] if m == "GET" => list(a, repo, Kind::Mr),
        ["pulls"] => create_mr(a, repo),
        ["pulls", n] if m == "GET" => get_mr(c, repo, number(n)?),
        ["pulls", n] => update(a, repo, Kind::Mr, number(n)?),
        ["pulls", n, "commits"] => {
            let list = get(c, &format!("{p}/merge_requests/{n}/commits?{}", a.paging()))?;
            Ok(shape_commits(c, repo, list.list("")))
        }
        ["pulls", n, "files"] => files(a, repo, number(n)?),
        ["pulls", n, "comments"] if m == "GET" => review_comments(c, repo, number(n)?),
        ["pulls", n, "comments"] => line_comment(a, repo, number(n)?),
        ["pulls", n, "comments", id, "replies"] => {
            let id: i64 = id.parse().unwrap_or(0);
            let threads = get_all(c, &format!("{p}/merge_requests/{n}/discussions"), 10)?;
            let thread = threads
                .iter()
                .find(|t| t.list("notes").iter().any(|n| n.i("id") == id))
                .ok_or_else(|| missing("thread for that comment"))?;
            call(c, "POST", &format!("{p}/merge_requests/{n}/discussions/{}/notes", thread.s("id")), Some(&json!({ "body": a.field("body") })))
        }
        ["pulls", n, "reviews"] if m == "GET" => reviews(c, repo, number(n)?),
        ["pulls", n, "reviews"] => review(a, repo, number(n)?),
        ["pulls", n, "requested_reviewers"] => {
            let n = number(n)?;
            let current: Vec<i64> = get(c, &format!("{p}/merge_requests/{n}"))?.list("reviewers").iter().map(|u| u.i("id")).collect();
            let ids = change(c, current, a.field("reviewers").as_array().map(Vec::as_slice).unwrap_or(&[]), m != "DELETE")?;
            call(c, "PUT", &format!("{p}/merge_requests/{n}"), Some(&json!({ "reviewer_ids": ids })))
        }
        ["pulls", n, "merge"] => merge(a, repo, number(n)?),
        ["pulls", n, "update-branch"] => call(c, "PUT", &format!("{p}/merge_requests/{n}/rebase"), None),
        _ => Err(missing(&format!("equivalent of {}", rest.join("/")))),
    }
}

/// The ids for `logins`, added to or taken from `current`.
fn change(c: &Client, mut current: Vec<i64>, logins: &[Value], add: bool) -> Result<Vec<i64>> {
    for login in logins.iter().filter_map(|l| l.as_str()) {
        let id = user_id(c, login)?;
        current.retain(|x| *x != id);
        if add {
            current.push(id);
        }
    }
    Ok(current)
}

fn ids(c: &Client, logins: &Value) -> Result<Vec<i64>> {
    change(c, Vec::new(), logins.as_array().map(Vec::as_slice).unwrap_or(&[]), true)
}

/// GitHub's list filters as GitLab's.
fn list(a: &Ask, repo: &str, kind: Kind) -> Result<Value> {
    let c = a.c;
    let state = match (a.query.get("state").unwrap_or("open"), kind) {
        ("open", _) => "opened",
        ("all", _) => "all",
        ("merged", Kind::Mr) => "merged",
        _ => "closed",
    };
    let order = match a.query.get("sort") {
        Some("updated") | Some("comments") => "updated_at",
        _ => "created_at",
    };
    let direction = if a.query.get("direction") == Some("asc") { "asc" } else { "desc" };
    let mut path = format!(
        "{}/{}?state={state}&order_by={order}&sort={direction}&with_labels_details=true&{}",
        proj(repo),
        kind.part(),
        a.paging()
    );
    if let Some(labels) = a.query.get("labels") {
        path.push_str(&format!("&labels={}", enc(labels)));
    }
    let list = get(c, &path)?;
    let none = HashMap::new();
    Ok(Value::Array(
        list.list("")
            .iter()
            .map(|v| match kind {
                Kind::Issue => shape::issue(c, repo, v, &none),
                Kind::Mr => shape::mr(c, repo, v, &none, ""),
            })
            .collect(),
    ))
}

/// GitHub's issue fields as GitLab's update.
fn update_body(c: &Client, a: &Ask, kind: Kind) -> Result<Value> {
    let mut body = json!({});
    if a.field("title").is_string() {
        body["title"] = a.field("title").clone();
    }
    if a.field("body").is_string() {
        body["description"] = a.field("body").clone();
    }
    match a.field("state").as_str() {
        Some("closed") => body["state_event"] = json!("close"),
        Some("open") => body["state_event"] = json!("reopen"),
        _ => {}
    }
    match a.field("milestone") {
        // `"milestone": null` takes it off.
        Value::Null if a.body.is_some_and(|b| b.get("milestone").is_some()) => body["milestone_id"] = json!(0),
        Value::Number(n) => body["milestone_id"] = json!(n),
        _ => {}
    }
    if let Some(labels) = a.field("labels").as_array() {
        body["labels"] = json!(labels.iter().filter_map(|l| l.as_str()).collect::<Vec<_>>().join(","));
    }
    if a.field("assignees").is_array() {
        body["assignee_ids"] = json!(ids(c, a.field("assignees"))?);
    }
    if kind == Kind::Mr {
        if let Some(base) = a.field("base").as_str() {
            body["target_branch"] = json!(base);
        }
    }
    Ok(body)
}

fn update(a: &Ask, repo: &str, kind: Kind, n: i64) -> Result<Value> {
    let c = a.c;
    let body = update_body(c, a, kind)?;
    let updated = call(c, "PUT", &format!("{}/{}/{n}", proj(repo), kind.part()), Some(&body))?;
    let colors = label_colors(c, repo);
    Ok(match kind {
        Kind::Issue => shape::issue(c, repo, &updated, &colors),
        Kind::Mr => shape::mr(c, repo, &updated, &colors, ""),
    })
}

fn create_issue(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let mut body = update_body(c, a, Kind::Issue)?;
    body.as_object_mut().map(|b| b.remove("state_event"));
    let created = call(c, "POST", &format!("{}/issues", proj(repo)), Some(&body))?;
    Ok(shape::issue(c, repo, &created, &label_colors(c, repo)))
}

fn create_mr(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let head = a.field("head").as_str().unwrap_or("");
    // `owner:branch` names a fork's branch; GitLab asks the fork for it.
    let (source_project, branch) = match head.split_once(':') {
        Some((owner, branch)) => (Some(owner.to_string()), branch.to_string()),
        None => (None, head.to_string()),
    };
    let title = a.field("title").as_str().unwrap_or("").to_string();
    let draft = a.field("draft").as_bool().unwrap_or(false);
    let mut body = json!({
        "source_branch": branch,
        "target_branch": a.field("base"),
        "title": if draft { format!("Draft: {title}") } else { title },
        "description": a.field("body"),
    });
    let on = match source_project {
        Some(owner) => {
            // The fork of this project under `owner`.
            let name = crate::forge::split_repo(repo).1;
            let fork = format!("{owner}/{name}");
            body["target_project_id"] = json!(get(c, &proj(repo))?.i("id"));
            fork
        }
        None => repo.to_string(),
    };
    let created = call(c, "POST", &format!("{}/merge_requests", proj(&on)), Some(&body))?;
    Ok(shape::mr(c, repo, &created, &label_colors(c, repo), &on))
}

/// One merge request, with what GitLab's REST leaves to GraphQL: how
/// many commits, and how many lines change.
pub fn get_mr(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let p = proj(repo);
    let v = get(c, &format!("{p}/merge_requests/{n}"))?;
    let source = if v.i("source_project_id") != v.i("target_project_id") {
        super::project_path(c, v.i("source_project_id"))
    } else {
        String::new()
    };
    let mut shaped = shape::mr(c, repo, &v, &label_colors(c, repo), &source);
    let query = "query($p: ID!, $n: String!) { project(fullPath: $p) { mergeRequest(iid: $n) { commitCount diffStatsSummary { additions deletions fileCount } } } }";
    if let Ok(data) = c.gitlab_graphql(query, json!({ "p": repo, "n": n.to_string() })) {
        let mr = data.at("project.mergeRequest");
        shaped["commits"] = json!(mr.i("commitCount"));
        shaped["additions"] = json!(mr.i("diffStatsSummary.additions"));
        shaped["deletions"] = json!(mr.i("diffStatsSummary.deletions"));
        if mr.has("diffStatsSummary.fileCount") {
            shaped["changed_files"] = json!(mr.i("diffStatsSummary.fileCount"));
        }
    }
    Ok(shaped)
}

/// An issue, or a merge request seen as one, and what hangs off it.
fn item(a: &Ask, repo: &str, kind: Kind, n: i64, tail: &[&str]) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/{}/{n}", proj(repo), kind.part());
    let m = a.method;
    match tail {
        [] if m == "GET" => {
            let mut shaped = match kind {
                Kind::Issue => {
                    let found = get(c, &format!("{}/issues?iids[]={n}&with_labels_details=true&scope=all&state=all", proj(repo)))?;
                    let v = found.list("").first().cloned().ok_or_else(|| {
                        anyhow::Error::new(api::Status { code: 404, message: format!("There's no issue #{n} here.") })
                    })?;
                    shape::issue(c, repo, &v, &HashMap::new())
                }
                Kind::Mr => get_mr(c, repo, n)?,
            };
            let awards = get(c, &format!("{base}/award_emoji?per_page=100"))?;
            shaped["reactions"] = shape::reactions(c, awards.list("")).1;
            Ok(shaped)
        }
        [] => update(a, repo, kind, n),
        ["comments"] if m == "GET" => {
            let notes = get(c, &format!("{base}/notes?sort=asc&order_by=created_at&{}", a.paging()))?;
            let page = item_url(c, repo, kind, n);
            Ok(Value::Array(
                notes.list("").iter().filter(|n| !n.b("system")).map(|note| shape::note(c, repo, kind.part(), n, note, &page)).collect(),
            ))
        }
        ["comments"] => call(c, "POST", &format!("{base}/notes"), Some(&json!({ "body": a.field("body") }))),
        ["timeline"] => timeline(a, repo, kind, n),
        ["reactions"] if m == "GET" => awards(a, &base),
        ["reactions"] => call(c, "POST", &format!("{base}/award_emoji"), Some(&json!({ "name": shape::reaction_name(a.field("content").as_str().unwrap_or("+1")) }))),
        ["reactions", id] => call(c, "DELETE", &format!("{base}/award_emoji/{id}"), None),
        ["notes", note] => match m {
            "DELETE" => call(c, "DELETE", &format!("{base}/notes/{note}"), None),
            _ => call(c, "PUT", &format!("{base}/notes/{note}"), Some(&json!({ "body": a.field("body") }))),
        },
        ["notes", note, "reactions"] if m == "GET" => awards(a, &format!("{base}/notes/{note}")),
        ["notes", note, "reactions"] => call(
            c,
            "POST",
            &format!("{base}/notes/{note}/award_emoji"),
            Some(&json!({ "name": shape::reaction_name(a.field("content").as_str().unwrap_or("+1")) })),
        ),
        ["notes", note, "reactions", id] => call(c, "DELETE", &format!("{base}/notes/{note}/award_emoji/{id}"), None),
        ["labels"] => {
            let names: Vec<&str> = a.field("labels").as_array().map(|l| l.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            call(c, "PUT", &base, Some(&json!({ "add_labels": names.join(",") })))
        }
        ["labels", name] => call(c, "PUT", &base, Some(&json!({ "remove_labels": name }))),
        ["assignees"] => {
            let current: Vec<i64> = get(c, &base)?.list("assignees").iter().map(|u| u.i("id")).collect();
            let ids = change(c, current, a.field("assignees").as_array().map(Vec::as_slice).unwrap_or(&[]), m != "DELETE")?;
            call(c, "PUT", &base, Some(&json!({ "assignee_ids": ids })))
        }
        ["lock"] => call(c, "PUT", &base, Some(&json!({ "discussion_locked": m != "DELETE" }))),
        _ => Err(missing(&format!("equivalent of {}", tail.join("/")))),
    }
}

/// The page an issue or merge request has on the site.
fn item_url(c: &Client, repo: &str, kind: Kind, n: i64) -> String {
    format!("{}/{repo}/-/{}/{n}", c.web, kind.part())
}

/// Award emoji as GitHub's reactions, filtered to one kind if asked.
fn awards(a: &Ask, base: &str) -> Result<Value> {
    let list = get(a.c, &format!("{base}/award_emoji?per_page=100"))?;
    let (reactions, _) = shape::reactions(a.c, list.list(""));
    let wanted = a.query.get("content");
    Ok(Value::Array(
        reactions.list("").iter().filter(|r| wanted.is_none_or(|w| r.s("content") == w)).cloned().collect(),
    ))
}

/// Comments and what happened, in order: comments as comments, the
/// system's notes ("added ~bug", "approved this merge request") as lines.
fn timeline(a: &Ask, repo: &str, kind: Kind, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/{}/{n}", proj(repo), kind.part());
    let notes = get(c, &format!("{base}/notes?sort=asc&order_by=created_at&{}", a.paging()))?;
    let notes = notes.list("").to_vec();
    let page = item_url(c, repo, kind, n);
    // Each comment's reactions; GitLab gives them one comment at a time.
    let awards = parallel(&notes, |note| {
        if note.b("system") {
            return Vec::new();
        }
        get(c, &format!("{base}/notes/{}/award_emoji?per_page=100", note.i("id")))
            .map(|v| v.list("").to_vec())
            .unwrap_or_default()
    });
    Ok(Value::Array(
        notes
            .iter()
            .zip(awards)
            .map(|(note, awards)| {
                if note.b("system") {
                    shape::system_note(c, note)
                } else {
                    let mut shaped = shape::note(c, repo, kind.part(), n, note, &page);
                    shaped["reactions"] = shape::reactions(c, &awards).1;
                    if note.s("type") == "DiffNote" {
                        // A comment on a line, read in the conversation too.
                        let path = note.s("position.new_path");
                        shaped["body"] = json!(format!("**`{path}`**\n\n{}", note.s("body")));
                    }
                    shaped
                }
            })
            .collect(),
    ))
}

/// A merge request's changed files, a page at a time.
fn files(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    match get(c, &format!("{p}/merge_requests/{n}/diffs?{}", a.paging())) {
        Ok(list) => Ok(Value::Array(list.list("").iter().map(shape::file).collect())),
        // Instances before 15.7 only give them all at once.
        Err(e) if api::status_of(&e) == Some(404) => {
            let all = get(c, &format!("{p}/merge_requests/{n}/changes"))?;
            let (per, page) = (a.query.per_page() as usize, a.query.page() as usize);
            Ok(Value::Array(all.list("changes").iter().skip(per * (page - 1)).take(per).map(shape::file).collect()))
        }
        Err(e) => Err(e),
    }
}

/// Comments on lines of the diff, as GitHub's review comments.
fn review_comments(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let threads = get_all(c, &format!("{}/merge_requests/{n}/discussions", proj(repo)), 10)?;
    let page = item_url(c, repo, Kind::Mr, n);
    let mut out = Vec::new();
    for thread in &threads {
        let notes = thread.list("notes");
        let first = notes.first().map(|n| n.i("id"));
        for note in notes {
            if note.s("type") != "DiffNote" || note.b("system") {
                continue;
            }
            let position = note.at("position");
            let (side, line) = if position.has("new_line") { ("RIGHT", position.i("new_line")) } else { ("LEFT", position.i("old_line")) };
            let id = note.i("id");
            out.push(json!({
                "id": id,
                "body": note.s("body"),
                "user": shape::user(c, note.at("author")),
                "created_at": note.s("created_at"),
                "updated_at": note.s("updated_at"),
                "path": if position.has("new_path") { position.s("new_path") } else { position.s("old_path") },
                "line": line,
                "side": side,
                "url": format!("/repos/{repo}/merge_requests/{n}/discussions/{}/notes/{id}", thread.s("id")),
                "pull_request_url": format!("/repos/{repo}/pulls/{n}"),
                "html_url": format!("{page}#note_{id}"),
                "in_reply_to_id": if first == Some(id) { Value::Null } else { json!(first) },
                "resolved": note.b("resolved"),
            }));
        }
    }
    Ok(Value::Array(out))
}

/// A comment on one line of the diff. GitLab places it by both line
/// numbers when the line is unchanged, so this reads the diff to find
/// the other one.
fn line_comment(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let p = proj(repo);
    let mr = get(c, &format!("{p}/merge_requests/{n}"))?;
    let refs = mr.at("diff_refs");
    let path = a.field("path").as_str().unwrap_or("").to_string();
    let line = a.field("line").as_i64().unwrap_or(0) as u32;
    let left = a.field("side").as_str() == Some("LEFT");
    let diffs = get_all(c, &format!("{p}/merge_requests/{n}/diffs"), 20)
        .or_else(|_| get(c, &format!("{p}/merge_requests/{n}/changes")).map(|v| v.list("changes").to_vec()))?;
    let diff = diffs.iter().find(|d| d.s("new_path") == path || d.s("old_path") == path).cloned().unwrap_or(Value::Null);
    let mut position = json!({
        "position_type": "text",
        "base_sha": refs.s("base_sha"),
        "start_sha": refs.s("start_sha"),
        "head_sha": refs.s("head_sha"),
        "new_path": if diff.is_null() { path.clone() } else { diff.s("new_path") },
        "old_path": if diff.is_null() { path.clone() } else { diff.s("old_path") },
    });
    let lines = crate::diff::parse(&diff.s("diff"));
    let here = lines.iter().find(|l| if left { l.old == Some(line) && l.new.is_none() } else { l.new == Some(line) });
    match here {
        Some(l) => {
            if let Some(old) = l.old {
                position["old_line"] = json!(old);
            }
            if let Some(new) = l.new {
                position["new_line"] = json!(new);
            }
        }
        None if left => position["old_line"] = json!(line),
        None => position["new_line"] = json!(line),
    }
    call(c, "POST", &format!("{p}/merge_requests/{n}/discussions"), Some(&json!({ "body": a.field("body"), "position": position })))
}

/// Approvals, and what reviewers have said, as GitHub's reviews.
fn reviews(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let p = proj(repo);
    let mut out = Vec::new();
    // Reviewer states came in GitLab 16; before then only approvals say.
    if let Ok(reviewers) = get(c, &format!("{p}/merge_requests/{n}/reviewers")) {
        for r in reviewers.list("") {
            let state = match r.s("state").as_str() {
                "requested_changes" => "CHANGES_REQUESTED",
                "reviewed" => "COMMENTED",
                "approved" => "APPROVED",
                _ => continue,
            };
            out.push(json!({ "user": shape::user(c, r.at("user")), "state": state, "submitted_at": r.s("created_at") }));
        }
    }
    let approvals = get(c, &format!("{p}/merge_requests/{n}/approvals"))?;
    for a in approvals.list("approved_by") {
        out.push(json!({ "user": shape::user(c, a.at("user")), "state": "APPROVED", "submitted_at": approvals.s("updated_at"), "body": "" }));
    }
    Ok(Value::Array(out))
}

/// GitHub's review verdicts: approving approves; asking for changes and
/// commenting leave the summary as a comment (and take back an approval).
fn review(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/merge_requests/{n}", proj(repo));
    let text = a.field("body").as_str().unwrap_or("").trim().to_string();
    let event = a.field("event").as_str().unwrap_or("COMMENT");
    match event {
        "APPROVE" => {
            call(c, "POST", &format!("{base}/approve"), None)?;
        }
        "REQUEST_CHANGES" => {
            let _ = call(c, "POST", &format!("{base}/unapprove"), None);
        }
        _ => {}
    }
    if !text.is_empty() || event == "REQUEST_CHANGES" {
        let body = if event == "REQUEST_CHANGES" {
            if text.is_empty() { "Requested changes.".to_string() } else { format!("**Requested changes:**\n\n{text}") }
        } else {
            text
        };
        call(c, "POST", &format!("{base}/notes"), Some(&json!({ "body": body })))?;
    }
    Ok(json!({ "state": event }))
}

fn merge(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let squash = a.field("merge_method").as_str() == Some("squash");
    let title = a.field("commit_title").as_str().unwrap_or("").trim().to_string();
    let message = a.field("commit_message").as_str().unwrap_or("").trim().to_string();
    let mut body = json!({ "squash": squash });
    if let Some(sha) = a.field("sha").as_str().filter(|s| !s.is_empty()) {
        body["sha"] = json!(sha);
    }
    if !title.is_empty() {
        let text = if message.is_empty() { title } else { format!("{title}\n\n{message}") };
        body[if squash { "squash_commit_message" } else { "merge_commit_message" }] = json!(text);
    }
    let merged = call(c, "PUT", &format!("{}/merge_requests/{n}/merge", proj(repo)), Some(&body))?;
    Ok(json!({ "merged": merged.s("state") == "merged", "sha": merged.s("merge_commit_sha") }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn kinds_name_their_paths() {
        assert_eq!(super::Kind::Issue.part(), "issues");
        assert_eq!(super::Kind::Mr.part(), "merge_requests");
    }
}
