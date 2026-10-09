//! The GitHub GraphQL the screens send, answered from GitLab.
//!
//! The app asks GitHub's GraphQL for a few things REST doesn't give: open
//! and closed counts, a commit with its co-authors and checks, each
//! file's last commit, a profile's contribution calendar, and a handful
//! of mutations. Each is recognised by what it asks for and answered in
//! the same shape from GitLab's REST API; anything else fails as GitLab
//! having no such thing.

use super::{avatars, call, get, missing, parallel, proj, shape, Client, MINUTE};
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
    if query.contains(": pullRequest(number:") {
        return pull_checks(c, &repo, query);
    }
    if query.contains("contributionsCollection") {
        return profile(c, &vars.s("l"));
    }
    if query.contains("repository(owner: $o, name: $n) { id }") {
        return Ok(json!({ "repository": { "id": format!("gl:project:{repo}") } }));
    }
    mutation(c, query, &vars)
}

/// Open and closed issues and merge requests. Issues have statistics;
/// merge requests are counted from their lists' `X-Total`.
fn counts(c: &Client, repo: &str) -> Result<Value> {
    let p = proj(repo);
    let stats = get(c, &format!("{p}/issues_statistics"))?;
    let states = ["opened", "closed", "merged"];
    let totals = parallel(&states, |state| {
        c.raw(
            "GET",
            &format!("{p}/merge_requests?state={state}&per_page=1"),
            None,
            None,
        )
        .ok()
        .and_then(|r| r.total)
        .unwrap_or(0)
    });
    let count = |n: u64| json!({ "totalCount": n });
    Ok(json!({ "repository": {
        "openIssues": { "totalCount": stats.i("statistics.counts.opened") },
        "closedIssues": { "totalCount": stats.i("statistics.counts.closed") },
        "openPulls": count(totals[0]),
        "closedPulls": count(totals[1] + totals[2]),
        // GitLab tells closed from merged; its lists can too.
        "closedOnlyPulls": count(totals[1]),
        "mergedPulls": count(totals[2]),
    } }))
}

/// A commit's authors: the author, then anyone in a `Co-authored-by:`
/// trailer, with faces looked up by email.
fn authors(c: &Client, commit: &Value) -> Value {
    let mut people = vec![(commit.s("author_name"), commit.s("author_email"))];
    for line in commit.s("message").lines() {
        let Some(rest) = line
            .trim()
            .strip_prefix("Co-authored-by:")
            .or_else(|| line.trim().strip_prefix("Co-Authored-By:"))
        else {
            continue;
        };
        if let Some((name, email)) = rest.trim().split_once('<') {
            people.push((
                name.trim().to_string(),
                email.trim_end_matches('>').trim().to_string(),
            ));
        }
    }
    let emails: Vec<String> = people.iter().map(|(_, e)| e.clone()).collect();
    let faces = avatars(c, &emails);
    json!({ "nodes": people.iter().map(|(name, email)| json!({ "name": name, "avatarUrl": faces.get(email).cloned().unwrap_or_default(), "user": Value::Null })).collect::<Vec<_>>() })
}

/// A commit, kept a little while: it never changes, though its pipeline
/// might, and running checks are asked after every few seconds.
fn commit(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    c.memo.get_or(&format!("commit:{repo}@{sha}"), SHORT, || {
        get(
            c,
            &format!("{}/repository/commits/{}", proj(repo), enc(sha)),
        )
    })
}

/// How long a pipeline's state is trusted.
const SHORT: std::time::Duration = std::time::Duration::from_secs(8);

/// A pipeline as GitHub's `statusCheckRollup`: its state, and its jobs
/// counted as check runs, the passed ones (succeeded or skipped) as
/// `SUCCESS`.
fn rollup(c: &Client, repo: &str, pipeline: &Value) -> Value {
    if !pipeline.has("status") {
        return Value::Null;
    }
    let id = pipeline.i("id");
    let jobs = c
        .memo
        .get_or(&format!("pipeline-jobs:{repo}:{id}"), SHORT, || {
            Ok(Value::Array(super::get_all(
                c,
                &format!("{}/pipelines/{id}/jobs", proj(repo)),
                3,
            )?))
        })
        .map(|v| v.list("").to_vec())
        .unwrap_or_default();
    let passed = jobs
        .iter()
        .filter(|j| matches!(j.s("status").as_str(), "success" | "skipped"))
        .count();
    json!({
        "state": shape::rollup(&pipeline.s("status")),
        "contexts": {
            "checkRunCount": jobs.len(),
            "checkRunCountsByState": [{ "state": "SUCCESS", "count": passed }],
            "statusContextCount": 0,
            "statusContextCountsByState": [],
        },
    })
}

fn checks(c: &Client, repo: &str, commit: &Value) -> Value {
    rollup(c, repo, commit.at("last_pipeline"))
}

/// The checks on each merge request's head, by alias.
fn pull_checks(c: &Client, repo: &str, query: &str) -> Result<Value> {
    let numbers = numbered(query, ": pullRequest(number: ");
    let found = parallel(&numbers, |(_, n)| {
        let path = format!("{}/merge_requests/{n}", proj(repo));
        let Ok(mr) = get(c, &path) else {
            return (Value::Null, Value::Null, String::new());
        };
        let approvals = if query.contains("reviewDecision") {
            get(c, &format!("{path}/approvals")).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        (
            rollup(c, repo, mr.at("head_pipeline")),
            review_decision(&mr, &approvals),
            mr.s("sha"),
        )
    });
    let mut repository = Map::new();
    for ((alias, _), (rollup, review, sha)) in numbers.iter().zip(found) {
        repository.insert(
            alias.clone(),
            json!({ "reviewDecision": review, "commits": { "nodes": [{ "commit": { "oid": sha, "statusCheckRollup": rollup } }] } }),
        );
    }
    Ok(json!({ "repository": repository }))
}

/// Where a merge request's review stands, as GitHub's `reviewDecision`.
/// GitLab calls a merge request approved once its rules are met, which
/// they are when it has none, so it only counts here once someone has.
fn review_decision(mr: &Value, approvals: &Value) -> Value {
    if mr.s("state") != "opened" {
        return Value::Null;
    }
    if mr.s("detailed_merge_status") == "requested_changes" {
        return json!("CHANGES_REQUESTED");
    }
    if approvals.is_null() {
        return Value::Null;
    }
    let approved_by = !approvals.list("approved_by").is_empty();
    if approvals.b("approved") && approved_by {
        json!("APPROVED")
    } else if !approvals.b("approved") {
        json!("REVIEW_REQUIRED")
    } else {
        Value::Null
    }
}

/// Every number after `marker` in `query`, with the alias before it:
/// `p3: pullRequest(number: 12)` gives `("p3", 12)`.
pub fn numbered(query: &str, marker: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = query[at..].find(marker) {
        let start = at + found;
        let alias = query[..start]
            .trim_end()
            .rsplit([' ', '{'])
            .next()
            .unwrap_or("")
            .to_string();
        let digits: String = query[start + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(n) = digits.parse() {
            out.push((alias, n));
        }
        at = start + marker.len();
    }
    out
}

fn commit_head(c: &Client, repo: &str, rev: &str) -> Result<Value> {
    let v = commit(c, repo, rev)?;
    // GitLab counts the default branch's commits only.
    let project = c
        .memo
        .get_or(&format!("project-stats:{repo}"), MINUTE, || {
            get(c, &format!("{}?statistics=true", proj(repo)))
        })?;
    let total = if rev == project.s("default_branch") || rev.is_empty() {
        project.i("statistics.commit_count")
    } else {
        0
    };
    Ok(json!({
        "oid": v.s("id"),
        "messageHeadline": v.s("title"),
        "committedDate": v.s("committed_date"),
        "history": { "totalCount": total },
        "authors": authors(c, &v),
        "statusCheckRollup": checks(c, repo, &v),
    }))
}

/// Every string literal after `marker` in `query`, with the alias before
/// it: `e3: history(first: 1, path: "src")` gives `("e3", "src")`.
pub fn aliased(query: &str, marker: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = query[at..].find(marker) {
        let start = at + found;
        let alias = query[..start]
            .trim_end()
            .trim_end_matches(':')
            .rsplit([' ', '{'])
            .next()
            .unwrap_or("")
            .to_string();
        let literal = &query[start + marker.len()..];
        let mut stream =
            serde_json::Deserializer::from_str(literal.trim_start()).into_iter::<String>();
        if let Some(Ok(value)) = stream.next() {
            out.push((alias, value));
        }
        at = start + marker.len();
    }
    out
}

/// The last commit to touch each path, asked for at once.
fn last_commits(c: &Client, repo: &str, git_ref: &str, query: &str) -> Result<Value> {
    let paths = aliased(query, "history(first: 1, path: ");
    let p = proj(repo);
    let found = parallel(&paths, |(_, path)| {
        get(
            c,
            &format!(
                "{p}/repository/commits?ref_name={}&path={}&per_page=1",
                enc(git_ref),
                enc(path)
            ),
        )
        .ok()
        .and_then(|list| list.list("").first().cloned())
    });
    let mut object = Map::new();
    for ((alias, _), commit) in paths.iter().zip(found) {
        let nodes = match commit {
            Some(v) => {
                json!([{ "oid": v.s("id"), "messageHeadline": v.s("title"), "committedDate": v.s("committed_date") }])
            }
            None => json!([]),
        };
        object.insert(alias.clone(), json!({ "nodes": nodes }));
    }
    Ok(json!({ "repository": { "object": object } }))
}

/// Commits' authors and checks, for a list of them.
fn commit_batch(c: &Client, repo: &str, query: &str) -> Result<Value> {
    let shas = aliased(query, ": object(oid: ");
    let found = parallel(&shas, |(_, sha)| commit(c, repo, sha).ok());
    let mut repository = Map::new();
    for ((alias, _), v) in shas.iter().zip(found) {
        if let Some(v) = v {
            repository.insert(
                alias.clone(),
                json!({ "authors": authors(c, &v), "statusCheckRollup": checks(c, repo, &v) }),
            );
        }
    }
    Ok(json!({ "repository": repository }))
}

/// Days since the epoch as a date, for the calendar.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// A profile's contribution calendar, from the one GitLab draws on
/// profiles: a year of weeks, Sunday first, shaded as GitHub shades.
fn profile(c: &Client, login: &str) -> Result<Value> {
    let counts = c
        .raw_json(
            "GET",
            &format!("{}/users/{}/calendar.json", c.web, enc(login)),
            None,
        )
        .unwrap_or(Value::Null);
    let today = crate::time::now().div_euclid(86_400);
    // Back to the Sunday 52 weeks before this week's.
    let start = today - (today + 4).rem_euclid(7) - 52 * 7;
    let max = match &counts {
        Value::Object(map) => map.values().filter_map(Value::as_i64).max().unwrap_or(0),
        _ => 0,
    };
    let shade = |n: i64| -> &'static str {
        match n {
            0 => "#ebedf0",
            n if n * 4 <= max => "#9be9a8",
            n if n * 2 <= max => "#40c463",
            n if n * 4 <= max * 3 => "#30a14e",
            _ => "#216e39",
        }
    };
    let mut weeks = Vec::new();
    let mut total = 0;
    let mut day = start;
    while day <= today {
        let mut days = Vec::new();
        for _ in 0..7 {
            if day > today {
                break;
            }
            let (y, m, d) = civil(day);
            let date = format!("{y:04}-{m:02}-{d:02}");
            let n = counts.i(&date);
            total += n;
            days.push(json!({ "contributionCount": n, "date": date, "color": shade(n) }));
            day += 1;
        }
        weeks.push(json!({ "contributionDays": days }));
    }
    Ok(json!({ "user": {
        "pinnedItems": { "nodes": [] },
        "contributionsCollection": { "contributionCalendar": { "totalContributions": total, "weeks": weeks } },
    } }))
}

/// `gl:mr:group/project!12` as the project and number.
fn node(id: &str, kind: &str, sep: char) -> Option<(String, i64)> {
    let rest = id.strip_prefix(&format!("gl:{kind}:"))?;
    let (repo, n) = rest.rsplit_once(sep)?;
    Some((repo.to_string(), n.parse().ok()?))
}

/// A title without GitLab's draft markers.
fn undraft(title: &str) -> String {
    let mut t = title.trim();
    for prefix in ["Draft:", "Draft -", "[Draft]", "(Draft)", "WIP:", "[WIP]"] {
        if t.len() >= prefix.len() && t[..prefix.len()].eq_ignore_ascii_case(prefix) {
            t = t[prefix.len()..].trim_start();
        }
    }
    t.to_string()
}

fn mutation(c: &Client, query: &str, vars: &Value) -> Result<Value> {
    let mr = || node(&vars.s("id"), "mr", '!').ok_or_else(|| missing("merge request by that id"));
    let issue =
        |key: &str| node(&vars.s(key), "issue", '#').ok_or_else(|| missing("issue by that id"));
    if query.contains("disablePullRequestAutoMerge") {
        let (repo, n) = mr()?;
        return call(
            c,
            "POST",
            &format!(
                "{}/merge_requests/{n}/cancel_merge_when_pipeline_succeeds",
                proj(&repo)
            ),
            None,
        );
    }
    if query.contains("enablePullRequestAutoMerge") {
        let (repo, n) = mr()?;
        let body = json!({ "merge_when_pipeline_succeeds": true, "auto_merge": true, "squash": vars.s("m") == "SQUASH" });
        return call(
            c,
            "PUT",
            &format!("{}/merge_requests/{n}/merge", proj(&repo)),
            Some(&body),
        );
    }
    if query.contains("markPullRequestReadyForReview")
        || query.contains("convertPullRequestToDraft")
    {
        let (repo, n) = mr()?;
        let path = format!("{}/merge_requests/{n}", proj(&repo));
        let title = undraft(&get(c, &path)?.s("title"));
        let title = if query.contains("convertPullRequestToDraft") {
            format!("Draft: {title}")
        } else {
            title
        };
        return call(c, "PUT", &path, Some(&json!({ "title": title })));
    }
    if query.contains("deleteIssue") {
        let (repo, n) = issue("id")?;
        return call(c, "DELETE", &format!("{}/issues/{n}", proj(&repo)), None);
    }
    if query.contains("transferIssue") {
        let (repo, n) = issue("i")?;
        let target = vars.s("r").trim_start_matches("gl:project:").to_string();
        let to = get(c, &proj(&target))?.i("id");
        let moved = call(
            c,
            "POST",
            &format!("{}/issues/{n}/move", proj(&repo)),
            Some(&json!({ "to_project_id": to })),
        )?;
        return Ok(
            json!({ "transferIssue": { "issue": { "number": moved.i("iid"), "url": moved.s("web_url") } } }),
        );
    }
    Err(missing("GraphQL query like that one"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_read_as_review_decisions() {
        let open = json!({ "state": "opened", "detailed_merge_status": "mergeable" });
        let by_ada =
            json!({ "approved": true, "approved_by": [{ "user": { "username": "ada" } }] });
        let no_rules = json!({ "approved": true, "approved_by": [] });
        let short =
            json!({ "approved": false, "approved_by": [{ "user": { "username": "ada" } }] });
        assert_eq!(review_decision(&open, &by_ada), json!("APPROVED"));
        assert_eq!(review_decision(&open, &no_rules), Value::Null);
        assert_eq!(review_decision(&open, &short), json!("REVIEW_REQUIRED"));
        let changes = json!({ "state": "opened", "detailed_merge_status": "requested_changes" });
        assert_eq!(
            review_decision(&changes, &by_ada),
            json!("CHANGES_REQUESTED")
        );
        let merged = json!({ "state": "merged" });
        assert_eq!(review_decision(&merged, &by_ada), Value::Null);
    }

    #[test]
    fn reads_aliases_and_ids() {
        let q = r#"query { repository(owner: $o, name: $n) { object(expression: $r) { ... on Commit { e0: history(first: 1, path: "src/a b.rs") { nodes { oid } } e1: history(first: 1, path: "README.md") { nodes { oid } } } } } }"#;
        assert_eq!(
            aliased(q, "history(first: 1, path: "),
            [
                ("e0".to_string(), "src/a b.rs".to_string()),
                ("e1".to_string(), "README.md".to_string())
            ]
        );
        assert_eq!(
            node("gl:mr:a/b/c!12", "mr", '!'),
            Some(("a/b/c".to_string(), 12))
        );
        assert_eq!(undraft("Draft: [WIP] faster"), "faster");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
    }

    #[test]
    fn reads_numbered_batches() {
        let q = "query { repository(owner: $o, name: $n) { p0: pullRequest(number: 205) { x } p1: pullRequest(number: 7) { x } } }";
        assert_eq!(
            numbered(q, ": pullRequest(number: "),
            [("p0".to_string(), 205), ("p1".to_string(), 7)]
        );
    }

    #[test]
    fn reads_commit_batches() {
        let q = r#"query($o: String!, $n: String!) { repository(owner: $o, name: $n) { c0: object(oid: "abc") { ... on Commit { authors(first: 10) { nodes { name } } } } c1: object(oid: "def") { ... on Commit { x } } } }"#;
        assert_eq!(
            aliased(q, ": object(oid: "),
            [
                ("c0".to_string(), "abc".to_string()),
                ("c1".to_string(), "def".to_string())
            ]
        );
    }
}
