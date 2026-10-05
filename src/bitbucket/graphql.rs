//! The GitHub GraphQL the screens send, answered from Bitbucket.
//!
//! The app asks GitHub's GraphQL for a few things REST doesn't give: open
//! and closed counts, a commit with its co-authors and checks, each file's
//! last commit, a profile's contribution calendar, and a handful of
//! mutations. Each is recognised by what it asks for and answered in the
//! same shape from Bitbucket's REST API, as the GitLab translation does;
//! anything else fails as Bitbucket having no such thing.

use super::repos::{resolve, statuses};
use super::{call, get, missing, parallel, repo_api, shape, size, Client};
use crate::gitlab::graphql::aliased;
use crate::gitlab::MINUTE;
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Map, Value};

pub fn graphql(c: &Client, query: &str, vars: Value) -> Result<Value> {
    let repo = format!("{}/{}", vars.s("o"), vars.s("n"));
    if query.contains("openIssues: issues(states: OPEN)") {
        return counts(c, &repo);
    }
    if query.contains("history(first: 1) { totalCount }") {
        return Ok(json!({ "repository": { "object": commit_head(c, &repo, &vars.s("r"))? } }));
    }
    if query.contains("history(first: 1, path:") {
        return last_commits(c, &repo, &vars.s("r"), query);
    }
    if query.contains(": object(oid:") {
        return commit_batch(c, &repo, query);
    }
    if query.contains("contributionsCollection") {
        // Bitbucket keeps no calendar; the profile goes without one.
        return Ok(json!({ "user": {
            "pinnedItems": { "nodes": [] },
            "contributionsCollection": { "contributionCalendar": { "totalContributions": 0, "weeks": [] } },
        } }));
    }
    if query.contains("repository(owner: $o, name: $n) { id }") {
        return Ok(json!({ "repository": { "id": format!("bb:repo:{repo}") } }));
    }
    mutation(c, query, &vars)
}

/// Open and closed pull requests, which Bitbucket counts; no issues.
fn counts(c: &Client, repo: &str) -> Result<Value> {
    let base = format!("{}/pullrequests", repo_api(repo));
    let states = ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"];
    let totals = parallel(&states, |state| size(c, &format!("{base}?state={state}")));
    let count = |n: u64| json!({ "totalCount": n });
    Ok(json!({ "repository": {
        "openIssues": count(0),
        "closedIssues": count(0),
        "openPulls": count(totals[0]),
        "closedPulls": count(totals[1] + totals[2] + totals[3]),
        "closedOnlyPulls": count(totals[2] + totals[3]),
        "mergedPulls": count(totals[1]),
    } }))
}

/// A commit's authors: the author, then anyone in a `Co-authored-by:`
/// trailer. Only the author has a face Bitbucket knows.
fn authors(commit: &Value) -> Value {
    let (name, _) = shape::author_parts(&commit.s("author.raw"));
    let mut people = vec![json!({
        "name": name,
        "avatarUrl": commit.s("author.user.links.avatar.href"),
        "user": Value::Null,
    })];
    for line in commit.s("message").lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Co-authored-by:")
            .or_else(|| line.strip_prefix("Co-Authored-By:"))
        else {
            continue;
        };
        let (name, _) = shape::author_parts(rest);
        people.push(json!({ "name": name, "avatarUrl": "", "user": Value::Null }));
    }
    json!({ "nodes": people })
}

/// A commit, kept a while: it never changes, though its checks might.
fn commit(c: &Client, repo: &str, rev: &str) -> Result<Value> {
    c.memo
        .get_or(&format!("bbcommit:{repo}@{rev}"), MINUTE, || {
            get(c, &format!("{}/commit/{}", repo_api(repo), enc(rev)))
        })
}

fn checks(c: &Client, repo: &str, sha: &str) -> Value {
    statuses(c, repo, sha)
        .map(|list| shape::rollup(&list))
        .unwrap_or(Value::Null)
}

fn commit_head(c: &Client, repo: &str, rev: &str) -> Result<Value> {
    let rev = resolve(c, repo, rev)?;
    let v = commit(c, repo, &rev)?;
    let sha = v.s("hash");
    Ok(json!({
        "oid": sha,
        "messageHeadline": v.s("message").lines().next().unwrap_or(""),
        "committedDate": v.s("date"),
        // Bitbucket doesn't count a branch's commits.
        "history": { "totalCount": 0 },
        "authors": authors(&v),
        "statusCheckRollup": checks(c, repo, &sha),
    }))
}

/// The last commit to touch each path, asked for at once.
fn last_commits(c: &Client, repo: &str, git_ref: &str, query: &str) -> Result<Value> {
    let paths = aliased(query, "history(first: 1, path: ");
    let at = resolve(c, repo, git_ref)?;
    let base = format!("{}/commits/{}", repo_api(repo), enc(&at));
    let found = parallel(&paths, |(_, path)| {
        get(c, &format!("{base}?path={}&pagelen=1", enc(path)))
            .ok()
            .and_then(|list| list.list("values").first().cloned())
    });
    let mut object = Map::new();
    for ((alias, _), v) in paths.iter().zip(found) {
        let nodes = match v {
            Some(v) => json!([{
                "oid": v.s("hash"),
                "messageHeadline": v.s("message").lines().next().unwrap_or(""),
                "committedDate": v.s("date"),
            }]),
            None => json!([]),
        };
        object.insert(alias.clone(), json!({ "nodes": nodes }));
    }
    Ok(json!({ "repository": { "object": object } }))
}

/// Commits' authors and checks, for a list of them.
fn commit_batch(c: &Client, repo: &str, query: &str) -> Result<Value> {
    let shas = aliased(query, ": object(oid: ");
    let found = parallel(&shas, |(_, sha)| {
        commit(c, repo, sha)
            .ok()
            .map(|v| json!({ "authors": authors(&v), "statusCheckRollup": checks(c, repo, sha) }))
    });
    let mut repository = Map::new();
    for ((alias, _), v) in shas.iter().zip(found) {
        if let Some(v) = v {
            repository.insert(alias.clone(), v);
        }
    }
    Ok(json!({ "repository": repository }))
}

/// `bb:pr:ws/repo#12` as the repository and number.
fn node(id: &str) -> Option<(String, i64)> {
    let rest = id.strip_prefix("bb:pr:")?;
    let (repo, n) = rest.rsplit_once('#')?;
    Some((repo.to_string(), n.parse().ok()?))
}

fn mutation(c: &Client, query: &str, vars: &Value) -> Result<Value> {
    let pr = || node(&vars.s("id")).ok_or_else(|| missing("pull request by that id"));
    if query.contains("markPullRequestReadyForReview")
        || query.contains("convertPullRequestToDraft")
    {
        let (repo, n) = pr()?;
        let path = format!("{}/pullrequests/{n}", repo_api(&repo));
        let title = get(c, &path)?.s("title");
        let draft = query.contains("convertPullRequestToDraft");
        return call(
            c,
            "PUT",
            &path,
            Some(&json!({ "title": title, "draft": draft })),
        );
    }
    if query.contains("PullRequestAutoMerge") {
        return Err(missing("auto-merge"));
    }
    Err(missing("GraphQL query like that one"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ids_and_coauthors() {
        assert_eq!(node("bb:pr:ws/r#12"), Some(("ws/r".to_string(), 12)));
        assert_eq!(node("gl:mr:a/b!1"), None);
        let commit = json!({
            "author": { "raw": "Ada <ada@x>", "user": { "links": { "avatar": { "href": "https://a" } } } },
            "message": "Fix\n\nCo-authored-by: Bob <bob@x>\n",
        });
        let people = authors(&commit);
        assert_eq!(people.s("nodes.0.avatarUrl"), "https://a");
        assert_eq!(people.s("nodes.1.name"), "Bob");
    }
}
