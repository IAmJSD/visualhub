//! Pull requests, asked for as GitHub's pull requests and issues.
//!
//! GitHub reaches a pull request's comments and timeline through
//! `/issues/{n}`. Data Center's issues are Jira's, so here an issue's
//! number is always a pull request's, and `/issues/{n}/…` is answered from
//! the pull request. Comments carry `url`s under `/pullrequests/{n}/…`,
//! where they can be edited and deleted.
//!
//! Comments come threaded (each holds its replies) and through the pull
//! request's activity, which is also where approvals and merges are. Every
//! change to a pull request or a comment names the version it changes.

use super::{call, get, get_all, me, missing, page, repo_api, shape, user_by_login, Ask};
use crate::api::Client;
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
    let base = |n: i64| format!("{}/pull-requests/{n}", repo_api(repo));
    match rest {
        ["issues"] if m == "GET" => Ok(json!([])),
        ["issues"] => Err(missing("issue tracker; its issues are in Jira")),
        ["issues", n, tail @ ..] => item(a, repo, number(n)?, tail),
        ["pullrequests", n, "comments", id, tail @ ..] => {
            let path = format!("{}/comments/{id}", base(number(n)?));
            match (m, tail) {
                (_, ["reactions", ..]) if m == "GET" => Ok(json!([])),
                (_, ["reactions", ..]) => Err(missing("reactions")),
                ("GET", _) => get(c, &path),
                ("DELETE", _) => {
                    let version = get(c, &path)?.i("version");
                    call(c, "DELETE", &format!("{path}?version={version}"), None)
                }
                _ => {
                    let version = get(c, &path)?.i("version");
                    call(
                        c,
                        "PUT",
                        &path,
                        Some(&json!({ "text": a.field("body"), "version": version })),
                    )
                }
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
            let n = number(n)?;
            let body = inline_body(
                c,
                repo,
                n,
                a.field("body"),
                a.field("path"),
                a.field("line"),
                a.field("side"),
            );
            call(c, "POST", &format!("{}/comments", base(n)), Some(&body))
        }
        ["pulls", n, "comments", id, "replies"] => call(
            c,
            "POST",
            &format!("{}/comments", base(number(n)?)),
            Some(
                &json!({ "text": a.field("body"), "parent": { "id": id.parse::<i64>().unwrap_or(0) } }),
            ),
        ),
        ["pulls", n, "reviews"] if m == "GET" => reviews(c, repo, number(n)?),
        ["pulls", n, "reviews"] => review(a, repo, number(n)?),
        ["pulls", n, "requested_reviewers"] => {
            let n = number(n)?;
            let pr = get(c, &base(n))?;
            let mut names: Vec<String> = pr
                .list("reviewers")
                .iter()
                .map(|r| r.s("user.name"))
                .collect();
            for login in a
                .field("reviewers")
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|l| l.as_str())
            {
                let name = user_by_login(c, login)?.s("name");
                names.retain(|x| *x != name);
                if m != "DELETE" {
                    names.push(name);
                }
            }
            let reviewers: Vec<Value> = names
                .iter()
                .map(|name| json!({ "user": { "name": name } }))
                .collect();
            let updated = call(
                c,
                "PUT",
                &base(n),
                Some(
                    &json!({ "version": pr.i("version"), "title": pr.s("title"), "reviewers": reviewers }),
                ),
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

/// GitHub's list filters as the instance's: the state, and a branch on
/// either side.
fn list(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let state = match a.query.get("state").unwrap_or("open") {
        "open" => "OPEN",
        "merged" => "MERGED",
        "all" => "ALL",
        // Merged ones have their own filter beside this one, as on GitLab.
        _ => "DECLINED",
    };
    let order = if a.query.get("direction") == Some("asc") {
        "OLDEST"
    } else {
        "NEWEST"
    };
    let mut path = format!(
        "{}/pull-requests?state={state}&order={order}",
        repo_api(repo)
    );
    if let Some(head) = a.query.get("head") {
        let branch = head.split_once(':').map(|(_, b)| b).unwrap_or(head);
        path.push_str(&format!(
            "&direction=OUTGOING&at={}",
            enc(&format!("refs/heads/{branch}"))
        ));
    } else if let Some(base) = a.query.get("base") {
        path.push_str(&format!(
            "&direction=INCOMING&at={}",
            enc(&format!("refs/heads/{base}"))
        ));
    }
    let list = page(c, &path, &a.query)?;
    Ok(Value::Array(
        list.iter().map(|v| shape::pr(c, repo, v)).collect(),
    ))
}

/// A branch's ref in another repository: `{ id, repository }`.
fn ref_in(branch: &str, repo: &str) -> Value {
    let (key, slug) = super::split(repo);
    json!({ "id": format!("refs/heads/{branch}"), "repository": { "slug": slug, "project": { "key": key } } })
}

fn create(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let head = a.field("head").as_str().unwrap_or("");
    // `owner:branch` names a fork's branch.
    let from = match head.split_once(':') {
        Some((owner, branch)) => {
            let slug = super::split(repo).1;
            ref_in(branch, &format!("{owner}/{slug}"))
        }
        None => ref_in(head, repo),
    };
    let body = json!({
        "title": a.field("title"),
        "description": a.field("body"),
        "fromRef": from,
        "toRef": ref_in(a.field("base").as_str().unwrap_or(""), repo),
        "draft": a.field("draft").as_bool().unwrap_or(false),
    });
    let made = call(
        c,
        "POST",
        &format!("{}/pull-requests", repo_api(repo)),
        Some(&body),
    )?;
    Ok(shape::pr(c, repo, &made))
}

/// One pull request, with what its list leaves out: how many commits, and
/// how many lines and files change, and whether it can merge and what
/// stops it.
pub fn get_pr(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let v = get(c, &super::with_avatars(&base))?;
    let mut shaped = shape::pr(c, repo, &v);
    let (files, commits, check) = std::thread::scope(|s| {
        let files = s.spawn(|| changed_files(c, repo, n, &v));
        let commits = s.spawn(|| get_all(c, &format!("{base}/commits"), 2));
        let check = s.spawn(|| get(c, &format!("{base}/merge")));
        (files.join(), commits.join(), check.join())
    });
    if let Ok(Ok(files)) = files {
        let list = files.list("");
        shaped["additions"] = json!(list.iter().map(|f| f.i("additions")).sum::<i64>());
        shaped["deletions"] = json!(list.iter().map(|f| f.i("deletions")).sum::<i64>());
        shaped["changed_files"] = json!(list.len());
    }
    if let Ok(Ok(commits)) = commits {
        shaped["commits"] = json!(commits.len());
    }
    if let (Ok(Ok(check)), true) = (check, v.s("state") == "OPEN") {
        let reasons: Vec<String> = check
            .list("vetoes")
            .iter()
            .map(|veto| veto.s("summaryMessage"))
            .filter(|s| !s.is_empty())
            .collect();
        if check.b("conflicted") {
            shaped["mergeable"] = json!(false);
            shaped["mergeable_state"] = json!("dirty");
        } else if !check.b("canMerge") {
            shaped["mergeable"] = json!(false);
            shaped["mergeable_state"] = json!("blocked");
        }
        shaped["merge_blocker"] = json!(reasons.join(" "));
    }
    Ok(shaped)
}

fn update(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let current = get(c, &base)?;
    let version = current.i("version");
    match a.field("state").as_str() {
        Some("closed") => {
            let declined = call(
                c,
                "POST",
                &format!("{base}/decline?version={version}"),
                None,
            )?;
            return Ok(shape::pr(c, repo, &declined));
        }
        Some("open") => {
            let reopened = call(c, "POST", &format!("{base}/reopen?version={version}"), None)?;
            return Ok(shape::pr(c, repo, &reopened));
        }
        _ => {}
    }
    // What isn't sent keeps its value, but the title and reviewers must be
    // there.
    let mut body = json!({
        "version": version,
        "title": current.s("title"),
        "description": current.s("description"),
        "reviewers": current.list("reviewers").iter().map(|r| json!({ "user": { "name": r.s("user.name") } })).collect::<Vec<_>>(),
    });
    if a.field("title").is_string() {
        body["title"] = a.field("title").clone();
    }
    if a.field("body").is_string() {
        body["description"] = a.field("body").clone();
    }
    if let Some(base_ref) = a.field("base").as_str() {
        body["toRef"] = ref_in(base_ref, repo);
    }
    if let Some(draft) = a.field("draft").as_bool() {
        body["draft"] = json!(draft);
    }
    let updated = call(c, "PUT", &base, Some(&body))?;
    Ok(shape::pr(c, repo, &updated))
}

/// What hangs off a pull request as GitHub hangs it off an issue.
fn item(a: &Ask, repo: &str, n: i64, tail: &[&str]) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let m = a.method;
    match tail {
        [] if m == "GET" => get_pr(c, repo, n),
        [] => update(a, repo, n),
        ["comments"] if m == "GET" => {
            let shaped: Vec<Value> = threads(c, repo, n)?
                .into_iter()
                .filter(|(_, place)| place.is_null())
                .map(|(cm, _)| cm)
                .collect();
            Ok(super::slice(shaped, &a.query))
        }
        ["comments"] => call(
            c,
            "POST",
            &format!("{base}/comments"),
            Some(&json!({ "text": a.field("body") })),
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

fn comment_url(repo: &str, n: i64, id: i64) -> String {
    format!("/repos/{repo}/pullrequests/{n}/comments/{id}")
}

/// The pull request's activity, oldest first.
fn activities(c: &Client, repo: &str, n: i64) -> Result<Vec<Value>> {
    let mut all = get_all(
        c,
        &format!("{}/pull-requests/{n}/activities", repo_api(repo)),
        10,
    )?;
    all.reverse();
    Ok(all)
}

/// Every comment, replies after the comment they answer, each with where
/// it sits on the diff (null for the conversation) and the first comment
/// of its thread.
fn threads(c: &Client, repo: &str, n: i64) -> Result<Vec<(Value, Value)>> {
    fn walk(
        c: &Client,
        repo: &str,
        n: i64,
        cm: &Value,
        place: &Value,
        root: i64,
        out: &mut Vec<(Value, Value)>,
    ) {
        let mut shaped = shape::comment(c, cm, &comment_url(repo, n, cm.i("id")));
        if root != cm.i("id") {
            shaped["in_reply_to_id"] = json!(root);
        }
        out.push((shaped, place.clone()));
        for reply in cm.list("comments") {
            walk(c, repo, n, reply, place, root, out);
        }
    }
    let mut out = Vec::new();
    for e in activities(c, repo, n)? {
        if e.s("action") != "COMMENTED" || e.s("commentAction") != "ADDED" {
            continue;
        }
        let cm = e.at("comment");
        let place = if cm.has("anchor.path") {
            cm.at("anchor").clone()
        } else {
            Value::Null
        };
        walk(c, repo, n, cm, &place, cm.i("id"), &mut out);
    }
    Ok(out)
}

/// Comments and what happened, oldest first: comments as comments,
/// approvals, merges and changes as lines.
fn timeline(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let line = |who: &Value, date: String, body: String| json!({ "event": "system_note", "id": 0, "actor": shape::user(c, who), "body": body, "created_at": date });
    let mut events = Vec::new();
    for e in activities(c, repo, n)? {
        let who = e.at("user");
        let date = shape::when(&e, "createdDate");
        let said = match e.s("action").as_str() {
            "COMMENTED" if e.s("commentAction") == "ADDED" => {
                let cm = e.at("comment");
                let mut flat = Vec::new();
                let mut stack = vec![cm.clone()];
                while let Some(one) = stack.pop() {
                    for reply in one.list("comments").iter().rev() {
                        stack.push(reply.clone());
                    }
                    flat.push(one);
                }
                for one in flat {
                    let mut shaped = shape::comment(c, &one, &comment_url(repo, n, one.i("id")));
                    if cm.has("anchor.path") {
                        // A comment on a line, read in the conversation too.
                        shaped["body"] = json!(format!(
                            "**`{}`**\n\n{}",
                            cm.s("anchor.path"),
                            shaped.s("body")
                        ));
                    }
                    events.push(shaped);
                }
                continue;
            }
            "OPENED" => continue,
            "APPROVED" => "approved these changes".to_string(),
            "UNAPPROVED" => "withdrew their approval".to_string(),
            "REVIEWED" => "requested changes".to_string(),
            "MERGED" => "merged this pull request".to_string(),
            "DECLINED" => "declined this pull request".to_string(),
            "REOPENED" => "reopened this pull request".to_string(),
            "RESCOPED" => {
                let added = e.list("added.commits").len();
                if added == 0 {
                    "updated the branch".to_string()
                } else {
                    format!("added {added} commit{}", if added == 1 { "" } else { "s" })
                }
            }
            "UPDATED" => {
                if e.has("previousTitle") {
                    format!(
                        "changed the title from **{}** to **{}**",
                        e.s("previousTitle"),
                        e.s("title")
                    )
                } else if e.has("previousToRef") {
                    format!("changed the destination to `{}`", e.s("toRef.displayId"))
                } else if e.has("previousDraft") {
                    if e.b("draft") {
                        "marked this pull request as a draft".to_string()
                    } else {
                        "marked this pull request as ready for review".to_string()
                    }
                } else {
                    "edited the description".to_string()
                }
            }
            "REVIEWERS_UPDATED" => "updated the reviewers".to_string(),
            _ => continue,
        };
        events.push(line(who, date, said));
    }
    Ok(super::slice(events, &a.query))
}

/// The pull request's changed files with their parts of the diff.
fn changed_files(c: &Client, repo: &str, n: i64, pr: &Value) -> Result<Value> {
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let changes = get_all(c, &format!("{base}/changes"), 4)?;
    Ok(shape::files(&changes, &diff_text(c, repo, n, pr)))
}

/// The whole diff's text. It changes when either branch does; their heads
/// say which one.
fn diff_text(c: &Client, repo: &str, n: i64, pr: &Value) -> String {
    let key = format!(
        "prdiff:{repo}:{n}:{}:{}",
        pr.s("fromRef.latestCommit"),
        pr.s("toRef.latestCommit")
    );
    c.memo
        .get_or(&key, crate::gitlab::MINUTE * 10, || {
            Ok(json!(c.raw_text(
                &format!("{}/pull-requests/{n}.diff?contextLines=3", repo_api(repo)),
                "text/plain"
            )?))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

fn files(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let pr = get(c, &format!("{}/pull-requests/{n}", repo_api(repo)))?;
    let all = changed_files(c, repo, n, &pr)?;
    Ok(super::slice(all.list("").to_vec(), &a.query))
}

/// Whether line `line` on `side` of `path` is one the pull request adds
/// (or removes) rather than one around them: the instance wants to know.
fn line_type(c: &Client, repo: &str, n: i64, path: &str, line: i64, left: bool) -> &'static str {
    let Ok(pr) = get(c, &format!("{}/pull-requests/{n}", repo_api(repo))) else {
        return "CONTEXT";
    };
    let diff = diff_text(c, repo, n, &pr);
    let Some((_, patch)) = shape::split_diff(&diff)
        .into_iter()
        .find(|(p, _)| p == path)
    else {
        return "CONTEXT";
    };
    let (mut old, mut new) = (0i64, 0i64);
    for text in patch.lines() {
        if let Some(header) = text.strip_prefix("@@ -") {
            // `@@ -a,b +c,d @@`
            let mut nums = header.split([' ', ',', '+']).filter(|s| !s.is_empty());
            old = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let _ = nums.next();
            new = header
                .split('+')
                .nth(1)
                .and_then(|s| s.split([',', ' ']).next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            continue;
        }
        match text.chars().next() {
            Some('+') => {
                if !left && new == line {
                    return "ADDED";
                }
                new += 1;
            }
            Some('-') => {
                if left && old == line {
                    return "REMOVED";
                }
                old += 1;
            }
            _ => {
                old += 1;
                new += 1;
            }
        }
    }
    "CONTEXT"
}

/// A comment's body, placed on a line when GitHub's request names one.
fn inline_body(
    c: &Client,
    repo: &str,
    n: i64,
    body: &Value,
    path: &Value,
    line: &Value,
    side: &Value,
) -> Value {
    let mut out = json!({ "text": body });
    if let Some(path) = path.as_str().filter(|p| !p.is_empty()) {
        let left = side.as_str() == Some("LEFT");
        let number = line.as_i64().unwrap_or(0);
        out["anchor"] = json!({
            "path": path,
            "line": number,
            "lineType": line_type(c, repo, n, path, number, left),
            "fileType": if left { "FROM" } else { "TO" },
            "diffType": "EFFECTIVE",
        });
    }
    out
}

/// Comments on lines of the diff, as GitHub's review comments.
fn review_comments(c: &Client, repo: &str, n: i64) -> Result<Value> {
    Ok(Value::Array(
        threads(c, repo, n)?
            .into_iter()
            .filter(|(_, place)| !place.is_null())
            .map(|(mut cm, place)| {
                cm["path"] = json!(place.s("path"));
                cm["line"] = json!(place.i("line"));
                cm["side"] = json!(if place.s("fileType") == "FROM" {
                    "LEFT"
                } else {
                    "RIGHT"
                });
                cm["pull_request_url"] = json!(format!("/repos/{repo}/pulls/{n}"));
                cm
            })
            .collect(),
    ))
}

/// Approvals and requests for changes, as GitHub's reviews.
fn reviews(c: &Client, repo: &str, n: i64) -> Result<Value> {
    let pr = get(
        c,
        &super::with_avatars(&format!("{}/pull-requests/{n}", repo_api(repo))),
    )?;
    let people = pr.list("reviewers").iter().chain(pr.list("participants"));
    Ok(Value::Array(
        people
            .filter_map(|p| {
                let state = match p.s("status").as_str() {
                    "APPROVED" => "APPROVED",
                    "NEEDS_WORK" => "CHANGES_REQUESTED",
                    _ => return None,
                };
                Some(json!({ "user": shape::user(c, p.at("user")), "state": state, "submitted_at": "", "body": "" }))
            })
            .collect(),
    ))
}

/// GitHub's review verdicts: approving approves, asking for changes marks
/// it as needing work, and what was written goes with it as comments.
fn review(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let event = a.field("event").as_str().unwrap_or("COMMENT");
    let status = match event {
        "APPROVE" => Some("APPROVED"),
        "REQUEST_CHANGES" => Some("NEEDS_WORK"),
        _ => None,
    };
    if let Some(status) = status {
        let mine = me(c)?;
        call(
            c,
            "PUT",
            &format!("{base}/participants/{}", enc(&mine.s("slug"))),
            Some(
                &json!({ "user": { "name": mine.s("name") }, "approved": status == "APPROVED", "status": status }),
            ),
        )?;
    }
    for line in a
        .field("comments")
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let body = inline_body(
            c,
            repo,
            n,
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
            Some(&json!({ "text": text })),
        )?;
    }
    Ok(json!({ "state": event }))
}

fn merge(a: &Ask, repo: &str, n: i64) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/pull-requests/{n}", repo_api(repo));
    let version = get(c, &base)?.i("version");
    let strategy = match a.field("merge_method").as_str() {
        Some("squash") => "squash",
        Some("rebase") => "rebase-ff-only",
        _ => "no-ff",
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
    let mut body = json!({ "strategyId": strategy });
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
        &format!("{base}/merge?version={version}"),
        Some(&body),
    )?;
    Ok(
        json!({ "merged": merged.s("state") == "MERGED", "sha": merged.s("properties.mergeCommit.id") }),
    )
}
