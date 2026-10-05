//! Bitbucket Cloud, answering as GitHub would.
//!
//! The screens ask for GitHub's paths and read GitHub's fields. For a
//! Bitbucket account the client hands those requests here: each is matched
//! to Bitbucket's REST API (2.0), and what Bitbucket answers is reshaped
//! into what GitHub would have said -- `values` as the list, `nickname`
//! as `user.login`, `OPEN` as `open`, a workspace as an organization
//! ([`shape`]). The few GraphQL queries the screens send are answered the
//! same way ([`graphql`]).
//!
//! Bitbucket's issue tracker is gone (its API went in August 2026), and it
//! has no stars, follows, releases or labels; those lists are empty and
//! their writes fail with a 404 that says so. Pages only Bitbucket has ask
//! for `/2.0/...` directly.

mod graphql;
mod people;
mod pulls;
mod repos;
mod search;
pub mod shape;

use crate::api::{self, Client, Reply, Status};
use crate::gitlab::{Ask, Query, HOUR};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};

pub use crate::gitlab::parallel;
pub use graphql::graphql;

/// The most Bitbucket gives a page of pull requests (and, to be safe, of
/// anything).
const MAX_PAGELEN: u64 = 50;

/// The 404 for something Bitbucket has no equivalent of.
pub fn missing(what: &str) -> anyhow::Error {
    anyhow::Error::new(Status {
        code: 404,
        message: format!("Bitbucket has no {what}."),
    })
}

pub fn get(c: &Client, path: &str) -> Result<Value> {
    c.raw_json("GET", path, None)
}

pub fn call(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    c.raw_json(method, path, body)
}

/// A repository's API root: `/repositories/workspace/slug`.
pub fn repo_api(repo: &str) -> String {
    let (ws, slug) = repo.split_once('/').unwrap_or((repo, ""));
    format!("/repositories/{}/{}", enc(ws), enc(slug))
}

/// One page of a list's items.
pub fn values(c: &Client, path: &str) -> Result<Vec<Value>> {
    Ok(get(c, path)?.list("values").to_vec())
}

/// GitHub's page `query` of a list: Bitbucket's pages hold at most 50, so
/// a page of 100 is two of Bitbucket's.
pub fn page(c: &Client, path: &str, query: &Query) -> Result<Vec<Value>> {
    page_at(c, path, query.per_page(), query.page())
}

pub fn page_at(c: &Client, path: &str, per: u64, page: u64) -> Result<Vec<Value>> {
    if per <= MAX_PAGELEN || !per.is_multiple_of(MAX_PAGELEN) {
        let per = per.min(MAX_PAGELEN);
        // Lists paged by token refuse even `page=1`.
        let query = if page > 1 {
            format!("pagelen={per}&page={page}")
        } else {
            format!("pagelen={per}")
        };
        return match values(c, &crate::hub::with_query(path, &query)) {
            // Some lists (a pull request's commits) page by tokens in
            // their `next` links rather than by number.
            Err(e) if page > 1 && api::status_of(&e) == Some(400) => walk(c, path, per, page),
            other => other,
        };
    }
    let parts = per / MAX_PAGELEN;
    let first = (page - 1) * parts + 1;
    let mut out = Vec::new();
    for p in first..first + parts {
        let items = page_at(c, path, MAX_PAGELEN, p)?;
        let done = (items.len() as u64) < MAX_PAGELEN;
        out.extend(items);
        if done {
            break;
        }
    }
    Ok(out)
}

/// Page `page` of `per`, reached by following `next` from the first.
fn walk(c: &Client, path: &str, per: u64, page: u64) -> Result<Vec<Value>> {
    let mut next = crate::hub::with_query(path, &format!("pagelen={per}"));
    for _ in 1..page {
        match get(c, &next)?.at("next").as_str() {
            Some(url) if !url.is_empty() => next = url.to_string(),
            _ => return Ok(Vec::new()),
        }
    }
    values(c, &next)
}

/// Every page of a list, following `next`, up to `max_pages` of 50.
pub fn get_all(c: &Client, path: &str, max_pages: u64) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    let mut next = crate::hub::with_query(path, &format!("pagelen={MAX_PAGELEN}"));
    for _ in 0..max_pages {
        let page = get(c, &next)?;
        all.extend(page.list("values").iter().cloned());
        match page.at("next").as_str() {
            Some(url) if !url.is_empty() => next = url.to_string(),
            _ => break,
        }
    }
    Ok(all)
}

/// How many items a list holds, where Bitbucket counts them.
pub fn size(c: &Client, path: &str) -> u64 {
    get(c, &crate::hub::with_query(path, "pagelen=1"))
        .map(|v| v.i("size").max(0) as u64)
        .unwrap_or(0)
}

/// A path to Bitbucket's own API, from a screen that pages as GitHub does:
/// `per_page` is `pagelen` here.
pub fn paged(path: &str) -> String {
    let Some((base, query)) = path.split_once('?') else {
        return path.to_string();
    };
    let query: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some(("per_page", n)) => {
                let n: u64 = n.parse().unwrap_or(30);
                format!("pagelen={}", n.clamp(1, MAX_PAGELEN))
            }
            _ => pair.to_string(),
        })
        .collect();
    format!("{base}?{}", query.join("&"))
}

/// One of Bitbucket's pages as the list it holds, the way the screens read
/// GitHub's lists; anything else as it is.
pub fn unpage(value: Value) -> Value {
    let is_page = value.get("values").is_some_and(Value::is_array)
        && (value.get("pagelen").is_some() || value.get("page").is_some());
    if is_page {
        value.get("values").cloned().unwrap_or(Value::Null)
    } else {
        value
    }
}

/// The signed-in user, as Bitbucket has them.
pub fn me(c: &Client) -> Result<Value> {
    c.memo.get_or("me", HOUR, || get(c, "/user"))
}

/// The signed-in user's UUID, `{…}`.
pub fn my_uuid(c: &Client) -> Result<String> {
    Ok(me(c)?.s("uuid"))
}

/// The workspaces the signed-in user is in, each with whether they
/// administer it.
pub fn workspaces(c: &Client) -> Result<Vec<Value>> {
    let list = c.memo.get_or("workspaces", HOUR, || {
        Ok(Value::Array(get_all(c, "/user/workspaces?sort=slug", 10)?))
    })?;
    Ok(list.list("").to_vec())
}

pub fn workspace_slugs(c: &Client) -> Result<Vec<String>> {
    Ok(workspaces(c)?
        .iter()
        .map(|w| w.s("workspace.slug"))
        .filter(|s| !s.is_empty())
        .collect())
}

/// The workspace that is the signed-in user's own, where repositories
/// "for yourself" go.
pub fn personal_workspace(c: &Client) -> Result<String> {
    let uuid = my_uuid(c)?;
    let list = workspaces(c)?;
    Ok(list
        .iter()
        .find(|w| w.s("workspace.uuid") == uuid)
        .or_else(|| list.first())
        .map(|w| w.s("workspace.slug"))
        .unwrap_or_default())
}

/// A person by the login the screens know them by: the nickname shown on
/// their comments, which [`shape::user`] remembers the account behind; or
/// an account ID or `{UUID}`, which Bitbucket looks up itself.
pub fn user_by_login(c: &Client, login: &str) -> Result<Value> {
    let login = login.trim_start_matches('@');
    if let Ok(found) = c.memo.get_or(&format!("login:{login}"), HOUR, || {
        Err(anyhow::anyhow!("not seen"))
    }) {
        return Ok(found);
    }
    if login == me(c)?.s("nickname") {
        return me(c);
    }
    get(c, &format!("/users/{}", enc(login))).map_err(|e| match api::status_of(&e) {
        Some(404) | Some(400) => anyhow::Error::new(Status {
            code: 404,
            message: format!(
                "No Bitbucket user goes by {login} that VisualHub has seen. \
                 Bitbucket looks people up by account ID."
            ),
        }),
        _ => e,
    })
}

/// A person's UUID, for the bodies that name people.
pub fn user_uuid(c: &Client, login: &str) -> Result<String> {
    Ok(user_by_login(c, login)?.s("uuid"))
}

// ---------------------------------------------------------------------------
// The way in.

/// A GitHub-style request, answered from Bitbucket.
pub fn rest(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let ask = Ask::new(c, method, path, body);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["user", rest @ ..] => people::user(&ask, rest),
        ["users", rest @ ..] => people::users(&ask, rest),
        ["orgs", rest @ ..] => people::orgs(&ask, rest),
        ["gists", rest @ ..] => people::gists(&ask, rest),
        ["search", kind] => search::search(&ask, kind),
        ["repos", ws, slug, rest @ ..] => repos::route(&ask, &format!("{ws}/{slug}"), rest),
        ["gitignore", "templates"] | ["licenses"] => Ok(json!([])),
        ["rate_limit"] => Ok(json!({})),
        ["notifications", ..] => Err(missing("notifications inbox")),
        _ => Err(missing(&format!("equivalent of {}", ask.parts.join("/")))),
    }
}

/// Raw text: a file, a README.
pub fn text(c: &Client, path: &str) -> Result<String> {
    let ask = Ask::new(c, "GET", path, None);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["repos", ws, slug, rest @ ..] => repos::text(&ask, &format!("{ws}/{slug}"), rest),
        _ => match rest(c, "GET", path, None)? {
            Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        },
    }
}

/// A yes-or-no question: starred, following, does the branch exist.
pub fn check(c: &Client, path: &str) -> Result<bool> {
    let ask = Ask::new(c, "GET", path, None);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["user", "starred", ..]
        | ["user", "following", ..]
        | ["user", "blocks", ..]
        | ["gists", .., "star"] => Ok(false),
        _ => match rest(c, "GET", path, None) {
            Ok(_) => Ok(true),
            Err(e) if api::status_of(&e) == Some(404) => Ok(false),
            Err(e) => Err(e),
        },
    }
}

/// [`rest`] as a [`Reply`], for the screens that look at the status.
pub fn send(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Reply> {
    let reply = |status: u16, body: String| Reply {
        status,
        body,
        scopes: None,
        accepted: None,
        total: None,
    };
    match rest(c, method, path, body) {
        Ok(value) => Ok(reply(200, value.to_string())),
        Err(e) => match api::status_of(&e) {
            Some(status) => Ok(reply(
                status,
                json!({ "message": format!("{e:#}") }).to_string(),
            )),
            None => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_read_as_lists() {
        assert_eq!(
            paged("/2.0/repositories/ws/r/hooks?per_page=100&page=2"),
            "/2.0/repositories/ws/r/hooks?pagelen=50&page=2"
        );
        assert_eq!(paged("/2.0/user"), "/2.0/user");
        assert_eq!(
            unpage(json!({ "values": [1, 2], "pagelen": 10, "page": 1 })),
            json!([1, 2])
        );
        assert_eq!(unpage(json!({ "values": [1] })), json!({ "values": [1] }));
        assert_eq!(repo_api("ws/my repo"), "/repositories/ws/my%20repo");
    }
}

/// The translation against Bitbucket itself, signed out, on a public
/// repository: `cargo test bitbucket_live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;
    use crate::forge::{Account, Forge};

    #[test]
    #[ignore]
    fn bitbucket_live() {
        let account = Account::new(Forge::Bitbucket, "", "");
        crate::forge::set_current(&account);
        let c = Client::new(&account);
        let repo = "atlassian/atlassian-connect-js";
        let ask = |path: &str| {
            let v = rest(&c, "GET", path, None).unwrap_or_else(|e| panic!("{path}: {e:#}"));
            println!("{path}: {}", crate::json::clip(&v.to_string(), 300));
            v
        };
        let info = ask(&format!("/repos/{repo}"));
        assert_eq!(info.s("full_name"), repo);
        let branch = info.s("default_branch");
        assert!(!branch.is_empty());
        let pulls = ask(&format!(
            "/repos/{repo}/pulls?state=closed&per_page=100&page=1"
        ));
        assert!(pulls.list("").len() > 50);
        let pr = &pulls.list("")[0];
        assert!(pr.has("head.ref") && pr.has("pull_request") && pr.has("user.login"));
        let n = pr.i("number");
        let one = ask(&format!("/repos/{repo}/pulls/{n}"));
        assert_eq!(one.i("number"), n);
        ask(&format!("/repos/{repo}/pulls/{n}/files?per_page=30&page=1"));
        ask(&format!(
            "/repos/{repo}/pulls/{n}/commits?per_page=5&page=1"
        ));
        ask(&format!(
            "/repos/{repo}/issues/{n}/timeline?per_page=30&page=1"
        ));
        ask(&format!("/repos/{repo}/pulls/{n}/comments"));
        let commits = ask(&format!(
            "/repos/{repo}/commits?sha={branch}&per_page=5&page=2"
        ));
        let sha = commits.list("")[0].s("sha");
        assert_eq!(sha.len(), 40);
        let commit = ask(&format!("/repos/{repo}/commits/{sha}"));
        assert!(commit.has("commit.message") && commit.has("files"));
        let checks = ask(&format!("/repos/{repo}/commits/{sha}/check-runs"));
        println!("{} check runs", checks.i("total_count"));
        let root = ask(&format!("/repos/{repo}/contents/?ref={branch}"));
        assert!(root.list("").iter().any(|e| e.s("type") == "dir"));
        let file = ask(&format!("/repos/{repo}/contents/package.json?ref={branch}"));
        assert_eq!(file.s("type"), "file");
        let readme = text(&c, &format!("/repos/{repo}/readme?ref={branch}")).unwrap();
        assert!(readme.len() > 100);
        let branches = ask(&format!("/repos/{repo}/branches?per_page=5&page=1"));
        let slashed = branches
            .list("")
            .iter()
            .map(|b| b.s("name"))
            .find(|b| b.contains('/'));
        if let Some(b) = slashed {
            ask(&format!("/repos/{repo}/contents/?ref={}", enc(&b)));
        }
        ask(&format!("/repos/{repo}/tags?per_page=5&page=1"));
        ask(&format!("/repos/{repo}/languages"));
        ask(&format!("/repos/{repo}/contributors?per_page=5&page=1"));
        ask(&format!("/repos/{repo}/compare/{branch}~3...{branch}"));
        ask(&format!("/repos/{repo}/releases?per_page=5"));
        ask("/orgs/atlassian");
        ask("/orgs/atlassian/repos?per_page=3");
        let vars = json!({ "o": "atlassian", "n": "atlassian-connect-js", "r": branch });
        let counts = graphql(
            &c,
            "query { repository { openIssues: issues(states: OPEN) { totalCount } } }",
            vars.clone(),
        )
        .unwrap();
        println!("counts: {counts}");
        // Zero when Bitbucket's rate limit for pull requests has run out.
        assert!(counts.has("repository.closedPulls.totalCount"));
        let head = graphql(
            &c,
            "{ object(expression: $r) { history(first: 1) { totalCount } } }",
            vars.clone(),
        )
        .unwrap();
        println!("head: {head}");
        assert_eq!(head.s("repository.object.oid").len(), 40);
        let q = "{ e0: history(first: 1, path: \"README.md\") { nodes { oid } } e1: history(first: 1, path: \"package.json\") { nodes { oid } } }";
        let last = graphql(&c, q, vars.clone()).unwrap();
        println!("last: {last}");
        assert_eq!(last.list("repository.object.e0.nodes").len(), 1);
        let batch = graphql(
            &c,
            &format!("{{ c0: object(oid: \"{sha}\") {{ x }} }}"),
            vars,
        )
        .unwrap();
        println!("batch: {batch}");
        assert!(batch.has("repository.c0.authors"));
    }
}
