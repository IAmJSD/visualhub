//! A Bitbucket Data Center repository's settings: its details, who has
//! access, webhooks and deploy keys. They're asked for as GitHub's paths,
//! which the client answers from the instance (`src/datacenter/`); what
//! the instance keeps elsewhere (branch permissions, merge checks) opens
//! on its site.

use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Req, Route};
use crate::json::{enc, Json as _};
use crate::resource::{ListSpec, Row};
use crate::ui::palette;
use crate::widgets::{self, rgb};
use gpui::{div, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::{json, Value};

const PERMISSIONS: [(&str, &str); 3] = [("pull", "Read"), ("push", "Write"), ("admin", "Admin")];

impl Hub {
    pub fn dc_repo_settings(
        &mut self,
        repo: &str,
        info: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_key = format!("settings.view:{repo}");
        let view = self.choice(&view_key, "general");
        let names = [
            ("general", "General"),
            ("access", "Access"),
            ("hooks", "Webhooks"),
            ("keys", "Deploy keys"),
        ];
        let chips = widgets::chips(
            names
                .iter()
                .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
                .collect(),
        );
        let body = match view.as_str() {
            "access" => self.dc_access(repo, cx),
            "hooks" => self.dc_hooks(repo, cx),
            "keys" => self.dc_deploy_keys(repo, cx),
            _ => self.dc_general(repo, info),
        };
        let site = format!(
            "{}/settings",
            crate::forge::datacenter_repo_web(repo, &crate::forge::web())
        );
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(chips)
                    .child(widgets::spacer())
                    .child(widgets::btn(
                        "dc-settings-site",
                        "Branch permissions and more on the site",
                        Act::Url(site),
                    )),
            )
            .child(body)
            .into_any_element()
    }

    fn dc_general(&mut self, repo: &str, info: &Value) -> AnyElement {
        let p = palette();
        let api = format!("/repos/{repo}");
        let edit = FormSpec::new("Repository details")
            .width(640.0)
            .field(
                Field::text("description", "Description")
                    .value(info.s("description"))
                    .keep_empty(),
            )
            .field(Field::bool("private", "Private", info.b("private")))
            .field(Field::text("default_branch", "Default branch").value(info.s("default_branch")))
            .rest("PATCH", api.clone())
            .ok("Settings saved")
            .inval(api.clone())
            .act();
        let rename = FormSpec::new("Rename repository")
            .danger()
            .note("Its address changes with its name; clones need their remote updated.")
            .field(Field::text("name", "Name").value(info.s("name")).required())
            .rest("PATCH", api.clone())
            .ok("Repository renamed")
            .then(|hub, value, cx| {
                hub.go(
                    Route::Repo {
                        repo: value.s("full_name"),
                        tab: crate::hub::RepoTab::Settings,
                    },
                    cx,
                )
            })
            .act();
        let delete = FormSpec::new(format!("Delete {repo}"))
            .submit("Delete repository")
            .danger()
            .note("This deletes the repository with its pull requests.")
            .field(Field::text("confirm", format!("Type “{repo}” to confirm").as_str()).required())
            .build_with({
                let (repo, api) = (repo.to_string(), api.clone());
                move |v| {
                    if v.s("confirm").trim() != repo {
                        return Err("The name does not match.".into());
                    }
                    Ok(Req::rest("DELETE", api.clone())
                        .ok("Repository deleted")
                        .inval("/user/repos")
                        .then(|hub, _, cx| hub.go(Route::Repos, cx))
                        .act())
                }
            })
            .act();
        let row = |title: &str, text: &str, button: AnyElement| {
            widgets::row()
                .p_4()
                .border_b_1()
                .border_color(rgb(p.divider))
                .child(
                    widgets::col()
                        .gap_0()
                        .flex_1()
                        .child(widgets::h3(title.to_string()))
                        .child(widgets::dim(text.to_string())),
                )
                .child(button)
        };
        widgets::col()
            .gap_4()
            .child(
                widgets::card()
                    .p_4()
                    .gap_2()
                    .child(widgets::h2("General"))
                    .child(widgets::dim(format!(
                        "{}  ·  {}  ·  project {}",
                        info.s("full_name"),
                        info.s("visibility"),
                        info.s("project.name")
                    )))
                    .child(div().child(widgets::primary(
                        "dc-edit-settings",
                        "Edit settings",
                        edit,
                    ))),
            )
            .child(widgets::h2("Danger zone"))
            .child(
                widgets::card()
                    .border_color(rgb(widgets::red()))
                    .child(row(
                        "Rename",
                        "Change the repository's name, and with it its address.",
                        widgets::danger("dc-rename", "Rename…", rename).into_any_element(),
                    ))
                    .child(row(
                        "Delete this repository",
                        "There's no going back.",
                        widgets::danger("dc-delete", "Delete…", delete).into_any_element(),
                    )),
            )
            .into_any_element()
    }

    /// People with access granted to them on the repository itself.
    fn dc_access(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("/repos/{repo}/collaborators");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |u| {
            let login = u.s("login");
            let name = if u.s("name").is_empty() {
                login.clone()
            } else {
                u.s("name")
            };
            let path = format!("{b}/{}", enc(&login));
            Row::new(name.clone())
                .avatar(u.s("avatar_url"))
                .meta(format!("{login}  ·  {} access", u.s("role_name")))
                .action(
                    "Change permission…",
                    FormSpec::new(format!("Access for {name}"))
                        .field(Field::choice("permission", "Permission", &PERMISSIONS))
                        .rest("PUT", path.clone())
                        .ok("Permission changed")
                        .inval(b.clone())
                        .act(),
                )
                .danger(
                    "Remove",
                    Req::rest("DELETE", path)
                        .ok("Access removed")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            format!("Remove {name}?"),
                            "They keep whatever access their project and groups give them.",
                            "Remove",
                        ),
                )
        })
        .empty("No one has access granted on this repository itself.");
        let add = FormSpec::new("Give someone access")
            .submit("Give access")
            .field(Field::text("login", "Username").required())
            .field(Field::choice("permission", "Permission", &PERMISSIONS).value("push"))
            .build_with(move |v| {
                Ok(
                    Req::rest("PUT", format!("{base}/{}", enc(v.s("login").trim())))
                        .body(json!({ "permission": v.s("permission") }))
                        .ok("Access given")
                        .inval(base.clone())
                        .act(),
                )
            })
            .act();
        let people = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::h3("People"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("dc-add-access", "Give access", add)),
            )
            .child(people)
            .into_any_element()
    }

    fn dc_hooks(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("/repos/{repo}/hooks");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |h| {
            let id = h.i("id");
            let events: Vec<String> = h.list("events").iter().map(|e| e.s("")).collect();
            Row::new(h.s("config.url"))
                .icon("webhook", widgets::gray())
                .meta(format!(
                    "{}  ·  {}",
                    if h.b("active") { "active" } else { "inactive" },
                    events.join(", ")
                ))
                .action(
                    "Test",
                    Req::rest("POST", format!("{b}/{id}/tests"))
                        .ok("Test sent")
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{id}"))
                        .ok("Webhook deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete this webhook?",
                            "It stops hearing about the repository.",
                            "Delete",
                        ),
                )
        })
        .empty("No webhooks.");
        let add = FormSpec::new("Add webhook")
            .field(Field::text("url", "URL").required())
            .field(Field::text("secret", "Secret").keep_empty())
            .rest("POST", base.clone())
            .map(|_, v| json!({ "config": { "url": v.s("url"), "secret": v.s("secret") }, "active": true }))
            .ok("Webhook added")
            .inval(base.clone())
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::dim("Pushes, and pull requests opened, changed, merged, declined and commented on."))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("dc-add-hook", "Add webhook", add)),
            )
            .child(list)
            .into_any_element()
    }

    fn dc_deploy_keys(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("/repos/{repo}/keys");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |k| {
            Row::new(k.s("title"))
                .icon("key", widgets::gray())
                .meta(format!(
                    "{}  ·  {}",
                    if k.b("read_only") {
                        "read"
                    } else {
                        "read and write"
                    },
                    crate::json::clip(&k.s("key"), 48)
                ))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{}", k.i("id")))
                        .ok("Key deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete this key?",
                            "Anything using it will lose access.",
                            "Delete",
                        ),
                )
        })
        .empty("No deploy keys.");
        let add = FormSpec::new("Add deploy key")
            .field(Field::text("title", "Title").required())
            .field(
                Field::multiline("key", "Key")
                    .required()
                    .hint("Begins with ssh-ed25519, ssh-rsa or ecdsa-sha2-…"),
            )
            .field(Field::bool("write", "Allow pushes", false))
            .rest("POST", base.clone())
            .map(|_, v| json!({ "title": v.s("title"), "key": v.s("key"), "read_only": !v.b("write") }))
            .ok("Key added")
            .inval(base.clone())
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("dc-add-key", "Add deploy key", add)),
            )
            .child(list)
            .into_any_element()
    }
}
