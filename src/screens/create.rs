//! The New repository and New gist pages, laid out as github.com's are
//! rather than as dialogs: numbered sections, settings with dropdowns and
//! switches, and a split button for the gist's visibility.

use crate::hub::{Act, Hub, Req, Route, RepoTab};
use crate::json::Json as _;
use crate::picker::{PickItem, Picker};
use crate::time;
use crate::ui::{icon, palette};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, FontWeight, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::{json, Map, Value};

const NAME: &str = "newrepo.name";
const DESC: &str = "newrepo.desc";
const OWNER: &str = "newrepo.owner";
const VISIBILITY: &str = "newrepo.visibility";
const TEMPLATE: &str = "newrepo.template";
const GITIGNORE: &str = "newrepo.gitignore";
const LICENSE: &str = "newrepo.license";
const README: &str = "newrepo.readme";
const DESC_MAX: usize = 350;

/// "shiny-octo-adventure", the way GitHub suggests names.
fn suggestion(seed: u64) -> String {
    const A: &[&str] = &["shiny", "fuzzy", "silver", "curly", "ideal", "jubilant", "laughing", "musical", "potential", "reimagined", "solid", "special", "super", "turbo", "upgraded", "vigilant", "verbose", "symmetrical", "friendly", "stunning"];
    const B: &[&str] = &["octo", "fiesta", "guacamole", "sniffle", "spoon", "train", "umbrella", "waffle", "winner", "giggle", "garbanzo", "fortnight", "eureka", "doodle", "disco", "computing-machine", "broccoli", "bassoon", "barnacle", "carnival"];
    const C: &[&str] = &["adventure", "succotash", "memory", "parakeet", "pancake", "potato", "robot", "chainsaw", "lamp", "journey", "invention", "happiness", "goggles", "fishstick", "engine", "enigma", "dollop", "disco", "carnival", "bassoon"];
    let n = seed as usize;
    format!("{}-{}-{}", A[n % A.len()], B[(n / 7) % B.len()], C[(n / 53) % C.len()])
}

impl Hub {
    pub fn new_repo(&mut self, owner: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let me = self.login();
        let owner = self.choice(OWNER, owner.unwrap_or(&me));
        let is_org = owner != me;
        let gitlab = crate::forge::is_gitlab();
        let visibility = self.choice(VISIBILITY, "public");
        let template = self.choice(TEMPLATE, "");
        let gitignore = self.choice(GITIGNORE, "");
        let license = self.choice(LICENSE, "");
        let readme = self.toggle(README);

        // Owner: you, or an organization you belong to.
        let mut owners = Picker::new("Choose an owner", "Filter owners…", false)
            .item(PickItem::new(me.clone(), !is_org, Act::choose(OWNER, me.clone())).avatar(self.me.s("avatar_url")));
        let mut owner_avatar = self.me.s("avatar_url");
        match self.fetch("/user/orgs?per_page=100", cx).ready().cloned() {
            Some(orgs) => {
                for org in orgs.list("") {
                    let login = org.s("login");
                    if login == owner {
                        owner_avatar = org.s("avatar_url");
                    }
                    owners = owners.item(PickItem::new(login.clone(), login == owner, Act::choose(OWNER, login)).avatar(org.s("avatar_url")));
                }
            }
            None => owners.loading = true,
        }
        let owner_btn = widgets::dropdown_btn("newrepo-owner", Some(self.avatar(&owner_avatar, 20.0, cx)), owner.clone(), owners.act());

        let name_box = self.input(NAME, "", cx).h(px(32.0)).flex_1();
        let desc_box = self.input(DESC, "", cx).h(px(32.0)).w_full();
        let desc_len = self.field_text(DESC).chars().count();
        let seed = self.choice("newrepo.seed", "").parse::<u64>().unwrap_or_else(|_| time::now() as u64);
        self.choices.insert("newrepo.seed".into(), seed.to_string());
        let suggested = suggestion(seed);

        let general = widgets::col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap_3()
                    .child(widgets::col().gap_2().child(widgets::field_label("Owner", true)).child(owner_btn))
                    .child(div().pb_1().text_size(px(22.0)).text_color(rgb(p.text_dim)).child("/"))
                    .child(widgets::col().flex_1().gap_2().child(widgets::field_label("Repository name", true)).child(name_box)),
            )
            .child(
                widgets::row()
                    .gap_1()
                    .child(widgets::dim("Great repository names are short and memorable. How about"))
                    .child(
                        crate::ui::Link::new("newrepo-suggest", suggested.clone())
                            .text_color(rgb(widgets::green()))
                            .on_click(crate::hub::on(Act::run(move |hub, _, cx| {
                                hub.set_field(NAME, suggested.clone());
                                cx.notify();
                            }))),
                    )
                    .child(widgets::dim("?")),
            )
            .child(
                widgets::col()
                    .gap_2()
                    .child(widgets::field_label("Description", false))
                    .child(desc_box)
                    .child(div().text_size(px(12.0)).text_color(rgb(if desc_len > DESC_MAX { widgets::red() } else { p.text_dim })).child(format!("{desc_len} / {DESC_MAX} characters"))),
            );

        // Visibility.
        let mut vis = Picker::new("Choose visibility", "Filter…", false)
            .item(PickItem::new("Public", visibility == "public", Act::choose(VISIBILITY, "public")).detail("Anyone on the internet can see this repository."))
            .item(PickItem::new("Private", visibility == "private", Act::choose(VISIBILITY, "private")).detail("You choose who can see and commit to this repository."));
        if gitlab {
            vis = vis.item(PickItem::new("Internal", visibility == "internal", Act::choose(VISIBILITY, "internal")).detail("Anyone signed in to this GitLab can see it."));
        } else if is_org {
            vis = vis.item(PickItem::new("Internal", visibility == "internal", Act::choose(VISIBILITY, "internal")).detail("Members of the enterprise can see this repository."));
        }
        let vis_label = match visibility.as_str() {
            "private" => "Private",
            "internal" => "Internal",
            _ => "Public",
        };
        let vis_icon = icon(if visibility == "public" { "repo" } else { "lock" }, 14.0, p.text_dim).into_any_element();

        // Templates: your template repositories.
        let mut templates = Picker::new("Start with a template", "Filter templates…", false)
            .item(PickItem::new("No template", template.is_empty(), Act::choose(TEMPLATE, "")));
        match self.fetch("/user/repos?per_page=100&sort=updated", cx).ready().cloned() {
            Some(repos) => {
                for r in repos.list("").iter().filter(|r| r.b("is_template")) {
                    let full = r.s("full_name");
                    templates = templates.item(PickItem::new(full.clone(), template == full, Act::choose(TEMPLATE, full)).avatar(r.s("owner.avatar_url")).detail(r.s("description")));
                }
            }
            None => templates.loading = true,
        }

        let mut ignores = Picker::new("Add .gitignore", "Filter templates…", false)
            .item(PickItem::new("No .gitignore", gitignore.is_empty(), Act::choose(GITIGNORE, "")));
        match self.fetch("/gitignore/templates", cx).ready().cloned() {
            Some(names) => {
                for name in names.list("").iter().filter_map(|v| v.as_str()) {
                    ignores = ignores.item(PickItem::new(name, gitignore == name, Act::choose(GITIGNORE, name)));
                }
            }
            None => ignores.loading = true,
        }

        let mut licenses = Picker::new("Add license", "Filter licenses…", false)
            .item(PickItem::new("No license", license.is_empty(), Act::choose(LICENSE, "")));
        let mut license_name = "No license".to_string();
        match self.fetch("/licenses?per_page=100", cx).ready().cloned() {
            Some(list) => {
                for l in list.list("") {
                    let key = l.s("key");
                    if key == license {
                        license_name = l.s("name");
                    }
                    licenses = licenses.item(PickItem::new(l.s("name"), key == license, Act::choose(LICENSE, key)).detail(l.s("spdx_id")));
                }
            }
            None => licenses.loading = true,
        }

        // A template brings its own files, so the starter files only
        // apply without one.
        let starters = template.is_empty();
        let mut config = widgets::col().gap_4().child(
            widgets::card().child(widgets::setting_row(
                "Choose visibility",
                &format!("Choose who can see and commit to this {}", crate::forge::repo_word()),
                widgets::dropdown_btn("newrepo-vis", Some(vis_icon), vis_label, vis.act()),
                true,
            )),
        );
        // GitLab has no template repositories of your own to start from.
        if !gitlab {
            config = config.child(widgets::card().child(widgets::setting_row(
                "Start with a template",
                "Templates pre-configure your repository with files.",
                widgets::dropdown_btn("newrepo-template", None, if template.is_empty() { "No template".to_string() } else { template.clone() }, templates.act()),
                true,
            )));
        }
        if starters {
            config = config.child(
                widgets::card()
                    .child(widgets::setting_row(
                        "Add README",
                        "READMEs can be used as longer descriptions.",
                        widgets::switch("newrepo-readme", readme, Act::run(|hub, _, cx| {
                            let now = hub.toggle(README);
                            hub.toggles.insert(README.into(), !now);
                            cx.notify();
                        })),
                        true,
                    ))
                    .child(widgets::setting_row(
                        "Add .gitignore",
                        ".gitignore tells git which files not to track.",
                        widgets::dropdown_btn("newrepo-gitignore", None, if gitignore.is_empty() { "No .gitignore".to_string() } else { gitignore.clone() }, ignores.act()),
                        false,
                    ))
                    .child(widgets::setting_row(
                        "Add license",
                        "Licenses explain how others can use your code.",
                        widgets::dropdown_btn("newrepo-license", None, license_name, licenses.act()),
                        false,
                    )),
            );
        }

        let create = Act::run(move |hub, window, cx| hub.create_repo(window, cx));
        widgets::page()
            .max_w(px(900.0))
            .gap_6()
            .child(
                widgets::col()
                    .gap_1()
                    .child(widgets::title(if gitlab { "Create a new project" } else { "Create a new repository" }))
                    .child(widgets::dim(if gitlab {
                        "Projects hold a repository's files and history, with its issues, merge requests and pipelines."
                    } else {
                        "Repositories contain a project's files and version history."
                    }))
                    .child(widgets::dim("Required fields are marked with an asterisk (*).").italic()),
            )
            .child(
                widgets::col()
                    .child(widgets::step(1, "General", false, general))
                    .child(widgets::step(2, "Configuration", true, config)),
            )
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("newrepo-create", if gitlab { "Create project" } else { "Create repository" }, create).h(px(32.0)).px_4().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD)),
            )
            .into_any_element()
    }

    fn create_repo(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let me = self.login();
        let owner = self.choice(OWNER, &me);
        let name = self.field_text(NAME).trim().to_string();
        let description = self.field_text(DESC).trim().to_string();
        if name.is_empty() {
            self.toast(format!("Give the {} a name.", crate::forge::repo_word()), true, cx);
            self.focus_field(NAME);
            return;
        }
        if description.chars().count() > DESC_MAX {
            self.toast(format!("Keep the description to {DESC_MAX} characters."), true, cx);
            return;
        }
        let visibility = self.choice(VISIBILITY, "public");
        let template = self.choice(TEMPLATE, "");
        let req = if !template.is_empty() {
            Req::rest("POST", format!("/repos/{template}/generate")).body(json!({
                "owner": owner,
                "name": name,
                "description": description,
                "private": visibility != "public",
            }))
        } else {
            let mut body = json!({
                "name": name,
                "description": description,
                "private": visibility == "private",
                "auto_init": self.toggle(README),
            });
            if visibility == "internal" || crate::forge::is_gitlab() {
                body["visibility"] = json!(visibility);
            }
            for (key, choice) in [("gitignore_template", GITIGNORE), ("license_template", LICENSE)] {
                let value = self.choice(choice, "");
                if !value.is_empty() {
                    body[key] = json!(value);
                }
            }
            let path = if owner == me { "/user/repos".to_string() } else { format!("/orgs/{owner}/repos") };
            Req::rest("POST", path).body(body)
        };
        let req = req.ok(format!("{} created", if crate::forge::is_gitlab() { "Project" } else { "Repository" })).inval("/user/repos").then(|hub, value: &Value, cx| {
            for key in [NAME, DESC] {
                hub.set_field(key, "");
            }
            for key in [OWNER, VISIBILITY, TEMPLATE, GITIGNORE, LICENSE, "newrepo.seed"] {
                hub.choices.remove(key);
            }
            hub.go(Route::Repo { repo: value.s("full_name"), tab: RepoTab::Code }, cx);
        });
        self.perform(req.act(), window, cx);
    }

    pub fn new_gist(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let count = self.choice("newgist.files", "1").parse::<usize>().unwrap_or(1).max(1);
        let public = self.choice("newgist.public", "false") == "true";
        let gitlab = crate::forge::is_gitlab();
        let desc = self.input("newgist.desc", if gitlab { "Snippet title…" } else { "Gist description…" }, cx).h(px(36.0)).w_full();

        let mut files = widgets::col().gap_4();
        for i in 0..count {
            let name = self.input(&format!("newgist.name.{i}"), "Filename including extension…", cx).h(px(32.0)).w(px(320.0));
            let body = self
                .textarea(&format!("newgist.body.{i}"), "", cx)
                .min_h(px(300.0))
                .font_family(widgets::MONO)
                .text_size(px(12.0));
            let remove = Act::run(move |hub, _, cx| hub.remove_gist_file(i, cx));
            files = files.child(
                widgets::card()
                    .child(
                        widgets::row()
                            .p_2()
                            .bg(rgb(p.deep_bg))
                            .border_b_1()
                            .border_color(rgb(p.divider))
                            .child(name)
                            .child(widgets::spacer())
                            .when(count > 1, |d| {
                                d.child(
                                    crate::ui::IconButton::new(("gist-remove", i), "trash")
                                        .size(28.0)
                                        .icon_size(14.0)
                                        .tooltip("Remove file", None)
                                        .on_click(crate::hub::on(remove)),
                                )
                            }),
                    )
                    .child(div().p_2().child(body)),
            );
        }

        let (label, modes) = if gitlab {
            (
                if public { "Create public snippet" } else { "Create private snippet" },
                Act::menu(vec![
                    crate::hub::MenuEntry::check("Create private snippet — only you can see it", !public, Act::choose("newgist.public", "false")),
                    crate::hub::MenuEntry::check("Create public snippet — visible to everyone", public, Act::choose("newgist.public", "true")),
                ]),
            )
        } else {
            (
                if public { "Create public gist" } else { "Create secret gist" },
                Act::menu(vec![
                    crate::hub::MenuEntry::check("Create secret gist — hidden from search, visible to anyone with the link", !public, Act::choose("newgist.public", "false")),
                    crate::hub::MenuEntry::check("Create public gist — visible to everyone", public, Act::choose("newgist.public", "true")),
                ]),
            )
        };
        let add = Act::run(move |hub, _, cx| {
            hub.choices.insert("newgist.files".into(), (count + 1).to_string());
            hub.focus_field(&format!("newgist.name.{count}"));
            cx.notify();
        });
        widgets::page()
            .gap_4()
            .child(widgets::title(if gitlab { "Create a snippet" } else { "Create a gist" }))
            .child(desc)
            .child(files)
            .child(
                widgets::row()
                    .items_start()
                    .child(widgets::btn("gist-add-file", "Add file", add).h(px(32.0)).px_4().text_size(px(13.0)))
                    .child(widgets::spacer())
                    .child(
                        widgets::col()
                            .items_end()
                            .gap_2()
                            .child(widgets::split_btn("gist-create", label, Act::run(|hub, window, cx| hub.create_gist(window, cx)), modes))
                            .when(!self.me.s("email").is_empty(), |d| d.child(widgets::faint(format!("Commit email: {}", self.me.s("email"))))),
                    ),
            )
            .into_any_element()
    }

    /// Drop file `i`, moving the ones after it up.
    fn remove_gist_file(&mut self, i: usize, cx: &mut Context<Self>) {
        let count = self.choice("newgist.files", "1").parse::<usize>().unwrap_or(1);
        for j in i..count.saturating_sub(1) {
            for part in ["name", "body"] {
                let next = self.field_text(&format!("newgist.{part}.{}", j + 1));
                self.set_field(&format!("newgist.{part}.{j}"), next);
            }
        }
        for part in ["name", "body"] {
            self.set_field(&format!("newgist.{part}.{}", count - 1), "");
        }
        self.choices.insert("newgist.files".into(), count.saturating_sub(1).max(1).to_string());
        cx.notify();
    }

    fn create_gist(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let count = self.choice("newgist.files", "1").parse::<usize>().unwrap_or(1);
        let mut files = Map::new();
        for i in 0..count {
            let content = self.field_text(&format!("newgist.body.{i}"));
            if content.trim().is_empty() {
                continue;
            }
            let mut name = self.field_text(&format!("newgist.name.{i}")).trim().to_string();
            if name.is_empty() {
                name = format!("gistfile{}.txt", i + 1);
            }
            files.insert(name, json!({ "content": content }));
        }
        if files.is_empty() {
            self.toast("A gist needs at least one file with something in it.", true, cx);
            return;
        }
        let body = json!({
            "description": self.field_text("newgist.desc"),
            "public": self.choice("newgist.public", "false") == "true",
            "files": files,
        });
        let req = Req::rest("POST", "/gists").body(body).ok(if crate::forge::is_gitlab() { "Snippet created" } else { "Gist created" }).inval("/gists").then(move |hub, value: &Value, cx| {
            hub.set_field("newgist.desc", "");
            for i in 0..count {
                hub.set_field(&format!("newgist.name.{i}"), "");
                hub.set_field(&format!("newgist.body.{i}"), "");
            }
            hub.choices.remove("newgist.files");
            hub.go(Route::Gist { id: value.s("id") }, cx);
        });
        self.perform(req.act(), window, cx);
    }
}
