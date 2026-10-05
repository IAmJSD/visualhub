//! The application's one view: where the user is, what has been fetched,
//! what is being typed, and the actions everything clickable performs.
//!
//! Screens are plain `impl Hub` methods spread across `screens/`. They
//! read GitHub through [`Hub::fetch`], which answers from the cache or
//! starts a background request and answers [`Load::Loading`] until it
//! lands -- so a screen is written as if the data were already there, and
//! re-renders when it is. Anything a click does is an [`Act`], performed
//! by [`Hub::perform`]; writes are [`Req`]s that name the cache prefixes
//! they make stale, which is how lists refresh after an edit.

use crate::api::{self, Client};
use crate::forge::{Account, Forge};
use crate::form::{FormSpec, FormValues};
use crate::json::Json as _;
use crate::ui::{LineEdit, LineEditKey, TextPress};
use anyhow::Result;
use gpui::{
    App, ClickEvent, ClipboardItem, Context, FocusHandle, Image, ImageFormat, KeyDownEvent, Pixels,
    Point, WeakEntity, Window,
};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Where the user is.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RepoTab {
    Code,
    Issues,
    Pulls,
    Discussions,
    Actions,
    Projects,
    Releases,
    Branches,
    Tags,
    Commits,
    Security,
    Insights,
    Settings,
    /// A GitLab project's package registry.
    Packages,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PullTab {
    Conversation,
    Commits,
    Files,
    Checks,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Route {
    Home,
    Notifications,
    Repos,
    Pulls,
    Issues,
    Repo {
        repo: String,
        tab: RepoTab,
    },
    /// A directory listing or a file, at a branch, tag or commit.
    Tree {
        repo: String,
        git_ref: String,
        path: String,
        file: bool,
    },
    Issue {
        repo: String,
        number: u64,
    },
    Pull {
        repo: String,
        number: u64,
        tab: PullTab,
    },
    /// Resolving a pull request's merge conflicts.
    Conflicts {
        repo: String,
        number: u64,
    },
    Commit {
        repo: String,
        sha: String,
    },
    Compare {
        repo: String,
        base: String,
        head: String,
    },
    Run {
        repo: String,
        id: u64,
    },
    /// A GitLab CI job and its log.
    Job {
        repo: String,
        id: u64,
    },
    Release {
        repo: String,
        id: u64,
    },
    Discussion {
        repo: String,
        number: u64,
    },
    User {
        login: String,
    },
    Org {
        login: String,
    },
    Team {
        org: String,
        slug: String,
    },
    Gists,
    Gist {
        id: String,
    },
    /// The New repository page, owned by you or `owner`.
    NewRepo {
        owner: Option<String>,
    },
    NewGist,
    Search,
    Projects,
    Project {
        id: String,
    },
    Codespaces,
    Packages,
    Settings,
}

impl Route {
    /// The repository this page belongs to, if any.
    pub fn repo(&self) -> Option<&str> {
        match self {
            Route::Repo { repo, .. }
            | Route::Tree { repo, .. }
            | Route::Issue { repo, .. }
            | Route::Pull { repo, .. }
            | Route::Conflicts { repo, .. }
            | Route::Commit { repo, .. }
            | Route::Compare { repo, .. }
            | Route::Run { repo, .. }
            | Route::Job { repo, .. }
            | Route::Release { repo, .. }
            | Route::Discussion { repo, .. } => Some(repo),
            _ => None,
        }
    }

    /// A short title for the top bar and the window.
    pub fn title(&self) -> String {
        match self {
            Route::Home => "Home".into(),
            Route::Notifications if crate::forge::is_gitlab() => "To-Do List".into(),
            Route::Notifications => "Notifications".into(),
            Route::Repos => crate::forge::repos_title().into(),
            Route::Pulls => crate::forge::prs_title().into(),
            Route::Issues => "Issues".into(),
            Route::Repo { repo, .. } => repo.clone(),
            Route::Tree { repo, path, .. } if path.is_empty() => repo.clone(),
            Route::Tree { repo, path, .. } => format!("{repo}/{path}"),
            Route::Issue { repo, number } => format!("{repo}#{number}"),
            Route::Pull { repo, number, .. } => format!("{repo}{}", crate::forge::pr_ref(number)),
            Route::Conflicts { repo, number } => {
                format!("Conflicts in {repo}{}", crate::forge::pr_ref(number))
            }
            Route::Commit { repo, sha } => format!("{repo}@{}", &sha[..sha.len().min(7)]),
            Route::Compare { repo, base, head } => format!("{repo} {base}...{head}"),
            Route::Run { repo, id } if crate::forge::is_gitlab() => format!("{repo} pipeline {id}"),
            Route::Run { repo, id } => format!("{repo} run {id}"),
            Route::Job { repo, id } => format!("{repo} job {id}"),
            Route::Release { repo, .. } => format!("{repo} release"),
            Route::Discussion { repo, number } => format!("{repo} discussion #{number}"),
            Route::User { login } | Route::Org { login } => login.clone(),
            Route::Team { org, slug } => format!("{org}/{slug}"),
            Route::Gists => crate::forge::gists_title().into(),
            Route::Gist { id } => format!("Gist {}", &id[..id.len().min(8)]),
            Route::NewRepo { .. } => "New repository".into(),
            Route::NewGist => "New gist".into(),
            Route::Search => "Search".into(),
            Route::Projects => "Projects".into(),
            Route::Project { .. } => "Project".into(),
            Route::Codespaces => "Codespaces".into(),
            Route::Packages => "Packages".into(),
            Route::Settings => "Settings".into(),
        }
    }

    /// The page manages its own scrolling (long logs and files are
    /// virtualised lists, which need a fixed-height parent).
    pub fn owns_scroll(&self) -> bool {
        matches!(
            self,
            Route::Tree { file: true, .. }
                | Route::Pull {
                    tab: PullTab::Files,
                    ..
                }
                | Route::Job { .. }
        )
    }

    /// The same page on the account's site.
    pub fn web_url(&self) -> String {
        let w = crate::forge::web();
        if crate::forge::is_gitlab() {
            return crate::forge::gitlab_url(self, &w);
        }
        let w = w.as_str();
        match self {
            Route::Home => w.to_string(),
            Route::Notifications => format!("{w}/notifications"),
            Route::Repos => format!("{w}/repositories"),
            Route::Pulls => format!("{w}/pulls"),
            Route::Issues => format!("{w}/issues"),
            Route::Repo { repo, tab } => {
                let suffix = match tab {
                    RepoTab::Code => "",
                    RepoTab::Issues => "/issues",
                    RepoTab::Pulls => "/pulls",
                    RepoTab::Discussions => "/discussions",
                    RepoTab::Actions => "/actions",
                    RepoTab::Projects => "/projects",
                    RepoTab::Releases => "/releases",
                    RepoTab::Branches => "/branches",
                    RepoTab::Tags => "/tags",
                    RepoTab::Commits => "/commits",
                    RepoTab::Security => "/security",
                    RepoTab::Insights => "/pulse",
                    RepoTab::Settings => "/settings",
                    RepoTab::Packages => "/packages",
                };
                format!("{w}/{repo}{suffix}")
            }
            Route::Tree {
                repo,
                git_ref,
                path,
                file,
            } => format!(
                "{w}/{repo}/{}/{git_ref}/{path}",
                if *file { "blob" } else { "tree" }
            ),
            Route::Issue { repo, number } => format!("{w}/{repo}/issues/{number}"),
            Route::Pull { repo, number, .. } => format!("{w}/{repo}/pull/{number}"),
            Route::Conflicts { repo, number } => format!("{w}/{repo}/pull/{number}/conflicts"),
            Route::Commit { repo, sha } => format!("{w}/{repo}/commit/{sha}"),
            Route::Compare { repo, base, head } => format!("{w}/{repo}/compare/{base}...{head}"),
            Route::Run { repo, id } => format!("{w}/{repo}/actions/runs/{id}"),
            Route::Job { repo, id } => format!("{w}/{repo}/actions/runs/{id}"),
            Route::Release { repo, .. } => format!("{w}/{repo}/releases"),
            Route::Discussion { repo, number } => format!("{w}/{repo}/discussions/{number}"),
            Route::User { login } | Route::Org { login } => format!("{w}/{login}"),
            Route::Team { org, slug } => format!("{w}/orgs/{org}/teams/{slug}"),
            Route::Gists | Route::Gist { .. } | Route::NewGist => "https://gist.github.com".into(),
            Route::NewRepo { .. } => format!("{w}/new"),
            Route::Search => format!("{w}/search"),
            Route::Projects | Route::Project { .. } => format!("{w}/projects"),
            Route::Codespaces => format!("{w}/codespaces"),
            Route::Packages => format!("{w}/packages"),
            Route::Settings => format!("{w}/settings/profile"),
        }
    }
}

/// The page a link points at, when the app has one: a github.com link on
/// a GitHub account, a link into the instance on a GitLab one.
pub fn route_for_url(url: &str) -> Option<Route> {
    if crate::forge::is_gitlab() {
        return crate::forge::gitlab_route(url, &crate::forge::web());
    }
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let reserved = [
        "settings",
        "notifications",
        "pulls",
        "issues",
        "marketplace",
        "explore",
        "topics",
        "sponsors",
        "features",
        "login",
        "orgs",
        "apps",
        "search",
        "codespaces",
        "new",
    ];
    match parts.as_slice() {
        [] => Some(Route::Home),
        ["new"] => Some(Route::NewRepo { owner: None }),
        [login] if !reserved.contains(login) => Some(Route::User {
            login: login.to_string(),
        }),
        ["orgs", org, "teams", slug, ..] => Some(Route::Team {
            org: org.to_string(),
            slug: slug.to_string(),
        }),
        [owner, name, rest @ ..] if !reserved.contains(owner) => {
            let repo = format!("{owner}/{name}");
            let num = |s: &str| s.parse::<u64>().ok();
            Some(match rest {
                [] => Route::Repo {
                    repo,
                    tab: RepoTab::Code,
                },
                ["issues", n, ..] if num(n).is_some() => Route::Issue {
                    repo,
                    number: num(n)?,
                },
                ["pull", n, "conflicts"] => Route::Conflicts {
                    repo,
                    number: num(n)?,
                },
                ["pull", n, rest @ ..] => Route::Pull {
                    repo,
                    number: num(n)?,
                    tab: match rest.first().copied() {
                        Some("files") => PullTab::Files,
                        Some("commits") => PullTab::Commits,
                        Some("checks") => PullTab::Checks,
                        _ => PullTab::Conversation,
                    },
                },
                ["discussions", n, ..] if num(n).is_some() => Route::Discussion {
                    repo,
                    number: num(n)?,
                },
                ["commit", sha, ..] => Route::Commit {
                    repo,
                    sha: sha.to_string(),
                },
                // A job opens on github.com, which can stream its log.
                ["actions", "runs", _, "job", ..] => return None,
                ["actions", "runs", id, ..] => Route::Run { repo, id: num(id)? },
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
                [tab, ..] => Route::Repo {
                    repo,
                    tab: match *tab {
                        "issues" => RepoTab::Issues,
                        "pulls" => RepoTab::Pulls,
                        "discussions" => RepoTab::Discussions,
                        "actions" => RepoTab::Actions,
                        "releases" => RepoTab::Releases,
                        "branches" => RepoTab::Branches,
                        "tags" => RepoTab::Tags,
                        "commits" => RepoTab::Commits,
                        "security" => RepoTab::Security,
                        "pulse" | "graphs" => RepoTab::Insights,
                        "settings" => RepoTab::Settings,
                        "projects" => RepoTab::Projects,
                        _ => return None,
                    },
                },
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// What has been fetched.

#[derive(Clone)]
pub enum Load {
    Loading,
    Ready(Rc<Value>),
    Failed(Rc<str>),
}

impl Load {
    pub fn ready(&self) -> Option<&Rc<Value>> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}

/// The value of a [`Load`], or return its placeholder from the render.
#[macro_export]
macro_rules! ready {
    ($load:expr) => {
        match $load {
            $crate::hub::Load::Ready(value) => value,
            other => return $crate::widgets::placeholder(&other),
        }
    };
}

/// Background work: something done with the client off the UI thread.
#[derive(Clone)]
pub enum Work {
    Rest {
        method: &'static str,
        path: String,
        body: Option<Value>,
    },
    Gql {
        query: String,
        vars: Value,
    },
    Text {
        path: String,
        accept: String,
    },
    Check {
        path: String,
    },
    Custom(Arc<dyn Fn(&Client) -> Result<Value> + Send + Sync>),
}

impl Work {
    pub fn run(&self, client: &Client) -> Result<Value> {
        match self {
            Work::Rest { method, path, body } => client.json(method, path, body.as_ref()),
            Work::Gql { query, vars } => client.graphql(query, vars.clone()),
            Work::Text { path, accept } => client.text(path, accept).map(Value::String),
            Work::Check { path } => client.check(path).map(Value::Bool),
            Work::Custom(f) => f(client),
        }
    }
}

// ---------------------------------------------------------------------------
// What a click does.

pub type Then = Rc<dyn Fn(&mut Hub, &Value, &mut Context<Hub>)>;
pub type RunFn = Rc<dyn Fn(&mut Hub, &mut Window, &mut Context<Hub>)>;

/// A write to GitHub, what to say when it lands, and what it makes stale.
#[derive(Clone)]
pub struct Req {
    pub work: Work,
    pub ok: String,
    pub invalidate: Vec<String>,
    pub then: Option<Then>,
}

impl Req {
    pub fn rest(method: &'static str, path: impl Into<String>) -> Self {
        Req {
            work: Work::Rest {
                method,
                path: path.into(),
                body: None,
            },
            ok: String::new(),
            invalidate: Vec::new(),
            then: None,
        }
    }

    pub fn gql(query: &str, vars: Value) -> Self {
        Req {
            work: Work::Gql {
                query: query.to_string(),
                vars,
            },
            ok: String::new(),
            invalidate: Vec::new(),
            then: None,
        }
    }

    pub fn custom(f: impl Fn(&Client) -> Result<Value> + Send + Sync + 'static) -> Self {
        Req {
            work: Work::Custom(Arc::new(f)),
            ok: String::new(),
            invalidate: Vec::new(),
            then: None,
        }
    }

    pub fn body(mut self, value: Value) -> Self {
        if let Work::Rest { body, .. } = &mut self.work {
            *body = Some(value);
        }
        self
    }

    /// The toast shown when it succeeds.
    pub fn ok(mut self, message: impl Into<String>) -> Self {
        self.ok = message.into();
        self
    }

    /// A cache prefix this write makes stale.
    pub fn inval(mut self, prefix: impl Into<String>) -> Self {
        self.invalidate.push(prefix.into());
        self
    }

    pub fn then(mut self, f: impl Fn(&mut Hub, &Value, &mut Context<Hub>) + 'static) -> Self {
        self.then = Some(Rc::new(f));
        self
    }

    pub fn act(self) -> Act {
        Act::Req(Rc::new(self))
    }
}

#[derive(Clone)]
pub enum MenuEntry {
    Item {
        label: String,
        checked: Option<bool>,
        act: Act,
    },
    Header(String),
    Sep,
}

impl MenuEntry {
    pub fn item(label: impl Into<String>, act: Act) -> Self {
        MenuEntry::Item {
            label: label.into(),
            checked: None,
            act,
        }
    }

    pub fn check(label: impl Into<String>, checked: bool, act: Act) -> Self {
        MenuEntry::Item {
            label: label.into(),
            checked: Some(checked),
            act,
        }
    }
}

#[derive(Clone, Default)]
pub enum Act {
    #[default]
    None,
    Go(Route),
    Url(String),
    Copy(String),
    Req(Rc<Req>),
    Confirm {
        title: String,
        message: String,
        label: String,
        then: Rc<Act>,
    },
    Form(Rc<FormSpec>),
    Menu(Rc<Vec<MenuEntry>>),
    Picker(Rc<crate::picker::Picker>),
    Run(RunFn),
}

impl Act {
    /// Ask first: a dialog with `label` on its (red) confirm button.
    pub fn confirm(
        self,
        title: impl Into<String>,
        message: impl Into<String>,
        label: impl Into<String>,
    ) -> Act {
        Act::Confirm {
            title: title.into(),
            message: message.into(),
            label: label.into(),
            then: Rc::new(self),
        }
    }

    pub fn run(f: impl Fn(&mut Hub, &mut Window, &mut Context<Hub>) + 'static) -> Act {
        Act::Run(Rc::new(f))
    }

    pub fn menu(entries: Vec<MenuEntry>) -> Act {
        Act::Menu(Rc::new(entries))
    }

    /// Set a remembered choice (a filter, a sub-tab) and re-render.
    pub fn choose(key: impl Into<String>, value: impl Into<String>) -> Act {
        let (key, value) = (key.into(), value.into());
        Act::run(move |hub, _, cx| {
            hub.choices.insert(key.clone(), value.clone());
            cx.notify();
        })
    }
}

thread_local! {
    static HUB: RefCell<Option<WeakEntity<Hub>>> = const { RefCell::new(None) };
}

/// A click handler performing `act`. Any widget can take one without
/// threading the view's context through it.
pub fn on(act: Act) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_event, window, cx| perform(act.clone(), window, cx)
}

/// Perform `act` on the hub from anywhere that has the app.
pub fn perform(act: Act, window: &mut Window, cx: &mut App) {
    if let Some(hub) = HUB.with(|h| h.borrow().clone()) {
        hub.update(cx, |hub, cx| hub.perform(act, window, cx)).ok();
    }
}

// ---------------------------------------------------------------------------
// The view.

pub enum Modal {
    Form {
        spec: Rc<FormSpec>,
        error: Option<String>,
        busy: bool,
    },
    Confirm {
        title: String,
        message: String,
        label: String,
        act: Rc<Act>,
        busy: bool,
        error: Option<String>,
    },
    /// A newer release, offered by the update check.
    Update { update: crate::update::Update },
}

pub struct MenuState {
    pub at: Point<Pixels>,
    pub entries: Rc<Vec<MenuEntry>>,
}

pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

pub enum Auth {
    Checking,
    SignedOut { error: Option<String> },
    SignedIn,
}

/// Signing in to another account while one shows.
#[derive(Default)]
pub struct Adding {
    pub checking: bool,
    pub error: Option<String>,
}

/// An account whose token works.
#[derive(Clone)]
pub struct Known {
    pub account: Account,
    /// Where its token came from, as [`api::discover_accounts`] names it.
    pub source: &'static str,
    /// Its user, as GitHub's `/user` reads.
    pub me: Value,
    /// What its token may do: a classic GitHub token's scopes, or a
    /// GitLab token's.
    pub scopes: String,
}

/// An account's pages, kept while another account shows.
struct Session {
    route: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
    cache: HashMap<String, Load>,
    generation: HashMap<String, u64>,
    stale: HashSet<String>,
    pages: HashMap<String, usize>,
    recent: Vec<String>,
    commit_info: HashMap<String, crate::resource::CommitInfo>,
    conflicts: HashMap<String, crate::screens::conflicts::SessionLoad>,
}

/// Whether `account`'s token works: its user, and what it may do.
fn check_account(account: &Account) -> Result<(Value, String), String> {
    let client = Client::new(account);
    match account.forge {
        Forge::GitHub => match client.raw("GET", "/user", None, None) {
            Ok(reply) if reply.status == 200 => Ok((
                serde_json::from_str(&reply.body).unwrap_or(Value::Null),
                reply.scopes.unwrap_or_default(),
            )),
            Ok(reply) => Err(format!("GitHub refused the token ({}).", reply.status)),
            Err(e) => Err(format!("{e:#}")),
        },
        Forge::GitLab => {
            let me = client
                .json("GET", "/user", None)
                .map_err(|e| format!("{} refused the token: {e:#}", account.host_name()))?;
            // OAuth tokens (the GitLab CLI's web sign-in) can't describe
            // themselves; they get no scopes listed.
            let scopes = client
                .raw_json("GET", "/api/v4/personal_access_tokens/self", None)
                .map(|t| {
                    t.list("scopes")
                        .iter()
                        .filter_map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            Ok((me, scopes))
        }
    }
}

enum Avatar {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub struct Hub {
    pub focus: FocusHandle,
    pub client: Option<Client>,
    pub auth: Auth,
    /// Every account whose token works.
    pub accounts: Vec<Known>,
    /// The one showing.
    current: Option<Account>,
    /// The others' pages, by [`Account::key`].
    stash: HashMap<String, Session>,
    /// Signing in to another account, over the app.
    pub adding: Option<Adding>,
    pub me: Rc<Value>,
    pub scopes: String,
    /// Where the token came from, as [`api::discover_accounts`] names it.
    pub token_source: &'static str,
    /// The walk-through for widening a token that's short of scopes.
    pub scope_fix: Option<crate::scopes::ScopeFix>,
    pub route: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
    cache: HashMap<String, Load>,
    /// Requests made while an older answer for the same key is still on
    /// its way; the newer generation wins.
    generation: HashMap<String, u64>,
    next_generation: u64,
    /// How many pages of each paged list are showing.
    pub pages: HashMap<String, usize>,
    /// Every text box's state, by id.
    pub fields: HashMap<String, LineEdit>,
    /// Checkbox states, by id.
    pub toggles: HashMap<String, bool>,
    /// Remembered choices: filters, sub-tabs, dropdown values.
    pub choices: HashMap<String, String>,
    /// What Enter (or Ctrl+Enter in a paragraph) does in a box, by id;
    /// registered by the render that drew the box.
    pub submits: HashMap<String, Act>,
    pub modal: Option<Modal>,
    /// A self-update downloading or installing, under its dialog.
    pub update_progress: Option<crate::update::Progress>,
    pub menu: Option<MenuState>,
    pub picker: Option<crate::picker::PickerState>,
    pub toasts: Vec<Toast>,
    next_toast: u64,
    pub busy: usize,
    images: HashMap<String, Avatar>,
    pub light: bool,
    pub recent: Vec<String>,
    /// A middle-click scroll in progress.
    pub autoscroll: Option<crate::autoscroll::AutoScroll>,
    /// Every scrolling pane's handle, with the frame it was last drawn in.
    pub scrollers: HashMap<String, (gpui::ScrollHandle, u64)>,
    pub list_scrollers: HashMap<String, gpui::UniformListScrollHandle>,
    /// Counts renders, to tell which panes are on screen.
    pub frame: u64,
    /// Cached answers to fetch again on next use, shown meanwhile.
    pub stale: HashSet<String>,
    /// Polls waiting to fire, by their prefixes.
    pub polls: HashSet<String>,
    /// Merge-conflict editors in progress, by "repo#number".
    pub conflicts: HashMap<String, crate::screens::conflicts::SessionLoad>,
    /// Commits' authors and checks as GraphQL resolved them, by SHA.
    pub commit_info: HashMap<String, crate::resource::CommitInfo>,
    /// Expanded rows and sections, by id.
    pub open: HashSet<String>,
    /// When everything shown was last asked for again, by hand or on
    /// coming back to the window.
    refreshed: std::time::Instant,
}

impl Hub {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        HUB.with(|h| *h.borrow_mut() = Some(cx.entity().downgrade()));
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        cx.observe_window_activation(window, |hub, window, cx| {
            if window.is_window_active() {
                hub.came_back(cx);
            }
        })
        .detach();
        let mut hub = Hub {
            focus,
            client: None,
            auth: Auth::Checking,
            accounts: Vec::new(),
            current: None,
            stash: HashMap::new(),
            adding: None,
            me: Rc::new(Value::Null),
            scopes: String::new(),
            token_source: "",
            scope_fix: None,
            route: Route::Home,
            back: Vec::new(),
            forward: Vec::new(),
            cache: HashMap::new(),
            generation: HashMap::new(),
            next_generation: 0,
            pages: HashMap::new(),
            fields: HashMap::new(),
            toggles: HashMap::new(),
            choices: HashMap::new(),
            submits: HashMap::new(),
            modal: None,
            update_progress: None,
            menu: None,
            picker: None,
            toasts: Vec::new(),
            next_toast: 0,
            busy: 0,
            images: HashMap::new(),
            light: false,
            recent: Vec::new(),
            autoscroll: None,
            scrollers: HashMap::new(),
            list_scrollers: HashMap::new(),
            frame: 0,
            commit_info: HashMap::new(),
            conflicts: HashMap::new(),
            stale: HashSet::new(),
            polls: HashSet::new(),
            open: HashSet::new(),
            refreshed: std::time::Instant::now(),
        };
        hub.discover(cx);
        hub.check_for_update_at_launch(cx);
        hub
    }

    // -- accounts -------------------------------------------------------

    /// Find every account the machine has a token for, check them all,
    /// and show the one used last (or, with `VISUALHUB_OPEN`, the one its
    /// page belongs to).
    fn discover(&mut self, cx: &mut Context<Self>) {
        self.auth = Auth::Checking;
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn(async move {
                    let found = api::discover_accounts();
                    let results =
                        crate::gitlab::parallel(&found, |(account, _)| check_account(account));
                    found.into_iter().zip(results).collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |hub, cx| {
                let mut errors = Vec::new();
                for ((account, source), result) in checked {
                    match result {
                        Ok((me, scopes)) => hub.know(Known {
                            account,
                            source,
                            me,
                            scopes,
                        }),
                        Err(error) => {
                            errors.push(format!("{source} ({}): {error}", account.host_name()))
                        }
                    }
                }
                if hub.accounts.is_empty() {
                    hub.auth = Auth::SignedOut {
                        error: (!errors.is_empty()).then(|| errors.join(" ")),
                    };
                } else {
                    let open = std::env::var("VISUALHUB_OPEN").unwrap_or_default();
                    let last = api::last_account();
                    let index = hub
                        .accounts
                        .iter()
                        .position(|k| !open.is_empty() && open.starts_with(&k.account.web()))
                        .or_else(|| {
                            hub.accounts
                                .iter()
                                .position(|k| Some(k.account.key()) == last)
                        })
                        .unwrap_or(0);
                    hub.switch_to(index, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Add `known`, or update the account it already is.
    fn know(&mut self, known: Known) {
        match self
            .accounts
            .iter_mut()
            .find(|k| k.account == known.account)
        {
            Some(existing) => *existing = known,
            None => self.accounts.push(known),
        }
    }

    /// Sign in with a pasted token, keeping it for next time.
    pub fn sign_in_with(
        &mut self,
        forge: Forge,
        host: String,
        token: String,
        cx: &mut Context<Self>,
    ) {
        let account = Account::new(
            forge,
            if forge == Forge::GitHub {
                "github.com"
            } else {
                &host
            },
            &token,
        );
        if account.token.is_empty() {
            self.sign_in_failed("Paste a personal access token first.".into());
            cx.notify();
            return;
        }
        match &mut self.adding {
            Some(adding) => adding.checking = true,
            None => self.auth = Auth::Checking,
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let checked = check_account(&account).map_err(anyhow::Error::msg)?;
                    api::save_account(&account)?;
                    Ok::<_, anyhow::Error>((account, checked))
                })
                .await;
            this.update(cx, |hub, cx| {
                match result {
                    Ok((account, (me, scopes))) => {
                        hub.know(Known {
                            account: account.clone(),
                            source: "saved sign-in",
                            me,
                            scopes,
                        });
                        if let Some(index) = hub.accounts.iter().position(|k| k.account == account)
                        {
                            hub.switch_to(index, cx);
                        }
                    }
                    Err(e) => hub.sign_in_failed(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn sign_in_failed(&mut self, error: String) {
        match &mut self.adding {
            Some(adding) => {
                adding.checking = false;
                adding.error = Some(error);
            }
            None => self.auth = Auth::SignedOut { error: Some(error) },
        }
    }

    pub fn rediscover(&mut self, cx: &mut Context<Self>) {
        self.adding = None;
        self.discover(cx);
        cx.notify();
    }

    /// Show the sign-in page over the app, to add another account.
    pub fn add_account(&mut self, cx: &mut Context<Self>) {
        self.blur_fields();
        self.menu = None;
        self.adding = Some(Adding::default());
        cx.notify();
    }

    pub fn cancel_adding(&mut self, cx: &mut Context<Self>) {
        self.adding = None;
        cx.notify();
    }

    /// Show `self.accounts[index]`: the current account's pages are put
    /// away, and the other's come back as they were left.
    pub fn switch_to(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(known) = self.accounts.get(index).cloned() else {
            return;
        };
        let first = self.current.is_none();
        if let Some(current) = self.current.take() {
            if current == known.account && matches!(self.auth, Auth::SignedIn) {
                self.current = Some(current);
                self.adding = None;
                cx.notify();
                return;
            }
            let session = self.take_session();
            self.stash.insert(current.key(), session);
        }
        crate::forge::set_current(&known.account);
        self.client = Some(Client::new(&known.account));
        self.me = Rc::new(known.me.clone());
        self.scopes = known.scopes.clone();
        self.token_source = known.source;
        // Coming back to an account's pages doesn't ask about its token
        // again; opening them the first time does.
        let fresh = match self.stash.remove(&known.account.key()) {
            Some(session) => {
                self.put_session(session);
                false
            }
            None => {
                self.recent = api::load_recent(&known.account, &known.me.s("login"));
                // VISUALHUB_OPEN=<a page's URL> starts on that page, for
                // screenshots and for working on one screen.
                self.route = first
                    .then(|| std::env::var("VISUALHUB_OPEN").ok())
                    .flatten()
                    .and_then(|url| route_for_url(&url))
                    .unwrap_or(Route::Home);
                true
            }
        };
        self.current = Some(known.account.clone());
        if first {
            self.choices.extend(api::load_merge_methods());
        }
        self.modal = None;
        self.menu = None;
        self.picker = None;
        self.scope_fix = None;
        self.adding = None;
        self.auth = Auth::SignedIn;
        if fresh {
            self.check_scopes();
        }
        api::save_last_account(&known.account);
        cx.notify();
    }

    /// The account's pages, taken out to keep while another shows.
    fn take_session(&mut self) -> Session {
        Session {
            route: std::mem::replace(&mut self.route, Route::Home),
            back: std::mem::take(&mut self.back),
            forward: std::mem::take(&mut self.forward),
            cache: std::mem::take(&mut self.cache),
            generation: std::mem::take(&mut self.generation),
            stale: std::mem::take(&mut self.stale),
            pages: std::mem::take(&mut self.pages),
            recent: std::mem::take(&mut self.recent),
            commit_info: std::mem::take(&mut self.commit_info),
            conflicts: std::mem::take(&mut self.conflicts),
        }
    }

    fn put_session(&mut self, mut session: Session) {
        // Answers still on their way when it was put away never landed.
        session
            .cache
            .retain(|_, load| !matches!(load, Load::Loading));
        self.route = session.route;
        self.back = session.back;
        self.forward = session.forward;
        self.cache = session.cache;
        self.generation = session.generation;
        self.stale = session.stale;
        self.pages = session.pages;
        self.recent = session.recent;
        self.commit_info = session.commit_info;
        self.conflicts = session.conflicts;
        self.scrollers.clear();
        self.list_scrollers.clear();
    }

    /// The account showing.
    pub fn account(&self) -> Option<&Account> {
        self.current.as_ref()
    }

    /// Swap in a token for the same account, dropping what the old one
    /// fetched (some of it was errors the new one can get past).
    pub fn refresh_token(&mut self, client: Client, me: Value, scopes: String) {
        if let Some(current) = &self.current {
            if let Some(known) = self.accounts.iter_mut().find(|k| k.account == *current) {
                known.me = me.clone();
                known.scopes = scopes.clone();
            }
        }
        self.client = Some(client);
        self.me = Rc::new(me);
        self.scopes = scopes;
        self.cache.clear();
    }

    /// Sign out of the account showing; another signed-in account shows
    /// instead, or the sign-in page.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if let Some(account) = self.current.take() {
            api::forget_account(&account);
            self.accounts.retain(|k| k.account != account);
            self.stash.remove(&account.key());
        }
        self.client = None;
        self.me = Rc::new(Value::Null);
        self.cache.clear();
        self.generation.clear();
        self.fields.clear();
        self.modal = None;
        self.menu = None;
        self.back.clear();
        self.forward.clear();
        if self.accounts.is_empty() {
            self.auth = Auth::SignedOut { error: None };
            cx.notify();
        } else {
            self.switch_to(0, cx);
        }
    }

    pub fn login(&self) -> String {
        self.me.s("login")
    }

    // -- navigation -----------------------------------------------------

    pub fn go(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == self.route {
            return;
        }
        let previous = std::mem::replace(&mut self.route, route);
        self.back.push(previous);
        self.forward.clear();
        self.arrived();
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(route) = self.back.pop() {
            let current = std::mem::replace(&mut self.route, route);
            self.forward.push(current);
            self.arrived();
            cx.notify();
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(route) = self.forward.pop() {
            let current = std::mem::replace(&mut self.route, route);
            self.back.push(current);
            self.arrived();
            cx.notify();
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    fn arrived(&mut self) {
        self.menu = None;
        self.picker = None;
        // A new page starts at the top; the sidebar stays where it was.
        self.scrollers.retain(|key, _| key == "sidebar");
        self.list_scrollers.clear();
        self.blur_fields();
        if let Some(repo) = self.route.repo().map(str::to_string) {
            if self.recent.first() != Some(&repo) {
                self.recent.retain(|r| r != &repo);
                self.recent.insert(0, repo);
                self.recent.truncate(8);
                if let Some(account) = &self.current {
                    api::save_recent(account, &self.me.s("login"), &self.recent);
                }
            }
        }
    }

    /// Follow a link: inside the app when it has the page, otherwise in
    /// the browser.
    pub fn open_link(&mut self, url: &str, cx: &mut Context<Self>) {
        match route_for_url(url) {
            Some(route) => self.go(route, cx),
            None => cx.open_url(url),
        }
    }

    // -- fetching -------------------------------------------------------

    /// A REST GET, from the cache or on its way.
    pub fn fetch(&mut self, path: &str, cx: &mut Context<Self>) -> Load {
        self.fetch_with(
            path.to_string(),
            Work::Rest {
                method: "GET",
                path: path.to_string(),
                body: None,
            },
            cx,
        )
    }

    /// Raw text (file contents, logs), cached under `path#text`.
    pub fn fetch_text(&mut self, path: &str, accept: &str, cx: &mut Context<Self>) -> Load {
        self.fetch_with(
            format!("{path}#text"),
            Work::Text {
                path: path.to_string(),
                accept: accept.to_string(),
            },
            cx,
        )
    }

    /// A yes/no "check" endpoint, cached under `path#check`.
    pub fn fetch_check(&mut self, path: &str, cx: &mut Context<Self>) -> Load {
        self.fetch_with(
            format!("{path}#check"),
            Work::Check {
                path: path.to_string(),
            },
            cx,
        )
    }

    /// A GraphQL query, cached under `scope` so writes to that REST path
    /// prefix make it stale too.
    pub fn fetch_gql(
        &mut self,
        scope: &str,
        query: &str,
        vars: Value,
        cx: &mut Context<Self>,
    ) -> Load {
        let key = format!("{scope}#gql#{}#{}", hash(query), vars);
        self.fetch_with(
            key,
            Work::Gql {
                query: query.to_string(),
                vars,
            },
            cx,
        )
    }

    pub fn fetch_with(&mut self, key: String, work: Work, cx: &mut Context<Self>) -> Load {
        if let Some(load) = self.cache.get(&key) {
            let load = load.clone();
            // Stale: keep showing it while a fresh copy comes in.
            if self.stale.remove(&key) {
                self.start_fetch(key, work, cx);
            }
            return load;
        }
        if self.client.is_none() {
            return Load::Failed("Not signed in".into());
        }
        self.cache.insert(key.clone(), Load::Loading);
        self.start_fetch(key, work, cx);
        Load::Loading
    }

    fn start_fetch(&mut self, key: String, work: Work, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.next_generation += 1;
        let generation = self.next_generation;
        self.generation.insert(key.clone(), generation);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work.run(&client).map_err(|e| format!("{e:#}")) })
                .await;
            this.update(cx, |hub, cx| {
                if hub.generation.get(&key) != Some(&generation) {
                    return;
                }
                let load = match result {
                    Ok(value) => Load::Ready(Rc::new(value)),
                    Err(error) => Load::Failed(error.into()),
                };
                // A failed refresh of something already shown keeps
                // what was there.
                if matches!(load, Load::Failed(_))
                    && matches!(hub.cache.get(&key), Some(Load::Ready(_)))
                {
                    return;
                }
                hub.cache.insert(key, load);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// While something is changing on GitHub (a running job), refresh
    /// what's cached under `prefixes` every `secs`, without blanking it.
    /// Call it on each render the page should keep live; it stops when
    /// renders stop asking.
    pub fn poll(&mut self, prefixes: &[String], secs: u64, cx: &mut Context<Self>) {
        let key = prefixes.join("|");
        if !self.polls.insert(key.clone()) {
            return;
        }
        let prefixes = prefixes.to_vec();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(secs))
                .await;
            this.update(cx, |hub, cx| {
                hub.polls.remove(&key);
                hub.mark_stale(&prefixes);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch what's cached under `prefixes` again next time it shows,
    /// showing the old answer meanwhile.
    fn mark_stale(&mut self, prefixes: &[String]) {
        let keys: Vec<String> = self
            .cache
            .keys()
            .filter(|k| prefixes.iter().any(|p| k.starts_with(p.as_str())))
            .cloned()
            .collect();
        self.stale.extend(keys);
    }

    /// What a write makes stale: the prefixes it names, and when they're
    /// about an issue or pull request, the repository's lists and counts
    /// of them and the searches that find them. GitHub's search index
    /// takes a moment to catch up with a write, so searches are asked
    /// again a few times over the next half minute too.
    pub fn after_write(&mut self, prefixes: &[String], cx: &mut Context<Self>) {
        let mut issues_changed = false;
        for prefix in prefixes {
            self.invalidate(prefix);
            if let Some(repo) = issues_repo(prefix) {
                issues_changed = true;
                for kind in ["issues", "pulls"] {
                    // The lists (`?state=…`) and counts (`#gql…`), not the
                    // other issues' own pages.
                    self.invalidate(&format!("/repos/{repo}/{kind}?"));
                    self.invalidate(&format!("/repos/{repo}/{kind}#"));
                }
            }
        }
        if !issues_changed {
            return;
        }
        self.invalidate("/search/issues");
        for secs in [3, 10, 30] {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(secs))
                    .await;
                this.update(cx, |hub, cx| {
                    hub.mark_stale(&["/search/issues".to_string()]);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Drop every cached answer whose key starts with `prefix`; the next
    /// render fetches them again.
    pub fn invalidate(&mut self, prefix: &str) {
        self.cache.retain(|key, _| !key.starts_with(prefix));
        self.generation.retain(|key, _| !key.starts_with(prefix));
    }

    /// Everything, fetched again.
    /// Back in the window after a while away: what was made elsewhere
    /// in the meantime (an issue opened on github.com, a push) shows up.
    /// Everything cached is asked again as it's shown, the old answer
    /// staying up until the new one lands.
    fn came_back(&mut self, cx: &mut Context<Self>) {
        if self.refreshed.elapsed() < Duration::from_secs(30)
            || !matches!(self.auth, Auth::SignedIn)
        {
            return;
        }
        self.refreshed = std::time::Instant::now();
        let keys: Vec<String> = self
            .cache
            .iter()
            .filter(|(_, load)| !matches!(load, Load::Loading))
            .map(|(k, _)| k.clone())
            .collect();
        self.stale.extend(keys);
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refreshed = std::time::Instant::now();
        self.cache.clear();
        self.generation.clear();
        self.toast("Refreshing…", false, cx);
        cx.notify();
    }

    // -- acting ---------------------------------------------------------

    pub fn perform(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match act {
            Act::None => {}
            Act::Go(route) => self.go(route, cx),
            Act::Url(url) => cx.open_url(&url),
            Act::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.toast("Copied to the clipboard", false, cx);
            }
            Act::Req(req) => self.run_req((*req).clone(), false, cx),
            Act::Confirm {
                title,
                message,
                label,
                then,
            } => {
                self.blur_fields();
                self.modal = Some(Modal::Confirm {
                    title,
                    message,
                    label,
                    act: then,
                    busy: false,
                    error: None,
                });
            }
            Act::Form(spec) => self.open_form(spec),
            Act::Menu(entries) => {
                self.menu = Some(MenuState {
                    at: window.mouse_position(),
                    entries,
                });
            }
            Act::Picker(picker) => self.open_picker(picker, window.mouse_position()),
            Act::Run(f) => f(self, window, cx),
        }
        cx.notify();
    }

    /// Send a write. From a dialog, the dialog shows it working, closes
    /// when it lands, and shows the error in place if it fails.
    pub fn run_req(&mut self, req: Req, from_modal: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.busy += 1;
        let work = req.work.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work.run(&client).map_err(|e| format!("{e:#}")) })
                .await;
            this.update(cx, |hub, cx| {
                hub.busy = hub.busy.saturating_sub(1);
                match result {
                    Ok(value) => {
                        if from_modal {
                            hub.modal = None;
                        }
                        hub.after_write(&req.invalidate, cx);
                        if !req.ok.is_empty() {
                            hub.toast(req.ok.clone(), false, cx);
                        }
                        if let Some(then) = &req.then {
                            then(hub, &value, cx);
                        }
                    }
                    Err(error) => {
                        let shown = match &mut hub.modal {
                            Some(Modal::Form { error: e, busy, .. })
                            | Some(Modal::Confirm { error: e, busy, .. })
                                if from_modal =>
                            {
                                *e = Some(error.clone());
                                *busy = false;
                                true
                            }
                            _ => false,
                        };
                        if !shown {
                            hub.toast(error, true, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.next_toast += 1;
        let id = self.next_toast;
        self.toasts.push(Toast {
            id,
            text: text.into(),
            error,
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        let wait = if error { 8 } else { 3 };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(wait))
                .await;
            this.update(cx, |hub, cx| {
                hub.toasts.retain(|t| t.id != id);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // -- dialogs --------------------------------------------------------

    pub fn open_form(&mut self, spec: Rc<FormSpec>) {
        self.blur_fields();
        crate::form::reset(self, &spec);
        self.modal = Some(Modal::Form {
            spec,
            error: None,
            busy: false,
        });
    }

    pub fn submit_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = match &self.modal {
            Some(Modal::Form { busy: true, .. })
            | Some(Modal::Confirm { busy: true, .. })
            | Some(Modal::Update { .. })
            | None => return,
            Some(Modal::Form { spec, .. }) => Some(spec.clone()),
            Some(Modal::Confirm { .. }) => None,
        };
        match form {
            Some(spec) => {
                let values = FormValues::read(self, &spec);
                match spec.build(&values) {
                    Ok(Act::Req(req)) => {
                        if let Some(Modal::Form { busy, error, .. }) = &mut self.modal {
                            *busy = true;
                            *error = None;
                        }
                        self.run_req((*req).clone(), true, cx);
                    }
                    Ok(other) => {
                        self.modal = None;
                        self.perform(other, window, cx);
                    }
                    Err(message) => {
                        if let Some(Modal::Form { error, .. }) = &mut self.modal {
                            *error = Some(message);
                        }
                    }
                }
            }
            None => {
                let Some(Modal::Confirm { act, busy, .. }) = &mut self.modal else {
                    return;
                };
                match (**act).clone() {
                    Act::Req(req) => {
                        *busy = true;
                        self.run_req((*req).clone(), true, cx);
                    }
                    other => {
                        self.modal = None;
                        self.perform(other, window, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        // Dismissing the update dialog while its download runs must not
        // leave an update to land on its own.
        if matches!(self.modal.take(), Some(Modal::Update { .. })) {
            self.update_progress = None;
        }
        self.blur_fields();
        cx.notify();
    }

    // -- text boxes -----------------------------------------------------

    pub fn field(&mut self, id: &str) -> &mut LineEdit {
        self.fields.entry(id.to_string()).or_default()
    }

    pub fn field_text(&self, id: &str) -> String {
        self.fields
            .get(id)
            .map(|f| f.text.clone())
            .unwrap_or_default()
    }

    pub fn set_field(&mut self, id: &str, text: impl Into<String>) {
        let multiline = self.fields.get(id).map(|f| f.multiline).unwrap_or(false);
        let field = self.field(id);
        field.set_text(text.into());
        field.multiline = multiline;
    }

    pub fn active_field(&self) -> Option<String> {
        self.fields
            .iter()
            .find(|(_, f)| f.active)
            .map(|(id, _)| id.clone())
    }

    pub fn blur_fields(&mut self) {
        for field in self.fields.values_mut() {
            field.active = false;
        }
    }

    pub fn focus_field(&mut self, id: &str) {
        self.blur_fields();
        self.field(id).focus();
    }

    pub fn press_field(&mut self, id: &str, press: &TextPress) {
        for (other, field) in self.fields.iter_mut() {
            if other != id {
                field.active = false;
            }
        }
        self.field(id).press(press);
    }

    pub fn toggle(&self, id: &str) -> bool {
        self.toggles.get(id).copied().unwrap_or(false)
    }

    pub fn choice(&self, id: &str, default: &str) -> String {
        self.choices
            .get(id)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    pub fn page(&self, id: &str) -> usize {
        self.pages.get(id).copied().unwrap_or(1)
    }

    pub fn is_open(&self, id: &str) -> bool {
        self.open.contains(id)
    }

    pub fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = &event.keystroke.modifiers;
        let primary = mods.control || mods.platform;

        if let Some(id) = self.active_field() {
            if key == "escape" {
                self.picker = None;
                self.blur_fields();
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if key == "tab" && self.modal.is_some() {
                crate::form::tab(self, &id, mods.shift);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            let Some(field) = self.fields.get_mut(&id) else {
                return;
            };
            match field.key(event, cx) {
                LineEditKey::Ignored => {}
                LineEditKey::Submitted => {
                    self.submit_field(&id, window, cx);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                _ => {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
            }
        }

        match key {
            "escape" => {
                if crate::select::clear() || self.autoscroll.take().is_some() {
                } else if self.menu.take().is_some() {
                } else if self.modal.is_some() {
                    self.close_modal(cx);
                } else {
                    self.scope_fix = None;
                }
                cx.notify();
            }
            "enter" if self.modal.is_some() => self.submit_modal(window, cx),
            "left" if mods.alt => self.go_back(cx),
            "right" if mods.alt => self.go_forward(cx),
            "r" if primary => self.refresh(cx),
            "c" if primary => {
                if let Some(text) = crate::select::selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            "k" if primary => {
                self.focus_field("jump");
                cx.notify();
            }
            "/" if self.modal.is_none() => {
                self.focus_field("jump");
                cx.notify();
            }
            _ => {}
        }
    }

    /// Enter in a one-line box, or Ctrl+Enter in a paragraph.
    fn submit_field(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if id.starts_with("form.") {
            // Enter in a dialog's paragraph box breaks the line (handled by
            // the edit); Ctrl+Enter, or Enter in a line, submits.
            self.submit_modal(window, cx);
            return;
        }
        if let Some(act) = self.submits.get(id).cloned() {
            self.perform(act, window, cx);
        }
        // A one-line box keeps the keyboard after Enter -- the edit
        // gave it back, take it again so typing can carry on.
        if let Some(field) = self.fields.get_mut(id) {
            if !field.multiline && id == "jump" {
                field.active = false;
            }
        }
    }

    // -- images -------------------------------------------------------

    /// An image by URL: fetched once, then served from memory. `None`
    /// while it is on its way or if it could not be read.
    fn load_image(&mut self, key: String, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        match self.images.get(&key) {
            Some(Avatar::Ready(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        self.images.insert(key.clone(), Avatar::Loading);
        let client = self.client.clone()?;
        cx.spawn(async move |this, cx| {
            let fetch_key = key.clone();
            let bytes = cx
                .background_executor()
                .spawn(async move { client.fetch_bytes(&fetch_key) })
                .await;
            this.update(cx, |hub, cx| {
                let image = match bytes {
                    Ok(bytes) => match sniff(&bytes) {
                        Some(format) => Avatar::Ready(Arc::new(Image::from_bytes(format, bytes))),
                        None => Avatar::Failed,
                    },
                    Err(_) => Avatar::Failed,
                };
                hub.images.insert(key, image);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    /// A round avatar, fetched once per URL and size.
    pub fn avatar(&mut self, url: &str, size: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui::{div, img, px, IntoElement as _, Styled as _};
        let p = crate::ui::palette();
        let pixels = (size * 2.0).round() as u32;
        let image = if url.is_empty() {
            None
        } else if url.contains('?') {
            self.load_image(format!("{url}&s={pixels}"), cx)
        } else {
            self.load_image(format!("{url}?s={pixels}"), cx)
        };
        match image {
            Some(image) => img(image)
                .size(px(size))
                .flex_none()
                .rounded_full()
                .into_any_element(),
            None => div()
                .size(px(size))
                .flex_none()
                .rounded_full()
                .bg(gpui::rgb(p.control_bg))
                .into_any_element(),
        }
    }

    /// An image in a document, at its own size up to the column's width.
    pub fn image(&mut self, url: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.image_sized(url, None, None, cx)
    }

    /// An image at the size its HTML asked for; the other side follows
    /// the picture's own proportions.
    pub fn image_sized(
        &mut self,
        url: &str,
        width: Option<f32>,
        height: Option<f32>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{
            div, img, prelude::FluentBuilder as _, px, IntoElement as _, ParentElement as _,
            Styled as _,
        };
        let p = crate::ui::palette();
        match self.load_image(url.to_string(), cx) {
            Some(image) => img(image)
                .when_some(width, |i, w| i.w(px(w)))
                .when_some(height, |i, h| i.h(px(h)))
                .max_w(px(760.0))
                .max_h(px(560.0))
                .into_any_element(),
            None => div()
                .w(px(240.0))
                .h(px(60.0))
                .rounded_md()
                .bg(gpui::rgb(p.control_bg))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(gpui::rgb(p.text_faint))
                .child("image")
                .into_any_element(),
        }
    }
}

/// An image's format from its first bytes.
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(ImageFormat::Webp),
        [b'B', b'M', ..] => Some(ImageFormat::Bmp),
        _ if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") => Some(ImageFormat::Svg),
        _ => None,
    }
}

/// The repository a write's prefix is about when it touched its issues or
/// pull requests (or the whole repository: a merge).
fn issues_repo(prefix: &str) -> Option<String> {
    let rest = prefix.strip_prefix("/repos/")?;
    let parts: Vec<&str> = rest.split(['/', '?', '#']).collect();
    // A GitLab project's path is as long as its groups nest.
    let n = if crate::forge::is_gitlab() {
        crate::forge::repo_len(&parts)
    } else {
        2
    };
    if n < 2 || parts.len() < n || parts[..n].iter().any(|p| p.is_empty()) {
        return None;
    }
    match parts.get(n) {
        None | Some(&"issues") | Some(&"pulls") | Some(&"merge_requests") => {
            Some(parts[..n].join("/"))
        }
        _ => None,
    }
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `path` with a query parameter added, whether or not it has a query yet.
pub fn with_query(path: &str, query: &str) -> String {
    if query.is_empty() {
        path.to_string()
    } else if path.contains('?') {
        format!("{path}&{query}")
    } else {
        format!("{path}?{query}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_links_become_pages() {
        assert_eq!(
            route_for_url("https://github.com/rust-lang/rust/issues/42"),
            Some(Route::Issue {
                repo: "rust-lang/rust".into(),
                number: 42
            })
        );
        assert_eq!(
            route_for_url("https://github.com/rust-lang/rust/pull/7#discussion"),
            Some(Route::Pull {
                repo: "rust-lang/rust".into(),
                number: 7,
                tab: PullTab::Conversation
            })
        );
        assert_eq!(
            route_for_url("https://github.com/octocat"),
            Some(Route::User {
                login: "octocat".into()
            })
        );
        assert_eq!(route_for_url("https://example.com/a"), None);
        assert_eq!(with_query("/a?x=1", "y=2"), "/a?x=1&y=2");
    }

    #[test]
    fn writes_to_issues_name_their_repository() {
        assert_eq!(issues_repo("/repos/o/r/issues"), Some("o/r".into()));
        assert_eq!(issues_repo("/repos/o/r/issues/12"), Some("o/r".into()));
        assert_eq!(issues_repo("/repos/o/r/pulls/3"), Some("o/r".into()));
        assert_eq!(issues_repo("/repos/o/r"), Some("o/r".into()));
        assert_eq!(issues_repo("/repos/o/r/labels"), None);
        assert_eq!(issues_repo("/user/starred"), None);
    }
}
