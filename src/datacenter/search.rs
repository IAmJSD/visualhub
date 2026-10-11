//! GitHub's search, from the instance's lists.
//!
//! Data Center can't search pull requests across repositories, but its
//! dashboard lists the signed-in user's by role: the ones they wrote, are
//! reviewing, or took part in. So a search about you asks the dashboard;
//! one naming repositories asks each of them; anything else asks the
//! repositories you opened lately. The qualifiers the app uses become
//! those filters, and the words left over search titles.

use super::{get_all, me, missing, page, parallel, repo_api, shape, slice, values_at, Ask, API};
use crate::api::Client;
use crate::gitlab::search::Parsed;
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};

/// How many of your recent repositories a search across them asks.
const REPOS_ASKED: u64 = 25;

pub fn search(a: &Ask, kind: &str) -> Result<Value> {
    let parsed = Parsed::new(a.query.get("q").unwrap_or(""));
    let items = match kind {
        "issues" => pulls(a, &parsed)?,
        "repositories" => repositories(a, &parsed)?,
        "code" => return Err(missing("code search through VisualHub")),
        _ => return Err(missing("search of that kind")),
    };
    let total = items.list("").len();
    Ok(json!({ "total_count": total, "items": items }))
}

/// Whether `value` (a login or `@me`) is the signed-in user.
fn is_me(c: &Client, value: &str) -> bool {
    value == "@me" || me(c).is_ok_and(|m| m.s("slug").eq_ignore_ascii_case(value))
}

/// The instance's state for GitHub's qualifiers.
fn state(parsed: &Parsed) -> &'static str {
    if parsed.is("merged") {
        "MERGED"
    } else if parsed.is("unmerged") {
        "DECLINED"
    } else if parsed.is("open") || parsed.all("state").contains(&"open") {
        "OPEN"
    } else if parsed.is("closed") || parsed.all("state").contains(&"closed") {
        // Merged ones have their own filter, as on GitLab.
        "DECLINED"
    } else {
        "ALL"
    }
}

/// Pull requests. Issues are Jira's, so a search for them finds nothing.
fn pulls(a: &Ask, parsed: &Parsed) -> Result<Value> {
    let c = a.c;
    if parsed.is("issue") && !parsed.is("pr") {
        return Ok(json!([]));
    }
    let state = state(parsed);
    let authors = parsed.all("author");
    let reviewers: Vec<&str> = parsed
        .all("review-requested")
        .into_iter()
        .chain(parsed.all("assignee"))
        .collect();
    let involved: Vec<&str> = ["reviewed-by", "involves", "mentions", "commenter"]
        .iter()
        .flat_map(|k| parsed.all(k))
        .collect();
    let named: Vec<String> = parsed.all("repo").iter().map(|r| r.to_string()).collect();
    // A search about you, across everything: the dashboard has it.
    let role = if named.is_empty() {
        if authors.len() == 1 && is_me(c, authors[0]) {
            Some("AUTHOR")
        } else if reviewers.len() == 1 && is_me(c, reviewers[0]) {
            Some("REVIEWER")
        } else if involved.len() == 1 && is_me(c, involved[0]) {
            Some("PARTICIPANT")
        } else {
            None
        }
    } else {
        None
    };
    let mut all: Vec<Value> = match role {
        Some(role) => get_all(
            c,
            &format!("{API}/dashboard/pull-requests?role={role}&state={state}&order=NEWEST"),
            2,
        )?
        .iter()
        .map(|v| shape::pr(c, &shape::full_name(v.at("toRef.repository")), v))
        .collect(),
        None => {
            let repos = if named.is_empty() { recent(c)? } else { named };
            let wanted = (a.query.per_page() * a.query.page()).min(200);
            parallel(&repos, |repo| {
                values_at(
                    c,
                    &format!(
                        "{}/pull-requests?state={state}&order=NEWEST",
                        repo_api(repo)
                    ),
                    0,
                    wanted,
                )
                .map(|list| {
                    list.iter()
                        .map(|v| shape::pr(c, repo, v))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
            })
            .into_iter()
            .flatten()
            .collect()
        }
    };
    // The rest of the qualifiers, on what came back.
    let login = |who: &str| -> String {
        if who == "@me" {
            me(c).map(|m| m.s("slug")).unwrap_or_default()
        } else {
            who.to_string()
        }
    };
    if role != Some("AUTHOR") {
        for who in &authors {
            let who = login(who);
            all.retain(|v| v.s("user.login").eq_ignore_ascii_case(&who));
        }
    }
    if role != Some("REVIEWER") {
        for who in &reviewers {
            let who = login(who);
            all.retain(|v| {
                v.list("requested_reviewers")
                    .iter()
                    .any(|r| r.s("login").eq_ignore_ascii_case(&who))
            });
        }
    }
    for branch in parsed.all("head") {
        all.retain(|v| v.s("head.ref") == branch);
    }
    for branch in parsed.all("base") {
        all.retain(|v| v.s("base.ref") == branch);
    }
    if parsed.is("draft") {
        all.retain(|v| v.b("draft"));
    }
    let text = parsed.text.trim().to_lowercase();
    if !text.is_empty() {
        all.retain(|v| v.s("title").to_lowercase().contains(&text));
    }
    let key = if a.query.get("sort") == Some("created") {
        "created_at"
    } else {
        "updated_at"
    };
    all.sort_by_key(|v| std::cmp::Reverse(v.s(key)));
    Ok(slice(all, &a.query))
}

/// The repositories you opened lately, as `KEY/slug`.
fn recent(c: &Client) -> Result<Vec<String>> {
    let list = values_at(c, &format!("{API}/profile/recent/repos"), 0, REPOS_ASKED)?;
    Ok(list.iter().map(shape::full_name).collect())
}

fn repositories(a: &Ask, parsed: &Parsed) -> Result<Value> {
    let c = a.c;
    let mut path = format!("{API}/repos");
    let text = parsed.text.trim();
    if !text.is_empty() {
        path = crate::hub::with_query(&path, &format!("name={}", enc(text)));
    }
    if let Some(project) = parsed
        .all("org")
        .into_iter()
        .chain(parsed.all("user"))
        .next()
    {
        path = crate::hub::with_query(&path, &format!("projectkey={}", enc(project)));
    }
    Ok(shape::repos(c, &page(c, &path, &a.query)?))
}
