//! Bitbucket Data Center (once Bitbucket Server), answering as GitHub
//! would.
//!
//! The screens ask for GitHub's paths and read GitHub's fields. For a Data
//! Center account the client hands those requests here: each is matched to
//! the instance's REST API (`/rest/api/latest`, and the branch, build
//! status, key and access-token APIs beside it), and what it answers is
//! reshaped into what GitHub would have said ([`shape`]).
//!
//! A repository is its project's key and slug, `WID/widget`; a person's
//! own repositories are in the project `~NAME`. A project stands in for an
//! organization. People are known by their slug, which the instance looks
//! up itself. Issues are Jira's, so `#12` is a pull request, as on
//! Bitbucket Cloud; there are no stars, follows, labels, releases,
//! reactions, snippets or notifications.
//!
//! Lists page by `start` and `limit` rather than by page number. Anything
//! that changes a pull request or a comment must say which version of it
//! it saw, so writes read the version first.

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

/// The core REST API, under the site.
pub const API: &str = "/rest/api/latest";

/// The most asked for in one page.
const MAX_LIMIT: u64 = 500;

/// The 404 for something Data Center has no equivalent of.
pub fn missing(what: &str) -> anyhow::Error {
    anyhow::Error::new(Status {
        code: 404,
        message: format!("Bitbucket Data Center has no {what}."),
    })
}

pub fn get(c: &Client, path: &str) -> Result<Value> {
    c.raw_json("GET", path, None)
}

pub fn call(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    c.raw_json(method, path, body)
}

/// `WID/widget` as the project key and the slug.
pub fn split(repo: &str) -> (&str, &str) {
    repo.split_once('/').unwrap_or((repo, ""))
}

/// A repository's API root: `/rest/api/latest/projects/WID/repos/widget`.
pub fn repo_api(repo: &str) -> String {
    let (key, slug) = split(repo);
    format!("{API}/projects/{}/repos/{}", enc(key), enc(slug))
}

/// The same under another of the instance's APIs (`branch-utils`, `git`,
/// `keys`): `/rest/{api}/latest/projects/WID/repos/widget`.
pub fn repo_api_at(api: &str, repo: &str) -> String {
    let (key, slug) = split(repo);
    format!(
        "/rest/{api}/latest/projects/{}/repos/{}",
        enc(key),
        enc(slug)
    )
}

/// `path` asking for people's avatars too.
pub fn with_avatars(path: &str) -> String {
    crate::hub::with_query(path, "avatarSize=64")
}

/// One page of a list's items, `per` long, from `start`.
pub fn values_at(c: &Client, path: &str, start: u64, per: u64) -> Result<Vec<Value>> {
    let path = crate::hub::with_query(path, &format!("start={start}&limit={per}"));
    Ok(get(c, &with_avatars(&path))?.list("values").to_vec())
}

/// GitHub's page `query` of a list.
pub fn page(c: &Client, path: &str, query: &Query) -> Result<Vec<Value>> {
    let per = query.per_page().min(MAX_LIMIT);
    values_at(c, path, (query.page() - 1) * per, per)
}

/// Every page of a list, up to `max_pages` of 500.
pub fn get_all(c: &Client, path: &str, max_pages: u64) -> Result<Vec<Value>> {
    let mut all = Vec::new();
    let mut start = 0;
    for _ in 0..max_pages {
        let path = crate::hub::with_query(path, &format!("start={start}&limit={MAX_LIMIT}"));
        let page = get(c, &with_avatars(&path))?;
        all.extend(page.list("values").iter().cloned());
        if page.b("isLastPage") || !page.has("nextPageStart") {
            break;
        }
        start = page.i("nextPageStart") as u64;
    }
    Ok(all)
}

/// GitHub's page of a list made whole here (filtered or put together from
/// several of the instance's).
pub fn slice(all: Vec<Value>, query: &Query) -> Value {
    let (per, page) = (query.per_page() as usize, query.page() as usize);
    Value::Array(all.into_iter().skip(per * (page - 1)).take(per).collect())
}

/// The signed-in user, by the name the instance answers any request
/// with (`X-AUSERNAME`).
pub fn me(c: &Client) -> Result<Value> {
    c.memo.get_or("me", HOUR, || {
        let reply = c.raw("GET", &format!("{API}/application-properties"), None, None)?;
        let Some(name) = reply.user.filter(|_| reply.status < 400) else {
            return Err(anyhow::Error::new(Status {
                code: 401,
                message: "The Bitbucket Data Center didn't recognise the token.".into(),
            }));
        };
        // The name is what people sign in with; the slug finds them.
        let found = get_all(c, &format!("{API}/users?filter={}", enc(&name)), 1)?;
        found
            .into_iter()
            .find(|u| u.s("name") == name)
            .ok_or_else(|| missing(&format!("person called {name}")))
    })
}

/// A person by their slug.
pub fn user_by_login(c: &Client, login: &str) -> Result<Value> {
    let login = login.trim_start_matches('@');
    c.memo.get_or(&format!("user:{login}"), HOUR, || {
        get(c, &with_avatars(&format!("{API}/users/{}", enc(login)))).map_err(|e| {
            match api::status_of(&e) {
                Some(404) => anyhow::Error::new(Status {
                    code: 404,
                    message: format!("No one on this Bitbucket goes by {login}."),
                }),
                _ => e,
            }
        })
    })
}

// ---------------------------------------------------------------------------
// The way in.

/// A GitHub-style request, answered from the Data Center.
pub fn rest(c: &Client, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let ask = Ask::new(c, method, path, body);
    let parts: Vec<&str> = ask.parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["user", rest @ ..] => people::user(&ask, rest),
        ["users", rest @ ..] => people::users(&ask, rest),
        ["orgs", rest @ ..] => people::orgs(&ask, rest),
        ["gists", ..] if ask.get() => Ok(json!([])),
        ["gists", ..] => Err(missing("snippets")),
        ["search", kind] => search::search(&ask, kind),
        ["repos", key, slug, rest @ ..] => repos::route(&ask, &format!("{key}/{slug}"), rest),
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
        ["repos", key, slug, rest @ ..] => repos::text(&ask, &format!("{key}/{slug}"), rest),
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
        | ["gists", ..] => Ok(false),
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
        user: None,
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
    fn repositories_have_their_project() {
        assert_eq!(
            repo_api("WID/my repo"),
            "/rest/api/latest/projects/WID/repos/my%20repo"
        );
        assert_eq!(
            repo_api_at("branch-utils", "~ADA/notes"),
            "/rest/branch-utils/latest/projects/~ADA/repos/notes"
        );
        assert_eq!(with_avatars("/x?start=0"), "/x?start=0&avatarSize=64");
    }
}

/// The translation against a Bitbucket Data Center, reading and writing:
/// `VISUALHUB_DC_HOST=localhost:7990 VISUALHUB_DC_TOKEN=… VISUALHUB_DC_OTHER=bob:password
/// cargo test datacenter_live -- --ignored --nocapture`. It expects the
/// repository `WID/widget` with an open pull request from `feature/speed`
/// that the other person reviews.
#[cfg(test)]
mod live {
    use super::*;
    use crate::forge::{Account, Forge};

    fn client(token: &str) -> Client {
        let host = std::env::var("VISUALHUB_DC_HOST").expect("VISUALHUB_DC_HOST");
        let host = if host.starts_with("http") {
            host
        } else {
            format!("http://{host}")
        };
        let account = Account::new(Forge::DataCenter, &host, token);
        crate::forge::set_current(&account);
        Client::new(&account)
    }

    #[test]
    #[ignore]
    fn datacenter_live() {
        let c = client(&std::env::var("VISUALHUB_DC_TOKEN").expect("VISUALHUB_DC_TOKEN"));
        let other = client(&std::env::var("VISUALHUB_DC_OTHER").expect("VISUALHUB_DC_OTHER"));
        let repo = "WID/widget";
        let ask = |c: &Client, method: &str, path: &str, body: Option<Value>| {
            let v = rest(c, method, path, body.as_ref())
                .unwrap_or_else(|e| panic!("{method} {path}: {e:#}"));
            println!(
                "{method} {path}: {}",
                crate::json::clip(&v.to_string(), 240)
            );
            v
        };
        let get = |path: &str| ask(&c, "GET", path, None);

        let me = get("/user");
        assert_eq!(me.s("login"), "admin");
        assert!(me.s("avatar_url").starts_with("http"));
        let info = get(&format!("/repos/{repo}"));
        assert_eq!(info.s("full_name"), repo);
        assert_eq!(info.s("default_branch"), "main");
        assert!(info.b("permissions.admin"));
        assert!(get("/user/repos?per_page=30")
            .list("")
            .iter()
            .any(|r| r.s("full_name") == repo));
        assert!(get("/user/orgs")
            .list("")
            .iter()
            .any(|o| o.s("login") == "WID"));
        assert_eq!(get("/orgs/WID").s("type"), "Organization");
        assert!(!get("/orgs/WID/repos").list("").is_empty());
        assert_eq!(get("/users/bob").s("name"), "Bob Builder");
        assert_eq!(get("/users/WID").s("login"), "WID");

        // Files.
        let root = get(&format!("/repos/{repo}/contents/?ref=main"));
        assert!(root
            .list("")
            .iter()
            .any(|e| e.s("name") == "src" && e.s("type") == "dir"));
        let src = get(&format!("/repos/{repo}/contents/src?ref=main"));
        assert_eq!(src.s("0.path"), "src/main.rs");
        let file = get(&format!(
            "/repos/{repo}/contents/src/main.rs?ref=feature/speed"
        ));
        assert_eq!(file.s("type"), "file");
        let readme = text(&c, &format!("/repos/{repo}/readme?ref=main")).unwrap();
        assert!(readme.starts_with("# Widget"));
        let tree = get(&format!("/repos/{repo}/git/trees/main?recursive=1"));
        assert!(tree
            .list("tree")
            .iter()
            .any(|t| t.s("path") == "docs" && t.s("type") == "tree"));

        // History.
        let branches = get(&format!("/repos/{repo}/branches"));
        assert!(branches
            .list("")
            .iter()
            .any(|b| b.s("name") == "feature/speed"));
        assert_eq!(
            get(&format!("/repos/{repo}/branches/feature/speed")).s("name"),
            "feature/speed"
        );
        assert_eq!(get(&format!("/repos/{repo}/tags")).s("0.name"), "v0.1.0");
        let commits = get(&format!(
            "/repos/{repo}/commits?sha=feature/speed&per_page=5"
        ));
        let head = commits.s("0.sha");
        assert_eq!(commits.list("").len(), 3);
        assert_eq!(commits.s("0.author.login"), "admin");
        let commit = get(&format!("/repos/{repo}/commits/{head}"));
        assert_eq!(commit.list("files").len(), 2);
        assert!(commit.s("files.1.patch").contains("+    7"));
        assert_eq!(
            get(&format!("/repos/{repo}/commits?author=admin&per_page=2"))
                .list("")
                .len(),
            2
        );
        let checks = get(&format!("/repos/{repo}/commits/{head}/check-runs"));
        assert_eq!(checks.i("total_count"), 2);
        assert_eq!(
            get(&format!("/repos/{repo}/commits/{head}/status")).s("state"),
            "pending"
        );
        let compare = get(&format!("/repos/{repo}/compare/main...feature/speed"));
        assert_eq!(
            (compare.i("ahead_by"), compare.s("status").as_str()),
            (1, "ahead")
        );
        assert_eq!(compare.list("files").len(), 2);

        // The pull request.
        let pulls = get(&format!("/repos/{repo}/pulls?state=open"));
        let n = pulls.i("0.number");
        assert_eq!(pulls.s("0.head.ref"), "feature/speed");
        let pr = get(&format!("/repos/{repo}/pulls/{n}"));
        assert_eq!((pr.i("changed_files"), pr.i("commits")), (2, 1));
        assert_eq!(pr.s("requested_reviewers.0.login"), "bob");
        println!(
            "mergeable: {} {}",
            pr.s("mergeable_state"),
            pr.s("merge_blocker")
        );
        let files = get(&format!("/repos/{repo}/pulls/{n}/files"));
        assert!(files.s("1.patch").starts_with("@@"));
        assert!(
            get(&format!("/repos/{repo}/pulls?head=WID:feature/speed"))
                .list("")
                .len()
                == 1
        );
        let timeline = get(&format!("/repos/{repo}/issues/{n}/timeline?per_page=100"));
        assert!(timeline
            .list("")
            .iter()
            .any(|e| e.s("body") == "approved these changes"));
        let lines = get(&format!("/repos/{repo}/pulls/{n}/comments"));
        assert_eq!(
            (
                lines.s("0.path").as_str(),
                lines.i("0.line"),
                lines.s("0.side").as_str()
            ),
            ("src/main.rs", 6, "RIGHT")
        );
        let reviews = get(&format!("/repos/{repo}/pulls/{n}/reviews"));
        assert_eq!(reviews.s("0.state"), "APPROVED");
        let counts = graphql(
            &c,
            "query { repository { openIssues: issues(states: OPEN) { totalCount } } }",
            json!({ "o": "WID", "n": "widget" }),
        )
        .unwrap();
        assert_eq!(counts.i("repository.openPulls.totalCount"), 1);
        let head_q = graphql(
            &c,
            "{ object(expression: $r) { history(first: 1) { totalCount } } }",
            json!({ "o": "WID", "n": "widget", "r": "main" }),
        )
        .unwrap();
        assert_eq!(head_q.s("repository.object.oid").len(), 40);
        assert_eq!(
            head_q.s("repository.object.authors.nodes.1.name"),
            "Bob Builder"
        );
        let last = graphql(&c, "{ e0: history(first: 1, path: \"README.md\") { nodes { oid } } e1: history(first: 1, path: \"src/main.rs\") { nodes { oid } } }", json!({ "o": "WID", "n": "widget", "r": "main" })).unwrap();
        assert_eq!(last.list("repository.object.e1.nodes").len(), 1);

        // Conversation: comment, edit, reply, a line comment, delete.
        let said = ask(
            &c,
            "POST",
            &format!("/repos/{repo}/issues/{n}/comments"),
            Some(json!({ "body": "From VisualHub" })),
        );
        let id = said.i("id");
        ask(
            &c,
            "PATCH",
            &format!("/repos/{repo}/pullrequests/{n}/comments/{id}"),
            Some(json!({ "body": "From VisualHub, edited" })),
        );
        ask(
            &c,
            "POST",
            &format!("/repos/{repo}/pulls/{n}/comments/{id}/replies"),
            Some(json!({ "body": "A reply" })),
        );
        let convo = get(&format!("/repos/{repo}/issues/{n}/comments?per_page=100"));
        assert!(convo
            .list("")
            .iter()
            .any(|cm| cm.s("body") == "From VisualHub, edited"));
        assert!(convo
            .list("")
            .iter()
            .any(|cm| cm.s("body") == "A reply" && cm.i("in_reply_to_id") == id));
        let line = ask(
            &c,
            "POST",
            &format!("/repos/{repo}/pulls/{n}/comments"),
            Some(
                json!({ "body": "On a context line", "path": "src/main.rs", "line": 5, "side": "RIGHT" }),
            ),
        );
        assert_eq!(line.s("anchor.lineType"), "CONTEXT");
        let removed = ask(
            &c,
            "POST",
            &format!("/repos/{repo}/pulls/{n}/comments"),
            Some(
                json!({ "body": "On a removed line", "path": "src/main.rs", "line": 6, "side": "LEFT" }),
            ),
        );
        assert_eq!(removed.s("anchor.lineType"), "REMOVED");
        ask(
            &c,
            "DELETE",
            &format!(
                "/repos/{repo}/pullrequests/{n}/comments/{}",
                removed.i("id")
            ),
            None,
        );

        // Reviews, as the reviewer; and the pull request's own edits.
        ask(
            &other,
            "POST",
            &format!("/repos/{repo}/pulls/{n}/reviews"),
            Some(json!({ "event": "REQUEST_CHANGES", "body": "Not yet" })),
        );
        assert_eq!(
            rest(&c, "GET", &format!("/repos/{repo}/pulls/{n}/reviews"), None)
                .unwrap()
                .s("0.state"),
            "CHANGES_REQUESTED"
        );
        ask(
            &other,
            "POST",
            &format!("/repos/{repo}/pulls/{n}/reviews"),
            Some(json!({ "event": "APPROVE" })),
        );
        let c = client(&std::env::var("VISUALHUB_DC_TOKEN").unwrap());
        let ask = |method: &str, path: &str, body: Option<Value>| ask(&c, method, path, body);
        let edited = ask(
            "PATCH",
            &format!("/repos/{repo}/pulls/{n}"),
            Some(json!({ "title": "Make the widget much faster" })),
        );
        assert_eq!(edited.s("title"), "Make the widget much faster");
        assert_eq!(edited.s("requested_reviewers.0.login"), "bob");
        let node = edited.s("node_id");
        graphql(&c, "mutation($id: ID!) { convertPullRequestToDraft(input: {pullRequestId: $id}) { clientMutationId } }", json!({ "id": node })).unwrap();
        assert!(ask("GET", &format!("/repos/{repo}/pulls/{n}"), None).b("draft"));
        graphql(&c, "mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { clientMutationId } }", json!({ "id": node })).unwrap();
        ask(
            "PATCH",
            &format!("/repos/{repo}/pulls/{n}"),
            Some(json!({ "state": "closed" })),
        );
        assert_eq!(
            ask("GET", &format!("/repos/{repo}/pulls/{n}"), None).s("state"),
            "closed"
        );
        ask(
            "PATCH",
            &format!("/repos/{repo}/pulls/{n}"),
            Some(json!({ "state": "open" })),
        );

        // Branches and tags, made and deleted; a new pull request, merged.
        let main = ask("GET", &format!("/repos/{repo}/branches/main"), None).s("commit.sha");
        ask(
            "POST",
            &format!("/repos/{repo}/git/refs"),
            Some(json!({ "ref": "refs/heads/try/vh", "sha": head })),
        );
        // Merged into a base of its own, so `main` stays as it was.
        ask(
            "POST",
            &format!("/repos/{repo}/git/refs"),
            Some(json!({ "ref": "refs/heads/try/base", "sha": main })),
        );
        ask(
            "POST",
            &format!("/repos/{repo}/git/refs"),
            Some(json!({ "ref": "refs/tags/vh-tag", "sha": main })),
        );
        assert!(check(&c, &format!("/repos/{repo}/branches/try/vh")).unwrap());
        ask(
            "DELETE",
            &format!("/repos/{repo}/git/refs/tags/vh-tag"),
            None,
        );
        let made = ask(
            "POST",
            &format!("/repos/{repo}/pulls"),
            Some(
                json!({ "title": "Try it", "head": "try/vh", "base": "try/base", "body": "made here" }),
            ),
        );
        let m = made.i("number");
        let merged = ask(
            "PUT",
            &format!("/repos/{repo}/pulls/{m}/merge"),
            Some(json!({ "merge_method": "merge", "commit_title": "Merge try" })),
        );
        assert!(merged.b("merged"), "{merged}");
        ask(
            "DELETE",
            &format!("/repos/{repo}/git/refs/heads/try/vh"),
            None,
        );
        ask(
            "DELETE",
            &format!("/repos/{repo}/git/refs/heads/try/base"),
            None,
        );
        assert!(!check(&c, &format!("/repos/{repo}/branches/try/vh")).unwrap());
        assert_eq!(
            ask("GET", &format!("/repos/{repo}/pulls/{m}"), None).s("merged"),
            "true"
        );

        // A repository of your own, its settings, keys, hooks and access.
        let mine = ask(
            "POST",
            "/user/repos",
            Some(json!({ "name": "vh scratch", "private": true })),
        );
        let r2 = mine.s("full_name");
        assert_eq!(r2, "~ADMIN/vh-scratch");
        ask(
            "PATCH",
            &format!("/repos/{r2}"),
            Some(json!({ "description": "made by the live test" })),
        );
        assert_eq!(
            ask("GET", &format!("/repos/{r2}"), None).s("description"),
            "made by the live test"
        );
        let key = ask(
            "POST",
            &format!("/repos/{r2}/keys"),
            Some(
                json!({ "title": "deploy", "key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl vh@test" }),
            ),
        );
        ask("DELETE", &format!("/repos/{r2}/keys/{}", key.i("id")), None);
        let hook = ask(
            "POST",
            &format!("/repos/{r2}/hooks"),
            Some(json!({ "config": { "url": "https://hooks.example.com/vh" } })),
        );
        assert_eq!(
            ask("GET", &format!("/repos/{r2}/hooks"), None).s("0.config.url"),
            "https://hooks.example.com/vh"
        );
        ask(
            "DELETE",
            &format!("/repos/{r2}/hooks/{}", hook.i("id")),
            None,
        );
        ask(
            "PUT",
            &format!("/repos/{r2}/collaborators/bob"),
            Some(json!({ "permission": "push" })),
        );
        assert_eq!(
            ask("GET", &format!("/repos/{r2}/collaborators"), None).s("0.login"),
            "bob"
        );
        ask("DELETE", &format!("/repos/{r2}/collaborators/bob"), None);
        ask("DELETE", &format!("/repos/{r2}"), None);

        // Searches the screens send.
        let mine = ask(
            "GET",
            "/search/issues?q=is:pr%20is:open%20author:@me&per_page=10",
            None,
        );
        assert!(mine.i("total_count") >= 1);
        let reviewing = rest(
            &other,
            "GET",
            "/search/issues?q=is:pr%20is:open%20review-requested:@me&per_page=10",
            None,
        )
        .unwrap();
        assert!(reviewing.i("total_count") >= 1);
        let found = ask("GET", "/search/repositories?q=widg&per_page=10", None);
        assert!(found.list("items").iter().any(|r| r.s("full_name") == repo));
    }
}
