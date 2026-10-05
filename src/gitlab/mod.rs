//! GitLab, answering as GitHub would.
//!
//! The screens ask for GitHub's paths and read GitHub's fields. For a
//! GitLab account the client hands those requests here: each is matched
//! to GitLab's REST API (v4), and what GitLab answers is reshaped into
//! what GitHub would have said -- `iid` as `number`, `author.username` as
//! `user.login`, `opened` as `open` ([`shape`]). The few GraphQL queries
//! the screens send are answered the same way ([`graphql`]).
//!
//! Where GitLab has nothing like the page, the request fails with a 404
//! that says so; the screens hide most of those pages on GitLab anyway.
//! Pages that only GitLab has skip all this and ask for `/api/v4/...`
//! directly.

mod graphql;
mod issues;
mod people;
mod repos;
mod search;
pub mod shape;

use crate::api::{self, Client, Reply, Status};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use graphql::graphql;

/// Answers worth keeping between requests: who a login is, a project's
/// labels, the avatar behind an email. Kept for a while, then asked again.
#[derive(Default)]
pub struct Memo {
    map: Mutex<HashMap<String, (Instant, Value)>>,
}

impl Memo {
    /// The value under `key` if it is younger than `ttl`, or `make`'s.
    pub fn get_or(
        &self,
        key: &str,
        ttl: Duration,
        make: impl FnOnce() -> Result<Value>,
    ) -> Result<Value> {
        if let Some((at, value)) = self.map.lock().unwrap().get(key) {
            if at.elapsed() < ttl {
                return Ok(value.clone());
            }
        }
        let value = make()?;
        let mut map = self.map.lock().unwrap();
        if map.len() > 4000 {
            map.clear();
        }
        map.insert(key.to_string(), (Instant::now(), value.clone()));
        Ok(value)
    }

    /// Forget everything under `prefix`, after a write changes it.
    pub fn forget(&self, prefix: &str) {
        self.map
            .lock()
            .unwrap()
            .retain(|k, _| !k.starts_with(prefix));
    }
}

pub const MINUTE: Duration = Duration::from_secs(60);
pub const HOUR: Duration = Duration::from_secs(3600);

/// A GitHub-style request, taken apart.
pub struct Ask<'a> {
    pub c: &'a Client,
    pub method: &'a str,
    /// The path's parts, percent-decoded.
    pub parts: Vec<String>,
    pub query: Query,
    pub body: Option<&'a Value>,
}

impl<'a> Ask<'a> {
    fn new(c: &'a Client, method: &'a str, path: &str, body: Option<&'a Value>) -> Self {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        Ask {
            c,
            method,
            parts: path
                .split('/')
                .filter(|p| !p.is_empty())
                .map(decode)
                .collect(),
            query: Query::parse(query),
            body,
        }
    }

    pub fn get(&self) -> bool {
        self.method == "GET"
    }

    /// A field of the request's body.
    pub fn field(&self, path: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.body.map(|b| b.at(path)).unwrap_or(&NULL)
    }

    /// GitHub's paging, as GitLab spells it too.
    pub fn paging(&self) -> String {
        format!(
            "per_page={}&page={}",
            self.query.per_page(),
            self.query.page()
        )
    }
}

/// A query string's pairs, decoded.
#[derive(Default, Clone)]
pub struct Query(pub Vec<(String, String)>);

impl Query {
    pub fn parse(query: &str) -> Self {
        Query(
            query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    // The screens encode spaces as %20; a `+` is a plus
                    // (`content=+1`).
                    (decode(k), decode(v))
                })
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    pub fn page(&self) -> u64 {
        self.get("page")
            .and_then(|p| p.parse().ok())
            .unwrap_or(1)
            .max(1)
    }

    pub fn per_page(&self) -> u64 {
        self.get("per_page")
            .and_then(|p| p.parse().ok())
            .unwrap_or(30)
            .clamp(1, 100)
    }
}

/// `%2F` and friends, back to what they spell.
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A project's API root: `/projects/group%2Fproject`.
pub fn proj(repo: &str) -> String {
    format!("/projects/{}", enc(repo))
}

/// A group's API root.
pub fn grp(group: &str) -> String {
    format!("/groups/{}", enc(group))
}

/// The 404 for something GitLab has no equivalent of.
pub fn missing(what: &str) -> anyhow::Error {
    anyhow::Error::new(Status {
        code: 404,
        message: format!("GitLab has no {what}."),
    })
}

pub fn get(c: &Client, path: &str) -> Result<Value> {
    c.raw_json("GET", path, None)
}

pub fn call(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    c.raw_json(method, path, body)
}

/// Every page of a list, up to `max_pages` of 100.
pub fn get_all(c: &Client, path: &str, max_pages: u64) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    for page in 1..=max_pages {
        let list = get(
            c,
            &crate::hub::with_query(path, &format!("per_page=100&page={page}")),
        )?;
        let items = list.list("");
        all.extend(items.iter().cloned());
        if items.len() < 100 {
            break;
        }
    }
    Ok(all)
}

/// `f` over `items` on a few threads at once, answers in order.
pub fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..items.len().min(8) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                *slots[i].lock().unwrap() = Some(f(item));
            });
        }
    });
    slots
        .into_iter()
        .map(|s| s.into_inner().unwrap().unwrap())
        .collect()
}

/// The signed-in user, as GitLab has them.
pub fn me(c: &Client) -> Result<Value> {
    c.memo.get_or("me", HOUR, || get(c, "/user"))
}

/// A user by login.
pub fn user_by_login(c: &Client, login: &str) -> Result<Value> {
    let login = login.trim_start_matches('@').to_string();
    c.memo
        .get_or(&format!("user:{}", login.to_lowercase()), HOUR, || {
            let found = get(c, &format!("/users?username={}", enc(&login)))?;
            found.list("").first().cloned().ok_or_else(|| {
                anyhow::Error::new(Status {
                    code: 404,
                    message: format!("No GitLab user is called {login}."),
                })
            })
        })
}

pub fn user_id(c: &Client, login: &str) -> Result<i64> {
    Ok(user_by_login(c, login)?.i("id"))
}

/// A project's path from its id, for answers that only carry the id.
pub fn project_path(c: &Client, id: i64) -> String {
    c.memo
        .get_or(&format!("project-path:{id}"), HOUR, || {
            Ok(get(c, &format!("/projects/{id}?simple=true"))?
                .at("path_with_namespace")
                .clone())
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

/// Avatars for commit emails, which GitLab can look up but not attach.
pub fn avatars(c: &Client, emails: &[String]) -> HashMap<String, String> {
    let mut distinct: Vec<String> = Vec::new();
    for e in emails {
        if !e.is_empty() && !distinct.contains(e) {
            distinct.push(e.clone());
        }
    }
    let found = parallel(&distinct, |email| {
        c.memo
            .get_or(&format!("avatar:{email}"), HOUR, || {
                Ok(get(c, &format!("/avatar?email={}&size=64", enc(email)))?
                    .at("avatar_url")
                    .clone())
            })
            .map(|v| v.s(""))
            .unwrap_or_default()
    });
    distinct.into_iter().zip(found).collect()
}

// ---------------------------------------------------------------------------
// The way in.

/// A GitHub-style request, answered from GitLab.
pub fn rest(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let ask = Ask::new(c, method, path, body);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["user", rest @ ..] => people::user(&ask, rest),
        ["users", rest @ ..] => people::users(&ask, rest),
        ["orgs", rest @ ..] => people::orgs(&ask, rest),
        ["gists", rest @ ..] => people::gists(&ask, rest),
        ["search", kind] => search::search(&ask, kind),
        ["repos", rest @ ..] if rest.len() >= 2 => {
            let n = crate::forge::repo_len(rest);
            let repo = rest[..n].join("/");
            repos::route(&ask, &repo, &rest[n..])
        }
        ["gitignore", "templates"] => {
            let list = get_all(c, "/templates/gitignores", 5)?;
            Ok(Value::Array(
                list.iter().map(|t| json!(t.s("name"))).collect(),
            ))
        }
        ["licenses"] => {
            let list = get(c, "/templates/licenses?popular=false&per_page=100")?;
            Ok(Value::Array(list.list("").iter().map(|l| json!({ "key": l.s("key"), "name": l.s("name"), "spdx_id": l.s("key") })).collect()))
        }
        ["rate_limit"] => Ok(json!({})),
        ["notifications", ..] => Err(missing(
            "notifications inbox (its to-do list has its own page)",
        )),
        _ => Err(missing(&format!("equivalent of {}", ask.parts.join("/")))),
    }
}

/// Raw text: a file, a README.
pub fn text(c: &Client, path: &str) -> Result<String> {
    let ask = Ask::new(c, "GET", path, None);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["repos", rest @ ..] if rest.len() >= 2 => {
            let n = crate::forge::repo_len(rest);
            let repo = rest[..n].join("/");
            repos::text(&ask, &repo, &rest[n..])
        }
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
        ["user", "starred", rest @ ..] => repos::starred(c, &rest.join("/")),
        ["user", "following", login] => people::following(c, login),
        ["user", "blocks", ..] | ["gists", _, "star"] => Ok(false),
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
    fn queries_and_paths_decode() {
        assert_eq!(decode("a%2Fb%20c"), "a/b c");
        assert_eq!(decode("100%"), "100%");
        let q = Query::parse("state=open&labels=good%20first&per_page=200");
        assert_eq!(q.get("labels"), Some("good first"));
        assert_eq!(q.per_page(), 100);
        assert_eq!(q.page(), 1);
        assert_eq!(proj("a/b/c"), "/projects/a%2Fb%2Fc");
    }

    #[test]
    fn parallel_keeps_order() {
        let out = parallel(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], |n| n * 2);
        assert_eq!(out, [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
    }
}

/// The translation against gitlab.com itself, signed out, on a public
/// project: `cargo test gitlab_live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;
    use crate::forge::{Account, Forge};

    #[test]
    #[ignore]
    fn gitlab_live() {
        let account = Account::new(Forge::GitLab, "gitlab.com", "");
        crate::forge::set_current(&account);
        let c = Client::new(&account);
        let repo = "gitlab-org/gitlab-runner";
        let ask = |path: &str| {
            let v = rest(&c, "GET", path, None).unwrap_or_else(|e| panic!("{path}: {e:#}"));
            println!("{path}: {}", crate::json::clip(&v.to_string(), 300));
            v
        };
        let info = ask(&format!("/repos/{repo}"));
        assert_eq!(info.s("full_name"), repo);
        assert!(!info.s("default_branch").is_empty());
        let issues = ask(&format!(
            "/repos/{repo}/issues?state=open&sort=created&direction=desc&per_page=5&page=1"
        ));
        assert!(issues.list("")[0].i("number") > 0 && issues.list("")[0].has("user.login"));
        let pulls = ask(&format!("/repos/{repo}/pulls?state=open&per_page=5&page=1"));
        let pr = &pulls.list("")[0];
        assert!(pr.has("head.ref") && pr.has("pull_request"));
        let n = pr.i("number");
        let one = ask(&format!("/repos/{repo}/pulls/{n}"));
        assert_eq!(one.i("number"), n);
        let files = ask(&format!("/repos/{repo}/pulls/{n}/files?per_page=30&page=1"));
        assert!(files.list("").iter().all(|f| f.has("filename")));
        let commits = ask(&format!(
            "/repos/{repo}/commits?sha={}&per_page=5&page=1",
            info.s("default_branch")
        ));
        let sha = commits.list("")[0].s("sha");
        assert_eq!(sha.len(), 40);
        let commit = ask(&format!("/repos/{repo}/commits/{sha}"));
        assert!(commit.has("commit.message"));
        let root = ask(&format!(
            "/repos/{repo}/contents/?ref={}",
            info.s("default_branch")
        ));
        assert!(root.list("").iter().any(|e| e.s("type") == "dir"));
        let readme = text(
            &c,
            &format!("/repos/{repo}/readme?ref={}", info.s("default_branch")),
        )
        .unwrap();
        assert!(readme.len() > 100);
        ask(&format!("/repos/{repo}/branches?per_page=5&page=1"));
        ask(&format!("/repos/{repo}/tags?per_page=5&page=1"));
        let releases = ask(&format!("/repos/{repo}/releases?per_page=2&page=1"));
        let release = releases.list("")[0].i("id");
        assert_eq!(
            ask(&format!("/repos/{repo}/releases/{release}")).i("id"),
            release
        );
        // Signed out, gitlab.com answers some of these 401.
        let maybe = |path: &str| match rest(&c, "GET", path, None) {
            Ok(v) => println!("{path}: {}", crate::json::clip(&v.to_string(), 200)),
            Err(e) if matches!(api::status_of(&e), Some(401 | 403)) => {
                println!("{path}: needs signing in")
            }
            Err(e) => panic!("{path}: {e:#}"),
        };
        maybe(&format!("/repos/{repo}/labels?per_page=100"));
        maybe(&format!("/repos/{repo}/milestones?state=open&per_page=5"));
        maybe(&format!("/repos/{repo}/languages"));
        maybe(&format!("/repos/{repo}/contributors?per_page=5&page=1"));
        maybe(&format!(
            "/repos/{repo}/issues/{}/timeline?per_page=5&page=1",
            issues.list("")[0].i("number")
        ));
        maybe(&format!(
            "/repos/{repo}/merge_requests/{n}/timeline?per_page=5&page=1"
        ));
        maybe(&format!(
            "/repos/{repo}/pulls/{n}/commits?per_page=5&page=1"
        ));
        maybe(&format!("/repos/{repo}/compare/v19.4.0...v19.4.1"));
        ask(&format!("/repos/{repo}/check-runs/1/annotations"));
        let checks = ask(&format!("/repos/{repo}/commits/{sha}/check-runs"));
        println!("{} check runs", checks.i("total_count"));
        maybe("/users/vtak");
        maybe("/users/gitlab-org");
        let group = ask("/orgs/gitlab-org");
        assert_eq!(group.s("type"), "Organization");
        ask("/orgs/gitlab-org/repos?per_page=3");
        let found = ask(&format!(
            "/search/issues?q={}&per_page=5&page=1",
            crate::json::enc(&format!("repo:{repo} is:pr is:open"))
        ));
        assert!(found.list("items").iter().all(|i| i.has("pull_request")));
        ask("/search/repositories?q=gitlab-runner&per_page=3&page=1");
        let (o, nm) = repo.split_once('/').unwrap();
        let vars = serde_json::json!({ "o": o, "n": nm, "r": info.s("default_branch") });
        match graphql(
            &c,
            "query { repository { openIssues: issues(states: OPEN) { totalCount } } }",
            vars.clone(),
        ) {
            Ok(counts) => {
                println!("counts: {counts}");
                assert!(counts.i("repository.openPulls.totalCount") > 0);
            }
            Err(e) => println!("counts: {e:#}"),
        }
        let head = graphql(
            &c,
            "{ object(expression: $r) { history(first: 1) { totalCount } } }",
            vars.clone(),
        )
        .unwrap();
        println!("head: {head}");
        assert_eq!(head.s("repository.object.oid").len(), 40);
        let q = "{ e0: history(first: 1, path: \"README.md\") { nodes { oid } } e1: history(first: 1, path: \"go.mod\") { nodes { oid } } }";
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
