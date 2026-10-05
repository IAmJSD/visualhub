//! A person and a workspace on Bitbucket. Bitbucket keeps no follows,
//! stars or contribution calendar, and people have no page of their own
//! on its site, so a person is their name and the pull requests they've
//! opened, and a workspace its repositories and members.

use crate::hub::{on, Act, Hub, Route};
use crate::json::Json as _;
use crate::ready;
use crate::resource::ListSpec;
use crate::screens::common::{repo_row, user_row};
use crate::widgets;
use gpui::{
    div, px, AnyElement, Context, FontWeight, IntoElement as _, ParentElement as _, Styled as _,
};

impl Hub {
    pub fn bb_user(&mut self, login: &str, cx: &mut Context<Self>) -> AnyElement {
        let user = ready!(self.fetch(&format!("/users/{login}"), cx));
        // A workspace's slug reads as a login too.
        if user.s("type") == "Organization" {
            return self.bb_workspace(login, cx);
        }
        let is_me = user.s("login") == self.login();
        let avatar = self.avatar_of(&user, 96.0, cx);
        let tab_key = format!("user.tab:{login}");
        let tab = self.choice(&tab_key, if is_me { "repos" } else { "pulls" });
        let mut tabs = vec![widgets::TabItem::new(
            crate::forge::prs_title(),
            "pr",
            tab == "pulls",
            Act::choose(&tab_key, "pulls"),
        )];
        if is_me {
            tabs.insert(
                0,
                widgets::TabItem::new(
                    "Your repositories",
                    "repo",
                    tab == "repos",
                    Act::choose(&tab_key, "repos"),
                ),
            );
        }
        let body = if tab == "repos" && is_me {
            let spec =
                ListSpec::new("/user/repos?sort=pushed", repo_row).empty("No repositories yet.");
            self.list(&spec, cx)
        } else {
            let spec =
                crate::screens::issues::search_spec(&format!("is:pr author:\"{login}\""), true)
                    .empty("No pull requests by them in your repositories.");
            self.list(&spec, cx)
        };
        widgets::page()
            .child(
                widgets::row()
                    .gap_4()
                    .child(avatar)
                    .child(
                        widgets::col()
                            .gap_1()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(user.s("name")),
                            )
                            .child(widgets::dim(user.s("login"))),
                    )
                    .child(if is_me {
                        widgets::btn("edit-profile", "Edit profile", Act::Go(Route::Settings))
                    } else {
                        widgets::btn(
                            "copy-account",
                            "Copy account ID",
                            Act::Copy(user.s("account_id")),
                        )
                    }),
            )
            .child(widgets::tabs("user", tabs))
            .child(body)
            .into_any_element()
    }

    pub fn bb_workspace(&mut self, slug: &str, cx: &mut Context<Self>) -> AnyElement {
        let ws = ready!(self.fetch(&format!("/orgs/{slug}"), cx));
        let avatar = self.avatar_of(&ws, 64.0, cx);
        let tab_key = format!("org.tab:{slug}");
        let tab = self.choice(&tab_key, "repos");
        let tabs = vec![
            widgets::TabItem::new(
                "Repositories",
                "repo",
                tab == "repos",
                Act::choose(&tab_key, "repos"),
            ),
            widgets::TabItem::new(
                "Members",
                "people",
                tab == "members",
                Act::choose(&tab_key, "members"),
            ),
        ];
        let body = if tab == "members" {
            let spec = ListSpec::new(format!("/orgs/{slug}/members"), user_row)
                .empty("No members you can see.")
                .people();
            self.list(&spec, cx)
        } else {
            let spec = ListSpec::new(format!("/orgs/{slug}/repos?sort=pushed"), repo_row)
                .empty("No repositories you can see.");
            self.list(&spec, cx)
        };
        let new_repo = Act::Go(Route::NewRepo {
            owner: Some(slug.to_string()),
        });
        widgets::page()
            .child(
                widgets::row()
                    .gap_4()
                    .child(avatar)
                    .child(
                        widgets::col()
                            .gap_1()
                            .flex_1()
                            .child(widgets::title(ws.s("name")))
                            .child(widgets::dim(format!("{slug}  ·  workspace"))),
                    )
                    .child(widgets::go_btn("ws-new-repo", "New repository", new_repo))
                    .child(
                        div().child(
                            crate::ui::Link::new("ws-web", crate::forge::open_on())
                                .on_click(on(Act::Url(ws.s("html_url")))),
                        ),
                    ),
            )
            .child(widgets::tabs("org", tabs))
            .child(body)
            .into_any_element()
    }
}
