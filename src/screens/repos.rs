//! Your repositories, stars and watches, invitations, and the New
//! repository form.

use super::common::repo_row;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, MenuEntry, Req, Route, RepoTab};
use crate::json::Json as _;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};

/// Create a repository for you, or in `org`.
pub fn new_repo_form(org: Option<&str>) -> Act {
    let path = match org {
        Some(org) => format!("/orgs/{org}/repos"),
        None => "/user/repos".to_string(),
    };
    FormSpec::new(match org {
        Some(org) => format!("Create a repository in {org}"),
        None => "Create a new repository".into(),
    })
    .submit("Create repository")
    .width(600.0)
    .field(Field::text("name", "Repository name").required())
    .field(Field::text("description", "Description"))
    .field(Field::text("homepage", "Website"))
    .field(if org.is_some() {
        Field::choice("visibility", "Visibility", &[("public", "Public"), ("private", "Private"), ("internal", "Internal")])
    } else {
        Field::choice("visibility", "Visibility", &[("public", "Public"), ("private", "Private")])
    })
    .field(Field::bool("auto_init", "Add a README file", true))
    .field(Field::text("gitignore_template", "Add .gitignore").hint("A template name such as Rust, Node, Python; empty for none."))
    .field(Field::text("license_template", "Choose a license").hint("A license key such as mit, apache-2.0, gpl-3.0; empty for none."))
    .field(Field::bool("is_template", "Template repository", false))
    .rest("POST", path)
    .map(|mut body, values| {
        if values.s("visibility") == "private" {
            body["private"] = serde_json::json!(true);
        }
        body
    })
    .ok("Repository created")
    .inval("/user/repos")
    .then(|hub, value, cx| {
        hub.go(
            Route::Repo {
                repo: value.s("full_name"),
                tab: RepoTab::Code,
            },
            cx,
        )
    })
    .act()
}

impl Hub {
    pub fn repos(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.choice("repos.tab", "yours");
        let kind = self.choice("repos.kind", "all");
        let sort = self.choice("repos.sort", "pushed");
        let tabs = widgets::chips(vec![
            ("Your repositories".into(), tab == "yours", Act::choose("repos.tab", "yours")),
            ("Stars".into(), tab == "starred", Act::choose("repos.tab", "starred")),
            ("Watching".into(), tab == "watching", Act::choose("repos.tab", "watching")),
            ("Invitations".into(), tab == "invites", Act::choose("repos.tab", "invites")),
        ]);
        let body = match tab.as_str() {
            "starred" => {
                let spec = ListSpec::new(format!("/user/starred?sort={}", if sort == "pushed" { "updated" } else { "created" }), |r| {
                    let full = r.s("full_name");
                    repo_row(r).action(
                        "Unstar",
                        Req::rest("DELETE", format!("/user/starred/{full}")).ok("Unstarred").inval("/user/starred").act(),
                    )
                })
                .empty("You haven't starred any repositories yet.");
                self.list(&spec, cx)
            }
            "watching" => {
                let spec = ListSpec::new("/user/subscriptions", |r| {
                    let full = r.s("full_name");
                    repo_row(r).action(
                        "Unwatch",
                        Req::rest("DELETE", format!("/repos/{full}/subscription")).ok("Unwatched").inval("/user/subscriptions").act(),
                    )
                })
                .empty("You aren't watching any repositories.");
                self.list(&spec, cx)
            }
            "invites" => {
                let spec = ListSpec::new("/user/repository_invitations", |i| {
                    let id = i.i("id");
                    Row::new(i.s("repository.full_name"))
                        .icon("mail", widgets::gray())
                        .meta(format!("{} invited you {} with {} access", i.s("inviter.login"), time::ago(&i.s("created_at")), i.s("permissions")))
                        .action(
                            "Accept",
                            Req::rest("PATCH", format!("/user/repository_invitations/{id}")).ok("Invitation accepted").inval("/user/repo").act(),
                        )
                        .danger(
                            "Decline",
                            Req::rest("DELETE", format!("/user/repository_invitations/{id}")).ok("Invitation declined").inval("/user/repository_invitations").act(),
                        )
                        .inline()
                })
                .empty("No pending repository invitations.");
                self.list(&spec, cx)
            }
            _ => {
                let path = match kind.as_str() {
                    "all" => format!("/user/repos?sort={sort}&affiliation=owner,collaborator,organization_member"),
                    "owner" => format!("/user/repos?sort={sort}&affiliation=owner"),
                    k => format!("/user/repos?sort={sort}&type={k}"),
                };
                let spec = ListSpec::new(path, repo_row).empty("No repositories.");
                self.list(&spec, cx)
            }
        };
        let sorts = [("pushed", "Last pushed"), ("updated", "Last updated"), ("created", "Newest"), ("full_name", "Name")];
        let kinds = [("all", "All"), ("owner", "Owned by you"), ("public", "Public"), ("private", "Private"), ("member", "Member")];
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title("Repositories"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-repo", "New repository", new_repo_form(None))),
            )
            .child(
                widgets::row()
                    .flex_wrap()
                    .child(tabs)
                    .child(widgets::spacer())
                    .when(tab == "yours", |d| {
                        d.child(widgets::btn(
                            "repo-kind",
                            format!("Type: {} ▾", kinds.iter().find(|k| k.0 == kind).map(|k| k.1).unwrap_or("All")),
                            Act::menu(kinds.iter().map(|(v, l)| MenuEntry::check(*l, kind == *v, Act::choose("repos.kind", *v))).collect()),
                        ))
                    })
                    .child(widgets::btn(
                        "repo-sort",
                        format!("Sort: {} ▾", sorts.iter().find(|s| s.0 == sort).map(|s| s.1).unwrap_or("Last pushed")),
                        Act::menu(sorts.iter().map(|(v, l)| MenuEntry::check(*l, sort == *v, Act::choose("repos.sort", *v))).collect()),
                    )),
            )
            .child(body)
            .into_any_element()
    }
}
