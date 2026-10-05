//! Codespaces and packages.

use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, RepoTab, Req, Route};
use crate::json::{enc, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::json;

const PACKAGE_TYPES: [(&str, &str); 6] = [
    ("container", "Containers"),
    ("npm", "npm"),
    ("maven", "Maven"),
    ("rubygems", "RubyGems"),
    ("nuget", "NuGet"),
    ("docker", "Docker (legacy)"),
];

impl Hub {
    pub fn codespaces(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let create = FormSpec::new("Create a codespace")
            .submit("Create codespace")
            .field(Field::text("repo", "Repository (owner/name)").required())
            .field(Field::text("ref", "Branch").hint("Empty for the default branch."))
            .field(
                Field::text("machine", "Machine type")
                    .hint("e.g. basicLinux32gb, standardLinux32gb; empty for the default."),
            )
            .field(
                Field::text("location", "Region")
                    .hint("EastUs, WestUs2, WestEurope, SoutheastAsia; empty for nearest."),
            )
            .build_with(|v| {
                let mut body = json!({});
                for key in ["ref", "machine", "location"] {
                    if !v.s(key).trim().is_empty() {
                        body[key] = json!(v.s(key).trim());
                    }
                }
                Ok(
                    Req::rest("POST", format!("/repos/{}/codespaces", v.s("repo").trim()))
                        .body(body)
                        .ok("Codespace is being created")
                        .inval("/user/codespaces")
                        .then(|_, value, cx| cx.open_url(&value.s("web_url")))
                        .act(),
                )
            })
            .act();
        let spec = ListSpec::new("/user/codespaces", |c| {
            let name = c.s("name");
            let base = format!("/user/codespaces/{name}");
            let state = c.s("state");
            let running = state == "Available";
            let mut row = Row::new(if c.s("display_name").is_empty() {
                name.clone()
            } else {
                c.s("display_name")
            })
            .icon(
                "codespace",
                if running {
                    widgets::green()
                } else {
                    widgets::gray()
                },
            )
            .meta(format!(
                "{}  ·  {}  ·  {}  ·  {}  ·  last used {}",
                c.s("repository.full_name"),
                c.s("git_status.ref"),
                state,
                c.s("machine.display_name"),
                time::ago(&c.s("last_used_at"))
            ))
            .open(Act::Url(c.s("web_url")))
            .action("Open in browser", Act::Url(c.s("web_url")))
            .action(
                "Open in VS Code",
                Act::Url(format!(
                    "vscode://github.codespaces/connect?name={}",
                    enc(&name)
                )),
            );
            row = if running {
                row.action(
                    "Stop",
                    Req::rest("POST", format!("{base}/stop"))
                        .ok("Stopping codespace")
                        .inval("/user/codespaces")
                        .act(),
                )
            } else {
                row.action(
                    "Start",
                    Req::rest("POST", format!("{base}/start"))
                        .ok("Starting codespace")
                        .inval("/user/codespaces")
                        .act(),
                )
            };
            row.action(
                "Rename…",
                FormSpec::new("Rename codespace")
                    .field(
                        Field::text("display_name", "Name")
                            .value(c.s("display_name"))
                            .required(),
                    )
                    .rest("PATCH", base.clone())
                    .ok("Renamed")
                    .inval("/user/codespaces")
                    .act(),
            )
            .action(
                "Change machine type…",
                FormSpec::new("Change machine type")
                    .field(
                        Field::text("machine", "Machine type")
                            .value(c.s("machine.name"))
                            .required(),
                    )
                    .rest("PATCH", base.clone())
                    .ok("Machine type will change on next start")
                    .inval("/user/codespaces")
                    .act(),
            )
            .action(
                "Export changes to a branch",
                Req::rest("POST", format!("{base}/exports"))
                    .ok("Export started")
                    .act(),
            )
            .action(
                "Repository",
                Act::Go(Route::Repo {
                    repo: c.s("repository.full_name"),
                    tab: RepoTab::Code,
                }),
            )
            .danger(
                "Delete",
                Req::rest("DELETE", base)
                    .ok("Codespace deleted")
                    .inval("/user/codespaces")
                    .act()
                    .confirm(
                        "Delete this codespace?",
                        "Unpushed changes in it are lost.",
                        "Delete",
                    ),
            )
        })
        .items("codespaces")
        .empty("You don't have any codespaces.");
        let list = self.list(&spec, cx);
        let secrets = ListSpec::new("/user/codespaces/secrets", |s| {
            let name = s.s("name");
            Row::new(name.clone())
                .icon("lock", widgets::gray())
                .meta(format!(
                    "{} repositories  ·  updated {}",
                    s.s("visibility"),
                    time::ago(&s.s("updated_at"))
                ))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("/user/codespaces/secrets/{name}"))
                        .ok("Secret deleted")
                        .inval("/user/codespaces/secrets")
                        .act(),
                )
        })
        .items("secrets")
        .empty("No codespaces secrets.");
        let secrets = self.list(&secrets, cx);
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title("Codespaces"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-codespace", "New codespace", create)),
            )
            .child(list)
            .child(widgets::h2("Codespaces secrets"))
            .child(secrets)
            .into_any_element()
    }

    pub fn packages(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let list = self.package_list("/user/packages", cx);
        widgets::page()
            .child(widgets::title("Packages"))
            .child(list)
            .into_any_element()
    }

    /// Packages under `base` (`/user/packages` or `/orgs/{org}/packages`),
    /// by type, with their versions.
    pub fn package_list(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let type_key = format!("packages.type:{base}");
        let kind = self.choice(&type_key, "container");
        let open_key = format!("packages.open:{base}");
        let open = self.choice(&open_key, "");
        let b = base.to_string();
        let k = kind.clone();
        let ok = open_key.clone();
        let spec = ListSpec::new(format!("{base}?package_type={kind}"), move |p| {
            let name = p.s("name");
            let path = format!("{b}/{k}/{}", enc(&name));
            let mut row = Row::new(name.clone())
                .icon("package", widgets::gray())
                .meta(format!(
                    "{}  ·  {} versions  ·  updated {}{}",
                    p.s("visibility"),
                    p.i("version_count"),
                    time::ago(&p.s("updated_at")),
                    if p.has("repository") {
                        format!("  ·  {}", p.s("repository.full_name"))
                    } else {
                        String::new()
                    }
                ))
                .open(Act::choose(ok.clone(), name.clone()))
                .action("Show versions", Act::choose(ok.clone(), name.clone()))
                .action("Open on GitHub", Act::Url(p.s("html_url")));
            if p.has("repository") {
                row = row.action(
                    "Repository",
                    Act::Go(Route::Repo {
                        repo: p.s("repository.full_name"),
                        tab: RepoTab::Code,
                    }),
                );
            }
            row.danger(
                "Delete package",
                Req::rest("DELETE", path).ok("Package deleted").inval(b.clone()).act().confirm(
                    format!("Delete {name}?"),
                    "All of its versions are deleted. You can restore it within 30 days on GitHub.",
                    "Delete",
                ),
            )
        })
        .empty("No packages of this type.");
        let list = self.list(&spec, cx);
        let mut col = widgets::col()
            .gap_3()
            .child(widgets::chips(
                PACKAGE_TYPES
                    .iter()
                    .map(|(v, l)| (l.to_string(), kind == *v, Act::choose(&type_key, *v)))
                    .collect(),
            ))
            .child(list);
        if !open.is_empty() {
            let path = format!("{base}/{kind}/{}/versions", enc(&open));
            let p2 = path.clone();
            let versions = ListSpec::new(path.clone(), move |v| {
                let tags: Vec<String> = v
                    .list("metadata.container.tags")
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect();
                Row::new(v.s("name"))
                    .icon("tag", widgets::gray())
                    .meta(format!(
                        "{}  ·  {}",
                        if tags.is_empty() {
                            "untagged".to_string()
                        } else {
                            tags.join(", ")
                        },
                        time::ago(&v.s("updated_at"))
                    ))
                    .open(Act::Url(v.s("html_url")))
                    .danger(
                        "Delete version",
                        Req::rest("DELETE", format!("{p2}/{}", v.i("id")))
                            .ok("Version deleted")
                            .inval(p2.clone())
                            .act()
                            .confirm(
                                "Delete this version?",
                                "It can be restored within 30 days on GitHub.",
                                "Delete",
                            ),
                    )
            })
            .empty("No versions.");
            let versions = self.list(&versions, cx);
            col = col
                .child(
                    widgets::row()
                        .child(widgets::h2(format!("Versions of {open}")))
                        .child(widgets::spacer())
                        .child(widgets::btn(
                            "close-versions",
                            "Close",
                            Act::choose(&open_key, ""),
                        )),
                )
                .child(versions);
        }
        col.into_any_element()
    }
}
