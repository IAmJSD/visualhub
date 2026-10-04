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
use crate::form::{FormSpec, FormValues};
use crate::json::Json as _;
use anyhow::Result;
use gpui::{
    App, ClickEvent, ClipboardItem, Context, FocusHandle, Image,
    ImageFormat, KeyDownEvent, Pixels, Point, WeakEntity, Window,
};
use crate::ui::{LineEdit, LineEditKey, TextPress};
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
    Repo { repo: String, tab: RepoTab },
    /// A directory listing or a file, at a branch, tag or commit.
    Tree { repo: String, git_ref: String, path: String, file: bool },
    Issue { repo: String, number: u64 },
    Pull { repo: String, number: u64, tab: PullTab },
    Commit { repo: String, sha: String },
    Compare { repo: String, base: String, head: String },
    Run { repo: String, id: u64 },
    Job { repo: String, id: u64, name: String },
    Release { repo: String, id: u64 },
    Discussion { repo: String, number: u64 },
    User { login: String },
    Org { login: String },
    Team { org: String, slug: String },
    Gists,
    Gist { id: String },
    /// The New repository page, owned by you or `owner`.
    NewRepo { owner: Option<String> },
    NewGist,
    Search,
    Projects,
    Project { id: String },
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
            Route::Notifications => "Notifications".into(),
            Route::Repos => "Repositories".into(),
            Route::Pulls => "Pull requests".into(),
            Route::Issues => "Issues".into(),
            Route::Repo { repo, .. } => repo.clone(),
            Route::Tree { repo, path, .. } if path.is_empty() => repo.clone(),
            Route::Tree { repo, path, .. } => format!("{repo}/{path}"),
            Route::Issue { repo, number } => format!("{repo}#{number}"),
            Route::Pull { repo, number, .. } => format!("{repo}#{number}"),
            Route::Commit { repo, sha } => format!("{repo}@{}", &sha[..sha.len().min(7)]),
            Route::Compare { repo, base, head } => format!("{repo} {base}...{head}"),
            Route::Run { repo, id } => format!("{repo} run {id}"),
            Route::Job { name, .. } => name.clone(),
            Route::Release { repo, .. } => format!("{repo} release"),
            Route::Discussion { repo, number } => format!("{repo} discussion #{number}"),
            Route::User { login } | Route::Org { login } => login.clone(),
            Route::Team { org, slug } => format!("{org}/{slug}"),
            Route::Gists => "Gists".into(),
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
            Route::Job { .. } | Route::Tree { file: true, .. } | Route::Pull { tab: PullTab::Files, .. }
        )
    }

    /// The same page on github.com.
    pub fn web_url(&self) -> String {
        let w = api::WEB;
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
                };
                format!("{w}/{repo}{suffix}")
            }
            Route::Tree { repo, git_ref, path, file } => format!(
                "{w}/{repo}/{}/{git_ref}/{path}",
                if *file { "blob" } else { "tree" }
            ),
            Route::Issue { repo, number } => format!("{w}/{repo}/issues/{number}"),
            Route::Pull { repo, number, .. } => format!("{w}/{repo}/pull/{number}"),
            Route::Commit { repo, sha } => format!("{w}/{repo}/commit/{sha}"),
            Route::Compare { repo, base, head } => format!("{w}/{repo}/compare/{base}...{head}"),
            Route::Run { repo, id } => format!("{w}/{repo}/actions/runs/{id}"),
            Route::Job { repo, id, .. } => format!("{w}/{repo}/actions/runs/0/job/{id}"),
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

/// The page a github.com link points at, when the app has one.
pub fn route_for_url(url: &str) -> Option<Route> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let reserved = [
        "settings", "notifications", "pulls", "issues", "marketplace", "explore", "topics",
        "sponsors", "features", "login", "orgs", "apps", "search", "codespaces", "new",
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
                ["pull", n, ..] => Route::Pull {
                    repo,
                    number: num(n)?,
                    tab: PullTab::Conversation,
                },
                ["discussions", n, ..] if num(n).is_some() => Route::Discussion {
                    repo,
                    number: num(n)?,
                },
                ["commit", sha, ..] => Route::Commit {
                    repo,
                    sha: sha.to_string(),
                },
                ["actions", "runs", id, ..] => Route::Run {
                    repo,
                    id: num(id)?,
                },
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

enum Avatar {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub struct Hub {
    pub focus: FocusHandle,
    pub client: Option<Client>,
    pub auth: Auth,
    pub me: Rc<Value>,
    pub scopes: String,
    /// Where the token came from, as [`api::discover_tokens`] names it.
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
    /// Commit authors as GraphQL resolved them, by SHA.
    pub commit_people: HashMap<String, Vec<Value>>,
    /// Expanded rows and sections, by id.
    pub open: HashSet<String>,
}

impl Hub {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        HUB.with(|h| *h.borrow_mut() = Some(cx.entity().downgrade()));
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let mut hub = Hub {
            focus,
            client: None,
            auth: Auth::Checking,
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
            commit_people: HashMap::new(),
            open: HashSet::new(),
        };
        hub.discover(cx);
        hub
    }

    // -- signing in -----------------------------------------------------

    /// Try every token the machine already has.
    fn discover(&mut self, cx: &mut Context<Self>) {
        self.auth = Auth::Checking;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut errors = Vec::new();
                    for (token, source) in api::discover_tokens() {
                        let client = Client::new(&token);
                        match client.send("GET", "/user", None, None) {
                            Ok(reply) if reply.status == 200 => {
                                let me: Value =
                                    serde_json::from_str(&reply.body).unwrap_or(Value::Null);
                                return Ok((client, me, reply.scopes.unwrap_or_default(), source));
                            }
                            Ok(reply) => errors.push(format!(
                                "The {source} token was refused ({}).",
                                reply.status
                            )),
                            Err(e) => errors.push(format!("{source}: {e:#}")),
                        }
                    }
                    Err(errors.join(" "))
                })
                .await;
            this.update(cx, |hub, cx| {
                match result {
                    Ok((client, me, scopes, source)) => hub.signed_in(client, me, scopes, source),
                    Err(error) => {
                        hub.auth = Auth::SignedOut {
                            error: (!error.is_empty()).then_some(error),
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Sign in with a pasted token, keeping it for next time.
    pub fn sign_in_with(&mut self, token: String, cx: &mut Context<Self>) {
        let token = token.trim().to_string();
        if token.is_empty() {
            self.auth = Auth::SignedOut {
                error: Some("Paste a personal access token first.".into()),
            };
            cx.notify();
            return;
        }
        self.auth = Auth::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = Client::new(&token);
                    let reply = client.send("GET", "/user", None, None)?;
                    if reply.status != 200 {
                        anyhow::bail!("GitHub refused the token ({}).", reply.status);
                    }
                    api::save_token(&token)?;
                    let me: Value = serde_json::from_str(&reply.body)?;
                    Ok((client, me, reply.scopes.unwrap_or_default()))
                })
                .await;
            this.update(cx, |hub, cx| {
                match result {
                    Ok((client, me, scopes)) => hub.signed_in(client, me, scopes, "saved sign-in"),
                    Err(e) => {
                        hub.auth = Auth::SignedOut {
                            error: Some(format!("{e:#}")),
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn rediscover(&mut self, cx: &mut Context<Self>) {
        self.discover(cx);
        cx.notify();
    }

    fn signed_in(&mut self, client: Client, me: Value, scopes: String, source: &'static str) {
        self.refresh_token(client, me, scopes);
        self.recent = api::load_recent(&self.me.s("login"));
        self.choices.extend(api::load_merge_methods());
        self.token_source = source;
        self.check_scopes();
        self.auth = Auth::SignedIn;
        self.route = Route::Home;
        self.back.clear();
        self.forward.clear();
    }

    /// Swap in a token for the same account, dropping what the old one
    /// fetched (some of it was errors the new one can get past).
    pub fn refresh_token(&mut self, client: Client, me: Value, scopes: String) {
        self.client = Some(client);
        self.me = Rc::new(me);
        self.scopes = scopes;
        self.cache.clear();
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        api::forget_token();
        self.client = None;
        self.me = Rc::new(Value::Null);
        self.cache.clear();
        self.fields.clear();
        self.modal = None;
        self.menu = None;
        self.auth = Auth::SignedOut { error: None };
        cx.notify();
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
                api::save_recent(&self.me.s("login"), &self.recent);
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
            return load.clone();
        }
        let Some(client) = self.client.clone() else {
            return Load::Failed("Not signed in".into());
        };
        self.cache.insert(key.clone(), Load::Loading);
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
                hub.cache.insert(key, load);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Load::Loading
    }

    /// Drop every cached answer whose key starts with `prefix`; the next
    /// render fetches them again.
    pub fn invalidate(&mut self, prefix: &str) {
        self.cache.retain(|key, _| !key.starts_with(prefix));
        self.generation.retain(|key, _| !key.starts_with(prefix));
    }

    /// Everything, fetched again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
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
                        for prefix in &req.invalidate {
                            hub.invalidate(prefix);
                        }
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
            Some(Modal::Form { busy: true, .. }) | Some(Modal::Confirm { busy: true, .. }) | None => return,
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
        self.modal = None;
        self.blur_fields();
        cx.notify();
    }

    // -- text boxes -----------------------------------------------------

    pub fn field(&mut self, id: &str) -> &mut LineEdit {
        self.fields.entry(id.to_string()).or_default()
    }

    pub fn field_text(&self, id: &str) -> String {
        self.fields.get(id).map(|f| f.text.clone()).unwrap_or_default()
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
                    self.modal = None;
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
        use gpui::{div, img, px, IntoElement as _, ParentElement as _, Styled as _};
        let p = crate::ui::palette();
        match self.load_image(url.to_string(), cx) {
            Some(image) => img(image)
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
}
