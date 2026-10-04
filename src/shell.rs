//! The window's frame: the sidebar, the top bar, the page, and what floats
//! over them (menus, dialogs, toasts). Also the sign-in screen.

use crate::hub::{on, Act, Auth, Hub, MenuEntry, Route};
use crate::json::Json as _;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    anchored, deferred, div, px, AnyElement, Context, ElementId, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Window, WindowAppearance,
};
use crate::ui::{icon, palette, Button, IconButton, MenuItem, Spinner};

impl Render for Hub {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame += 1;
        // Always the system's appearance.
        self.light = matches!(
            window.appearance(),
            WindowAppearance::Light | WindowAppearance::VibrantLight
        );
        crate::ui::set_light(self.light);
        let p = palette();
        // Enter-handlers are registered by whatever drew their box.
        self.submits.clear();

        let body: AnyElement = match &self.auth {
            Auth::SignedIn => {
                window.set_window_title(&format!("{} · VisualHub", self.route.title()));
                div()
                    .flex()
                    .flex_row()
                    .size_full()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.top_bar(cx))
                            .child(self.page_content(cx)),
                    )
                    .into_any_element()
            }
            _ => {
                window.set_window_title("VisualHub");
                self.sign_in_screen(cx)
            }
        };

        let scope_fix = match self.auth {
            Auth::SignedIn => self.render_scope_fix(cx),
            _ => None,
        };
        let modal = self.render_modal(cx);
        let menu = self.render_menu(cx);
        let picker = self.render_picker(cx);
        let toasts = self.render_toasts();

        div()
            .relative()
            .size_full()
            .bg(rgb(p.window_bg))
            .text_color(rgb(p.text))
            .text_size(px(widgets::TEXT))
            .track_focus(&self.focus)
            .key_context("VisualHub")
            .on_key_down(cx.listener(|hub, event: &KeyDownEvent, window, cx| {
                hub.on_key(event, window, cx)
            }))
            // Any press takes the keyboard from whichever box had it; the
            // box under the pointer (if any) takes it back on its own press.
            .capture_any_mouse_down(cx.listener(|hub, event: &MouseDownEvent, window, cx| {
                // While autoscrolling, any other click only stops it.
                if event.button != MouseButton::Middle && hub.autoscroll_cancel(cx) {
                    cx.stop_propagation();
                    return;
                }
                hub.autoscroll_down(event, window, cx);
                if hub.active_field().is_some() {
                    hub.blur_fields();
                    cx.notify();
                }
                window.focus(&hub.focus);
            }))
            .capture_any_mouse_up(cx.listener(|hub, event: &MouseUpEvent, _, cx| hub.autoscroll_up(event, cx)))
            .on_mouse_move(cx.listener(|hub, event: &MouseMoveEvent, _, cx| hub.autoscroll_move(event, cx)))
            .child(body)
            .children(scope_fix)
            .children(modal)
            .children(menu)
            .children(picker)
            .child(toasts)
            .children(self.render_autoscroll())
    }
}

impl Hub {
    // -- the sidebar ----------------------------------------------------

    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let unread = self
            .fetch("/notifications?per_page=50", cx)
            .ready()
            .map(|v| v.list("").len() as i64);
        let route = self.route.clone();
        let is = |r: &Route| std::mem::discriminant(r) == std::mem::discriminant(&route);
        let items: Vec<(&str, &str, Route, Option<i64>)> = vec![
            ("Home", "home", Route::Home, None),
            ("Notifications", "bell", Route::Notifications, unread),
            ("Repositories", "repo", Route::Repos, None),
            ("Pull requests", "pr", Route::Pulls, None),
            ("Issues", "issue", Route::Issues, None),
            ("Projects", "project", Route::Projects, None),
            ("Gists", "gist", Route::Gists, None),
            ("Codespaces", "codespace", Route::Codespaces, None),
            ("Packages", "package", Route::Packages, None),
            ("Search", "search", Route::Search, None),
            ("Settings", "settings", Route::Settings, None),
        ];
        let mut nav = div().flex().flex_col().gap(px(2.0)).px_2();
        for (i, (label, icon_name, target, count)) in items.into_iter().enumerate() {
            let selected = is(&target);
            nav = nav.child(nav_item(("nav", i), icon_name, label, selected, count, Act::Go(target)));
        }

        let mut recent = div().flex().flex_col().gap(px(2.0)).px_2();
        for (i, repo) in self.recent.clone().into_iter().enumerate() {
            let selected = self.route.repo() == Some(repo.as_str());
            recent = recent.child(nav_item(
                ("recent", i),
                "repo",
                &repo,
                selected,
                None,
                Act::Go(Route::Repo {
                    repo: repo.clone(),
                    tab: crate::hub::RepoTab::Code,
                }),
            ));
        }

        let mut orgs = div().flex().flex_col().gap(px(2.0)).px_2();
        if let Some(list) = self.fetch("/user/orgs?per_page=100", cx).ready().cloned() {
            for (i, org) in list.list("").iter().enumerate() {
                let login = org.s("login");
                let selected = self.route == Route::Org { login: login.clone() };
                let avatar = self.avatar(&org.s("avatar_url"), 16.0, cx);
                orgs = orgs.child(
                    div()
                        .id(("org", i))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .h(px(28.0))
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(rgb(p.selection_bg)))
                        .hover(|s| s.bg(rgb(p.hover)))
                        .child(avatar)
                        .child(div().text_ellipsis().overflow_hidden().child(login.clone()))
                        .on_click(on(Act::Go(Route::Org { login }))),
                );
            }
        }

        let me = self.me.clone();
        let avatar = self.avatar(&me.s("avatar_url"), 28.0, cx);
        let login = me.s("login");
        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(232.0))
            .h_full()
            .bg(rgb(p.panel_bg))
            .border_r_1()
            .border_color(rgb(p.panel_edge))
            .overflow_y_scroll()
            .track_scroll(&self.scroller("sidebar"))
            .pt_2()
            .child(
                div()
                    .id("me")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .h(px(52.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.hover)))
                    .on_click(on(Act::Go(Route::User {
                        login: login.clone(),
                    })))
                    .child(avatar)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .child(login.clone()),
                            )
                            .child(widgets::faint(me.s("name"))),
                    ),
            )
            .child(div().h(px(1.0)).bg(rgb(p.divider)).mb_2())
            .child(nav)
            .when(!self.recent.is_empty(), |d| {
                d.child(section_label("Recent"))
                    .child(recent)
            })
            .child(section_label("Organizations"))
            .child(orgs)
            .child(div().h(px(16.0)))
            .into_any_element()
    }

    // -- the top bar ----------------------------------------------------

    fn top_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let route = self.route.clone();
        self.submits.insert(
            "jump".into(),
            Act::run(|hub, _, cx| {
                let text = hub.field_text("jump").trim().to_string();
                hub.jump(&text, cx);
            }),
        );
        let placeholder = match &route {
            Route::User { login } | Route::Org { login } => format!("Search in @{login}, or jump to owner/repo, #123, @user…   ( / )"),
            _ => "Jump to owner/repo, #123, @user, or search…   ( / )".to_string(),
        };
        let jump = self
            .input("jump", &placeholder, cx)
            .w(px(380.0));
        let me = self.me.clone();
        let avatar = self.avatar(&me.s("avatar_url"), 22.0, cx);
        let login = me.s("login");
        let new_menu = self.new_menu();
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(52.0))
            .px_4()
            .bg(rgb(p.panel_bg))
            .border_b_1()
            .border_color(rgb(p.panel_edge))
            .child(
                IconButton::new("back", "arrow-left")
                    .size(28.0)
                    .icon_size(16.0)
                    .disabled(!self.can_go_back())
                    .tooltip("Back", Some("Alt+←".into()))
                    .on_click(cx.listener(|hub, _, _, cx| hub.go_back(cx))),
            )
            .child(
                IconButton::new("forward", "arrow-right")
                    .size(28.0)
                    .icon_size(16.0)
                    .disabled(!self.can_go_forward())
                    .tooltip("Forward", Some("Alt+→".into()))
                    .on_click(cx.listener(|hub, _, _, cx| hub.go_forward(cx))),
            )
            .child(self.breadcrumbs())
            .child(div().flex_1())
            .when(self.busy > 0, |d| d.child(Spinner::new("busy").size(16.0)))
            .child(jump)
            .child(
                IconButton::new("new", "plus")
                    .size(28.0)
                    .icon_size(16.0)
                    .tooltip("Create new…", None)
                    .on_click(on(new_menu)),
            )
            .child(
                IconButton::new("refresh", "refresh")
                    .size(28.0)
                    .icon_size(16.0)
                    .tooltip("Refresh", Some("Ctrl+R".into()))
                    .on_click(cx.listener(|hub, _, _, cx| hub.refresh(cx))),
            )
            .child(
                IconButton::new("web", "external")
                    .size(28.0)
                    .icon_size(16.0)
                    .tooltip("Open this page on github.com", None)
                    .on_click(on(Act::Url(route.web_url()))),
            )
            .child(
                div()
                    .id("account")
                    .cursor_pointer()
                    .rounded_full()
                    .child(avatar)
                    .on_click(on(Act::menu(vec![
                        MenuEntry::Header(format!("Signed in as {login}")),
                        MenuEntry::item(
                            "Your profile",
                            Act::Go(Route::User {
                                login: login.clone(),
                            }),
                        ),
                        MenuEntry::item("Your repositories", Act::Go(Route::Repos)),
                        MenuEntry::item("Your stars", Act::run(|hub, _, cx| {
                            hub.choices.insert("repos.tab".into(), "starred".into());
                            hub.go(Route::Repos, cx);
                        })),
                        MenuEntry::item("Your gists", Act::Go(Route::Gists)),
                        MenuEntry::item("Settings", Act::Go(Route::Settings)),
                        MenuEntry::Sep,
                        MenuEntry::item("Sign out", Act::run(|hub, _, cx| hub.sign_out(cx))),
                    ]))),
            )
            .into_any_element()
    }

    fn breadcrumbs(&self) -> AnyElement {
        let p = palette();
        let mut el = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl_2()
            .min_w_0()
            .overflow_hidden()
            .text_size(px(14.0));
        if let Some(repo) = self.route.repo() {
            let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
            el = el
                .child(
                    div()
                        .id("crumb-owner")
                        .cursor_pointer()
                        .text_color(rgb(p.text_dim))
                        .hover(|s| s.text_color(rgb(p.text)))
                        .child(owner.to_string())
                        .on_click(on(Act::Go(Route::User {
                            login: owner.to_string(),
                        }))),
                )
                .child(div().text_color(rgb(p.text_faint)).child("/"))
                .child(
                    div()
                        .id("crumb-repo")
                        .cursor_pointer()
                        .font_weight(FontWeight::SEMIBOLD)
                        .hover(|s| s.text_color(rgb(p.accent_hover)))
                        .child(name.to_string())
                        .on_click(on(Act::Go(Route::Repo {
                            repo: repo.to_string(),
                            tab: crate::hub::RepoTab::Code,
                        }))),
                );
        } else {
            el = el.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.route.title()),
            );
        }
        el.into_any_element()
    }

    /// The "+" menu: what can be made from here.
    fn new_menu(&self) -> Act {
        let mut entries = vec![
            MenuEntry::item("New repository", crate::screens::repos::new_repo_form(None)),
            MenuEntry::item("New gist", crate::screens::gists::new_gist_form()),
            MenuEntry::item(
                "Import repository",
                Act::Url(format!("{}/new/import", crate::api::WEB)),
            ),
            MenuEntry::item(
                "New organization",
                Act::Url(format!("{}/account/organizations/new", crate::api::WEB)),
            ),
        ];
        if let Some(repo) = self.route.repo() {
            entries.insert(0, MenuEntry::Sep);
            entries.insert(
                0,
                MenuEntry::item(
                    "New pull request",
                    crate::screens::pulls::new_pull_form(repo, "", ""),
                ),
            );
            entries.insert(0, MenuEntry::item("New issue", crate::screens::issues::new_issue_form(repo)));
            entries.insert(0, MenuEntry::Header(repo.to_string()));
        }
        Act::menu(entries)
    }

    /// The jump box: `owner/repo`, `owner/repo#12`, `#12` in the current
    /// repository, `@user`, a github.com URL, or a search.
    pub fn jump(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.set_field("jump", "");
        if let Some(route) = crate::hub::route_for_url(text) {
            self.go(route, cx);
            return;
        }
        if let Some(login) = text.strip_prefix('@') {
            self.go(Route::User { login: login.to_string() }, cx);
            return;
        }
        let number = |s: &str| s.parse::<u64>().ok();
        if let Some(n) = text.strip_prefix('#').and_then(number) {
            if let Some(repo) = self.route.repo().map(str::to_string) {
                self.go(Route::Issue { repo, number: n }, cx);
                return;
            }
        }
        if let Some((repo, n)) = text.split_once('#') {
            if let (true, Some(n)) = (repo.contains('/'), number(n)) {
                self.go(Route::Issue { repo: repo.to_string(), number: n }, cx);
                return;
            }
        }
        let is_slug = |s: &str| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && !s.is_empty()
        };
        if let Some((owner, name)) = text.split_once('/') {
            if is_slug(owner) && is_slug(name) {
                self.go(
                    Route::Repo {
                        repo: text.to_string(),
                        tab: crate::hub::RepoTab::Code,
                    },
                    cx,
                );
                return;
            }
        }
        // On someone's profile, a search looks only at their things.
        let text = match &self.route {
            Route::User { login } | Route::Org { login } if !text.contains("user:") && !text.contains("org:") => format!("user:{login} {text}"),
            _ => text.to_string(),
        };
        self.set_field("search.q", &text);
        self.choices.insert("search.applied".into(), text);
        self.go(Route::Search, cx);
        cx.notify();
    }

    // -- the page -------------------------------------------------------

    fn page_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let route = self.route.clone();
        let content = match &route {
            Route::Home => self.home(cx),
            Route::Notifications => self.notifications(cx),
            Route::Repos => self.repos(cx),
            Route::Pulls => self.my_pulls(cx),
            Route::Issues => self.my_issues(cx),
            Route::Repo { repo, tab } => self.repo(repo, *tab, cx),
            Route::Tree {
                repo,
                git_ref,
                path,
                file,
            } => self.tree(repo, git_ref, path, *file, cx),
            Route::Issue { repo, number } => self.issue(repo, *number, cx),
            Route::Pull { repo, number, tab } => self.pull(repo, *number, *tab, cx),
            Route::Commit { repo, sha } => self.commit(repo, sha, cx),
            Route::Compare { repo, base, head } => self.compare(repo, base, head, cx),
            Route::Run { repo, id } => self.run(repo, *id, cx),
            Route::Job { repo, id, name } => self.job(repo, *id, name, cx),
            Route::Release { repo, id } => self.release(repo, *id, cx),
            Route::Discussion { repo, number } => self.discussion(repo, *number, cx),
            Route::User { login } => self.user(login, cx),
            Route::Org { login } => self.org(login, cx),
            Route::Team { org, slug } => self.team(org, slug, cx),
            Route::Gists => self.gists(cx),
            Route::Gist { id } => self.gist(id, cx),
            Route::Search => self.search(cx),
            Route::Projects => self.projects(cx),
            Route::Project { id } => self.project(id, cx),
            Route::Codespaces => self.codespaces(cx),
            Route::Packages => self.packages(cx),
            Route::Settings => self.settings(cx),
        };
        let id = ElementId::Name(format!("page-{route:?}").into());
        if route.owns_scroll() {
            div()
                .id(id)
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(content)
                .into_any_element()
        } else {
            div()
                .id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroller("page"))
                .child(content)
                .into_any_element()
        }
    }

    // -- floating things ------------------------------------------------

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let (at, entries) = (menu.at, menu.entries.clone());
        let mut popover = crate::ui::Popover::new("menu")
            .in_flow()
            .min_w(px(220.0))
            .max_w(px(420.0))
            .max_h(px(480.0))
            .text_size(px(13.0))
            .on_dismiss(cx.listener(|hub, _, _, cx| {
                hub.menu = None;
                cx.notify();
            }));
        for (i, entry) in entries.iter().enumerate() {
            popover = match entry {
                MenuEntry::Item {
                    label,
                    checked,
                    act,
                } => popover.child(
                    MenuItem::new(("menu-item", i), label.clone())
                        .checked(*checked)
                        .h(px(28.0))
                        .on_click(on(act.clone())),
                ),
                MenuEntry::Header(text) => popover.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(rgb(palette().text_dim))
                        .child(text.clone()),
                ),
                MenuEntry::Sep => popover.child(crate::ui::menu_separator()),
            };
        }
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.0))
                    .child(div().id("menu-scroll").overflow_y_scroll().child(popover)),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    fn render_toasts(&self) -> AnyElement {
        let p = palette();
        let mut stack = div()
            .absolute()
            .bottom_4()
            .right_4()
            .flex()
            .flex_col()
            .gap_2()
            .items_end();
        for toast in &self.toasts {
            let color = if toast.error { widgets::red() } else { widgets::green() };
            stack = stack.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .max_w(px(460.0))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(p.popup_bg))
                    .border_1()
                    .border_color(rgb(color))
                    .shadow_lg()
                    .child(icon(
                        if toast.error { "alert" } else { "check-circle" },
                        16.0,
                        color,
                    ))
                    .child(div().flex_1().child(toast.text.clone())),
            );
        }
        stack.into_any_element()
    }

    // -- signing in -----------------------------------------------------

    fn sign_in_screen(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let checking = matches!(self.auth, Auth::Checking);
        let error = match &self.auth {
            Auth::SignedOut { error } => error.clone(),
            _ => None,
        };
        self.submits.insert(
            "token".into(),
            Act::run(|hub, _, cx| {
                let token = hub.field_text("token");
                hub.sign_in_with(token, cx);
            }),
        );
        let token_url = crate::scopes::new_token_url();
        let mut card = widgets::card()
            .w(px(480.0))
            .p_6()
            .gap_4()
            .bg(rgb(p.panel_bg))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(44.0))
                            .rounded_lg()
                            .bg(rgb(0x7C3AED))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0xFFFFFF))
                            .text_size(px(24.0))
                            .font_weight(FontWeight::BOLD)
                            .child("V"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(widgets::title("VisualHub"))
                            .child(widgets::dim("A native GitHub client")),
                    ),
            );
        if checking {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(Spinner::new("checking").size(16.0))
                    .child("Signing in…"),
            );
        } else {
            let input = self
                .secret_input("token", "ghp_… or github_pat_…", cx)
                .w_full();
            card = card
                .child(div().child(
                    "Sign in with a personal access token. VisualHub also picks up \
                     GH_TOKEN, GITHUB_TOKEN and the GitHub CLI's login automatically.",
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(widgets::h3("Personal access token"))
                        .child(input),
                )
                .when_some(error, |c, error| {
                    c.child(div().text_color(rgb(widgets::red())).child(error))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            Button::new("sign-in", "Sign in")
                                .primary()
                                .h(px(30.0))
                                .px_4()
                                .on_click(cx.listener(|hub, _, _, cx| {
                                    let token = hub.field_text("token");
                                    hub.sign_in_with(token, cx);
                                })),
                        )
                        .child(widgets::btn("make-token", "Create a token on GitHub", Act::Url(token_url)))
                        .child(
                            Button::new("retry", "Detect again")
                                .ghost()
                                .h(px(30.0))
                                .on_click(cx.listener(|hub, _, _, cx| {
                                    hub.rediscover(cx);
                                })),
                        ),
                )
                .child(widgets::faint(
                    "The token is stored in your user configuration folder and never \
                     leaves this machine except to talk to api.github.com.",
                ));
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(card)
            .into_any_element()
    }
}

fn nav_item(
    id: impl Into<ElementId>,
    icon_name: &str,
    label: &str,
    selected: bool,
    count: Option<i64>,
    act: Act,
) -> AnyElement {
    let p = palette();
    let color = if selected { p.text } else { p.text_dim };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .h(px(30.0))
        .rounded_md()
        .cursor_pointer()
        .text_color(rgb(if selected { p.text } else { p.text }))
        .when(selected, |d| {
            d.bg(rgb(p.selection_bg)).font_weight(FontWeight::MEDIUM)
        })
        .hover(|s| s.bg(rgb(p.hover)))
        .child(icon(icon_name, 16.0, color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.to_string()),
        )
        .when_some(count.filter(|n| *n > 0), |d, n| {
            d.child(
                div()
                    .px(px(6.0))
                    .rounded_full()
                    .bg(rgb(p.accent))
                    .text_color(rgb(p.accent_text))
                    .text_size(px(11.0))
                    .child(if n >= 50 { "50+".to_string() } else { n.to_string() }),
            )
        })
        .on_click(on(act))
        .into_any_element()
}

fn section_label(text: &str) -> AnyElement {
    div()
        .px_4()
        .pt_4()
        .pb_1()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette().text_faint))
        .child(text.to_uppercase())
        .into_any_element()
}
