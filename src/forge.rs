//! Which forge an account lives on -- GitHub, a GitLab instance, or
//! Bitbucket Cloud -- and what the screens draw differently because of it.
//!
//! The screens are written against GitHub's API and GitHub's words. On a
//! GitLab or Bitbucket account the client translates those requests
//! ([`crate::gitlab`], [`crate::bitbucket`]), so most pages need nothing
//! from here. What they do need is what they
//! draw themselves: what a pull request is called, where a page lives on
//! the web, how a link reads back as a page, and where a repository's path
//! ends now that GitLab's groups nest (`group/subgroup/project`).

use crate::hub::{PullTab, RepoTab, Route};
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Forge {
    GitHub,
    GitLab,
    Bitbucket,
}

impl Forge {
    pub fn name(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub",
            Forge::GitLab => "GitLab",
            Forge::Bitbucket => "Bitbucket",
        }
    }

    /// How it is written in the accounts file.
    pub fn key(self) -> &'static str {
        match self {
            Forge::GitHub => "github",
            Forge::GitLab => "gitlab",
            Forge::Bitbucket => "bitbucket",
        }
    }

    pub fn from_key(key: &str) -> Option<Forge> {
        match key {
            "github" => Some(Forge::GitHub),
            "gitlab" => Some(Forge::GitLab),
            "bitbucket" => Some(Forge::Bitbucket),
            _ => None,
        }
    }
}

/// Somewhere to sign in: a forge, its host, and the token that opens it.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Account {
    pub forge: Forge,
    /// `github.com`, `gitlab.com`, a self-managed GitLab's host, or
    /// `bitbucket.org`. An `http://` prefix is kept for instances without
    /// TLS.
    pub host: String,
    pub token: String,
}

impl Account {
    pub fn new(forge: Forge, host: &str, token: &str) -> Self {
        Account {
            forge,
            // Bitbucket Cloud is the one site.
            host: match forge {
                Forge::Bitbucket => BITBUCKET_HOST.into(),
                _ => normalize_host(host),
            },
            token: token.trim().to_string(),
        }
    }

    /// The site, without a trailing slash.
    pub fn web(&self) -> String {
        if self.host.starts_with("http://") {
            self.host.clone()
        } else {
            format!("https://{}", self.host)
        }
    }

    /// The REST API's root.
    pub fn api(&self) -> String {
        match self.forge {
            Forge::GitHub => "https://api.github.com".into(),
            Forge::GitLab => format!("{}/api/v4", self.web()),
            Forge::Bitbucket => BITBUCKET_API.into(),
        }
    }

    /// The host as people write it: `gitlab.example.com`.
    pub fn host_name(&self) -> &str {
        self.host.trim_start_matches("http://")
    }

    /// Tells accounts apart without showing the token.
    pub fn key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.token.hash(&mut h);
        format!("{}:{}:{:x}", self.forge.key(), self.host, h.finish())
    }
}

pub const BITBUCKET_HOST: &str = "bitbucket.org";
/// Bitbucket Cloud's REST API. Pages only Bitbucket has ask for paths
/// under `/2.0/` on [`BITBUCKET_ROOT`] directly.
pub const BITBUCKET_API: &str = "https://api.bitbucket.org/2.0";
pub const BITBUCKET_ROOT: &str = "https://api.bitbucket.org";

/// `https://gitlab.example.com/` as `gitlab.example.com`.
pub fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    let host = host.strip_prefix("https://").unwrap_or(host);
    if host.is_empty() {
        "gitlab.com".into()
    } else {
        host.to_string()
    }
}

// ---------------------------------------------------------------------------
// The account the window shows.

struct Current {
    forge: Forge,
    web: String,
}

thread_local! {
    static CURRENT: RefCell<Current> = RefCell::new(Current { forge: Forge::GitHub, web: "https://github.com".into() });
}

/// Make `account` the one pages are drawn for.
pub fn set_current(account: &Account) {
    CURRENT.with(|c| {
        *c.borrow_mut() = Current {
            forge: account.forge,
            web: account.web(),
        }
    });
}

pub fn current() -> Forge {
    CURRENT.with(|c| c.borrow().forge)
}

pub fn is_gitlab() -> bool {
    current() == Forge::GitLab
}

pub fn is_bitbucket() -> bool {
    current() == Forge::Bitbucket
}

pub fn is_github() -> bool {
    current() == Forge::GitHub
}

/// The site the current account lives on: `https://github.com`.
pub fn web() -> String {
    CURRENT.with(|c| c.borrow().web.clone())
}

/// `github.com`, `gitlab.example.com`.
pub fn host() -> String {
    let web = web();
    web.trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string()
}

/// A link the forge gave as a path (`/uploads/…`), made whole.
pub fn absolute(url: &str) -> String {
    if url.starts_with('/') {
        format!("{}{url}", web())
    } else {
        url.to_string()
    }
}

// ---------------------------------------------------------------------------
// Words.

pub fn name() -> &'static str {
    current().name()
}

/// "Open on GitHub", "Open on GitLab".
pub fn open_on() -> String {
    format!("Open on {}", name())
}

pub fn pr() -> &'static str {
    if is_gitlab() {
        "merge request"
    } else {
        "pull request"
    }
}

pub fn prs() -> &'static str {
    if is_gitlab() {
        "merge requests"
    } else {
        "pull requests"
    }
}

pub fn pr_title() -> &'static str {
    if is_gitlab() {
        "Merge request"
    } else {
        "Pull request"
    }
}

pub fn prs_title() -> &'static str {
    if is_gitlab() {
        "Merge requests"
    } else {
        "Pull requests"
    }
}

/// `#12` on GitHub; `!12` for a merge request on GitLab.
pub fn pr_ref(number: impl std::fmt::Display) -> String {
    if is_gitlab() {
        format!("!{number}")
    } else {
        format!("#{number}")
    }
}

pub fn repo_word() -> &'static str {
    if is_gitlab() {
        "project"
    } else {
        "repository"
    }
}

pub fn repos_title() -> &'static str {
    if is_gitlab() {
        "Projects"
    } else {
        "Repositories"
    }
}

pub fn orgs_title() -> &'static str {
    match current() {
        Forge::GitHub => "Organizations",
        Forge::GitLab => "Groups",
        Forge::Bitbucket => "Workspaces",
    }
}

pub fn gists_title() -> &'static str {
    if is_github() {
        "Gists"
    } else {
        "Snippets"
    }
}

/// The command-line tool: `gh` or `glab`. Bitbucket has none of its own.
pub fn cli() -> Option<&'static str> {
    match current() {
        Forge::GitHub => Some("gh"),
        Forge::GitLab => Some("glab"),
        Forge::Bitbucket => None,
    }
}

// ---------------------------------------------------------------------------
// Repository paths.

/// What can follow a repository in a GitHub-style API path. On GitLab a
/// repository's path has as many parts as its groups nest, so the first
/// of these after the second part is where it ends.
const AFTER_REPO: &[&str] = &[
    "actions",
    "activity",
    "assignees",
    "autolinks",
    "automated-security-fixes",
    "branches",
    "check-runs",
    "check-suites",
    "code-scanning",
    "codespaces",
    "collaborators",
    "comments",
    "commits",
    "community",
    "compare",
    "contents",
    "contributors",
    "dependabot",
    "dependency-graph",
    "deployments",
    "discussions",
    "dispatches",
    "environments",
    "events",
    "forks",
    "generate",
    "git",
    "hooks",
    "invitations",
    "issues",
    "keys",
    "labels",
    "languages",
    "license",
    "merge_requests",
    "merges",
    "milestones",
    "notifications",
    "pages",
    "private-vulnerability-reporting",
    "projects",
    "pulls",
    "readme",
    "releases",
    "rulesets",
    "secret-scanning",
    "security-advisories",
    "stargazers",
    "stats",
    "statuses",
    "subscribers",
    "subscription",
    "tags",
    "teams",
    "topics",
    "traffic",
    "transfer",
    "vulnerability-alerts",
    "tarball",
    "zipball",
    "variables",
    "pipelines",
    "jobs",
    "packages",
    "members",
    "-",
];

/// How many of `parts` (a path after `/repos/`) name the repository.
pub fn repo_len(parts: &[&str]) -> usize {
    if parts.len() <= 2 {
        return parts.len();
    }
    (2..parts.len())
        .find(|&i| AFTER_REPO.contains(&parts[i]))
        .unwrap_or(parts.len())
}

/// Where an issue's (or a pull request's) comments, timeline, labels and
/// reactions are. GitHub reaches a pull request's through `/issues/{n}`,
/// as both share one numbering; GitLab numbers merge requests apart.
pub fn issue_api(repo: &str, number: impl std::fmt::Display, is_pr: bool) -> String {
    if is_pr && is_gitlab() {
        format!("/repos/{repo}/merge_requests/{number}")
    } else {
        format!("/repos/{repo}/issues/{number}")
    }
}

/// A repository as its owner and name: `group/sub` and `project`.
pub fn split_repo(repo: &str) -> (&str, &str) {
    repo.rsplit_once('/').unwrap_or((repo, ""))
}

/// The repository a URL or API path is about: `https://github.com/o/r/…`,
/// `https://api.github.com/repos/o/r/…`, `/repos/a/b/c/issues/1`, or a
/// GitLab page (`…/a/b/c/-/issues/1`).
pub fn repo_of(url: &str) -> String {
    let web = web();
    let rest = url
        .strip_prefix("https://api.github.com/repos/")
        .or_else(|| url.strip_prefix("/repos/"))
        .or_else(|| url.strip_prefix(&format!("{web}/")))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .unwrap_or(url);
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    if let Some((repo, _)) = rest.split_once("/-/") {
        return repo.to_string();
    }
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return String::new();
    }
    let n = if is_gitlab() { repo_len(&parts) } else { 2 };
    parts[..n].join("/")
}

// ---------------------------------------------------------------------------
// Pages on GitLab's site.

/// The page for `route` on a GitLab instance at `w`.
pub fn gitlab_url(route: &Route, w: &str) -> String {
    match route {
        Route::Home => w.to_string(),
        Route::Notifications => format!("{w}/dashboard/todos"),
        Route::Repos => format!("{w}/dashboard/projects"),
        Route::Pulls => format!("{w}/dashboard/merge_requests"),
        Route::Issues => format!("{w}/dashboard/issues"),
        Route::Repo { repo, tab } => {
            let suffix = match tab {
                RepoTab::Code => "",
                RepoTab::Issues => "/-/issues",
                RepoTab::Pulls => "/-/merge_requests",
                RepoTab::Actions => "/-/pipelines",
                RepoTab::Releases => "/-/releases",
                RepoTab::Branches => "/-/branches",
                RepoTab::Tags => "/-/tags",
                RepoTab::Commits => "/-/commits",
                RepoTab::Insights => "/-/graphs",
                RepoTab::Settings => "/edit",
                RepoTab::Packages => "/-/packages",
                RepoTab::Discussions | RepoTab::Projects | RepoTab::Security => "",
            };
            format!("{w}/{repo}{suffix}")
        }
        Route::Tree {
            repo,
            git_ref,
            path,
            file,
        } => format!(
            "{w}/{repo}/-/{}/{git_ref}/{path}",
            if *file { "blob" } else { "tree" }
        ),
        Route::Issue { repo, number } => format!("{w}/{repo}/-/issues/{number}"),
        Route::Pull { repo, number, tab } => {
            let suffix = match tab {
                PullTab::Conversation => "",
                PullTab::Commits => "/commits",
                PullTab::Files => "/diffs",
                PullTab::Checks => "/pipelines",
            };
            format!("{w}/{repo}/-/merge_requests/{number}{suffix}")
        }
        Route::Conflicts { repo, number } => {
            format!("{w}/{repo}/-/merge_requests/{number}/conflicts")
        }
        Route::Commit { repo, sha } => format!("{w}/{repo}/-/commit/{sha}"),
        Route::Compare { repo, base, head } => format!("{w}/{repo}/-/compare/{base}...{head}"),
        Route::Run { repo, id } => format!("{w}/{repo}/-/pipelines/{id}"),
        Route::Job { repo, id } => format!("{w}/{repo}/-/jobs/{id}"),
        Route::Release { repo, .. } => format!("{w}/{repo}/-/releases"),
        Route::Discussion { repo, .. } => format!("{w}/{repo}"),
        Route::User { login } | Route::Org { login } => format!("{w}/{login}"),
        Route::Team { org, slug } => format!("{w}/{org}/{slug}"),
        Route::Gists | Route::NewGist => format!("{w}/dashboard/snippets"),
        Route::Gist { id } => format!("{w}/-/snippets/{id}"),
        Route::NewRepo { .. } => format!("{w}/projects/new"),
        Route::Search => format!("{w}/search"),
        Route::Projects | Route::Project { .. } | Route::Codespaces => w.to_string(),
        Route::Packages => w.to_string(),
        Route::Settings => format!("{w}/-/user_settings/profile"),
    }
}

/// The page a link on the GitLab instance at `w` points at, when the app
/// has one.
pub fn gitlab_route(url: &str, w: &str) -> Option<Route> {
    let rest = url.strip_prefix(w)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let num = |s: &str| s.parse::<u64>().ok();
    if rest.is_empty() {
        return Some(Route::Home);
    }
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["dashboard", "merge_requests", ..] => return Some(Route::Pulls),
        ["dashboard", "issues", ..] => return Some(Route::Issues),
        ["dashboard", "todos", ..] => return Some(Route::Notifications),
        ["dashboard", "snippets", ..] => return Some(Route::Gists),
        ["dashboard", ..] => return Some(Route::Repos),
        ["projects", "new", ..] => return Some(Route::NewRepo { owner: None }),
        ["search", ..] => return Some(Route::Search),
        ["-", "snippets", id, ..] => return Some(Route::Gist { id: id.to_string() }),
        ["-", "user_settings", ..] | ["-", "profile", ..] | ["profile", ..] => {
            return Some(Route::Settings)
        }
        ["users", login, ..] => {
            return Some(Route::User {
                login: login.to_string(),
            })
        }
        ["groups", ..]
        | ["explore", ..]
        | ["admin", ..]
        | ["help", ..]
        | ["-", ..]
        | ["api", ..]
        | ["oauth", ..] => return None,
        _ => {}
    }
    let Some((repo, page)) = rest.split_once("/-/") else {
        // A user, a group or a project: one part is a user (whose page
        // knows a group when it sees one), more is a project (whose page
        // does the same for subgroups).
        return Some(match parts.as_slice() {
            [login] => Route::User {
                login: login.to_string(),
            },
            _ => Route::Repo {
                repo: rest.to_string(),
                tab: RepoTab::Code,
            },
        });
    };
    let repo = repo.to_string();
    let page: Vec<&str> = page.split('/').filter(|p| !p.is_empty()).collect();
    let tab = |tab: RepoTab| Route::Repo {
        repo: repo.clone(),
        tab,
    };
    Some(match page.as_slice() {
        ["issues", n, ..] | ["work_items", n, ..] if num(n).is_some() => Route::Issue {
            repo,
            number: num(n)?,
        },
        ["issues", ..]
        | ["work_items", ..]
        | ["boards", ..]
        | ["labels", ..]
        | ["milestones", ..] => tab(RepoTab::Issues),
        ["merge_requests", n, "conflicts"] if num(n).is_some() => Route::Conflicts {
            repo,
            number: num(n)?,
        },
        ["merge_requests", n, rest @ ..] if num(n).is_some() => Route::Pull {
            repo,
            number: num(n)?,
            tab: match rest.first().copied() {
                Some("diffs") => PullTab::Files,
                Some("commits") => PullTab::Commits,
                Some("pipelines") => PullTab::Checks,
                _ => PullTab::Conversation,
            },
        },
        ["merge_requests", ..] => tab(RepoTab::Pulls),
        ["commit", sha, ..] => Route::Commit {
            repo,
            sha: sha.to_string(),
        },
        ["commits", ..] => tab(RepoTab::Commits),
        ["pipelines", id, ..] if num(id).is_some() => Route::Run { repo, id: num(id)? },
        ["jobs", id, ..] if num(id).is_some() => Route::Job { repo, id: num(id)? },
        ["pipelines", ..] | ["jobs", ..] | ["pipeline_schedules", ..] | ["environments", ..] => {
            tab(RepoTab::Actions)
        }
        ["releases", tag, ..] => Route::Release {
            repo,
            id: release_id(tag),
        },
        ["releases", ..] => tab(RepoTab::Releases),
        ["tags", ..] => tab(RepoTab::Tags),
        ["branches", ..] => tab(RepoTab::Branches),
        ["compare", range, ..] if range.contains("...") => {
            let (base, head) = range.split_once("...")?;
            Route::Compare {
                repo,
                base: base.to_string(),
                head: head.to_string(),
            }
        }
        ["tree", git_ref, path @ ..] => Route::Tree {
            repo,
            git_ref: git_ref.to_string(),
            path: path.join("/"),
            file: false,
        },
        ["blob", git_ref, path @ ..] => Route::Tree {
            repo,
            git_ref: git_ref.to_string(),
            path: path.join("/"),
            file: true,
        },
        ["settings", ..] | ["project_members", ..] => tab(RepoTab::Settings),
        ["graphs", ..] | ["network", ..] | ["forks", ..] | ["starrers", ..] => {
            tab(RepoTab::Insights)
        }
        ["packages", ..] => tab(RepoTab::Packages),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Pages on Bitbucket's site.

/// A pipeline step as the app's job number: Bitbucket names steps by
/// UUID, so a job is its pipeline's build number and its place in the
/// pipeline, which the step page turns back into the step by looking.
pub fn step_job_id(build: u64, index: usize) -> u64 {
    build * 1000 + index as u64
}

/// [`step_job_id`] taken apart: the build number and the step's place.
pub fn job_step(id: u64) -> (u64, usize) {
    (id / 1000, (id % 1000) as usize)
}

/// The page for `route` on Bitbucket at `w`. Bitbucket's issue tracker
/// is gone, so an issue's number is a pull request's.
pub fn bitbucket_url(route: &Route, w: &str) -> String {
    match route {
        Route::Home | Route::Notifications | Route::Issues => w.to_string(),
        Route::Repos => format!("{w}/dashboard/repositories"),
        Route::Pulls => format!("{w}/dashboard/pullrequests"),
        Route::Repo { repo, tab } => {
            let suffix = match tab {
                RepoTab::Pulls | RepoTab::Issues => "/pull-requests/",
                RepoTab::Actions => "/pipelines",
                RepoTab::Branches => "/branches/",
                RepoTab::Tags => "/downloads/?tab=tags",
                RepoTab::Releases | RepoTab::Packages => "/downloads/",
                RepoTab::Commits => "/commits/",
                RepoTab::Settings => "/admin",
                RepoTab::Code
                | RepoTab::Insights
                | RepoTab::Discussions
                | RepoTab::Projects
                | RepoTab::Security => "",
            };
            format!("{w}/{repo}{suffix}")
        }
        Route::Tree {
            repo,
            git_ref,
            path,
            file,
        } => format!(
            "{w}/{repo}/src/{git_ref}/{path}{}",
            if *file || path.is_empty() { "" } else { "/" }
        ),
        Route::Issue { repo, number } | Route::Conflicts { repo, number } => {
            format!("{w}/{repo}/pull-requests/{number}")
        }
        Route::Pull { repo, number, tab } => {
            let suffix = match tab {
                PullTab::Files => "/diff",
                PullTab::Commits => "/commits",
                PullTab::Conversation | PullTab::Checks => "",
            };
            format!("{w}/{repo}/pull-requests/{number}{suffix}")
        }
        Route::Commit { repo, sha } => format!("{w}/{repo}/commits/{sha}"),
        Route::Compare { repo, base, head } => {
            format!("{w}/{repo}/branches/compare/{head}%0D{base}")
        }
        Route::Run { repo, id } => format!("{w}/{repo}/pipelines/results/{id}"),
        Route::Job { repo, id } => format!("{w}/{repo}/pipelines/results/{}", job_step(*id).0),
        Route::Release { repo, .. } => format!("{w}/{repo}/downloads/"),
        Route::Discussion { repo, .. } => format!("{w}/{repo}"),
        Route::Org { login } => format!("{w}/{login}/"),
        Route::Team { org, .. } => format!("{w}/{org}/"),
        Route::User { .. } => w.to_string(),
        Route::Gists | Route::NewGist => format!("{w}/dashboard/snippets"),
        Route::Gist { id } => format!("{w}/snippets/{id}"),
        Route::NewRepo { .. } => format!("{w}/repo/create"),
        Route::Search => format!("{w}/search"),
        Route::Settings => format!("{w}/account/settings/"),
        Route::Projects | Route::Project { .. } | Route::Codespaces | Route::Packages => {
            w.to_string()
        }
    }
}

/// The page a link on Bitbucket at `w` points at, when the app has one.
pub fn bitbucket_route(url: &str, w: &str) -> Option<Route> {
    let rest = url.strip_prefix(w)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let path = rest.split(['?', '#']).next().unwrap_or("");
    // Bitbucket ends a directory's address with a slash, a file's without.
    let dir = path.ends_with('/');
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Some(Route::Home);
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let num = |s: &str| s.parse::<u64>().ok();
    match parts.as_slice() {
        ["dashboard", "pullrequests", ..] => return Some(Route::Pulls),
        ["dashboard", "repositories", ..] => return Some(Route::Repos),
        ["dashboard", "snippets", ..] => return Some(Route::Gists),
        ["dashboard", ..] => return Some(Route::Home),
        ["account", ..] => return Some(Route::Settings),
        ["repo", "create", ..] => return Some(Route::NewRepo { owner: None }),
        ["search", ..] => return Some(Route::Search),
        ["snippets", ws, id, ..] => {
            return Some(Route::Gist {
                id: format!("{ws}/{id}"),
            })
        }
        ["site", ..] | ["product", ..] | ["support", ..] | ["blog", ..] | ["snippets", ..] => {
            return None
        }
        [ws] | [ws, "workspace", ..] => {
            return Some(Route::Org {
                login: ws.to_string(),
            })
        }
        _ => {}
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    let tab = |tab: RepoTab| Route::Repo {
        repo: repo.clone(),
        tab,
    };
    Some(match &parts[2..] {
        [] => tab(RepoTab::Code),
        ["pull-requests", n, rest @ ..] if num(n).is_some() => Route::Pull {
            repo,
            number: num(n)?,
            tab: match rest.first().copied() {
                Some("diff") => PullTab::Files,
                Some("commits") => PullTab::Commits,
                _ => PullTab::Conversation,
            },
        },
        ["pull-requests", ..] => tab(RepoTab::Pulls),
        ["src", git_ref, path @ ..] => Route::Tree {
            repo,
            git_ref: git_ref.to_string(),
            path: path.join("/"),
            file: !path.is_empty() && !dir,
        },
        ["src"] => tab(RepoTab::Code),
        ["commits", "branch", ..] | ["commits"] => tab(RepoTab::Commits),
        ["commits", sha, ..] => Route::Commit {
            repo,
            sha: sha.to_string(),
        },
        ["branches", "compare", spec, ..] => {
            let spec = crate::gitlab::decode(spec);
            let (head, base) = spec.split_once(['\r', '\n'])?;
            Route::Compare {
                repo,
                base: base.to_string(),
                head: head.to_string(),
            }
        }
        ["branches", ..] => tab(RepoTab::Branches),
        ["pipelines", "results", n, ..] if num(n).is_some() => Route::Run { repo, id: num(n)? },
        ["pipelines", ..] | ["addon", "pipelines", ..] => tab(RepoTab::Actions),
        ["downloads", ..] => tab(RepoTab::Releases),
        ["admin", ..] => tab(RepoTab::Settings),
        _ => return None,
    })
}

/// GitLab names a release by its tag; the app's routes number them. The
/// number is the tag's hash, kept within the integers JSON carries
/// exactly, which the client turns back into the tag by looking.
pub fn release_id(tag: &str) -> u64 {
    // FNV-1a: the same tag gives the same number on every run.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in tag.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h & ((1 << 52) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_paths_nest() {
        assert_eq!(repo_len(&["a", "b", "issues", "1"]), 2);
        assert_eq!(repo_len(&["a", "b", "c", "issues", "1"]), 3);
        assert_eq!(repo_len(&["a", "b", "c"]), 3);
        assert_eq!(split_repo("a/b/c"), ("a/b", "c"));
        assert_eq!(
            normalize_host("https://gitlab.example.com/"),
            "gitlab.example.com"
        );
        assert_eq!(
            Account::new(Forge::GitLab, "http://10.0.0.5:8080", "t").api(),
            "http://10.0.0.5:8080/api/v4"
        );
    }

    #[test]
    fn gitlab_links_become_pages() {
        let w = "https://gitlab.com";
        assert_eq!(
            gitlab_route("https://gitlab.com/a/b/c/-/merge_requests/7/diffs", w),
            Some(Route::Pull {
                repo: "a/b/c".into(),
                number: 7,
                tab: PullTab::Files
            })
        );
        assert_eq!(
            gitlab_route("https://gitlab.com/a/b/-/issues/3#note_1", w),
            Some(Route::Issue {
                repo: "a/b".into(),
                number: 3
            })
        );
        assert_eq!(
            gitlab_route("https://gitlab.com/a/b/-/blob/main/src/x.rs", w),
            Some(Route::Tree {
                repo: "a/b".into(),
                git_ref: "main".into(),
                path: "src/x.rs".into(),
                file: true
            })
        );
        assert_eq!(
            gitlab_route("https://gitlab.com/someone", w),
            Some(Route::User {
                login: "someone".into()
            })
        );
        assert_eq!(gitlab_route("https://github.com/a/b", w), None);
        assert_eq!(gitlab_route("https://gitlab.com.evil.example/a/b", w), None);
        let route = Route::Pull {
            repo: "a/b/c".into(),
            number: 7,
            tab: PullTab::Files,
        };
        assert_eq!(gitlab_route(&gitlab_url(&route, w), w), Some(route));
        assert_eq!(release_id("v1.0"), release_id("v1.0"));
        assert!(release_id("v1.0") < (1 << 53));
    }

    #[test]
    fn bitbucket_links_become_pages() {
        let w = "https://bitbucket.org";
        assert_eq!(
            bitbucket_route("https://bitbucket.org/ws/repo/pull-requests/7/diff", w),
            Some(Route::Pull {
                repo: "ws/repo".into(),
                number: 7,
                tab: PullTab::Files
            })
        );
        assert_eq!(
            bitbucket_route("https://bitbucket.org/ws/repo/src/main/src/x.rs", w),
            Some(Route::Tree {
                repo: "ws/repo".into(),
                git_ref: "main".into(),
                path: "src/x.rs".into(),
                file: true
            })
        );
        assert_eq!(
            bitbucket_route("https://bitbucket.org/ws/repo/src/main/src/", w),
            Some(Route::Tree {
                repo: "ws/repo".into(),
                git_ref: "main".into(),
                path: "src".into(),
                file: false
            })
        );
        assert_eq!(
            bitbucket_route("https://bitbucket.org/ws/", w),
            Some(Route::Org { login: "ws".into() })
        );
        assert_eq!(
            bitbucket_route("https://bitbucket.org.evil.example/a/b", w),
            None
        );
        for route in [
            Route::Pull {
                repo: "ws/r".into(),
                number: 3,
                tab: PullTab::Commits,
            },
            Route::Compare {
                repo: "ws/r".into(),
                base: "main".into(),
                head: "feature".into(),
            },
            Route::Run {
                repo: "ws/r".into(),
                id: 42,
            },
            Route::Gist {
                id: "ws/abc12".into(),
            },
        ] {
            assert_eq!(bitbucket_route(&bitbucket_url(&route, w), w), Some(route));
        }
        assert_eq!(job_step(step_job_id(5758, 2)), (5758, 2));
        assert_eq!(
            Account::new(Forge::Bitbucket, "", "t").api(),
            "https://api.bitbucket.org/2.0"
        );
    }
}
