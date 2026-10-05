//! A GitLab project's settings, and the members, invitations and CI/CD
//! variables that projects and groups both have.

use super::{project_api, role, ROLES};
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, RepoTab, Req, Route};
use crate::json::{enc, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::palette;
use crate::widgets::{self, rgb};
use gpui::{div, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::{json, Value};

/// Who may push or merge to a protected branch.
const BRANCH_ACCESS: [(&str, &str); 3] = [
    ("40", "Maintainers"),
    ("30", "Developers and maintainers"),
    ("0", "No one"),
];

/// A role picker's value as a number for the body.
fn level(v: &crate::form::FormValues, key: &str) -> i64 {
    v.s(key).parse().unwrap_or(30)
}

impl Hub {
    /// A project's or group's direct members, and adding more: by login,
    /// or by email as an invitation.
    pub fn gl_members(&mut self, base: &str, manage: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("{base}/members");
        let invite = {
            let base = base.to_string();
            FormSpec::new("Add a member")
                .submit("Add member")
                .field(Field::text("who", "Username or email").required())
                .field(Field::choice("access_level", "Role", &ROLES).value("30"))
                .field(Field::text("expires_at", "Access expires").hint("YYYY-MM-DD, optional."))
                .build_with(move |v| {
                    let (base, who, level, expires) = (
                        base.clone(),
                        v.s("who").trim().to_string(),
                        level(v, "access_level"),
                        v.s("expires_at").trim().to_string(),
                    );
                    let inval = format!("{base}/");
                    Ok(Req::custom(move |client| {
                        let mut body = json!({ "access_level": level });
                        if !expires.is_empty() {
                            body["expires_at"] = json!(expires);
                        }
                        if who.contains('@') {
                            body["email"] = json!(who);
                            return client.json(
                                "POST",
                                &format!("{base}/invitations"),
                                Some(&body),
                            );
                        }
                        let found = client.json(
                            "GET",
                            &format!(
                                "/api/v4/users?username={}",
                                enc(who.trim_start_matches('@'))
                            ),
                            None,
                        )?;
                        let id =
                            found.list("").first().map(|u| u.i("id")).ok_or_else(|| {
                                anyhow::anyhow!("No GitLab user is called {who}.")
                            })?;
                        body["user_id"] = json!(id);
                        client.json("POST", &format!("{base}/members"), Some(&body))
                    })
                    .ok("Member added")
                    .inval(inval)
                    .act())
                })
                .act()
        };
        let b = base.to_string();
        let spec = ListSpec::new(path, move |m| {
            let id = m.i("id");
            let login = m.s("username");
            let member = format!("{b}/members/{id}");
            let mut row = Row::new(login.clone())
                .avatar(crate::forge::absolute(&m.s("avatar_url")))
                .meta(format!(
                    "{}  ·  {}{}",
                    role(m.i("access_level")),
                    m.s("name"),
                    if m.has("expires_at") {
                        format!("  ·  until {}", m.s("expires_at"))
                    } else {
                        String::new()
                    }
                ))
                .open(Act::Go(Route::User {
                    login: login.clone(),
                }));
            if manage {
                row = row
                    .action(
                        "Change role…",
                        FormSpec::new(format!("Role for {login}"))
                            .field(
                                Field::choice("access_level", "Role", &ROLES)
                                    .value(m.i("access_level").to_string()),
                            )
                            .rest("PUT", member.clone())
                            .map(|_, v| json!({ "access_level": level(v, "access_level") }))
                            .ok("Role changed")
                            .inval(format!("{b}/members"))
                            .act(),
                    )
                    .danger(
                        "Remove",
                        Req::rest("DELETE", member)
                            .ok("Member removed")
                            .inval(format!("{b}/members"))
                            .act()
                            .confirm(
                                format!("Remove {login}?"),
                                "They lose the access this membership gave them.",
                                "Remove",
                            ),
                    );
            }
            row
        })
        .empty("No direct members.");
        let list = self.list(&spec, cx);
        let mut col = widgets::col().gap_3();
        if manage {
            col = col.child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("gl-add-member", "Add member", invite)),
            );
        }
        col.child(list).into_any_element()
    }

    pub fn gl_invitations(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("{base}/invitations");
        let p2 = path.clone();
        let spec = ListSpec::new(path, move |i| {
            let email = i.s("invite_email");
            Row::new(email.clone())
                .icon("mail", widgets::gray())
                .meta(format!(
                    "{}  ·  invited {} by {}",
                    role(i.i("access_level")),
                    time::ago(&i.s("created_at")),
                    i.s("created_by_name")
                ))
                .danger(
                    "Revoke",
                    Req::rest("DELETE", format!("{p2}/{}", enc(&email)))
                        .ok("Invitation revoked")
                        .inval(p2.clone())
                        .act(),
                )
        })
        .empty("No pending invitations.");
        self.list(&spec, cx)
    }

    /// CI/CD variables for a project or group: masked ones show no value.
    pub fn gl_variables(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("{base}/variables");
        let fields = |v: &Value| {
            vec![
                Field::multiline("value", "Value")
                    .value(if v.b("masked") {
                        String::new()
                    } else {
                        v.s("value")
                    })
                    .required(),
                Field::bool(
                    "protected",
                    "Protected: only protected branches and tags get it",
                    v.b("protected"),
                ),
                Field::bool("masked", "Masked: hidden in job logs", v.b("masked")),
                Field::text("environment_scope", "Environments").value(
                    if v.s("environment_scope").is_empty() {
                        "*".to_string()
                    } else {
                        v.s("environment_scope")
                    },
                ),
            ]
        };
        let add = FormSpec::new("Add variable")
            .submit("Add variable")
            .field(
                Field::text("key", "Key")
                    .required()
                    .hint("Letters, digits and underscores."),
            )
            .fields(fields(&Value::Null))
            .rest("POST", path.clone())
            .ok("Variable added")
            .inval(path.clone())
            .act();
        let p2 = path.clone();
        let spec = ListSpec::new(path.clone(), move |v| {
            let key = v.s("key");
            let at = format!(
                "{p2}/{}?filter[environment_scope]={}",
                enc(&key),
                enc(&v.s("environment_scope"))
            );
            let mut flags = Vec::new();
            if v.b("protected") {
                flags.push("protected");
            }
            if v.b("masked") {
                flags.push("masked");
            }
            Row::new(key.clone())
                .icon("code", widgets::gray())
                .meta(format!(
                    "{}  ·  {}{}",
                    v.s("environment_scope"),
                    if v.b("masked") {
                        "••••••".to_string()
                    } else {
                        crate::json::clip(&v.s("value"), 60)
                    },
                    if flags.is_empty() {
                        String::new()
                    } else {
                        format!("  ·  {}", flags.join(", "))
                    }
                ))
                .action(
                    "Edit…",
                    FormSpec::new(format!("Edit {key}"))
                        .fields(fields(v))
                        .rest("PUT", at.clone())
                        .ok("Variable saved")
                        .inval(p2.clone())
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", at)
                        .ok("Variable deleted")
                        .inval(p2.clone())
                        .act()
                        .confirm(format!("Delete {key}?"), "Jobs stop getting it.", "Delete"),
                )
        })
        .empty("No CI/CD variables.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::faint("Jobs get these as environment variables."))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("gl-add-var", "Add variable", add)),
            )
            .child(list)
            .into_any_element()
    }

    /// A project's Settings tab.
    pub fn gl_repo_settings(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let view_key = format!("settings.view:{repo}");
        let view = self.choice(&view_key, "general");
        let names = [
            ("general", "General"),
            ("members", "Members"),
            ("invitations", "Invitations"),
            ("branches", "Protected branches"),
            ("hooks", "Webhooks"),
            ("keys", "Deploy keys"),
            ("variables", "CI/CD variables"),
        ];
        let chips = widgets::chips(
            names
                .iter()
                .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
                .collect(),
        );
        let api = project_api(repo);
        let body = match view.as_str() {
            "members" => self.gl_members(&api, true, cx),
            "invitations" => self.gl_invitations(&api, cx),
            "branches" => self.gl_protected_branches(&api, cx),
            "hooks" => self.gl_hooks(&api, cx),
            "keys" => self.gl_deploy_keys(&api, cx),
            "variables" => self.gl_variables(&api, cx),
            _ => self.gl_general(repo, cx),
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    fn gl_general(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let api = project_api(repo);
        let info = crate::ready!(self.fetch(&api, cx));
        let on = |key: &str| info.s(&format!("{key}_access_level")) != "disabled";
        let route_back = repo.to_string();
        let edit = FormSpec::new("Project settings")
            .width(640.0)
            .field(
                Field::text("name", "Project name")
                    .value(info.s("name"))
                    .required(),
            )
            .field(
                Field::text("description", "Description")
                    .value(info.s("description"))
                    .keep_empty(),
            )
            .field(
                Field::list("topics", "Topics")
                    .value(
                        info.list("topics")
                            .iter()
                            .filter_map(|t| t.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                    .keep_empty(),
            )
            .field(Field::text("default_branch", "Default branch").value(info.s("default_branch")))
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
                .value(info.s("visibility")),
            )
            .field(Field::bool("issues", "Issues", on("issues")))
            .field(Field::bool(
                "merge_requests",
                "Merge requests",
                on("merge_requests"),
            ))
            .field(Field::bool("builds", "CI/CD", on("builds")))
            .field(Field::bool("wiki", "Wiki", on("wiki")))
            .field(
                Field::choice(
                    "merge_method",
                    "Merge method",
                    &[
                        ("merge", "Merge commit"),
                        ("rebase_merge", "Merge commit with semi-linear history"),
                        ("ff", "Fast-forward merge"),
                    ],
                )
                .value(info.s("merge_method")),
            )
            .field(
                Field::choice(
                    "squash_option",
                    "Squash commits when merging",
                    &[
                        ("never", "Do not allow"),
                        ("default_off", "Allow"),
                        ("default_on", "Encourage"),
                        ("always", "Require"),
                    ],
                )
                .value(info.s("squash_option")),
            )
            .field(Field::bool(
                "remove_source_branch_after_merge",
                "Delete the source branch by default",
                info.b("remove_source_branch_after_merge"),
            ))
            .field(Field::bool(
                "only_allow_merge_if_pipeline_succeeds",
                "Pipelines must succeed",
                info.b("only_allow_merge_if_pipeline_succeeds"),
            ))
            .field(Field::bool(
                "only_allow_merge_if_all_discussions_are_resolved",
                "All threads must be resolved",
                info.b("only_allow_merge_if_all_discussions_are_resolved"),
            ))
            .rest("PUT", api.clone())
            .map(|mut body, v| {
                for key in ["issues", "merge_requests", "builds", "wiki"] {
                    if let Some(map) = body.as_object_mut() {
                        map.remove(key);
                        map.insert(
                            format!("{key}_access_level"),
                            json!(if v.b(key) { "enabled" } else { "disabled" }),
                        );
                    }
                }
                body
            })
            .ok("Settings saved")
            .inval(api.clone())
            .inval(format!("/repos/{repo}"))
            .act();
        let path_form = FormSpec::new("Change the project's path")
            .danger()
            .note("Its address changes; links to the old one redirect, but clones need their remote updated.")
            .field(Field::text("path", "Path").value(info.s("path")).required())
            .rest("PUT", api.clone())
            .ok("Path changed")
            .then(move |hub, value, cx| {
                hub.go(Route::Repo { repo: value.s("path_with_namespace"), tab: RepoTab::Settings }, cx);
            })
            .act();
        let transfer = FormSpec::new("Transfer project")
            .submit("Transfer project")
            .danger()
            .note("Move this project to another group or user you can create projects in.")
            .field(
                Field::text("namespace", "New namespace")
                    .required()
                    .hint("A group's path, or your username."),
            )
            .rest("PUT", format!("{api}/transfer"))
            .ok("Project transferred")
            .then(|hub, value, cx| {
                hub.go(
                    Route::Repo {
                        repo: value.s("path_with_namespace"),
                        tab: RepoTab::Code,
                    },
                    cx,
                )
            })
            .act();
        let archived = info.b("archived");
        let archive = Req::rest(
            "POST",
            format!("{api}/{}", if archived { "unarchive" } else { "archive" }),
        )
        .ok(if archived {
            "Project unarchived"
        } else {
            "Project archived"
        })
        .inval(api.clone())
        .inval(format!("/repos/{repo}"))
        .act()
        .confirm(
            if archived {
                "Unarchive this project?"
            } else {
                "Archive this project?"
            },
            "Archived projects are read-only.",
            if archived { "Unarchive" } else { "Archive" },
        );
        let delete = FormSpec::new(format!("Delete {repo}"))
            .submit("Delete project")
            .danger()
            .note("This deletes the project with its repository, issues, merge requests, pipelines and packages.")
            .field(Field::text("confirm", format!("Type “{repo}” to confirm").as_str()).required())
            .build_with({
                let (repo, api) = (route_back.clone(), api.clone());
                move |v| {
                    if v.s("confirm").trim() != repo {
                        return Err("The path does not match.".into());
                    }
                    Ok(Req::rest("DELETE", api.clone()).ok("Project scheduled for deletion").inval("/user/repos").then(|hub, _, cx| hub.go(Route::Repos, cx)).act())
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
                        "{}  ·  {} visibility  ·  merges by {}",
                        info.s("path_with_namespace"),
                        info.s("visibility"),
                        info.s("merge_method").replace('_', " ")
                    )))
                    .child(div().child(widgets::primary(
                        "gl-edit-settings",
                        "Edit settings",
                        edit,
                    ))),
            )
            .child(widgets::h2("Danger zone"))
            .child(
                widgets::card()
                    .border_color(rgb(widgets::red()))
                    .child(row(
                        "Change path",
                        "Change the project's address.",
                        widgets::danger("gl-path", "Change path", path_form).into_any_element(),
                    ))
                    .child(row(
                        "Transfer project",
                        "Move it to another group or user.",
                        widgets::danger("gl-transfer", "Transfer", transfer).into_any_element(),
                    ))
                    .child(row(
                        if archived {
                            "Unarchive project"
                        } else {
                            "Archive project"
                        },
                        "Archived projects are read-only.",
                        widgets::danger(
                            "gl-archive",
                            if archived { "Unarchive" } else { "Archive" },
                            archive,
                        )
                        .into_any_element(),
                    ))
                    .child(row(
                        "Delete project",
                        "Once it's gone, it's gone.",
                        widgets::danger("gl-delete", "Delete project", delete).into_any_element(),
                    )),
            )
            .into_any_element()
    }

    fn gl_protected_branches(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("{api}/protected_branches");
        let add = FormSpec::new("Protect a branch")
            .submit("Protect")
            .field(
                Field::text("name", "Branch")
                    .required()
                    .hint("A name, or a wildcard like release/*."),
            )
            .field(Field::choice(
                "push_access_level",
                "Allowed to push",
                &BRANCH_ACCESS,
            ))
            .field(Field::choice(
                "merge_access_level",
                "Allowed to merge",
                &BRANCH_ACCESS,
            ))
            .field(Field::bool("allow_force_push", "Allow force pushes", false))
            .rest("POST", path.clone())
            .map(|_, v| {
                json!({
                    "name": v.s("name"),
                    "push_access_level": level(v, "push_access_level"),
                    "merge_access_level": level(v, "merge_access_level"),
                    "allow_force_push": v.b("allow_force_push"),
                })
            })
            .ok("Branch protected")
            .inval(path.clone())
            .act();
        let p2 = path.clone();
        let spec = ListSpec::new(path.clone(), move |b| {
            let name = b.s("name");
            let at = format!("{p2}/{}", enc(&name));
            let who = |key: &str| {
                b.list(key)
                    .iter()
                    .map(|l| l.s("access_level_description"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let force = b.b("allow_force_push");
            Row::new(name.clone())
                .icon("shield", widgets::yellow())
                .meta(format!(
                    "push: {}  ·  merge: {}{}",
                    who("push_access_levels"),
                    who("merge_access_levels"),
                    if force {
                        "  ·  force pushes allowed"
                    } else {
                        ""
                    }
                ))
                .action(
                    if force {
                        "Disallow force pushes"
                    } else {
                        "Allow force pushes"
                    },
                    Req::rest("PATCH", at.clone())
                        .body(json!({ "allow_force_push": !force }))
                        .ok("Protection updated")
                        .inval(p2.clone())
                        .act(),
                )
                .danger(
                    "Unprotect",
                    Req::rest("DELETE", at)
                        .ok("Branch unprotected")
                        .inval(p2.clone())
                        .act()
                        .confirm(
                            format!("Unprotect {name}?"),
                            "Anyone who can push will be able to push to it.",
                            "Unprotect",
                        ),
                )
        })
        .empty("No protected branches.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("gl-protect", "Protect a branch", add)),
            )
            .child(list)
            .into_any_element()
    }

    fn gl_hooks(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        const EVENTS: [(&str, &str); 8] = [
            ("push_events", "Pushes"),
            ("tag_push_events", "Tags"),
            ("merge_requests_events", "Merge requests"),
            ("issues_events", "Issues"),
            ("note_events", "Comments"),
            ("pipeline_events", "Pipelines"),
            ("job_events", "Jobs"),
            ("releases_events", "Releases"),
        ];
        let path = format!("{api}/hooks");
        let fields = |h: &Value| {
            let mut fields = vec![
                Field::text("url", "URL").value(h.s("url")).required(),
                Field::secret("token", "Secret token"),
            ];
            for (key, label) in EVENTS {
                fields.push(Field::bool(
                    key,
                    label,
                    if h.is_null() {
                        key == "push_events"
                    } else {
                        h.b(key)
                    },
                ));
            }
            fields.push(Field::bool(
                "enable_ssl_verification",
                "Verify SSL",
                h.is_null() || h.b("enable_ssl_verification"),
            ));
            fields
        };
        let add = FormSpec::new("Add webhook")
            .submit("Add webhook")
            .fields(fields(&Value::Null))
            .rest("POST", path.clone())
            .ok("Webhook added")
            .inval(path.clone())
            .act();
        let p2 = path.clone();
        let spec = ListSpec::new(path.clone(), move |h| {
            let at = format!("{p2}/{}", h.i("id"));
            let on: Vec<&str> = EVENTS
                .iter()
                .filter(|(k, _)| h.b(k))
                .map(|(_, l)| *l)
                .collect();
            Row::new(h.s("url"))
                .icon("webhook", widgets::green())
                .meta(on.join(", "))
                .action(
                    "Edit…",
                    FormSpec::new("Edit webhook")
                        .fields(fields(h))
                        .rest("PUT", at.clone())
                        .ok("Webhook saved")
                        .inval(p2.clone())
                        .act(),
                )
                .action(
                    "Test push",
                    Req::rest("POST", format!("{at}/test/push_events"))
                        .ok("Test sent")
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", at)
                        .ok("Webhook deleted")
                        .inval(p2.clone())
                        .act()
                        .confirm(
                            "Delete webhook?",
                            "Deliveries to this URL will stop.",
                            "Delete",
                        ),
                )
        })
        .empty("No webhooks.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("gl-add-hook", "Add webhook", add)),
            )
            .child(list)
            .into_any_element()
    }

    fn gl_deploy_keys(&mut self, api: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("{api}/deploy_keys");
        let add = FormSpec::new("Add deploy key")
            .submit("Add key")
            .field(Field::text("title", "Title").required())
            .field(Field::multiline("key", "Public key").required())
            .field(Field::bool("can_push", "Allow pushes", false))
            .rest("POST", path.clone())
            .ok("Deploy key added")
            .inval(path.clone())
            .act();
        let p2 = path.clone();
        let spec = ListSpec::new(path.clone(), move |k| {
            Row::new(k.s("title"))
                .icon("key", widgets::gray())
                .meta(format!(
                    "{}  ·  added {}  ·  {}",
                    if k.b("can_push") {
                        "read/write"
                    } else {
                        "read-only"
                    },
                    time::ago(&k.s("created_at")),
                    crate::json::clip(&k.s("key"), 40)
                ))
                .danger(
                    "Remove",
                    Req::rest("DELETE", format!("{p2}/{}", k.i("id")))
                        .ok("Deploy key removed")
                        .inval(p2.clone())
                        .act()
                        .confirm(
                            "Remove this deploy key?",
                            "Anything using it loses access.",
                            "Remove",
                        ),
                )
        })
        .empty("No deploy keys.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("gl-add-key", "Add deploy key", add)),
            )
            .child(list)
            .into_any_element()
    }

    /// Your personal access tokens, for Settings on GitLab.
    pub fn gl_tokens(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let id = self.me.i("id");
        let path = format!("/api/v4/personal_access_tokens?user_id={id}&state=active");
        let spec = ListSpec::new(path, |t| {
            Row::new(t.s("name"))
                .icon("key", widgets::gray())
                .meta(format!(
                    "{}  ·  {}  ·  last used {}",
                    t.list("scopes").iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                    if t.has("expires_at") { format!("expires {}", t.s("expires_at")) } else { "never expires".into() },
                    if t.has("last_used_at") { time::ago(&t.s("last_used_at")) } else { "never".into() }
                ))
                .danger(
                    "Revoke",
                    Req::rest("DELETE", format!("/api/v4/personal_access_tokens/{}", t.i("id")))
                        .ok("Token revoked")
                        .inval("/api/v4/personal_access_tokens")
                        .act()
                        .confirm("Revoke this token?", "Anything using it stops working, this app too if it's the one it signed in with.", "Revoke"),
                )
        })
        .empty("No active tokens.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(widgets::row().child(widgets::spacer()).child(widgets::btn(
                "gl-new-token",
                "New token on GitLab",
                Act::Url(format!(
                    "{}/-/user_settings/personal_access_tokens",
                    crate::forge::web()
                )),
            )))
            .child(list)
            .into_any_element()
    }
}
