//! The home page (your feed, what needs you, your top repositories) and
//! the notifications inbox.

use super::issues::search_spec;
use crate::hub::{Act, Hub, Load, MenuEntry, Req, Route, RepoTab, PullTab};
use crate::json::{self, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::{div, px, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::Value;

/// What an event says, without who did it: an icon, a sentence starting
/// with the verb ("pushed 2 commits to main"), any detail under it, and
/// where clicking it goes.
pub struct EventText {
    pub icon: &'static str,
    pub text: String,
    pub body: String,
    pub open: Act,
}

pub fn describe_event(e: &Value) -> EventText {
    let repo = e.s("repo.name");
    let p = e.at("payload");
    let repo_act = Act::Go(Route::Repo { repo: repo.clone(), tab: RepoTab::Code });
    let (icon_name, text, body, open) = match e.s("type").as_str() {
        "WatchEvent" => ("star", format!("starred {repo}"), String::new(), repo_act),
        "ForkEvent" => (
            "fork",
            format!("forked {repo} to {}", p.s("forkee.full_name")),
            String::new(),
            Act::Go(Route::Repo { repo: p.s("forkee.full_name"), tab: RepoTab::Code }),
        ),
        "PushEvent" => {
            let branch = p.s("ref").trim_start_matches("refs/heads/").to_string();
            let commits: Vec<String> = p.list("commits").iter().map(|c| format!("• {}", json::first_line(&c.s("message")))).collect();
            // The events API stopped sending commit counts, so only say
            // how many when it does.
            let n = if p.i("size") > 0 { p.i("size") } else { p.i("distinct_size") };
            (
                "commit",
                match n {
                    0 => format!("pushed to {branch}"),
                    1 => format!("pushed 1 commit to {branch}"),
                    n => format!("pushed {n} commits to {branch}"),
                },
                commits.join("\n"),
                Act::Go(Route::Repo { repo: repo.clone(), tab: RepoTab::Commits }),
            )
        }
        "CreateEvent" => (
            if p.s("ref_type") == "repository" { "repo" } else { "branch" },
            if p.s("ref_type") == "repository" {
                format!("created repository {repo}")
            } else {
                format!("created {} {}", p.s("ref_type"), p.s("ref"))
            },
            p.s("description"),
            repo_act,
        ),
        "DeleteEvent" => ("trash", format!("deleted {} {}", p.s("ref_type"), p.s("ref")), String::new(), repo_act),
        "IssuesEvent" => (
            "issue",
            format!("{} issue #{}", p.s("action"), p.i("issue.number")),
            p.s("issue.title"),
            Act::Go(Route::Issue { repo: repo.clone(), number: p.i("issue.number") as u64 }),
        ),
        "IssueCommentEvent" => (
            "comment",
            format!("commented on #{}", p.i("issue.number")),
            format!("{}\n{}", p.s("issue.title"), json::clip(&p.s("comment.body"), 200)),
            if p.has("issue.pull_request") {
                Act::Go(Route::Pull { repo: repo.clone(), number: p.i("issue.number") as u64, tab: PullTab::Conversation })
            } else {
                Act::Go(Route::Issue { repo: repo.clone(), number: p.i("issue.number") as u64 })
            },
        ),
        "PullRequestEvent" => (
            "pr",
            format!(
                "{} pull request #{}",
                if p.s("action") == "closed" && p.b("pull_request.merged") { "merged".to_string() } else { p.s("action") },
                p.i("number")
            ),
            p.s("pull_request.title"),
            Act::Go(Route::Pull { repo: repo.clone(), number: p.i("number") as u64, tab: PullTab::Conversation }),
        ),
        "PullRequestReviewEvent" => (
            "eye",
            format!("reviewed pull request #{}", p.i("pull_request.number")),
            p.s("pull_request.title"),
            Act::Go(Route::Pull { repo: repo.clone(), number: p.i("pull_request.number") as u64, tab: PullTab::Conversation }),
        ),
        "PullRequestReviewCommentEvent" => (
            "comment",
            format!("commented on a review in #{}", p.i("pull_request.number")),
            json::clip(&p.s("comment.body"), 200),
            Act::Go(Route::Pull { repo: repo.clone(), number: p.i("pull_request.number") as u64, tab: PullTab::Files }),
        ),
        "ReleaseEvent" => (
            "tag",
            format!("released {}", if p.s("release.name").is_empty() { p.s("release.tag_name") } else { p.s("release.name") }),
            json::clip(&p.s("release.body"), 200),
            Act::Go(Route::Release { repo: repo.clone(), id: p.i("release.id") as u64 }),
        ),
        "PublicEvent" => ("globe", format!("made {repo} public"), String::new(), repo_act),
        "MemberEvent" => ("person", format!("{} {} to {repo}", p.s("action"), p.s("member.login")), String::new(), repo_act),
        "GollumEvent" => ("book", format!("edited the wiki"), String::new(), Act::Url(format!("{}/{repo}/wiki", crate::api::WEB))),
        "CommitCommentEvent" => ("comment", format!("commented on a commit"), json::clip(&p.s("comment.body"), 200), repo_act),
        "DiscussionEvent" => (
            "discussion",
            format!("{} a discussion", p.s("action")),
            p.s("discussion.title"),
            Act::Go(Route::Discussion { repo: repo.clone(), number: p.i("discussion.number") as u64 }),
        ),
        "SponsorshipEvent" => ("heart", "sponsored someone".to_string(), String::new(), Act::None),
        other => ("dot", format!("did {}", other.trim_end_matches("Event")), String::new(), repo_act),
    };
    EventText { icon: icon_name, text, body, open }
}

/// How a feed event reads.
fn event_row(e: &Value) -> Row {
    let repo = e.s("repo.name");
    let EventText { icon, text, body, open } = describe_event(e);
    // The repository goes under the line unless the line already names it.
    let ago = time::ago(&e.s("created_at"));
    let meta = if text.contains(&repo) { ago } else { format!("{repo}  ·  {ago}") };
    Row::new(format!("{} {text}", e.s("actor.login")))
        .avatar(e.s("actor.avatar_url"))
        .icon(icon, widgets::gray())
        .meta(meta)
        .body(body)
        .open(open)
}

/// The page a notification is about.
fn notification_route(n: &Value) -> Act {
    let url = n.s("subject.url");
    let repo = n.s("repository.full_name");
    let number = url.rsplit('/').next().and_then(|s| s.parse::<u64>().ok());
    match (n.s("subject.type").as_str(), number) {
        ("Issue", Some(number)) => Act::Go(Route::Issue { repo, number }),
        ("PullRequest", Some(number)) => Act::Go(Route::Pull { repo, number, tab: PullTab::Conversation }),
        ("Discussion", Some(number)) => Act::Go(Route::Discussion { repo, number }),
        ("Release", Some(id)) => Act::Go(Route::Release { repo, id }),
        ("Commit", _) => Act::Go(Route::Commit { repo, sha: url.rsplit('/').next().unwrap_or("").to_string() }),
        ("CheckSuite", _) | ("WorkflowRun", _) => Act::Go(Route::Repo { repo, tab: RepoTab::Actions }),
        ("RepositoryVulnerabilityAlert", _) | ("RepositoryDependabotAlertsThread", _) => Act::Go(Route::Repo { repo, tab: RepoTab::Security }),
        _ => Act::Go(Route::Repo { repo, tab: RepoTab::Code }),
    }
}

impl Hub {
    pub fn home(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let login = self.login();
        let feed = ListSpec::new(format!("/users/{login}/received_events"), event_row).empty("Your feed is empty. Follow people and star repositories to fill it.");
        let feed = self.list(&feed, cx);

        let mut review = search_spec("is:open is:pr review-requested:@me archived:false", true);
        review.id = "home-review".into();
        let review = self.compact_list(&review, 6, cx);
        let assigned = search_spec("is:open assignee:@me archived:false", true);
        let assigned = self.compact_list(&assigned, 8, cx);
        let mine = search_spec("is:open is:pr author:@me archived:false", true);
        let mine = self.compact_list(&mine, 6, cx);

        let top = self.fetch("/user/repos?sort=pushed&per_page=12&affiliation=owner,collaborator,organization_member", cx);
        let mut top_list = widgets::col().gap_1();
        match top {
            Load::Ready(v) => {
                for (i, r) in v.list("").iter().enumerate() {
                    let full = r.s("full_name");
                    let avatar = self.avatar(&r.s("owner.avatar_url"), 18.0, cx);
                    top_list = top_list.child(
                        widgets::list_row(("top", i), Act::Go(Route::Repo { repo: full.clone(), tab: RepoTab::Code }))
                            .px_2()
                            .py_1()
                            .border_b_0()
                            .rounded_md()
                            .items_center()
                            .child(avatar)
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(full)),
                    );
                }
            }
            other => top_list = top_list.child(widgets::placeholder(&other)),
        }

        let name = self.me.s("name");
        widgets::page()
            .max_w(px(1320.0))
            .child(widgets::title(format!("Welcome back{}", if name.is_empty() { String::new() } else { format!(", {name}") })))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_6()
                    .items_start()
                    .child(
                        widgets::col()
                            .w(px(260.0))
                            .flex_none()
                            .child(
                                widgets::row()
                                    .child(widgets::h3("Top repositories"))
                                    .child(widgets::spacer())
                                    .child(widgets::go_btn("home-new-repo", "New", Act::Go(Route::NewRepo { owner: None })).h(px(24.0))),
                            )
                            .child(top_list),
                    )
                    .child(widgets::col().gap_3().flex_1().min_w_0().child(widgets::h2("Feed")).child(feed))
                    .child(
                        widgets::col()
                            .gap_3()
                            .w(px(380.0))
                            .flex_none()
                            .child(widgets::h3("Review requests"))
                            .child(review)
                            .child(widgets::h3("Assigned to you"))
                            .child(assigned)
                            .child(widgets::h3("Your open pull requests"))
                            .child(mine),
                    ),
            )
            .into_any_element()
    }

    /// The first `n` items of a list, without paging.
    fn compact_list(&mut self, spec: &ListSpec, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let path = crate::hub::with_query(&spec.path, &format!("per_page={n}"));
        match self.fetch(&path, cx) {
            Load::Ready(v) => {
                let items = json::items(&v, spec.items);
                if items.is_empty() {
                    return widgets::card().child(div().p_3().child(widgets::dim("Nothing right now."))).into_any_element();
                }
                let mut card = widgets::card();
                for (i, item) in items.iter().enumerate() {
                    let row = (spec.row)(item);
                    card = card.child(self.render_row(&format!("{}-{i}", spec.id), row, cx));
                }
                card.into_any_element()
            }
            other => widgets::placeholder(&other),
        }
    }

    pub fn notifications(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.choice("notif.filter", "unread");
        let repo = self.choice("notif.repo", "");
        let base = if repo.is_empty() { "/notifications".to_string() } else { format!("/repos/{repo}/notifications") };
        let path = match filter.as_str() {
            "all" => format!("{base}?all=true"),
            "participating" => format!("{base}?participating=true"),
            _ => base.clone(),
        };
        let spec = ListSpec::new(path, |n| {
            let id = n.s("id");
            let thread = format!("/notifications/threads/{id}");
            let (icon_name, color) = match n.s("subject.type").as_str() {
                "Issue" => ("issue", widgets::green()),
                "PullRequest" => ("pr", widgets::green()),
                "Release" => ("tag", widgets::gray()),
                "Discussion" => ("discussion", widgets::gray()),
                "Commit" => ("commit", widgets::gray()),
                "CheckSuite" | "WorkflowRun" => ("play", widgets::red()),
                "RepositoryVulnerabilityAlert" | "RepositoryDependabotAlertsThread" => ("alert", widgets::yellow()),
                _ => ("bell", widgets::gray()),
            };
            let open = notification_route(n);
            let mark_and_open = {
                let thread = thread.clone();
                let unread = n.b("unread");
                Act::run(move |hub, window, cx| {
                    if unread {
                        hub.perform(Req::rest("PATCH", thread.clone()).inval("/notifications").act(), window, cx);
                    }
                    hub.perform(open.clone(), window, cx);
                })
            };
            let reason = n.s("reason").replace('_', " ");
            let mut row = Row::new(n.s("subject.title"))
                .icon(icon_name, color)
                .meta(format!("{}  ·  {reason}  ·  {}", n.s("repository.full_name"), time::ago(&n.s("updated_at"))))
                .open(mark_and_open);
            if n.b("unread") {
                row = row.tag("unread", widgets::purple());
                row = row.action("Mark as read", Req::rest("PATCH", thread.clone()).ok("Marked as read").inval("/notifications").act());
            }
            row.action("Done", Req::rest("DELETE", thread.clone()).ok("Marked as done").inval("/notifications").act())
                .action(
                    "Unsubscribe",
                    Req::rest("DELETE", format!("{thread}/subscription")).ok("Unsubscribed").inval("/notifications").act(),
                )
                .action(
                    "Mute thread",
                    Req::rest("PUT", format!("{thread}/subscription"))
                        .body(serde_json::json!({ "ignored": true }))
                        .ok("Thread muted")
                        .inval("/notifications")
                        .act(),
                )
                .action("Open on GitHub", Act::Url(format!("{}/{}", crate::api::WEB, n.s("repository.full_name"))))
        })
        .empty("All caught up!");
        let list = self.list(&spec, cx);

        // Repositories seen in the unread notifications, for the filter.
        let mut repos: Vec<String> = Vec::new();
        if let Some(v) = self.fetch("/notifications?all=true&per_page=50", cx).ready() {
            for n in v.list("") {
                let r = n.s("repository.full_name");
                if !repos.contains(&r) {
                    repos.push(r);
                }
            }
        }
        let mut repo_menu = vec![MenuEntry::check("All repositories", repo.is_empty(), Act::choose("notif.repo", ""))];
        for r in repos {
            repo_menu.push(MenuEntry::check(r.clone(), r == repo, Act::choose("notif.repo", r)));
        }
        let mark_all = if repo.is_empty() {
            Req::rest("PUT", "/notifications").body(serde_json::json!({})).ok("All notifications marked as read").inval("/notifications")
        } else {
            Req::rest("PUT", format!("/repos/{repo}/notifications")).body(serde_json::json!({})).ok("Marked as read").inval("/notifications").inval(format!("/repos/{repo}/notifications"))
        };
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title("Notifications"))
                    .child(widgets::spacer())
                    .child(widgets::btn("notif-settings", "Notification settings", Act::Url(format!("{}/settings/notifications", crate::api::WEB))))
                    .child(widgets::primary("mark-all", "Mark all as read", mark_all.act())),
            )
            .child(
                widgets::row()
                    .child(widgets::chips(vec![
                        ("Unread".into(), filter == "unread", Act::choose("notif.filter", "unread")),
                        ("Participating".into(), filter == "participating", Act::choose("notif.filter", "participating")),
                        ("All".into(), filter == "all", Act::choose("notif.filter", "all")),
                    ]))
                    .child(widgets::spacer())
                    .child(widgets::btn(
                        "notif-repo",
                        if repo.is_empty() { "Repository ▾".to_string() } else { format!("{repo} ▾") },
                        Act::menu(repo_menu),
                    )),
            )
            .child(list)
            .into_any_element()
    }
}

