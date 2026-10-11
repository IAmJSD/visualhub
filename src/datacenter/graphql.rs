//! The GitHub GraphQL the screens send, answered from Data Center.
//!
//! The app asks GitHub's GraphQL for a few things REST doesn't give: open
//! and closed counts, a commit with its co-authors and checks, each file's
//! last commit, a profile's contribution calendar, and a handful of
//! mutations. Each is recognised by what it asks for and answered in the
//! same shape from the instance's REST API, as the GitLab and Bitbucket
//! translations do; anything else fails as Data Center having no such
//! thing.

use super::repos::statuses;
use super::{call, get, get_all, missing, parallel, repo_api, shape, with_avatars, Client};
use crate::gitlab::graphql::{aliased, numbered};
use crate::gitlab::MINUTE;
use crate::json::{enc, enc_path, Json as _};
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
    if query.contains(": pullRequest(number:") {
        return pull_checks(c, &repo, query);
    }
    if query.contains("contributionsCollection") {
        // The instance keeps no calendar; the profile goes without one.
        return Ok(json!({ "user": {
            "pinnedItems": { "nodes": [] },
            "contributionsCollection": { "contributionCalendar": { "totalContributions": 0, "weeks": [] } },
        } }));
    }
    if query.contains("repository(owner: $o, name: $n) { id }") {
        return Ok(json!({ "repository": { "id": format!("dc:repo:{repo}") } }));
    }
    mutation(c, query, &vars)
}

/// Open, merged and declined pull requests, counted; no issues.
fn counts(c: &Client, repo: &str) -> Result<Value> {
    let base = format!("{}/pull-requests", repo_api(repo));
    let states = ["OPEN", "MERGED", "DECLINED"];
    let totals = parallel(&states, |state| {
        c.memo
            .get_or(&format!("count:{repo}:{state}"), MINUTE, || {
                Ok(json!(get_all(c, &format!("{base}?state={state}"), 4)?.len()))
            })
            .map(|v| v.i("") as u64)
            .unwrap_or(0)
    });
    let count = |n: u64| json!({ "totalCount": n });
    Ok(json!({ "repository": {
        "openIssues": count(0),
        "closedIssues": count(0),
        "openPulls": count(totals[0]),
        "closedPulls": count(totals[1] + totals[2]),
        "closedOnlyPulls": count(totals[2]),
        "mergedPulls": count(totals[1]),
    } }))
}

/// A commit's authors: the author, then anyone in a `Co-authored-by:`
/// trailer.
fn authors(c: &Client, commit: &Value) -> Value {
    let who = commit.at("author");
    let mut people = vec![json!({
        "name": if who.has("displayName") { who.s("displayName") } else { who.s("name") },
        "avatarUrl": shape::absolute(c, &who.s("avatarUrl")),
        "user": if who.has("slug") { json!({ "login": who.s("slug") }) } else { Value::Null },
    })];
    for line in commit.s("message").lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Co-authored-by:")
            .or_else(|| line.strip_prefix("Co-Authored-By:"))
        else {
            continue;
        };
        let name = rest.split('<').next().unwrap_or("").trim().to_string();
        people.push(json!({ "name": name, "avatarUrl": "", "user": Value::Null }));
    }
    json!({ "nodes": people })
}

/// A commit, kept a while: it never changes, though its checks might.
fn commit(c: &Client, repo: &str, rev: &str) -> Result<Value> {
    c.memo
        .get_or(&format!("dccommit:{repo}@{rev}"), MINUTE, || {
            get(
                c,
                &with_avatars(&format!("{}/commits/{}", repo_api(repo), enc(rev))),
            )
        })
}

fn checks(c: &Client, sha: &str) -> Value {
    statuses(c, sha)
        .map(|list| shape::rollup(&list))
        .unwrap_or(Value::Null)
}

fn commit_head(c: &Client, repo: &str, rev: &str) -> Result<Value> {
    let rev = if rev.is_empty() {
        super::repos::default_branch(c, repo)
    } else {
        rev.to_string()
    };
    let v = commit(c, repo, &rev)?;
    let sha = v.s("id");
    Ok(json!({
        "oid": sha,
        "messageHeadline": v.s("message").lines().next().unwrap_or(""),
        "committedDate": shape::when(&v, "committerTimestamp"),
        // The instance doesn't count a branch's commits.
        "history": { "totalCount": 0 },
        "authors": authors(c, &v),
        "statusCheckRollup": checks(c, &sha),
    }))
}

/// The last commit to touch each path, asked for at once: the instance
/// answers a directory's worth in one request.
fn last_commits(c: &Client, repo: &str, git_ref: &str, query: &str) -> Result<Value> {
    let paths = aliased(query, "history(first: 1, path: ");
    let mut dirs: Vec<String> = paths
        .iter()
        .map(|(_, p)| {
            p.rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default()
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    let at = if git_ref.is_empty() {
        String::new()
    } else {
        format!("?at={}", enc(git_ref))
    };
    let found = parallel(&dirs, |dir| {
        get(
            c,
            &format!("{}/last-modified/{}{at}", repo_api(repo), enc_path(dir)),
        )
        .map(|v| v.at("files").clone())
        .unwrap_or(Value::Null)
    });
    let mut object = Map::new();
    for (alias, path) in &paths {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        let files = dirs
            .iter()
            .position(|d| d == dir)
            .map(|i| &found[i])
            .unwrap_or(&Value::Null);
        let nodes = match files.get(name) {
            Some(v) => json!([{
                "oid": v.s("id"),
                "messageHeadline": v.s("message").lines().next().unwrap_or(""),
                "committedDate": shape::when(v, "committerTimestamp"),
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
            .map(|v| json!({ "authors": authors(c, &v), "statusCheckRollup": checks(c, sha) }))
    });
    let mut repository = Map::new();
    for ((alias, _), v) in shas.iter().zip(found) {
        if let Some(v) = v {
            repository.insert(alias.clone(), v);
        }
    }
    Ok(json!({ "repository": repository }))
}

/// The checks on each pull request's head, by alias.
fn pull_checks(c: &Client, repo: &str, query: &str) -> Result<Value> {
    let numbers = numbered(query, ": pullRequest(number: ");
    let found = parallel(&numbers, |(_, n)| {
        get(c, &format!("{}/pull-requests/{n}", repo_api(repo)))
            .map(|pr| {
                let sha = pr.s("fromRef.latestCommit");
                (checks(c, &sha), sha)
            })
            .unwrap_or((Value::Null, String::new()))
    });
    let mut repository = Map::new();
    for ((alias, _), (rollup, sha)) in numbers.iter().zip(found) {
        repository.insert(
            alias.clone(),
            json!({ "commits": { "nodes": [{ "commit": { "oid": sha, "statusCheckRollup": rollup } }] } }),
        );
    }
    Ok(json!({ "repository": repository }))
}

/// `dc:pr:WID/widget#12` as the repository and number.
fn node(id: &str) -> Option<(String, i64)> {
    let rest = id.strip_prefix("dc:pr:")?;
    let (repo, n) = rest.rsplit_once('#')?;
    Some((repo.to_string(), n.parse().ok()?))
}

fn mutation(c: &Client, query: &str, vars: &Value) -> Result<Value> {
    let pr = || node(&vars.s("id")).ok_or_else(|| missing("pull request by that id"));
    if query.contains("markPullRequestReadyForReview")
        || query.contains("convertPullRequestToDraft")
    {
        let (repo, n) = pr()?;
        let path = format!("{}/pull-requests/{n}", repo_api(&repo));
        let current = get(c, &path)?;
        let reviewers: Vec<Value> = current
            .list("reviewers")
            .iter()
            .map(|r| json!({ "user": { "name": r.s("user.name") } }))
            .collect();
        let draft = query.contains("convertPullRequestToDraft");
        return call(
            c,
            "PUT",
            &path,
            Some(
                &json!({ "version": current.i("version"), "title": current.s("title"), "description": current.s("description"), "reviewers": reviewers, "draft": draft }),
            ),
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
    fn reads_ids() {
        assert_eq!(node("dc:pr:WID/w#12"), Some(("WID/w".to_string(), 12)));
        assert_eq!(node("bb:pr:ws/r#1"), None);
    }
}
