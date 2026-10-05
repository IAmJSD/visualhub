//! A Bitbucket repository's settings: details and visibility, who has
//! access, branch restrictions, webhooks, deploy keys, and Pipelines with
//! its variables.

use super::repo_api;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Req, Route};
use crate::json::{enc, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::palette;
use crate::widgets::{self, rgb};
use gpui::{div, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::{json, Value};

const PERMISSIONS: [(&str, &str); 3] = [("read", "Read"), ("write", "Write"), ("admin", "Admin")];

/// The kinds of branch restriction, as Bitbucket names them.
const RESTRICTIONS: [(&str, &str); 8] = [
    ("push", "Only some people may push"),
    ("force", "No force pushes"),
    ("delete", "No deleting"),
    ("restrict_merges", "Only some people may merge"),
    ("require_approvals_to_merge", "Approvals needed to merge"),
    (
        "require_passing_builds_to_merge",
        "Passing builds needed to merge",
    ),
    (
        "require_no_changes_requested",
        "No changes requested to merge",
    ),
    ("require_tasks_to_be_completed", "Tasks done to merge"),
];

fn restriction_name(kind: &str) -> String {
    RESTRICTIONS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| kind.replace('_', " "))
}

/// The events a new webhook hears unless told otherwise.
const HOOK_EVENTS: &str = "repo:push, pullrequest:created, pullrequest:updated, pullrequest:fulfilled, pullrequest:rejected, pullrequest:comment_created";

impl Hub {
    pub fn bb_repo_settings(
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
            ("branches", "Branch restrictions"),
            ("hooks", "Webhooks"),
            ("keys", "Deploy keys"),
            ("pipelines", "Pipelines"),
        ];
        let chips = widgets::chips(
            names
                .iter()
                .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
                .collect(),
        );
        let api = repo_api(repo);
        let body = match view.as_str() {
            "access" => self.bb_access(&api, cx),
            "branches" => self.bb_restrictions(&api, cx),
            "hooks" => self.bb_hooks(&api, cx),
            "keys" => self.bb_deploy_keys(&api, cx),
            "pipelines" => self.bb_pipeline_settings(&api, cx),
            _ => self.bb_general(repo, info),
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    fn bb_general(&mut self, repo: &str, info: &Value) -> AnyElement {
        let p = palette();
        let api = repo_api(repo);
        let edit = FormSpec::new("Repository details")
            .width(640.0)
            .field(
                Field::text("description", "Description")
                    .value(info.s("description"))
                    .keep_empty(),
            )
            .field(
                Field::text("website", "Website")
                    .value(info.s("homepage"))
                    .keep_empty(),
            )
            .field(Field::bool("is_private", "Private", info.b("private")))
            .field(
                Field::choice(
                    "fork_policy",
                    "Forking",
                    &[
                        ("allow_forks", "Allow forks"),
                        ("no_public_forks", "Only private forks"),
                        ("no_forks", "No forks"),
                    ],
                )
                .value(info.s("fork_policy")),
            )
            .field(Field::text("mainbranch", "Main branch").value(info.s("default_branch")))
            .rest("PUT", api.clone())
            .map(|mut body, v| {
                let branch = v.s("mainbranch").trim().to_string();
                if let Some(map) = body.as_object_mut() {
                    map.remove("mainbranch");
                    if !branch.is_empty() {
                        map.insert("mainbranch".into(), json!({ "name": branch }));
                    }
                }
                body
            })
            .ok("Settings saved")
            .inval(api.clone())
            .inval(format!("/repos/{repo}"))
            .act();
        let rename = FormSpec::new("Rename repository")
            .danger()
            .note("Its address changes with its name; clones need their remote updated.")
            .field(Field::text("name", "Name").value(info.s("name")).required())
            .rest("PUT", api.clone())
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
            .note("This deletes the repository with its pull requests, pipelines and downloads.")
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
                        "bb-edit-settings",
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
                        widgets::danger("bb-rename", "Rename…", rename).into_any_element(),
                    ))
                    .child(row(
                        "Delete this repository",
                        "There's no going back.",
                        widgets::danger("bb-delete", "Delete…", delete).into_any_element(),
                    )),
            )
            .into_any_element()
    }

    /// People and groups with access, and changing it.
    fn bb_access(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let users = format!("{api}/permissions-config/users");
        let u = users.clone();
        let spec = ListSpec::new(users.clone(), move |entry| {
            let who = entry.at("user");
            let name = who.s("display_name");
            let path = format!("{u}/{}", enc(&who.s("account_id")));
            Row::new(name.clone())
                .avatar(who.s("links.avatar.href"))
                .meta(format!("{} access", entry.s("permission")))
                .action(
                    "Change permission…",
                    FormSpec::new(format!("Access for {name}"))
                        .field(
                            Field::choice("permission", "Permission", &PERMISSIONS)
                                .value(entry.s("permission")),
                        )
                        .rest("PUT", path.clone())
                        .ok("Permission changed")
                        .inval(u.clone())
                        .act(),
                )
                .danger(
                    "Remove",
                    Req::rest("DELETE", path)
                        .ok("Access removed")
                        .inval(u.clone())
                        .act()
                        .confirm(
                            format!("Remove {name}?"),
                            "They keep whatever access their workspace and groups give them.",
                            "Remove",
                        ),
                )
        })
        .empty("No one has access granted to them directly.");
        let groups = format!("{api}/permissions-config/groups");
        let g = groups.clone();
        let group_spec = ListSpec::new(groups, move |entry| {
            let name = entry.s("group.name");
            Row::new(name.clone())
                .icon("people", widgets::gray())
                .meta(format!("{} access", entry.s("permission")))
                .danger(
                    "Remove",
                    Req::rest("DELETE", format!("{g}/{}", enc(&entry.s("group.slug"))))
                        .ok("Group removed")
                        .inval(g.clone())
                        .act(),
                )
        })
        .empty("No groups have access.");
        let add = FormSpec::new("Give someone access")
            .submit("Give access")
            .field(
                Field::text("account", "Account ID")
                    .required()
                    .hint("Their Atlassian account ID, from their profile's address."),
            )
            .field(Field::choice("permission", "Permission", &PERMISSIONS).value("write"))
            .build_with(move |v| {
                Ok(
                    Req::rest("PUT", format!("{users}/{}", enc(v.s("account").trim())))
                        .body(json!({ "permission": v.s("permission") }))
                        .ok("Access given")
                        .inval(users.clone())
                        .act(),
                )
            })
            .act();
        let people = self.list(&spec, cx);
        let groups = self.list(&group_spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::h3("People"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("bb-add-access", "Give access", add)),
            )
            .child(people)
            .child(widgets::h3("Groups"))
            .child(groups)
            .into_any_element()
    }

    fn bb_restrictions(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("{api}/branch-restrictions");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |r| {
            let pattern = if r.s("branch_match_kind") == "branching_model" {
                format!("{} branches", r.s("branch_type").replace('_', " "))
            } else {
                r.s("pattern")
            };
            let value = if r.i("value") > 0 {
                format!("  ·  {}", r.i("value"))
            } else {
                String::new()
            };
            Row::new(restriction_name(&r.s("kind")))
                .icon("shield", widgets::gray())
                .meta(format!("{pattern}{value}"))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{}", r.i("id")))
                        .ok("Restriction deleted")
                        .inval(b.clone())
                        .act(),
                )
        })
        .empty("No branch restrictions.");
        let add = FormSpec::new("Add a branch restriction")
            .submit("Add restriction")
            .field(Field::choice("kind", "Restriction", &RESTRICTIONS).value("force"))
            .field(
                Field::text("pattern", "Branches")
                    .value("main")
                    .required()
                    .hint("A branch name or a glob, such as release/*."),
            )
            .field(
                Field::number("value", "How many")
                    .hint("For approvals and passing builds: how many are needed."),
            )
            .rest("POST", base.clone())
            .map(|_, v| {
                let mut body = json!({
                    "kind": v.s("kind"),
                    "branch_match_kind": "glob",
                    "pattern": v.s("pattern").trim(),
                });
                if let Ok(n) = v.s("value").trim().parse::<i64>() {
                    body["value"] = json!(n);
                }
                body
            })
            .ok("Restriction added")
            .inval(base)
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn(
                        "bb-add-restriction",
                        "Add restriction",
                        add,
                    )),
            )
            .child(list)
            .into_any_element()
    }

    fn bb_hooks(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("{api}/hooks");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |h| {
            let path = format!("{b}/{}", enc(&h.s("uuid")));
            let active = h.b("active");
            let events: Vec<String> = h.list("events").iter().map(|e| e.s("")).collect();
            Row::new(if h.s("description").is_empty() {
                h.s("url")
            } else {
                h.s("description")
            })
            .icon(
                "webhook",
                if active {
                    widgets::green()
                } else {
                    widgets::gray()
                },
            )
            .meta(format!("{}  ·  {}", h.s("url"), events.join(", ")))
            .action(
                if active { "Deactivate" } else { "Activate" },
                Req::rest("PUT", path.clone())
                    .body(json!({ "active": !active, "url": h.s("url"), "description": h.s("description"), "events": events }))
                    .ok("Webhook updated")
                    .inval(b.clone())
                    .act(),
            )
            .danger(
                "Delete",
                Req::rest("DELETE", path)
                    .ok("Webhook deleted")
                    .inval(b.clone())
                    .act()
                    .confirm("Delete this webhook?", "It stops hearing about this repository.", "Delete"),
            )
        })
        .empty("No webhooks.");
        let add = FormSpec::new("Add webhook")
            .submit("Add webhook")
            .field(Field::text("description", "Title").required())
            .field(Field::text("url", "URL").required())
            .field(
                Field::list("events", "Events")
                    .value(HOOK_EVENTS)
                    .required(),
            )
            .field(Field::bool("active", "Active", true))
            .rest("POST", base.clone())
            .ok("Webhook added")
            .inval(base)
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("bb-add-hook", "Add webhook", add)),
            )
            .child(list)
            .into_any_element()
    }

    fn bb_deploy_keys(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("{api}/deploy-keys");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |k| {
            Row::new(k.s("label"))
                .icon("key", widgets::gray())
                .meta(format!(
                    "read-only  ·  added {}  ·  {}",
                    time::ago(&k.s("created_on")),
                    crate::json::clip(&k.s("key"), 48)
                ))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{}", k.i("id")))
                        .ok("Deploy key deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete this deploy key?",
                            "Anything using it loses access.",
                            "Delete",
                        ),
                )
        })
        .empty("No deploy keys.");
        let add = FormSpec::new("Add deploy key")
            .submit("Add key")
            .field(Field::text("label", "Label").required())
            .field(Field::multiline("key", "Key").required())
            .rest("POST", base.clone())
            .ok("Deploy key added")
            .inval(base)
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("bb-add-key", "Add deploy key", add)),
            )
            .child(list)
            .into_any_element()
    }

    /// Pipelines on or off, and the variables its steps get.
    fn bb_pipeline_settings(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let config_path = format!("{api}/pipelines_config");
        let enabled = self.fetch(&config_path, cx).ready().map(|v| v.b("enabled"));
        let toggle = Req::rest("PUT", config_path.clone())
            .body(json!({ "enabled": !enabled.unwrap_or(false) }))
            .ok(if enabled == Some(true) {
                "Pipelines turned off"
            } else {
                "Pipelines turned on"
            })
            .inval(config_path)
            .act();
        let base = format!("{api}/pipelines_config/variables");
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |v| {
            let secured = v.b("secured");
            Row::new(v.s("key"))
                .icon(if secured { "lock" } else { "code" }, widgets::gray())
                .meta(if secured {
                    "secured".to_string()
                } else {
                    crate::json::clip(&v.s("value"), 60)
                })
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{}", enc(&v.s("uuid"))))
                        .ok("Variable deleted")
                        .inval(b.clone())
                        .act(),
                )
        })
        .empty("No repository variables.");
        let add = FormSpec::new("Add variable")
            .submit("Add variable")
            .field(Field::text("key", "Name").required())
            .field(Field::multiline("value", "Value").required())
            .field(Field::bool(
                "secured",
                "Secured: hidden in logs and here",
                true,
            ))
            .rest("POST", base.clone())
            .ok("Variable added")
            .inval(base)
            .act();
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(widgets::card().child(widgets::setting_row(
                "Pipelines",
                "Run the steps in bitbucket-pipelines.yml on pushes and pull requests.",
                widgets::switch("bb-pipelines-on", enabled.unwrap_or(false), toggle),
                true,
            )))
            .child(
                widgets::row()
                    .child(widgets::h3("Repository variables"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("bb-add-variable", "Add variable", add)),
            )
            .child(list)
            .into_any_element()
    }
}
