//! GitHub's search, from Bitbucket's lists.
//!
//! Bitbucket has no search across repositories; it filters each
//! repository's lists with its query language (`author.uuid = "{…}"`).
//! So a search for pull requests asks each repository it could be about
//! -- the ones named, or your most recently active -- and puts the
//! answers together. The qualifiers the app uses become those filters;
//! the words left over search titles.

use super::{me, missing, page_at, parallel, shape, user_uuid, values, workspace_slugs, Ask};
use crate::api::Client;
use crate::gitlab::search::Parsed;
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};

/// How many of your repositories a search across them asks.
const REPOS_ASKED: usize = 30;

pub fn search(a: &Ask, kind: &str) -> Result<Value> {
    let parsed = Parsed::new(a.query.get("q").unwrap_or(""));
    let items = match kind {
        "issues" => pulls(a, &parsed)?,
        "repositories" => repositories(a, &parsed)?,
        "code" => return Err(missing("code search through its API any more")),
        _ => return Err(missing("search of that kind")),
    };
    let total = items.len();
    Ok(json!({ "total_count": total, "items": items }))
}

/// `@me` as the signed-in user's UUID; anyone else's from their login.
fn uuid_of(c: &Client, value: &str) -> Result<String> {
    if value == "@me" {
        Ok(me(c)?.s("uuid"))
    } else {
        user_uuid(c, value)
    }
}

/// A BBQL string literal.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The repositories a search is about: those named with `repo:`, the
/// workspaces named with `org:`/`user:`, or your most active.
fn scope(c: &Client, parsed: &Parsed) -> Result<Vec<String>> {
    let named: Vec<String> = parsed.all("repo").iter().map(|r| r.to_string()).collect();
    if !named.is_empty() {
        return Ok(named);
    }
    let mut spaces: Vec<String> = parsed
        .all("org")
        .into_iter()
        .chain(parsed.all("user"))
        .map(str::to_string)
        .collect();
    if spaces.is_empty() {
        spaces = workspace_slugs(c)?;
    }
    let found = parallel(&spaces, |ws| {
        values(
            c,
            &format!(
                "/repositories/{}?role=member&sort=-updated_on&pagelen={REPOS_ASKED}",
                enc(ws)
            ),
        )
        .unwrap_or_default()
    });
    let mut repos: Vec<Value> = found.into_iter().flatten().collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r.s("updated_on")));
    Ok(repos
        .iter()
        .take(REPOS_ASKED)
        .map(|r| r.s("full_name"))
        .collect())
}

/// Pull requests. Bitbucket has no issues, so a search for them finds
/// nothing.
fn pulls(a: &Ask, parsed: &Parsed) -> Result<Vec<Value>> {
    let c = a.c;
    if parsed.is("issue") && !parsed.is("pr") {
        return Ok(Vec::new());
    }
    let states = if parsed.is("merged") {
        "state=MERGED"
    } else if parsed.is("unmerged") {
        "state=DECLINED&state=SUPERSEDED"
    } else if parsed.is("open") || parsed.all("state").contains(&"open") {
        "state=OPEN"
    } else if parsed.is("closed") || parsed.all("state").contains(&"closed") {
        // Merged ones have their own filter, as on GitLab.
        "state=DECLINED&state=SUPERSEDED"
    } else {
        "state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED"
    };
    let mut filters = Vec::new();
    for login in parsed.all("author") {
        filters.push(format!("author.uuid = {}", quoted(&uuid_of(c, login)?)));
    }
    for login in parsed
        .all("review-requested")
        .into_iter()
        .chain(parsed.all("assignee"))
    {
        filters.push(format!("reviewers.uuid = {}", quoted(&uuid_of(c, login)?)));
    }
    for key in ["reviewed-by", "involves", "mentions", "commenter"] {
        for login in parsed.all(key) {
            filters.push(format!(
                "participants.uuid = {}",
                quoted(&uuid_of(c, login)?)
            ));
        }
    }
    for branch in parsed.all("head") {
        filters.push(format!("source.branch.name = {}", quoted(branch)));
    }
    for branch in parsed.all("base") {
        filters.push(format!("destination.branch.name = {}", quoted(branch)));
    }
    if parsed.is("draft") {
        filters.push("draft = true".into());
    }
    let text = parsed.text.trim();
    if !text.is_empty() {
        filters.push(format!("title ~ {}", quoted(text)));
    }
    let created = a.query.get("sort") == Some("created");
    let sort = if created {
        "-created_on"
    } else {
        "-updated_on"
    };
    let (per, page) = (a.query.per_page(), a.query.page());
    let wanted = (per * page).min(100);
    let query = if filters.is_empty() {
        String::new()
    } else {
        format!("&q={}", enc(&filters.join(" AND ")))
    };
    let repos = scope(c, parsed)?;
    let found = parallel(&repos, |repo| {
        page_at(
            c,
            &format!(
                "{}/pullrequests?{states}&sort={sort}{query}",
                super::repo_api(repo)
            ),
            wanted,
            1,
        )
        .map(|list| {
            list.iter()
                .map(|v| shape::pr(c, repo, v))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    });
    let mut all: Vec<Value> = found.into_iter().flatten().collect();
    let key = if created { "created_at" } else { "updated_at" };
    all.sort_by_key(|v| std::cmp::Reverse(v.s(key)));
    Ok(all
        .into_iter()
        .skip((per * (page - 1)) as usize)
        .take(per as usize)
        .collect())
}

fn repositories(a: &Ask, parsed: &Parsed) -> Result<Vec<Value>> {
    let c = a.c;
    let mut spaces: Vec<String> = parsed
        .all("org")
        .into_iter()
        .chain(parsed.all("user"))
        .map(str::to_string)
        .collect();
    if spaces.is_empty() {
        spaces = workspace_slugs(c)?;
    }
    let text = parsed.text.trim();
    let query = if text.is_empty() {
        String::new()
    } else {
        format!(
            "&q={}",
            enc(&format!(
                "name ~ {} OR description ~ {}",
                quoted(text),
                quoted(text)
            ))
        )
    };
    let (per, page) = (a.query.per_page(), a.query.page());
    let wanted = (per * page).min(100);
    let found = parallel(&spaces, |ws| {
        page_at(
            c,
            &format!("/repositories/{}?sort=-updated_on{query}", enc(ws)),
            wanted,
            1,
        )
        .unwrap_or_default()
    });
    let mut all: Vec<Value> = found.into_iter().flatten().collect();
    all.sort_by_key(|r| std::cmp::Reverse(r.s("updated_on")));
    Ok(all
        .iter()
        .skip((per * (page - 1)) as usize)
        .take(per as usize)
        .map(|r| shape::repo(r, ""))
        .collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn literals_are_quoted() {
        assert_eq!(super::quoted(r#"say "hi""#), r#""say \"hi\"""#);
    }
}
