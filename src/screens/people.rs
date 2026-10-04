//! People and organizations: profiles with the contribution calendar,
//! following and blocking, organizations with members, teams,
//! invitations and settings, and teams.

use super::common::{repo_row, user_row};
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, Req, Route, RepoTab};
use crate::json::{self, Json as _};
use crate::ready;
use super::home::{describe_event, EventText};
use crate::resource::{Fetched, ListSpec, Row};
use crate::time;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _};
use crate::ui::{icon, palette};
use serde_json::{json, Value};

const PROFILE: &str = "query($l: String!) {
  user(login: $l) {
    pinnedItems(first: 6, types: [REPOSITORY]) {
      nodes { ... on Repository { nameWithOwner description stargazerCount forkCount primaryLanguage { name color } } }
    }
    contributionsCollection {
      contributionCalendar { totalContributions weeks { contributionDays { contributionCount date color } } }
    }
  }
}";

fn gists_spec(path: String) -> ListSpec {
    ListSpec::new(path, super::gists::gist_row).empty("No gists.")
}

impl Hub {
    pub fn user(&mut self, login: &str, cx: &mut Context<Self>) -> AnyElement {
        let user = ready!(self.fetch(&format!("/users/{login}"), cx));
        if user.s("type") == "Organization" {
            return self.org(login, cx);
        }
        let p = palette();
        let me = self.login();
        let is_me = me.eq_ignore_ascii_case(login);
        let avatar = self.avatar(&user.s("avatar_url"), 220.0, cx);
        let following = self.fetch_check(&format!("/user/following/{login}"), cx).ready().map(|v| v.b(""));
        let blocked = if is_me { None } else { self.fetch_check(&format!("/user/blocks/{login}"), cx).ready().map(|v| v.b("")) };
        let follow = match following {
            Some(true) => widgets::btn("follow", "Unfollow", Req::rest("DELETE", format!("/user/following/{login}")).ok(format!("Unfollowed {login}")).inval("/user/following").inval(format!("/users/{login}")).act()),
            _ => widgets::primary("follow", "Follow", Req::rest("PUT", format!("/user/following/{login}")).ok(format!("Following {login}")).inval("/user/following").inval(format!("/users/{login}")).act()),
        };
        let block = match blocked {
            Some(true) => Some(widgets::btn("block", "Unblock", Req::rest("DELETE", format!("/user/blocks/{login}")).ok("Unblocked").inval("/user/blocks").act())),
            Some(false) => Some(widgets::danger(
                "block",
                "Block",
                Req::rest("PUT", format!("/user/blocks/{login}"))
                    .ok("Blocked")
                    .inval("/user/blocks")
                    .act()
                    .confirm(format!("Block {login}?"), "They won't be able to follow you, open issues on your repositories or comment on your content.", "Block"),
            )),
            None => None,
        };
        let mut info = widgets::col()
            .gap_2()
            .w(px(260.0))
            .flex_none()
            .child(avatar)
            .child(div().text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child(user.s("name")))
            .child(div().text_size(px(17.0)).text_color(rgb(p.text_dim)).child(login.to_string()))
            .when(!user.s("bio").is_empty(), |d| d.child(div().child(user.s("bio"))));
        info = if is_me {
            info.child(widgets::btn("edit-profile", "Edit profile", Act::Go(Route::Settings)).w_full())
        } else {
            info.child(follow.w_full())
        };
        let people = |label: &str, n: i64, tab: &str| {
            let key = format!("user.tab:{login}");
            crate::ui::Link::new(ElementId::Name(format!("people-{tab}").into()), format!("{n} {label}"))
                .text_size(px(12.0))
                .on_click(on(Act::choose(key, tab)))
        };
        info = info
            .child(widgets::row().child(icon("people", 14.0, p.text_dim)).child(people("followers", user.i("followers"), "followers")).child(people("following", user.i("following"), "following")))
            .when(!user.s("company").is_empty(), |d| d.child(widgets::icon_text("org", user.s("company"), p.text)))
            .when(!user.s("location").is_empty(), |d| d.child(widgets::icon_text("globe", user.s("location"), p.text)))
            .when(!user.s("email").is_empty(), |d| d.child(widgets::icon_text("mail", user.s("email"), p.text)))
            .when(!user.s("blog").is_empty(), |d| d.child(crate::ui::Link::new("blog", user.s("blog")).url(if user.s("blog").starts_with("http") { user.s("blog") } else { format!("https://{}", user.s("blog")) })))
            .when(!user.s("twitter_username").is_empty(), |d| d.child(widgets::icon_text("mention", format!("@{}", user.s("twitter_username")), p.text)))
            .child(widgets::faint(format!("Joined {}", time::date(&user.s("created_at")))));
        if let Some(orgs) = self.fetch(&format!("/users/{login}/orgs"), cx).ready().cloned() {
            if !orgs.list("").is_empty() {
                let mut grid = div().flex().flex_row().flex_wrap().gap_1();
                for (i, o) in orgs.list("").iter().enumerate() {
                    let avatar = self.avatar(&o.s("avatar_url"), 32.0, cx);
                    grid = grid.child(
                        div()
                            .id(("user-org", i))
                            .cursor_pointer()
                            .tooltip(crate::ui::tip(o.s("login"), None))
                            .child(avatar)
                            .on_click(on(Act::Go(Route::Org { login: o.s("login") }))),
                    );
                }
                info = info.child(div().h(px(1.0)).bg(rgb(p.divider))).child(widgets::h3("Organizations")).child(grid);
            }
        }
        info = info
            .child(div().h(px(1.0)).bg(rgb(p.divider)))
            .children(block)
            .child(widgets::btn("sponsor", "Sponsor", Act::Url(format!("{}/sponsors/{login}", crate::api::WEB))))
            .child(widgets::btn("report", "Report abuse", Act::Url(format!("{}/contact/report-abuse?report={login}", crate::api::WEB))));

        let tab_key = format!("user.tab:{login}");
        let tab = self.choice(&tab_key, "overview");
        let tabs = widgets::tabs(
            "user",
            vec![
                widgets::TabItem::new("Overview", "book", tab == "overview", Act::choose(&tab_key, "overview")),
                widgets::TabItem::new("Repositories", "repo", tab == "repos", Act::choose(&tab_key, "repos")).count(Some(user.i("public_repos"))),
                widgets::TabItem::new("Stars", "star", tab == "stars", Act::choose(&tab_key, "stars")),
                widgets::TabItem::new("Gists", "gist", tab == "gists", Act::choose(&tab_key, "gists")).count(Some(user.i("public_gists"))),
                widgets::TabItem::new("Followers", "people", tab == "followers", Act::choose(&tab_key, "followers")).count(Some(user.i("followers"))),
                widgets::TabItem::new("Following", "people", tab == "following", Act::choose(&tab_key, "following")).count(Some(user.i("following"))),
            ],
        );
        let body = match tab.as_str() {
            "repos" => {
                let spec = ListSpec::new(format!("/users/{login}/repos?sort=pushed"), repo_row).empty("No public repositories.");
                self.list(&spec, cx)
            }
            "stars" => {
                let spec = ListSpec::new(format!("/users/{login}/starred"), repo_row).empty("No stars yet.");
                self.list(&spec, cx)
            }
            "gists" => {
                let spec = gists_spec(format!("/users/{login}/gists"));
                self.list(&spec, cx)
            }
            "followers" => {
                let spec = ListSpec::new(format!("/users/{login}/followers"), user_row).empty("No followers yet.");
                self.list(&spec, cx)
            }
            "following" => {
                let spec = ListSpec::new(format!("/users/{login}/following"), user_row).empty("Not following anyone.");
                self.list(&spec, cx)
            }
            _ => self.user_overview(login, cx),
        };
        widgets::page()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_8()
                    .items_start()
                    .child(info)
                    .child(widgets::col().gap_4().flex_1().min_w_0().child(tabs).child(body)),
            )
            .into_any_element()
    }

    /// Pinned repositories, the contribution calendar, and recent activity.
    fn user_overview(&mut self, login: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let mut col = widgets::col().gap_4();
        if let Load::Ready(data) = self.fetch_gql(&format!("/users/{login}"), PROFILE, json!({ "l": login }), cx) {
            let pinned = data.list("user.pinnedItems.nodes").to_vec();
            if !pinned.is_empty() {
                let mut grid = div().flex().flex_row().flex_wrap().gap_3();
                for (i, r) in pinned.iter().enumerate() {
                    let full = r.s("nameWithOwner");
                    let lang_color = widgets::parse_hex(&r.s("primaryLanguage.color"));
                    grid = grid.child(
                        widgets::card()
                            .id(("pinned", i))
                            .w(px(340.0))
                            .p_3()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgb(p.accent)))
                            .on_click(on(Act::Go(Route::Repo { repo: full.clone(), tab: RepoTab::Code })))
                            .child(widgets::row().child(icon("repo", 14.0, p.text_dim)).child(div().text_color(rgb(p.accent_hover)).font_weight(FontWeight::SEMIBOLD).child(full)))
                            .child(widgets::dim(json::clip(&r.s("description"), 120)))
                            .child(
                                widgets::row()
                                    .gap_3()
                                    .when(r.has("primaryLanguage"), |d| {
                                        d.child(widgets::row().gap_1().child(div().size(px(10.0)).rounded_full().bg(rgb(lang_color))).child(widgets::dim(r.s("primaryLanguage.name"))))
                                    })
                                    .child(widgets::dim(format!("★ {}", json::count(r.i("stargazerCount")))))
                                    .child(widgets::dim(format!("⑂ {}", json::count(r.i("forkCount"))))),
                            ),
                    );
                }
                col = col.child(widgets::h3("Pinned")).child(grid);
            }
            let calendar = data.at("user.contributionsCollection.contributionCalendar");
            let weeks = calendar.list("weeks");
            if !weeks.is_empty() {
                let mut grid = div().flex().flex_row().gap(px(3.0));
                for (wi, week) in weeks.iter().enumerate() {
                    let mut column = div().flex().flex_col().gap(px(3.0));
                    for (di, day) in week.list("contributionDays").iter().enumerate() {
                        let n = day.i("contributionCount");
                        let fill = if n == 0 { p.control_bg } else { widgets::parse_hex(&day.s("color")) };
                        column = column.child(
                            div()
                                .id(ElementId::Name(format!("day-{wi}-{di}").into()))
                                .size(px(11.0))
                                .rounded(px(2.0))
                                .bg(rgb(fill))
                                .tooltip(crate::ui::tip(format!("{n} contributions on {}", time::date(&format!("{}T00:00:00Z", day.s("date")))), None)),
                        );
                    }
                    grid = grid.child(column);
                }
                col = col
                    .child(widgets::h3(format!("{} contributions in the last year", calendar.i("totalContributions"))))
                    .child(widgets::card().p_3().child(div().id("calendar").overflow_x_scroll().track_scroll(&self.scroller("calendar")).child(grid)));
            }
        }
        let activity = self.activity_timeline(login, cx);
        col.child(widgets::h3("Recent activity")).child(activity).into_any_element()
    }

    /// A user's public events as a timeline, grouped by day.
    fn activity_timeline(&mut self, login: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let spec = ListSpec::new(format!("/users/{login}/events/public"), |_| Row::new(""));
        let (mut events, loading, more) = match self.fetch_list(&spec, cx) {
            Fetched::Items { items, loading, more } => (items, loading, more),
            Fetched::Failed(error) => return widgets::error_box(&error),
        };
        if events.is_empty() {
            return if loading { widgets::loading() } else { widgets::card().child(widgets::empty("No recent public activity.")).into_any_element() };
        }

        // GitHub doesn't always send events newest first.
        events.sort_by_key(|e| std::cmp::Reverse(time::parse(&e.s("created_at")).unwrap_or(0)));
        let today = time::now().div_euclid(86_400);
        let day_of = |e: &Value| time::parse(&e.s("created_at")).map(|t| t.div_euclid(86_400));
        let mut timeline = widgets::card().p_4().gap_0();
        for (i, e) in events.iter().enumerate() {
            let day = day_of(e);
            let first_of_day = i == 0 || day_of(&events[i - 1]) != day;
            let last_of_day = i + 1 == events.len() || day_of(&events[i + 1]) != day;
            if first_of_day {
                let heading = match day {
                    Some(d) if d == today => "Today".to_string(),
                    Some(d) if d == today - 1 => "Yesterday".to_string(),
                    _ => time::date(&e.s("created_at")),
                };
                timeline = timeline.child(
                    div()
                        .when(i > 0, |d| d.pt_3())
                        .pb_2()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text_dim))
                        .child(heading),
                );
            }

            let repo = e.s("repo.name");
            let EventText { icon: icon_name, text, body, open } = describe_event(e);
            let tint = match icon_name {
                "star" => widgets::yellow(),
                "trash" => widgets::red(),
                "pr" if text.starts_with("merged") => widgets::purple(),
                "pr" | "tag" | "repo" | "branch" => widgets::green(),
                _ => p.text_dim,
            };
            let mut sentence = text.clone();
            if let Some(first) = sentence.get(0..1) {
                sentence.replace_range(0..1, &first.to_uppercase());
            }
            let rail = div()
                .flex()
                .flex_col()
                .items_center()
                .flex_none()
                .w(px(28.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .size(px(28.0))
                        .rounded_full()
                        .bg(rgb(p.control_bg))
                        .border_1()
                        .border_color(rgb(p.panel_edge))
                        .child(icon(icon_name, 14.0, tint)),
                )
                .when(!last_of_day, |d| d.child(div().w(px(2.0)).flex_1().bg(rgb(p.divider))));
            let mut detail = widgets::col().gap(px(2.0)).min_w_0();
            for line in body.lines().filter(|l| !l.trim().is_empty()).take(4) {
                detail = detail.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(p.text_dim))
                        .text_ellipsis()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(json::clip(line, 140)),
                );
            }
            let content = widgets::col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .pt(px(5.0))
                .when(!last_of_day, |d| d.pb_4())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_x_1()
                        .child(sentence)
                        .when(!text.contains(&repo), |d| {
                            d.child(widgets::dim("in")).child(div().font_weight(FontWeight::SEMIBOLD).child(repo.clone()))
                        }),
                )
                .child(detail);
            timeline = timeline.child(
                div()
                    .id(("activity", i))
                    .flex()
                    .flex_row()
                    .gap_3()
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.hover)))
                    .on_click(on(open))
                    .child(rail)
                    .child(content)
                    .child(div().flex_none().pt(px(6.0)).child(widgets::faint(time::ago(&e.s("created_at"))))),
            );
        }
        if loading {
            timeline = timeline.child(widgets::loading());
        } else if more {
            let id = spec.id.clone();
            timeline = timeline.child(
                div().flex().justify_center().pt_3().child(widgets::btn(
                    "activity-more",
                    "Show older activity",
                    Act::run(move |hub, _, cx| {
                        let next = hub.page(&id) + 1;
                        hub.pages.insert(id.clone(), next);
                        cx.notify();
                    }),
                )),
            );
        }
        timeline.into_any_element()
    }

    pub fn org(&mut self, login: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let org = ready!(self.fetch(&format!("/orgs/{login}"), cx));
        let me = self.login();
        let membership = self.fetch(&format!("/user/memberships/orgs/{login}"), cx);
        let admin = membership.ready().map(|m| m.s("role") == "admin").unwrap_or(false);
        let member = membership.ready().map(|m| m.s("state") == "active").unwrap_or(false);
        let avatar = self.avatar(&org.s("avatar_url"), 72.0, cx);
        let tab_key = format!("org.tab:{login}");
        let tab = self.choice(&tab_key, "repos");
        let mut names = vec![("repos", "Repositories", "repo"), ("people", "People", "people"), ("teams", "Teams", "people"), ("packages", "Packages", "package")];
        if admin {
            names.extend([("invites", "Invitations", "mail"), ("outside", "Outside collaborators", "person"), ("hooks", "Webhooks", "webhook"), ("variables", "Variables", "code"), ("secrets", "Secrets", "lock"), ("blocked", "Blocked users", "x-circle")]);
        }
        let tabs = widgets::tabs(
            "org",
            names.iter().map(|(v, l, i)| widgets::TabItem::new(*l, i, tab == *v, Act::choose(&tab_key, *v))).collect(),
        );
        let o = login.to_string();
        let body = match tab.as_str() {
            "people" => {
                let invite = FormSpec::new(format!("Invite a member to {login}"))
                    .submit("Send invitation")
                    .field(Field::text("who", "Login or email").required())
                    .field(Field::choice("role", "Role", &[("direct_member", "Member"), ("admin", "Owner"), ("billing_manager", "Billing manager")]))
                    .build_with({
                        let o = o.clone();
                        move |v| {
                            let (o, who, role) = (o.clone(), v.s("who").trim().to_string(), v.s("role"));
                            let inval = format!("/orgs/{o}/invitations");
                            Ok(Req::custom(move |client| {
                                let body = if who.contains('@') {
                                    json!({ "email": who, "role": role })
                                } else {
                                    let id = client.get(&format!("/users/{who}"))?.i("id");
                                    json!({ "invitee_id": id, "role": role })
                                };
                                client.json("POST", &format!("/orgs/{o}/invitations"), Some(&body))
                            })
                            .ok("Invitation sent")
                            .inval(inval)
                            .act())
                        }
                    })
                    .act();
                let me2 = me.clone();
                let spec = ListSpec::new(format!("/orgs/{login}/members"), move |u| {
                    let user = u.s("login");
                    let mut row = user_row(u);
                    if admin {
                        row = row
                            .action(
                                "Change role…",
                                FormSpec::new(format!("Role for {user}"))
                                    .field(Field::choice("role", "Role", &[("member", "Member"), ("admin", "Owner")]))
                                    .rest("PUT", format!("/orgs/{o}/memberships/{user}"))
                                    .ok("Role changed")
                                    .inval(format!("/orgs/{o}/members"))
                                    .act(),
                            )
                            .danger(
                                "Remove from organization",
                                Req::rest("DELETE", format!("/orgs/{o}/members/{user}")).ok("Member removed").inval(format!("/orgs/{o}/members")).act().confirm(
                                    format!("Remove {user}?"),
                                    "They lose access to the organization's repositories.",
                                    "Remove",
                                ),
                            );
                    }
                    if user == me2 {
                        row = row
                            .action("Make membership public", Req::rest("PUT", format!("/orgs/{o}/public_members/{user}")).ok("Membership is public").act())
                            .action("Make membership private", Req::rest("DELETE", format!("/orgs/{o}/public_members/{user}")).ok("Membership is private").act());
                    }
                    row
                })
                .empty("No members you can see.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().when(admin, |d| d.child(widgets::row().child(widgets::spacer()).child(widgets::go_btn("invite-member", "Invite member", invite)))).child(list).into_any_element()
            }
            "teams" => {
                let create = FormSpec::new("Create a team")
                    .submit("Create team")
                    .field(Field::text("name", "Team name").required())
                    .field(Field::text("description", "Description"))
                    .field(Field::choice("privacy", "Visibility", &[("closed", "Visible"), ("secret", "Secret")]))
                    .field(Field::choice("notification_setting", "Notifications", &[("notifications_enabled", "Enabled"), ("notifications_disabled", "Disabled")]))
                    .rest("POST", format!("/orgs/{login}/teams"))
                    .ok("Team created")
                    .inval(format!("/orgs/{login}/teams"))
                    .act();
                let spec = ListSpec::new(format!("/orgs/{login}/teams"), move |t| {
                    let slug = t.s("slug");
                    Row::new(t.s("name"))
                        .icon("people", widgets::gray())
                        .meta(format!("{}  ·  {}", t.s("privacy"), t.s("description")))
                        .open(Act::Go(Route::Team { org: o.clone(), slug }))
                })
                .empty("No teams.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().child(widgets::row().child(widgets::spacer()).child(widgets::go_btn("new-team", "New team", create))).child(list).into_any_element()
            }
            "packages" => self.package_list(&format!("/orgs/{login}/packages"), cx),
            "invites" => {
                let spec = ListSpec::new(format!("/orgs/{login}/invitations"), move |i| {
                    Row::new(if i.s("login").is_empty() { i.s("email") } else { i.s("login") })
                        .icon("mail", widgets::gray())
                        .meta(format!("{}  ·  invited {} by {}", i.s("role"), time::ago(&i.s("created_at")), i.s("inviter.login")))
                        .danger("Cancel invitation", Req::rest("DELETE", format!("/orgs/{o}/invitations/{}", i.i("id"))).ok("Invitation cancelled").inval(format!("/orgs/{o}/invitations")).act())
                })
                .empty("No pending invitations.");
                self.list(&spec, cx)
            }
            "outside" => {
                let spec = ListSpec::new(format!("/orgs/{login}/outside_collaborators"), move |u| {
                    let user = u.s("login");
                    user_row(u)
                        .action("Convert to member", Req::rest("PUT", format!("/orgs/{o}/outside_collaborators/{user}")).ok("Converted to member").inval(format!("/orgs/{o}")).act())
                        .danger("Remove", Req::rest("DELETE", format!("/orgs/{o}/outside_collaborators/{user}")).ok("Removed").inval(format!("/orgs/{o}/outside_collaborators")).act())
                })
                .empty("No outside collaborators.");
                self.list(&spec, cx)
            }
            "hooks" => self.webhooks(&format!("/orgs/{login}/hooks"), cx),
            "variables" => self.variables(&format!("/orgs/{login}/actions/variables"), cx),
            "secrets" => {
                let base = format!("/orgs/{login}/actions/secrets");
                let add = super::repo_extra::secret_form("New organization secret", format!("{base}/public-key"), base.clone(), base.clone(), "");
                let b = base.clone();
                let spec = ListSpec::new(base.clone(), move |s| {
                    let name = s.s("name");
                    Row::new(name.clone())
                        .icon("lock", widgets::gray())
                        .meta(format!("{} repositories  ·  updated {}", s.s("visibility"), time::ago(&s.s("updated_at"))))
                        .danger("Delete", Req::rest("DELETE", format!("{b}/{name}")).ok("Secret deleted").inval(b.clone()).act())
                })
                .items("secrets")
                .empty("No organization secrets.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().child(widgets::row().child(widgets::faint("New organization secrets are available to private repositories.")).child(widgets::spacer()).child(widgets::go_btn("org-secret", "New secret", add))).child(list).into_any_element()
            }
            "blocked" => {
                let block = FormSpec::new("Block a user")
                    .field(Field::text("login", "Login").required())
                    .build_with({
                        let o = o.clone();
                        move |v| Ok(Req::rest("PUT", format!("/orgs/{o}/blocks/{}", v.s("login").trim())).ok("User blocked").inval(format!("/orgs/{o}/blocks")).act())
                    })
                    .act();
                let spec = ListSpec::new(format!("/orgs/{login}/blocks"), move |u| {
                    user_row(u).action("Unblock", Req::rest("DELETE", format!("/orgs/{o}/blocks/{}", u.s("login"))).ok("Unblocked").inval(format!("/orgs/{o}/blocks")).act())
                })
                .empty("No blocked users.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().child(widgets::row().child(widgets::spacer()).child(widgets::danger("block-user", "Block user", block))).child(list).into_any_element()
            }
            _ => {
                let spec = ListSpec::new(format!("/orgs/{login}/repos?sort=pushed"), repo_row).empty("No repositories.");
                self.list(&spec, cx)
            }
        };
        let settings = FormSpec::new(format!("{login} settings"))
            .width(600.0)
            .field(Field::text("name", "Display name").value(org.s("name")).keep_empty())
            .field(Field::text("description", "Description").value(org.s("description")).keep_empty())
            .field(Field::text("email", "Public email").value(org.s("email")).keep_empty())
            .field(Field::text("blog", "URL").value(org.s("blog")).keep_empty())
            .field(Field::text("location", "Location").value(org.s("location")).keep_empty())
            .field(Field::text("billing_email", "Billing email").value(org.s("billing_email")))
            .field(Field::choice("default_repository_permission", "Base permission", &[("read", "Read"), ("write", "Write"), ("admin", "Admin"), ("none", "No permission")]).value(org.s("default_repository_permission")))
            .field(Field::bool("members_can_create_repositories", "Members can create repositories", org.b("members_can_create_repositories")))
            .field(Field::bool("web_commit_signoff_required", "Require sign off on web commits", org.b("web_commit_signoff_required")))
            .rest("PATCH", format!("/orgs/{login}"))
            .ok("Organization saved")
            .inval(format!("/orgs/{login}"))
            .act();
        widgets::page()
            .child(
                widgets::row()
                    .gap_4()
                    .child(avatar)
                    .child(
                        widgets::col()
                            .gap_1()
                            .flex_1()
                            .child(widgets::title(if org.s("name").is_empty() { login.to_string() } else { org.s("name") }))
                            .when(!org.s("description").is_empty(), |d| d.child(widgets::dim(org.s("description"))))
                            .child(
                                widgets::row()
                                    .gap_4()
                                    .when(org.b("is_verified"), |d| d.child(widgets::tag("Verified", widgets::green())))
                                    .when(!org.s("location").is_empty(), |d| d.child(widgets::icon_text("globe", org.s("location"), p.text_dim)))
                                    .when(!org.s("blog").is_empty(), |d| d.child(crate::ui::Link::new("org-blog", org.s("blog")).url(org.s("blog"))))
                                    .child(widgets::dim(format!("{} public repositories  ·  {} followers", org.i("public_repos"), org.i("followers")))),
                            ),
                    )
                    .when(member || admin, |d| d.child(widgets::go_btn("org-new-repo", "New repository", super::repos::new_repo_form(Some(login)))))
                    .when(admin, |d| d.child(widgets::btn("org-settings", "Settings", settings)))
                    .child(widgets::btn("org-projects", "Projects", Act::Go(Route::Projects)))
                    .when(member, |d| d.child(widgets::btn("org-leave", "Leave…", Act::Url(format!("{}/settings/organizations", crate::api::WEB))))),
            )
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    pub fn team(&mut self, org: &str, slug: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("/orgs/{org}/teams/{slug}");
        let team = ready!(self.fetch(&base, cx));
        let tab_key = format!("team.tab:{org}/{slug}");
        let tab = self.choice(&tab_key, "members");
        let b = base.clone();
        let body = match tab.as_str() {
            "repos" => {
                let add = FormSpec::new("Add a repository")
                    .field(Field::text("repo", "Repository (owner/name)").required())
                    .field(Field::choice("permission", "Role", &[("pull", "Read"), ("triage", "Triage"), ("push", "Write"), ("maintain", "Maintain"), ("admin", "Admin")]))
                    .build_with({
                        let b = b.clone();
                        move |v| Ok(Req::rest("PUT", format!("{b}/repos/{}", v.s("repo").trim())).body(json!({ "permission": v.s("permission") })).ok("Repository added").inval(format!("{b}/repos")).act())
                    })
                    .act();
                let spec = ListSpec::new(format!("{base}/repos"), move |r| {
                    let full = r.s("full_name");
                    repo_row(r).danger("Remove from team", Req::rest("DELETE", format!("{b}/repos/{full}")).ok("Repository removed").inval(format!("{b}/repos")).act())
                })
                .empty("This team has no repositories.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().child(widgets::row().child(widgets::spacer()).child(widgets::go_btn("team-add-repo", "Add repository", add))).child(list).into_any_element()
            }
            "children" => {
                let o = org.to_string();
                let spec = ListSpec::new(format!("{base}/teams"), move |t| {
                    Row::new(t.s("name")).icon("people", widgets::gray()).meta(t.s("description")).open(Act::Go(Route::Team { org: o.clone(), slug: t.s("slug") }))
                })
                .empty("No child teams.");
                self.list(&spec, cx)
            }
            _ => {
                let add = FormSpec::new("Add a member")
                    .field(Field::text("login", "Login").required())
                    .field(Field::choice("role", "Role", &[("member", "Member"), ("maintainer", "Maintainer")]))
                    .build_with({
                        let b = b.clone();
                        move |v| Ok(Req::rest("PUT", format!("{b}/memberships/{}", v.s("login").trim())).body(json!({ "role": v.s("role") })).ok("Member added").inval(format!("{b}/members")).act())
                    })
                    .act();
                let spec = ListSpec::new(format!("{base}/members"), move |u| {
                    let login = u.s("login");
                    user_row(u).danger("Remove from team", Req::rest("DELETE", format!("{b}/memberships/{login}")).ok("Member removed").inval(format!("{b}/members")).act())
                })
                .empty("No members.");
                let list = self.list(&spec, cx);
                widgets::col().gap_3().child(widgets::row().child(widgets::spacer()).child(widgets::go_btn("team-add-member", "Add member", add))).child(list).into_any_element()
            }
        };
        let edit = FormSpec::new("Edit team")
            .field(Field::text("name", "Name").value(team.s("name")).required())
            .field(Field::text("description", "Description").value(team.s("description")).keep_empty())
            .field(Field::choice("privacy", "Visibility", &[("closed", "Visible"), ("secret", "Secret")]).value(team.s("privacy")))
            .rest("PATCH", base.clone())
            .ok("Team saved")
            .inval(base.clone())
            .act();
        let org_back = org.to_string();
        let delete = Req::rest("DELETE", base.clone())
            .ok("Team deleted")
            .inval(format!("/orgs/{org}/teams"))
            .then(move |hub, _, cx| hub.go(Route::Org { login: org_back.clone() }, cx))
            .act()
            .confirm("Delete this team?", "Members lose the access the team granted them.", "Delete team");
        widgets::page()
            .child(
                widgets::row()
                    .child(icon("people", 20.0, palette().text_dim))
                    .child(widgets::title(team.s("name")))
                    .child(widgets::tag(team.s("privacy"), palette().text_dim))
                    .child(widgets::spacer())
                    .child(widgets::btn("edit-team", "Edit", edit))
                    .child(widgets::danger("delete-team", "Delete", delete)),
            )
            .when(!team.s("description").is_empty(), |d| d.child(widgets::dim(team.s("description"))))
            .child(widgets::tabs(
                "team",
                vec![
                    widgets::TabItem::new("Members", "person", tab == "members", Act::choose(&tab_key, "members")).count(Some(team.i("members_count"))),
                    widgets::TabItem::new("Repositories", "repo", tab == "repos", Act::choose(&tab_key, "repos")).count(Some(team.i("repos_count"))),
                    widgets::TabItem::new("Child teams", "people", tab == "children", Act::choose(&tab_key, "children")),
                ],
            ))
            .child(body)
            .into_any_element()
    }
}
