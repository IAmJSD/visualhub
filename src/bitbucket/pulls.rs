//! Pull requests, asked for as GitHub's pull requests and issues.
//!
//! GitHub reaches a pull request's comments and timeline through
//! `/issues/{n}`. Bitbucket has no issues any more, so here an issue's
//! number is always a pull request's, and `/issues/{n}/…` is answered from
//! the pull request. Comments carry `url`s under `/pullrequests/{n}/…`,
//! where they can be edited and deleted, since Bitbucket needs the pull
//! request to find a comment.

use super::{call, get, get_all, missing, page, repo_api, shape, user_uuid, Ask};
use crate::api::{self, Client};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};

pub fn route(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let m = a.method;
    let number = |n: &str| {
        n.parse::<i64>()
            .map_err(|_| missing(&format!("pull request called {n}")))
    };
    let base = |n: i64| format!("{}/pullrequests/{n}", repo_api(repo));
    match rest {
        ["issues"] if m == "GET" => Ok(json!([])),
        ["issues"] => Err(missing(
            "issue tracker any more (Atlassian moved issues to Jira)",
        )),
        ["issues", n, tail @ ..] => item(a, repo, number(n)?, tail),
        ["pullrequests", n, "comments", id, tail @ ..] => {
            let path = format!("{}/comments/{id}", base(number(n)?));
            match (m, tail) {
                (_, ["reactions", ..]) if m == "GET" => Ok(json!([])),
                (_, ["reactions", ..]) => Err(missing("reactions")),
                ("DELETE", _) => call(c, "DELETE", &path, None),
                ("GET", _) => get(c, &path),
                _ => call(
                    c,
                    "PUT",
                    &path,
                    Some(&json!({ "content": { "raw": a.field("body") } })),
                ),
            }
        }
        ["pulls"] if m == "GET" => list(a, repo),
        ["pulls"] => create(a, repo),
        ["pulls", n] if m == "GET" => get_pr(c, repo, number(n)?),
        ["pulls", n] => update(a, repo, number(n)?),
        ["pulls", n, "commits"] => {
            let list = page(c, &format!("{}/commits", base(number(n)?)), &a.query)?;
            Ok(Value::Array(
                list.iter().map(|v| shape::commit(c, repo, v)).collect(),
            ))
        }
        ["pulls", n, "files"] => files(a, repo, number(n)?),
        ["pulls", n, "comments"] if m == "GET" => review_comments(c, repo, number(n)?),
        ["pulls", n, "comments"] => {
            let body = inline_body(
                a.field("body"),
                a.field("path"),
                a.field("line"),
                a.field("side"),
            );
            call(
                c,
                "POST",
                &format!("{}/comments", base(number(n)?)),
                Some(&body),
            )
        }
        ["pulls", n, "comments", id, "replies"] => call(
            c,
            "POST",
            &format!("{}/comments", base(number(n)?)),
            Some(
                &json!({ "content": { "raw": a.field("body") }, "parent": { "id": id.parse::<i64>().unwrap_or(0) } }),
            ),
        ),
        ["pulls", n, "reviews"] if m == "GET" => reviews(c, repo, number(n)?),
        ["pulls", n, "reviews"] => review(a, repo, number(n)?),
        ["pulls", n, "requested_reviewers"] => {
            let n = number(n)?;
            let pr = get(c, &base(n))?;
            let mut ids: Vec<String> = pr.list("reviewers").iter().map(|u| u.s("uuid")).collect();
            for login in a
                .field("reviewers")
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|l| l.as_str())
            {
                let id = user_uuid(c, login)?;
                ids.retain(|x| *x != id);
                if m != "DELETE" {
                    ids.push(id);
                }
            }
            let reviewers: Vec<Value> = ids.iter().map(|id| json!({ "uuid": id })).collect();
            let updated = call(
                c,
                "PUT",
                &base(n),
                Some(&json!({ "title": pr.s("title"), "reviewers": reviewers })),
            )?;
            Ok(shape::pr(c, repo, &updated))
        }
        ["pulls", n, "merge"] => merge(a, repo, number(n)?),
        ["pulls", _, "update-branch"] => Err(missing(
            "way to update a pull request's branch through its API",
        )),
        _ => Err(missing(&format!("equivalent of {}", rest.join("/")))),
    }
}

/// GitHub's list filters as Bitbucket's.
fn list(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let states = match a.query.get("state").unwrap_or("open") {
        "open" => "state=OPEN",
        "merged" => "state=MERGED",
        "all" => "state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED",
        // Merged ones have their own filter beside this one, as on GitLab.
        _ => "state=DECLINED&state=SUPERSEDED",
    };
    let field = match a.query.get("sort") {
        Some("updated") | Some("popularity") | Some("long-running") => "updated_on",
        _ => "created_on",
    };
    let sort = if a.query.get("direction") == Some("asc") {
        field.to_string()
    } else {
        format!("-{field}")
    };
    let mut filters = Vec::new();
    if let Some(head) = a.query.get("head") {
        let branch = head.split_once(':').map(|(_, b)| b).unwrap_or(head);
        filters.push(format!("source.branch.name=\"{branch}\""));
    }
    if let Some(base) = a.query.get("base") {
        filters.push(format!("destination.branch.name=\"{base}\""));
    }
    let mut path = format!("{}/pullrequests?{states}&sort={sort}", repo_api(repo));
    if !filters.is_empty() {
        path.push_str(&format!("&q={}", enc(&filters.join(" AND "))));
    }
    let list = page(c, &path, &a.query)?;
    Ok(Value::Array(
        list.iter().map(|v| shape::pr(c, repo, v)).collect(),
    ))
}

fn create(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let head = a.field("head").as_str().unwrap_or("");
    let mut source = json!({ "branch": { "name": head } });
    // `owner:branch` names a fork's branch.
    if let Some((owner, branch)) = head.split_once(':') {
        let name = repo.split('/').nth(1).unwrap_or("");
        source = json!({ "branch": { "name": branch }, "repository": { "full_name": format!("{owner}/{name}") } });
    }
    let body = json!({
        "title": a.field("title"),
        "description": a.field("body"),
        "source": source,
        "destination": { "branch": { "name": a.field("base") } },
        "draft": a.field("draft").as_bool().unwrap_or(false),
    });
    let made = call(
        c,
        "POST",
        &format!("{}/pullrequests", repo_api(repo)),
        Some(&body),
    )?;
    Ok(shape::pr(c, repo, &made))
}

/// One pull request, with what its list leaves out: how many commits, and
/// how many lines and files change -- and whether any of them conflict.
pub fn get_pr(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    let v = get(c, &base)?;
    let mut shaped = shape::pr(c, repo, &v);
    let (stats, commits) = std::thread::scope(|s| {
        let stats = s.spawn(|| get_all(c, &format!("{base}/diffstat"), 20));
        let commits = s.spawn(|| get_all(c, &format!("{base}/commits"), 5));
        (stats.join(), commits.join())
    });
    if let Ok(Ok(stats)) = stats {
        shaped["additions"] = json!(stats.iter().map(|d| d.i("lines_added")).sum::<i64>());
        shaped["deletions"] = json!(stats.iter().map(|d| d.i("lines_removed")).sum::<i64>());
        shaped["changed_files"] = json!(stats.len());
        let conflicts = stats.iter().any(|d| d.s("status").contains("conflict"));
        if conflicts && shaped.s("state") == "open" {
            shaped["mergeable"] = json!(false);
            shaped["mergeable_state"] = json!("dirty");
        }
    }
    if let Ok(Ok(commits)) = commits {
        shaped["commits"] = json!(commits.len());
    }
    Ok(shaped)
}

fn update(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    match a.field("state").as_str() {
        Some("closed") => {
            let declined = call(c, "POST", &format!("{base}/decline"), None)?;
            return Ok(shape::pr(c, repo, &declined));
        }
        Some("open") => {
            return Err(anyhow::Error::new(api::Status {
                code: 422,
                message: "Bitbucket can't reopen a declined pull request; open a new one from the same branch.".into(),
            }))
        }
        _ => {}
    }
    let mut body = json!({});
    if a.field("title").is_string() {
        body["title"] = a.field("title").clone();
    }
    if a.field("body").is_string() {
        body["description"] = a.field("body").clone();
    }
    if let Some(base_ref) = a.field("base").as_str() {
        body["destination"] = json!({ "branch": { "name": base_ref } });
    }
    if let Some(draft) = a.field("draft").as_bool() {
        body["draft"] = json!(draft);
    }
    if !body.has("title") {
        body["title"] = json!(get(c, &base)?.s("title"));
    }
    let updated = call(c, "PUT", &base, Some(&body))?;
    Ok(shape::pr(c, repo, &updated))
}

/// What hangs off a pull request as GitHub hangs it off an issue.
fn item(a: &Ask, repo: &str, n: i64, tail: &[&str]) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    let m = a.method;
    match tail {
        [] if m == "GET" => get_pr(c, repo, n),
        [] => update(a, repo, n),
        ["comments"] if m == "GET" => {
            let all = get_all(c, &format!("{base}/comments"), 20)?;
            let shaped: Vec<Value> = all
                .iter()
                .filter(|cm| !cm.has("inline") && !cm.b("deleted"))
                .map(|cm| shape::comment(c, cm, &comment_url(repo, n, cm)))
                .collect();
            let (per, page) = (a.query.per_page() as usize, a.query.page() as usize);
            Ok(Value::Array(
                shaped
                    .into_iter()
                    .skip(per * (page - 1))
                    .take(per)
                    .collect(),
            ))
        }
        ["comments"] => call(
            c,
            "POST",
            &format!("{base}/comments"),
            Some(&json!({ "content": { "raw": a.field("body") } })),
        ),
        ["timeline"] => timeline(a, repo, n),
        ["reactions", ..] if m == "GET" => Ok(json!([])),
        ["reactions", ..] => Err(missing("reactions")),
        ["labels", ..] => Err(missing("labels")),
        ["assignees"] => Err(missing(
            "assignees on pull requests; ask for reviewers instead",
        )),
        ["lock"] => Err(missing("way to lock a conversation")),
        _ => Err(missing(&format!("equivalent of {}", tail.join("/")))),
    }
}

fn comment_url(repo: &str, n: i64, cm: &Value) -> String {
    format!("/repos/{repo}/pullrequests/{n}/comments/{}", cm.i("id"))
}

/// Comments and what happened, oldest first: comments as comments,
/// approvals and changes of state as lines.
fn timeline(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    let mut activity = get_all(c, &format!("{base}/activity"), 20)?;
    activity.reverse();
    let line = |who: &Value, date: &str, body: String| json!({ "event": "system_note", "id": 0, "actor": shape::user(c, who), "body": body, "created_at": date });
    let mut events = Vec::new();
    let mut opened = false;
    for e in &activity {
        if e.has("comment") {
            let cm = e.at("comment");
            if cm.b("deleted") {
                continue;
            }
            let mut shaped = shape::comment(c, cm, &comment_url(repo, n, cm));
            if cm.has("inline.path") {
                // A comment on a line, read in the conversation too.
                shaped["body"] = json!(format!(
                    "**`{}`**\n\n{}",
                    cm.s("inline.path"),
                    cm.s("content.raw")
                ));
            }
            events.push(shaped);
        } else if e.has("approval") {
            let ap = e.at("approval");
            events.push(line(
                ap.at("user"),
                &ap.s("date"),
                "approved these changes".into(),
            ));
        } else if e.has("changes_requested") {
            let cr = e.at("changes_requested");
            events.push(line(
                cr.at("user"),
                &cr.s("date"),
                "requested changes".into(),
            ));
        } else if e.has("update") {
            let u = e.at("update");
            let changes = u.at("changes");
            // The first update is the pull request being opened.
            if !opened {
                opened = true;
                if !changes.is_object() || changes.as_object().is_some_and(|m| m.is_empty()) {
                    continue;
                }
            }
            let who = u.at("author");
            let date = u.s("date");
            if changes.has("status") {
                let word = match changes.s("status.new").as_str() {
                    "fulfilled" => "merged this pull request".to_string(),
                    "rejected" => "declined this pull request".to_string(),
                    "superseded" => "superseded this pull request".to_string(),
                    other => format!("changed the state to {other}"),
                };
                events.push(line(who, &date, word));
            }
            if changes.has("title") {
                events.push(line(
                    who,
                    &date,
                    format!(
                        "changed the title from **{}** to **{}**",
                        changes.s("title.old"),
                        changes.s("title.new")
                    ),
                ));
            }
            if changes.has("destination") {
                events.push(line(
                    who,
                    &date,
                    format!(
                        "changed the destination to `{}`",
                        u.s("destination.branch.name")
                    ),
                ));
            }
            if changes.has("reviewers") {
                events.push(line(who, &date, "updated the reviewers".into()));
            }
            if changes.has("draft") {
                events.push(line(
                    who,
                    &date,
                    if u.b("draft") {
                        "marked this pull request as a draft".into()
                    } else {
                        "marked this pull request as ready for review".into()
                    },
                ));
            }
        }
    }
    let (per, page) = (a.query.per_page() as usize, a.query.page() as usize);
    Ok(Value::Array(
        events
            .into_iter()
            .skip(per * (page - 1))
            .take(per)
            .collect(),
    ))
}

/// A pull request's changed files, a page at a time, with their parts of
/// the diff.
fn files(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    let stats = page(c, &format!("{base}/diffstat"), &a.query)?;
    // The diff changes when the branch does; its head says which one.
    let head = get(c, &base)
        .map(|v| v.s("source.commit.hash"))
        .unwrap_or_default();
    let diff = c
        .memo
        .get_or(
            &format!("prdiff:{repo}:{n}:{head}"),
            crate::gitlab::MINUTE * 10,
            || Ok(json!(c.raw_text(&format!("{base}/diff"), "text/plain")?)),
        )
        .map(|v| v.s(""))
        .unwrap_or_default();
    Ok(shape::files(&stats, &diff))
}

/// A comment's body, placed on a line when GitHub's request names one.
fn inline_body(body: &Value, path: &Value, line: &Value, side: &Value) -> Value {
    let mut out = json!({ "content": { "raw": body } });
    if let Some(path) = path.as_str().filter(|p| !p.is_empty()) {
        let key = if side.as_str() == Some("LEFT") {
            "from"
        } else {
            "to"
        };
        out["inline"] = json!({ "path": path });
        out["inline"][key] = line.clone();
    }
    out
}

/// Comments on lines of the diff, as GitHub's review comments.
fn review_comments(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let all = get_all(
        c,
        &format!("{}/pullrequests/{n}/comments", repo_api(repo)),
        20,
    )?;
    let parent_of: std::collections::HashMap<i64, i64> = all
        .iter()
        .filter(|cm| cm.has("parent.id"))
        .map(|cm| (cm.i("id"), cm.i("parent.id")))
        .collect();
    // Replies hang off the thread's first comment, as GitHub's do.
    let root = |mut id: i64| {
        let mut hops = 0;
        while let Some(parent) = parent_of.get(&id) {
            id = *parent;
            hops += 1;
            if hops > 100 {
                break;
            }
        }
        id
    };
    let inline: std::collections::HashMap<i64, Value> = all
        .iter()
        .filter(|cm| cm.has("inline"))
        .map(|cm| (cm.i("id"), cm.at("inline").clone()))
        .collect();
    let mut out = Vec::new();
    for cm in &all {
        if cm.b("deleted") {
            continue;
        }
        let first = root(cm.i("id"));
        // Replies don't repeat where they are; their thread's first says.
        let Some(place) = inline.get(&cm.i("id")).or_else(|| inline.get(&first)) else {
            continue;
        };
        let (side, line) = if place.has("to") {
            ("RIGHT", place.i("to"))
        } else {
            ("LEFT", place.i("from"))
        };
        let mut shaped = shape::comment(c, cm, &comment_url(repo, n, cm));
        shaped["path"] = json!(place.s("path"));
        shaped["line"] = json!(line);
        shaped["side"] = json!(side);
        shaped["pull_request_url"] = json!(format!("/repos/{repo}/pulls/{n}"));
        shaped["in_reply_to_id"] = if first == cm.i("id") {
            Value::Null
        } else {
            json!(first)
        };
        out.push(shaped);
    }
    Ok(Value::Array(out))
}

/// Approvals and requests for changes, as GitHub's reviews.
fn reviews(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let pr = get(c, &format!("{}/pullrequests/{n}", repo_api(repo)))?;
    Ok(Value::Array(
        pr.list("participants")
            .iter()
            .filter_map(|p| {
                let state = match p.s("state").as_str() {
                    "approved" => "APPROVED",
                    "changes_requested" => "CHANGES_REQUESTED",
                    _ if p.b("approved") => "APPROVED",
                    _ => return None,
                };
                Some(json!({ "user": shape::user(c, p.at("user")), "state": state, "submitted_at": p.s("participated_on"), "body": "" }))
            })
            .collect(),
    ))
}

/// GitHub's review verdicts: approving approves, asking for changes asks
/// for them, and what was written goes with it as a comment.
fn review(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pullrequests/{n}", repo_api(repo));
    let event = a.field("event").as_str().unwrap_or("COMMENT");
    match event {
        "APPROVE" => {
            // Approving takes back a request for changes, as on the site.
            let _ = c.raw("DELETE", &format!("{base}/request-changes"), None, None);
            call(c, "POST", &format!("{base}/approve"), None)?;
        }
        "REQUEST_CHANGES" => {
            let _ = c.raw("DELETE", &format!("{base}/approve"), None, None);
            call(c, "POST", &format!("{base}/request-changes"), None)?;
        }
        _ => {}
    }
    for line in a
        .field("comments")
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let body = inline_body(
            line.at("body"),
            line.at("path"),
            line.at("line"),
            line.at("side"),
        );
        call(c, "POST", &format!("{base}/comments"), Some(&body))?;
    }
    let text = a.field("body").as_str().unwrap_or("").trim().to_string();
    if !text.is_empty() {
        call(
            c,
            "POST",
            &format!("{base}/comments"),
            Some(&json!({ "content": { "raw": text } })),
        )?;
    }
    Ok(json!({ "state": event }))
}

fn merge(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let strategy = match a.field("merge_method").as_str() {
        Some("squash") => "squash",
        Some("rebase") => "fast_forward",
        _ => "merge_commit",
    };
    let title = a
        .field("commit_title")
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let message = a
        .field("commit_message")
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut body = json!({ "type": "pullrequest_merge_parameters", "merge_strategy": strategy });
    if !title.is_empty() {
        body["message"] = json!(if message.is_empty() {
            title
        } else {
            format!("{title}\n\n{message}")
        });
    }
    let merged = call(
        c,
        "POST",
        &format!("{}/pullrequests/{n}/merge", repo_api(repo)),
        Some(&body),
    )?;
    Ok(
        json!({ "merged": merged.s("state") == "MERGED" || merged.is_null(), "sha": merged.s("merge_commit.hash") }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_comments_say_which_side() {
        assert_eq!(
            inline_body(&json!("hi"), &json!("a.rs"), &json!(4), &json!("LEFT")),
            json!({ "content": { "raw": "hi" }, "inline": { "path": "a.rs", "from": 4 } })
        );
        assert_eq!(
            inline_body(&json!("hi"), &Value::Null, &Value::Null, &Value::Null),
            json!({ "content": { "raw": "hi" } })
        );
    }
}
