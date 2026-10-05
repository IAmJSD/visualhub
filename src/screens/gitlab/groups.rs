//! A GitLab group, where GitHub has an organization: its projects,
//! subgroups and members, and for those who run it, invitations, CI/CD
//! variables, packages and its settings.

use super::{group_api, role};
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Req, Route};
use crate::json::Json as _;
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::screens::common::repo_row;
use crate::widgets;
use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};

impl Hub {
    pub fn gl_group(&mut self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let api = group_api(path);
        let group = ready!(self.fetch(&api, cx));
        let me = self.me.i("id");
        let level = self
            .fetch(&format!("{api}/members/all/{me}"), cx)
            .ready()
            .map(|m| m.i("access_level"))
            .unwrap_or(0);
        let (owner, maintainer) = (level >= 50, level >= 40);
        let tab_key = format!("group.tab:{path}");
        let tab = self.choice(&tab_key, "projects");
        let mut names = vec![
            ("projects", "Projects", "repo"),
            ("subgroups", "Subgroups", "org"),
            ("members", "Members", "people"),
            ("packages", "Packages", "package"),
        ];
        if owner {
            names.push(("invitations", "Invitations", "mail"));
        }
        if maintainer {
            names.push(("variables", "CI/CD variables", "code"));
        }
        let tabs = widgets::tabs(
            "group",
            names
                .iter()
                .map(|(v, l, i)| widgets::TabItem::new(*l, i, tab == *v, Act::choose(&tab_key, *v)))
                .collect(),
        );
        let body = match tab.as_str() {
            "subgroups" => {
                let spec = ListSpec::new(format!("{api}/subgroups"), |g| {
                    Row::new(g.s("full_path"))
                        .avatar(crate::forge::absolute(&g.s("avatar_url")))
                        .meta(format!("{}  ·  {}", g.s("visibility"), g.s("description")))
                        .open(Act::Go(Route::Org {
                            login: g.s("full_path"),
                        }))
                })
                .empty("No subgroups.");
                self.list(&spec, cx)
            }
            "members" => self.gl_members(&api, owner, cx),
            "invitations" => self.gl_invitations(&api, cx),
            "variables" => self.gl_variables(&api, cx),
            "packages" => self.gl_packages(&api, cx),
            _ => {
                // Subgroups have their own tab.
                let spec = ListSpec::new(
                    format!("/orgs/{path}/repos?sort=pushed&include_subgroups=false"),
                    repo_row,
                )
                .empty("No projects in this group.");
                self.list(&spec, cx)
            }
        };
        let settings = FormSpec::new(format!("{} settings", group.s("full_name")))
            .width(600.0)
            .field(
                Field::text("name", "Group name")
                    .value(group.s("name"))
                    .required(),
            )
            .field(
                Field::text("description", "Description")
                    .value(group.s("description"))
                    .keep_empty(),
            )
            .field(
                Field::choice(
                    "visibility",
                    "Visibility",
                    &[
                        ("private", "Private"),
                        ("internal", "Internal"),
                        ("public", "Public"),
                    ],
                )
                .value(group.s("visibility")),
            )
            .rest("PUT", api.clone())
            .ok("Group saved")
            .inval(api.clone())
            .act();
        let leave = Req::rest("DELETE", format!("{api}/members/{me}"))
            .ok("You left the group")
            .inval("/user/orgs")
            .then(|hub, _, cx| hub.go(crate::hub::Route::Home, cx))
            .act()
            .confirm(
                format!("Leave {}?", group.s("full_name")),
                "You lose the access the group gave you.",
                "Leave",
            );
        let avatar = self.avatar_of(&serde_json::json!({ "login": group.s("full_name"), "avatar_url": crate::forge::absolute(&group.s("avatar_url")) }), 72.0, cx);
        let parent = group
            .s("full_path")
            .rsplit_once('/')
            .map(|(p, _)| p.to_string());
        widgets::page()
            .child(
                widgets::row()
                    .gap_4()
                    .child(avatar)
                    .child(
                        widgets::col()
                            .gap_1()
                            .flex_1()
                            .when_some(parent, |d, parent| {
                                d.child(
                                    crate::ui::Link::new("group-parent", format!("{parent} /"))
                                        .text_size(gpui::px(12.0))
                                        .on_click(crate::hub::on(Act::Go(Route::Org {
                                            login: parent,
                                        }))),
                                )
                            })
                            .child(widgets::title(group.s("name")))
                            .when(!group.s("description").is_empty(), |d| {
                                d.child(widgets::dim(group.s("description")))
                            })
                            .child(
                                widgets::row()
                                    .gap_4()
                                    .child(widgets::tag(group.s("visibility"), widgets::gray()))
                                    .when(level > 0, |d| {
                                        d.child(widgets::dim(format!(
                                            "You're a {}",
                                            role(level).to_lowercase()
                                        )))
                                    }),
                            ),
                    )
                    .when(level >= 30, |d| {
                        d.child(widgets::go_btn(
                            "group-new-project",
                            "New project",
                            Act::Go(Route::NewRepo {
                                owner: Some(path.to_string()),
                            }),
                        ))
                    })
                    .when(maintainer, |d| {
                        d.child(widgets::btn(
                            "group-new-subgroup",
                            "New subgroup",
                            Act::Url(format!(
                                "{}/groups/new?parent_id={}",
                                crate::forge::web(),
                                group.i("id")
                            )),
                        ))
                    })
                    .when(owner, |d| {
                        d.child(widgets::btn("group-settings", "Settings", settings))
                    })
                    .when(level > 0, |d| {
                        d.child(widgets::btn("group-leave", "Leave…", leave))
                    }),
            )
            .child(tabs)
            .child(body)
            .into_any_element()
    }
}
