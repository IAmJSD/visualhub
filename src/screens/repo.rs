//! A repository: its header (star, watch, fork, clone), its tabs, the
//! Code tab with the README and the About sidebar, and branches, tags and
//! commits.

use super::pulls::commit_row;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, RepoTab, Req, Route, Work};
use crate::json::{self, enc, enc_path, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette, DropdownButton};
use crate::widgets::{self, rgb, TabItem};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// A commit with its authors, its checks and the length of its history.
const COMMIT_HEAD: &str = "query($o: String!, $n: String!, $r: String!) { repository(owner: $o, name: $n) { object(expression: $r) { ... on Commit { oid messageHeadline committedDate history(first: 1) { totalCount } authors(first: 10) { nodes { name avatarUrl user { login avatarUrl } } } statusCheckRollup { state } } } } }";

/// Commit authors (GraphQL `authors.nodes`) with repeats dropped.
pub fn distinct_authors(authors: &[Value]) -> Vec<Value> {
    let mut seen = Vec::new();
    authors
        .iter()
        .filter(|a| {
            let key = if a.has("user.login") {
                a.s("user.login")
            } else {
                a.s("name")
            };
            !seen.contains(&key) && {
                seen.push(key);
                true
            }
        })
        .cloned()
        .collect()
}

/// "a and b" or "a, b and c", each name with an account opening its
/// profile. Clicks stop here, so a name inside a clickable row opens the
/// person rather than the row.
pub fn author_names(id: &str, authors: &[Value]) -> gpui::Div {
    let mut names = widgets::row().gap_1();
    for (i, a) in authors.iter().enumerate() {
        if i > 0 {
            names = names.child(widgets::dim(if i + 1 == authors.len() {
                "and"
            } else {
                ","
            }));
        }
        let login = a.s("user.login");
        names = names.child(if login.is_empty() {
            div()
                .text_color(rgb(palette().text))
                .child(a.s("name"))
                .into_any_element()
        } else {
            let go = Act::Go(Route::User {
                login: login.clone(),
            });
            crate::ui::Link::new(ElementId::Name(format!("{id}-author-{i}").into()), login)
                .text_color(rgb(palette().text))
                .on_click(move |_e, window, cx| {
                    cx.stop_propagation();
                    crate::hub::perform(go.clone(), window, cx);
                })
                .into_any_element()
        });
    }
    names
}

/// 12345 as "12,345".
fn thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A form that makes a branch from another branch, tag or commit.
pub fn new_branch_form(repo: &str, name: &str) -> Act {
    let repo = repo.to_string();
    FormSpec::new("Create a branch")
        .submit("Create branch")
        .field(Field::text("name", "Branch name").value(name).required())
        .field(
            Field::text("from", "From")
                .hint("A branch, tag or commit SHA. Leave empty for the default branch."),
        )
        .build_with(move |values| {
            let (repo, name, from) = (repo.clone(), values.s("name"), values.s("from"));
            let refs = format!("/repos/{repo}/branches");
            Ok(Req::custom(move |client| {
                let from = if from.trim().is_empty() {
                    client.get(&format!("/repos/{repo}"))?.s("default_branch")
                } else {
                    from.trim().to_string()
                };
                let sha = client
                    .get(&format!("/repos/{repo}/commits/{}", enc(&from)))?
                    .s("sha");
                client.json(
                    "POST",
                    &format!("/repos/{repo}/git/refs"),
                    Some(&json!({ "ref": format!("refs/heads/{}", name.trim()), "sha": sha })),
                )
            })
            .ok("Branch created")
            .inval(refs)
            .act())
        })
        .act()
}

fn new_tag_form(repo: &str) -> Act {
    let repo = repo.to_string();
    FormSpec::new("Create a tag")
        .submit("Create tag")
        .note("A lightweight tag pointing at a commit. To publish it, draft a release instead.")
        .field(Field::text("name", "Tag name").required())
        .field(
            Field::text("from", "Target")
                .hint("A branch or commit SHA. Leave empty for the default branch."),
        )
        .build_with(move |values| {
            let (repo, name, from) = (repo.clone(), values.s("name"), values.s("from"));
            let refs = format!("/repos/{repo}/tags");
            Ok(Req::custom(move |client| {
                let from = if from.trim().is_empty() {
                    client.get(&format!("/repos/{repo}"))?.s("default_branch")
                } else {
                    from.trim().to_string()
                };
                let sha = client
                    .get(&format!("/repos/{repo}/commits/{}", enc(&from)))?
                    .s("sha");
                client.json(
                    "POST",
                    &format!("/repos/{repo}/git/refs"),
                    Some(&json!({ "ref": format!("refs/tags/{}", name.trim()), "sha": sha })),
                )
            })
            .ok("Tag created")
            .inval(refs)
            .act())
        })
        .act()
}

fn protection_form(repo: &str, branch: &str) -> Act {
    FormSpec::new(format!("Protect {branch}"))
        .submit("Save protection")
        .width(600.0)
        .note("Replaces any existing protection rule on this branch.")
        .field(Field::bool("pr", "Require a pull request before merging", true))
        .field(Field::number("reviews", "Required approvals").value("1"))
        .field(Field::bool("stale", "Dismiss stale approvals when new commits are pushed", false))
        .field(Field::bool("owners", "Require review from code owners", false))
        .field(Field::bool("checks", "Require status checks to pass", false))
        .field(Field::list("contexts", "Required checks").hint("Check names, comma separated."))
        .field(Field::bool("strict", "Require branches to be up to date before merging", false))
        .field(Field::bool("conversation", "Require conversation resolution", false))
        .field(Field::bool("linear", "Require linear history", false))
        .field(Field::bool("admins", "Include administrators", false))
        .field(Field::bool("force", "Allow force pushes", false))
        .field(Field::bool("deletions", "Allow deletions", false))
        .rest("PUT", format!("/repos/{repo}/branches/{}/protection", enc(branch)))
        .map(|_, v| {
            json!({
                "required_status_checks": if v.b("checks") {
                    json!({ "strict": v.b("strict"), "contexts": v.items("contexts") })
                } else { Value::Null },
                "enforce_admins": v.b("admins"),
                "required_pull_request_reviews": if v.b("pr") {
                    json!({
                        "dismiss_stale_reviews": v.b("stale"),
                        "require_code_owner_reviews": v.b("owners"),
                        "required_approving_review_count": v.s("reviews").trim().parse::<i64>().unwrap_or(1),
                    })
                } else { Value::Null },
                "restrictions": Value::Null,
                "required_linear_history": v.b("linear"),
                "allow_force_pushes": v.b("force"),
                "allow_deletions": v.b("deletions"),
                "required_conversation_resolution": v.b("conversation"),
            })
        })
        .ok("Branch protection saved")
        .inval(format!("/repos/{repo}/branches"))
        .act()
}

/// GitLab's protection: who may push, who may merge, and force pushes.
fn gitlab_protection_form(repo: &str, branch: &str) -> Act {
    let levels = [
        ("40", "Maintainers"),
        ("30", "Developers and maintainers"),
        ("0", "No one"),
    ];
    let api = crate::screens::gitlab::project_api(repo);
    let (branch_s, repo_s) = (branch.to_string(), repo.to_string());
    FormSpec::new(format!("Protect {branch}"))
        .submit("Save protection")
        .note("Replaces any existing protection of this branch.")
        .field(Field::choice("push", "Allowed to push", &levels))
        .field(Field::choice("merge", "Allowed to merge", &levels))
        .field(Field::bool("force", "Allow force pushes", false))
        .build_with(move |v| {
            let (api, branch) = (api.clone(), branch_s.clone());
            let level = |key: &str| v.s(key).parse::<i64>().unwrap_or(40);
            let body = json!({ "name": branch, "push_access_level": level("push"), "merge_access_level": level("merge"), "allow_force_push": v.b("force") });
            Ok(Req::custom(move |client| {
                let _ = client.json("DELETE", &format!("{api}/protected_branches/{}", enc(&branch)), None);
                client.json("POST", &format!("{api}/protected_branches"), Some(&body))
            })
            .ok("Branch protection saved")
            .inval(format!("/repos/{repo_s}/branches"))
            .act())
        })
        .act()
}

/// A source archive's address: a branch's (`tag` false) or a tag's.
fn archive_url(repo: &str, git_ref: &str, format: &str, tag: bool) -> String {
    let web = crate::forge::web();
    if crate::forge::is_bitbucket() {
        format!("{web}/{repo}/get/{}.{format}", enc(git_ref))
    } else if crate::forge::is_gitlab() {
        let name = crate::forge::split_repo(repo).1;
        let file = format!("{name}-{}", git_ref.replace('/', "-"));
        format!("{web}/{repo}/-/archive/{git_ref}/{file}.{format}")
    } else {
        format!(
            "{web}/{repo}/archive/refs/{}/{git_ref}.{format}",
            if tag { "tags" } else { "heads" }
        )
    }
}

fn fork_form(repo: &str, name: &str) -> Act {
    FormSpec::new(format!("Fork {repo}"))
        .submit("Create fork")
        .field(
            Field::text("organization", "Owner")
                .hint("An organization to fork into; leave empty for your account."),
        )
        .field(Field::text("name", "Repository name").value(name))
        .field(Field::bool(
            "default_branch_only",
            "Copy the default branch only",
            true,
        ))
        .rest("POST", format!("/repos/{repo}/forks"))
        .ok("Fork created — it may take a moment to appear")
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

fn template_form(repo: &str) -> Act {
    FormSpec::new("Create a repository from this template")
        .submit("Create repository")
        .field(Field::text("owner", "Owner").hint("Your login or an organization."))
        .field(Field::text("name", "Repository name").required())
        .field(Field::text("description", "Description"))
        .field(Field::bool("private", "Private", false))
        .field(Field::bool(
            "include_all_branches",
            "Include all branches",
            false,
        ))
        .rest("POST", format!("/repos/{repo}/generate"))
        .ok("Repository created")
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
    /// The ref a repository's pages are showing: the one picked, or the
    /// default branch.
    pub fn current_ref(&self, repo: &str, default_branch: &str) -> String {
        self.choice(&format!("ref:{repo}"), default_branch)
    }

    /// A dropdown of branches and tags that sets the shown ref.
    pub fn ref_picker(&mut self, repo: &str, current: &str, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("ref:{repo}");
        let mut entries = vec![MenuEntry::Header("Branches".into())];
        if let Some(branches) = self
            .fetch(&format!("/repos/{repo}/branches?per_page=100"), cx)
            .ready()
            .cloned()
        {
            for b in branches.list("") {
                let name = b.s("name");
                entries.push(MenuEntry::check(
                    name.clone(),
                    name == current,
                    Act::choose(&key, name),
                ));
            }
        }
        if let Some(tags) = self
            .fetch(&format!("/repos/{repo}/tags?per_page=30"), cx)
            .ready()
            .cloned()
        {
            if !tags.list("").is_empty() {
                entries.push(MenuEntry::Sep);
                entries.push(MenuEntry::Header("Tags".into()));
            }
            for t in tags.list("") {
                let name = t.s("name");
                entries.push(MenuEntry::check(
                    name.clone(),
                    name == current,
                    Act::choose(&key, name),
                ));
            }
        }
        let act = Act::menu(entries);
        DropdownButton::new(
            ElementId::Name(format!("ref-{repo}").into()),
            current.to_string(),
        )
        .h(px(28.0))
        .px_2()
        .min_w(px(140.0))
        .text_size(px(13.0))
        .bg(rgb(palette().button_bg))
        .child(icon("branch", 14.0, palette().text_dim))
        .on_press(move |_, window, cx| crate::hub::perform(act.clone(), window, cx))
        .into_any_element()
    }

    pub fn repo(&mut self, repo: &str, tab: RepoTab, cx: &mut Context<Self>) -> AnyElement {
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
        let info = match self.fetch(&format!("/repos/{repo}"), cx) {
            Load::Ready(info) => info,
            // GitLab's addresses don't say whether `a/b` is a project or a
            // subgroup; a group page shows when there's no project.
            Load::Failed(_)
                if gitlab && self.fetch(&format!("/orgs/{repo}"), cx).ready().is_some() =>
            {
                return self.gl_group(repo, cx)
            }
            other => return widgets::placeholder(&other),
        };
        let header = self.repo_header(repo, &info, cx);
        let admin = info.b("permissions.admin");
        let go = |tab: RepoTab| {
            Act::Go(Route::Repo {
                repo: repo.to_string(),
                tab,
            })
        };
        let counts = self.issue_counts(repo, cx);
        let open = |key: &str| counts.as_ref().map(|c| c.i(&format!("{key}.totalCount")));
        let mut items = vec![TabItem::new(
            "Code",
            "code",
            tab == RepoTab::Code,
            go(RepoTab::Code),
        )];
        if info.b("has_issues") {
            items.push(
                TabItem::new(
                    "Issues",
                    "issue",
                    tab == RepoTab::Issues,
                    go(RepoTab::Issues),
                )
                .count(open("openIssues")),
            );
        }
        items.push(
            TabItem::new(
                crate::forge::prs_title(),
                "pr",
                tab == RepoTab::Pulls,
                go(RepoTab::Pulls),
            )
            .count(open("openPulls")),
        );
        if info.b("has_discussions") {
            items.push(TabItem::new(
                "Discussions",
                "discussion",
                tab == RepoTab::Discussions,
                go(RepoTab::Discussions),
            ));
        }
        if crate::forge::is_bitbucket() {
            items.extend([
                TabItem::new(
                    "Pipelines",
                    "play",
                    tab == RepoTab::Actions,
                    go(RepoTab::Actions),
                ),
                TabItem::new(
                    "Branches",
                    "branch",
                    tab == RepoTab::Branches,
                    go(RepoTab::Branches),
                ),
                TabItem::new("Tags", "tag", tab == RepoTab::Tags, go(RepoTab::Tags)),
                TabItem::new(
                    "Commits",
                    "commit",
                    tab == RepoTab::Commits,
                    go(RepoTab::Commits),
                ),
                TabItem::new(
                    "Insights",
                    "graph",
                    tab == RepoTab::Insights,
                    go(RepoTab::Insights),
                ),
            ]);
        } else if gitlab {
            items.extend([
                TabItem::new(
                    "CI/CD",
                    "play",
                    tab == RepoTab::Actions,
                    go(RepoTab::Actions),
                ),
                TabItem::new(
                    "Releases",
                    "tag",
                    tab == RepoTab::Releases,
                    go(RepoTab::Releases),
                ),
                TabItem::new(
                    "Branches",
                    "branch",
                    tab == RepoTab::Branches,
                    go(RepoTab::Branches),
                ),
                TabItem::new("Tags", "tag", tab == RepoTab::Tags, go(RepoTab::Tags)),
                TabItem::new(
                    "Commits",
                    "commit",
                    tab == RepoTab::Commits,
                    go(RepoTab::Commits),
                ),
                TabItem::new(
                    "Packages",
                    "package",
                    tab == RepoTab::Packages,
                    go(RepoTab::Packages),
                ),
                TabItem::new(
                    "Insights",
                    "graph",
                    tab == RepoTab::Insights,
                    go(RepoTab::Insights),
                ),
            ]);
        } else {
            items.extend([
                TabItem::new(
                    "Actions",
                    "play",
                    tab == RepoTab::Actions,
                    go(RepoTab::Actions),
                ),
                TabItem::new(
                    "Projects",
                    "project",
                    tab == RepoTab::Projects,
                    go(RepoTab::Projects),
                ),
                TabItem::new(
                    "Releases",
                    "tag",
                    tab == RepoTab::Releases,
                    go(RepoTab::Releases),
                ),
                TabItem::new(
                    "Branches",
                    "branch",
                    tab == RepoTab::Branches,
                    go(RepoTab::Branches),
                ),
                TabItem::new("Tags", "tag", tab == RepoTab::Tags, go(RepoTab::Tags)),
                TabItem::new(
                    "Commits",
                    "commit",
                    tab == RepoTab::Commits,
                    go(RepoTab::Commits),
                ),
                TabItem::new(
                    "Security",
                    "shield",
                    tab == RepoTab::Security,
                    go(RepoTab::Security),
                ),
                TabItem::new(
                    "Insights",
                    "graph",
                    tab == RepoTab::Insights,
                    go(RepoTab::Insights),
                ),
            ]);
        }
        if admin {
            items.push(TabItem::new(
                "Settings",
                "settings",
                tab == RepoTab::Settings,
                go(RepoTab::Settings),
            ));
        }
        let default_branch = info.s("default_branch");
        let body = match tab {
            RepoTab::Actions if bitbucket => self.bb_pipelines(repo, &default_branch, cx),
            RepoTab::Settings if bitbucket => self.bb_repo_settings(repo, &info, cx),
            RepoTab::Issues
            | RepoTab::Releases
            | RepoTab::Packages
            | RepoTab::Discussions
            | RepoTab::Projects
            | RepoTab::Security
                if bitbucket =>
            {
                self.gl_elsewhere(
                    &Route::Repo {
                        repo: repo.to_string(),
                        tab,
                    },
                    "That",
                )
            }
            RepoTab::Code => self.repo_code(repo, &info, cx),
            RepoTab::Issues => self.repo_issues(repo, cx),
            RepoTab::Pulls => self.repo_pulls(repo, &default_branch, cx),
            RepoTab::Actions if gitlab => self.gl_ci(repo, &default_branch, cx),
            RepoTab::Settings if gitlab => self.gl_repo_settings(repo, cx),
            RepoTab::Packages => self.gl_packages(&crate::screens::gitlab::project_api(repo), cx),
            RepoTab::Discussions | RepoTab::Projects | RepoTab::Security if gitlab => self
                .gl_elsewhere(
                    &Route::Repo {
                        repo: repo.to_string(),
                        tab,
                    },
                    "That",
                ),
            RepoTab::Discussions => self.repo_discussions(repo, cx),
            RepoTab::Actions => self.repo_actions(repo, &default_branch, cx),
            RepoTab::Projects => self.repo_projects(repo, cx),
            RepoTab::Releases => self.repo_releases(repo, &default_branch, cx),
            RepoTab::Branches => self.repo_branches(repo, &info, cx),
            RepoTab::Tags => self.repo_tags(repo, cx),
            RepoTab::Commits => self.repo_commits(repo, &default_branch, cx),
            RepoTab::Security => self.repo_security(repo, cx),
            RepoTab::Insights => self.repo_insights(repo, &info, cx),
            RepoTab::Settings => self.repo_settings(repo, &info, cx),
        };
        widgets::page()
            .child(header)
            .child(widgets::tabs("repo", items))
            .child(body)
            .into_any_element()
    }

    fn repo_header(&mut self, repo: &str, info: &Value, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let owner_avatar = self.avatar(&info.s("owner.avatar_url"), 24.0, cx);
        let starred = self
            .fetch_check(&format!("/user/starred/{repo}"), cx)
            .ready()
            .map(|v| v.b(""))
            .unwrap_or(false);
        let sub_path = format!("/repos/{repo}/subscription");
        let subscription = self.fetch_with(
            sub_path.clone(),
            Work::Custom(Arc::new({
                let sub_path = sub_path.clone();
                move |client| {
                    let reply = client.send("GET", &sub_path, None, None)?;
                    Ok(if reply.status == 200 {
                        serde_json::from_str(&reply.body).unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    })
                }
            })),
            cx,
        );
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
        let watching = match subscription.ready() {
            Some(v) if v.b("ignored") => "Ignoring",
            Some(v) if v.b("subscribed") => "Watching",
            _ => "Watch",
        };
        // GitLab doesn't count watchers.
        let watch_label = if gitlab {
            watching.to_string()
        } else {
            format!("{watching} · {}", json::count(info.i("subscribers_count")))
        };
        let watch_menu = Act::menu(vec![
            MenuEntry::check(
                if gitlab {
                    "Participate"
                } else {
                    "Participating and @mentions"
                },
                watching == "Watch",
                Req::rest("DELETE", sub_path.clone())
                    .ok("Watching participating only")
                    .inval(sub_path.clone())
                    .act(),
            ),
            MenuEntry::check(
                if gitlab { "Watch" } else { "All activity" },
                watching == "Watching",
                Req::rest("PUT", sub_path.clone())
                    .body(json!({ "subscribed": true }))
                    .ok("Watching all activity")
                    .inval(sub_path.clone())
                    .act(),
            ),
            MenuEntry::check(
                if gitlab { "Disabled" } else { "Ignore" },
                watching == "Ignoring",
                Req::rest("PUT", sub_path.clone())
                    .body(json!({ "ignored": true }))
                    .ok("Ignoring this repository")
                    .inval(sub_path.clone())
                    .act(),
            ),
        ]);
        let star = Req::rest(
            if starred { "DELETE" } else { "PUT" },
            format!("/user/starred/{repo}"),
        )
        .ok(if starred { "Unstarred" } else { "Starred" })
        .inval("/user/starred")
        .inval(format!("/repos/{repo}"))
        .act();
        let name = info.s("name");
        let https = info.s("clone_url");
        let ssh = info.s("ssh_url");
        let mut clone = vec![
            MenuEntry::Header("Clone".into()),
            MenuEntry::item(format!("Copy HTTPS  {https}"), Act::Copy(https.clone())),
            MenuEntry::item(format!("Copy SSH  {ssh}"), Act::Copy(ssh)),
        ];
        if let Some(cli) = crate::forge::cli() {
            clone.push(MenuEntry::item(
                format!("Copy  {cli} repo clone {repo}"),
                Act::Copy(format!("{cli} repo clone {repo}")),
            ));
        }
        clone.push(MenuEntry::Sep);
        if crate::forge::is_github() {
            clone.push(MenuEntry::item(
                "Open with GitHub Desktop",
                Act::Url(format!("x-github-client://openRepo/{}", info.s("html_url"))),
            ));
        }
        clone.extend([
            MenuEntry::item(
                "Open with Visual Studio Code",
                Act::Url(format!("vscode://vscode.git/clone?url={}", enc(&https))),
            ),
            MenuEntry::item(
                "Download ZIP",
                Act::Url(archive_url(repo, &info.s("default_branch"), "zip", false)),
            ),
        ]);
        if crate::forge::is_github() {
            clone.extend([
                MenuEntry::Sep,
                MenuEntry::item(
                    "Create a codespace",
                    Req::rest("POST", format!("/repos/{repo}/codespaces"))
                        .body(json!({ "ref": info.s("default_branch") }))
                        .ok("Codespace is being created")
                        .inval("/user/codespaces")
                        .then(|_, value, cx| cx.open_url(&value.s("web_url")))
                        .act(),
                ),
            ]);
        }
        let clone = Act::menu(clone);
        let owner = info.s("owner.login");
        let owner_route = if info.s("owner.type") == "Organization" {
            Route::Org {
                login: owner.clone(),
            }
        } else {
            Route::User {
                login: owner.clone(),
            }
        };
        let mut topics = div().flex().flex_row().flex_wrap().gap_1();
        for t in info.list("topics") {
            let topic = t.as_str().unwrap_or("").to_string();
            topics = topics.child(
                crate::ui::Chip::new(
                    ElementId::Name(format!("topic-{topic}").into()),
                    topic.clone(),
                )
                .colors(crate::ui::ChipColors {
                    bg: if crate::ui::is_light() {
                        0xDDF4FF
                    } else {
                        0x121D2F
                    },
                    hover: p.hover,
                    text: p.accent_hover,
                    selected_bg: p.selection_bg,
                    selected_text: p.text,
                })
                .on_click(on(Act::run(move |hub, _, cx| {
                    hub.choices
                        .insert("search.kind".into(), "repositories".into());
                    hub.set_field("search.q", format!("topic:{topic}"));
                    hub.choices
                        .insert("search.applied".into(), format!("topic:{topic}"));
                    hub.go(Route::Search, cx);
                }))),
            );
        }
        let visibility = info.s("visibility");
        widgets::col()
            .gap_2()
            .child(
                widgets::row()
                    .flex_wrap()
                    .child(owner_avatar)
                    .child(
                        div()
                            .id("repo-owner")
                            .text_size(px(20.0))
                            .text_color(rgb(p.accent_hover))
                            .cursor_pointer()
                            .child(owner)
                            .on_click(on(Act::Go(owner_route))),
                    )
                    .child(
                        div()
                            .text_size(px(20.0))
                            .text_color(rgb(p.text_faint))
                            .child("/"),
                    )
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .child(widgets::tag(
                        if visibility.is_empty() {
                            "public".to_string()
                        } else {
                            visibility
                        },
                        p.text_dim,
                    ))
                    .when(info.b("archived"), |d| {
                        d.child(widgets::tag("Archived", widgets::yellow()))
                    })
                    .when(info.b("is_template"), |d| {
                        d.child(widgets::tag("Template", p.text_dim))
                    })
                    .child(widgets::spacer())
                    .when(info.b("is_template"), |d| {
                        d.child(widgets::go_btn(
                            "use-template",
                            "Use this template",
                            template_form(repo),
                        ))
                    })
                    // Bitbucket has no stars, and no watching through its
                    // API; it doesn't count forks.
                    .when(!bitbucket, |d| {
                        d.child(widgets::ibtn("watch", "eye", watch_label, watch_menu))
                    })
                    .child(widgets::ibtn(
                        "fork",
                        "fork",
                        if bitbucket {
                            "Fork".to_string()
                        } else {
                            format!("Fork · {}", json::count(info.i("forks_count")))
                        },
                        fork_form(repo, &name),
                    ))
                    .when(!bitbucket, |d| {
                        d.child(widgets::ibtn(
                            "star",
                            if starred { "star-fill" } else { "star" },
                            format!(
                                "{} · {}",
                                if starred { "Starred" } else { "Star" },
                                json::count(info.i("stargazers_count"))
                            ),
                            star,
                        ))
                    })
                    .child(widgets::primary("clone", "Code ▾", clone)),
            )
            .when(info.has("parent"), |d| {
                let parent = info.s("parent.full_name");
                d.child(
                    widgets::row().child(widgets::dim("forked from")).child(
                        crate::ui::Link::new("parent", parent.clone())
                            .text_size(px(12.0))
                            .on_click(on(Act::Go(Route::Repo {
                                repo: parent,
                                tab: RepoTab::Code,
                            }))),
                    ),
                )
            })
            .when(!info.s("description").is_empty(), |d| {
                d.child(div().text_color(rgb(p.text)).child(info.s("description")))
            })
            .when(!info.s("homepage").is_empty(), |d| {
                d.child(
                    crate::ui::Link::new("homepage", info.s("homepage")).url(info.s("homepage")),
                )
            })
            .when(!info.list("topics").is_empty(), |d| d.child(topics))
            .into_any_element()
    }

    /// The Code tab: the ref picker, the root listing, the README, and
    /// the About sidebar.
    fn repo_code(&mut self, repo: &str, info: &Value, cx: &mut Context<Self>) -> AnyElement {
        let default_branch = info.s("default_branch");
        let git_ref = self.current_ref(repo, &default_branch);
        if info.i("size") == 0 && !info.has("pushed_at") {
            return widgets::card()
                .p_6()
                .gap_2()
                .child(widgets::h2("This repository is empty"))
                .child(widgets::dim(
                    "Push an existing repository from the command line:",
                ))
                .child(crate::markdown::code_block(
                    "empty-help".into(),
                    "sh",
                    &format!(
                        "git remote add origin {}\ngit branch -M main\ngit push -u origin main",
                        info.s("clone_url")
                    ),
                ))
                .into_any_element();
        }
        let listing = self.directory(repo, &git_ref, "", cx);
        let readme = self.readme(repo, &git_ref, "", cx);
        let about = self.repo_about(repo, info, cx);
        let picker = self.ref_picker(repo, &git_ref, cx);
        let branches = self
            .fetch(&format!("/repos/{repo}/branches?per_page=100"), cx)
            .ready()
            .map(|v| v.list("").len())
            .unwrap_or(0);
        let tags = self
            .fetch(&format!("/repos/{repo}/tags?per_page=100"), cx)
            .ready()
            .map(|v| v.list("").len())
            .unwrap_or(0);
        let mut latest_row = widgets::row()
            .px_4()
            .py_2()
            .bg(rgb(palette().deep_bg))
            .border_b_1()
            .border_color(rgb(palette().divider));
        let head = self.commit_head(repo, &git_ref, cx);
        let total = head
            .as_ref()
            .map(|c| c.i("history.totalCount"))
            .unwrap_or(0);
        if let Some(c) = head {
            let sha = c.s("oid");
            let authors = self.commit_authors("latest", c.list("authors.nodes"), cx);
            latest_row = latest_row
                .child(authors)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(palette().text_dim))
                        .child(c.s("messageHeadline")),
                )
                .children(super::pulls::ci_mark_el(
                    "latest-checks",
                    &c.s("statusCheckRollup.state"),
                ))
                .child(
                    widgets::btn(
                        "latest-sha",
                        sha.chars().take(7).collect::<String>(),
                        Act::Go(Route::Commit {
                            repo: repo.to_string(),
                            sha,
                        }),
                    )
                    .h(px(22.0)),
                )
                .child(widgets::dim(time::ago(&c.s("committedDate"))));
        }
        let repo_s = repo.to_string();
        let main = widgets::col()
            .gap_3()
            .flex_1()
            .min_w_0()
            .child(
                widgets::row()
                    .child(picker)
                    .child(widgets::ibtn(
                        "branches-count",
                        "branch",
                        format!("{branches} branches"),
                        Act::Go(Route::Repo {
                            repo: repo_s.clone(),
                            tab: RepoTab::Branches,
                        }),
                    ))
                    .child(widgets::ibtn(
                        "tags-count",
                        "tag",
                        format!("{tags} tags"),
                        Act::Go(Route::Repo {
                            repo: repo_s.clone(),
                            tab: RepoTab::Tags,
                        }),
                    ))
                    .child(widgets::spacer())
                    .child(widgets::ibtn(
                        "history",
                        "history",
                        if total > 0 {
                            format!("{} commits", thousands(total))
                        } else {
                            "History".to_string()
                        },
                        Act::Go(Route::Repo {
                            repo: repo_s,
                            tab: RepoTab::Commits,
                        }),
                    )),
            )
            .child(widgets::card().child(latest_row).child(listing))
            .child(readme);
        div()
            .flex()
            .flex_row()
            .gap_6()
            .items_start()
            .child(main)
            .child(div().w(px(300.0)).flex_none().child(about))
            .into_any_element()
    }

    /// A directory's entries, folders first.
    pub fn directory(
        &mut self,
        repo: &str,
        git_ref: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let api = format!(
            "/repos/{repo}/contents/{}?ref={}",
            enc_path(path),
            enc(git_ref)
        );
        let listing = ready!(self.fetch(&api, cx));
        let mut entries: Vec<Value> = listing.list("").to_vec();
        entries.sort_by_key(|e| (e.s("type") != "dir", e.s("name").to_lowercase()));
        let paths: Vec<String> = entries.iter().map(|e| e.s("path")).collect();
        let last = self.last_commits(repo, git_ref, &paths, cx);
        let p = palette();
        let mut col = div().flex().flex_col();
        if !path.is_empty() {
            let parent = path
                .rsplit_once('/')
                .map(|(a, _)| a.to_string())
                .unwrap_or_default();
            col = col.child(
                widgets::list_row(
                    "up",
                    Act::Go(Route::Tree {
                        repo: repo.to_string(),
                        git_ref: git_ref.to_string(),
                        path: parent,
                        file: false,
                    }),
                )
                .py_1()
                .child(icon("folder", 16.0, p.accent_hover))
                .child(".."),
            );
        }
        for (i, e) in entries.iter().enumerate() {
            let kind = e.s("type");
            let (icon_name, color) = match kind.as_str() {
                "dir" => ("folder", p.accent_hover),
                "submodule" => ("repo", p.text_dim),
                "symlink" => ("link", p.text_dim),
                _ => ("file", p.text_dim),
            };
            let act = match kind.as_str() {
                "submodule" => Act::Url(e.s("html_url")),
                _ => Act::Go(Route::Tree {
                    repo: repo.to_string(),
                    git_ref: git_ref.to_string(),
                    path: e.s("path"),
                    file: kind != "dir",
                }),
            };
            let last = last.get(&e.s("path"));
            let message = last.map(|c| {
                let sha = c.s("oid");
                let go = Act::Go(Route::Commit {
                    repo: repo.to_string(),
                    sha,
                });
                div()
                    .id(ElementId::Name(format!("entry-commit-{i}").into()))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(rgb(p.text_dim))
                    .hover(|s| s.text_color(rgb(p.accent_hover)))
                    .child(c.s("messageHeadline"))
                    .on_click(move |_e, window, cx| {
                        cx.stop_propagation();
                        crate::hub::perform(go.clone(), window, cx);
                    })
            });
            col = col.child(
                widgets::list_row(ElementId::Name(format!("entry-{i}").into()), act)
                    .py_1()
                    .items_center()
                    .child(icon(icon_name, 16.0, color))
                    .child(
                        div()
                            .w(px(240.0))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(e.s("name")),
                    )
                    .child(match message {
                        Some(m) => m.into_any_element(),
                        None => div().flex_1().into_any_element(),
                    })
                    .when_some(last, |d, c| {
                        d.child(widgets::faint(time::ago(&c.s("committedDate"))))
                    }),
            );
        }
        col.into_any_element()
    }

    /// The README of a directory, rendered.
    pub fn readme(
        &mut self,
        repo: &str,
        git_ref: &str,
        dir: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let api = if dir.is_empty() {
            format!("/repos/{repo}/readme?ref={}", enc(git_ref))
        } else {
            format!(
                "/repos/{repo}/readme/{}?ref={}",
                enc_path(dir),
                enc(git_ref)
            )
        };
        let load = self.fetch_text(&api, "application/vnd.github.raw", cx);
        let text = match load {
            Load::Ready(v) => v.s(""),
            Load::Loading => return widgets::loading(),
            Load::Failed(_) => return div().into_any_element(),
        };
        let body = self.markdown(&format!("readme-{dir}"), &text, cx);
        widgets::card()
            .child(
                widgets::card_header()
                    .child(icon("book", 16.0, palette().text_dim))
                    .child(widgets::h3("README")),
            )
            .child(div().p_6().child(body))
            .into_any_element()
    }

    /// The commit `rev` points at, with its authors (co-authors resolved
    /// to accounts) and how many commits lead up to it.
    pub fn commit_head(&mut self, repo: &str, rev: &str, cx: &mut Context<Self>) -> Option<Value> {
        let (owner, name) = repo.split_once('/')?;
        let vars = json!({ "o": owner, "n": name, "r": rev });
        let data = self.fetch_gql(&format!("/repos/{repo}/commits"), COMMIT_HEAD, vars, cx);
        data.ready()
            .map(|v| v.at("repository.object").clone())
            .filter(|c| c.has("oid"))
    }

    /// The last commit to touch each of `paths` at `git_ref`, by path.
    /// Asked for in batches so a big directory doesn't make one huge query.
    fn last_commits(
        &mut self,
        repo: &str,
        git_ref: &str,
        paths: &[String],
        cx: &mut Context<Self>,
    ) -> HashMap<String, Value> {
        let mut found = HashMap::new();
        let Some((owner, name)) = repo.split_once('/') else {
            return found;
        };
        for chunk in paths.chunks(50) {
            let fields: String = chunk
                .iter()
                .enumerate()
                .map(|(i, path)| format!("e{i}: history(first: 1, path: {}) {{ nodes {{ oid messageHeadline committedDate }} }} ", Value::String(path.clone())))
                .collect();
            let query = format!("query($o: String!, $n: String!, $r: String!) {{ repository(owner: $o, name: $n) {{ object(expression: $r) {{ ... on Commit {{ {fields}}} }} }} }}");
            let vars = json!({ "o": owner, "n": name, "r": git_ref });
            if let Some(data) = self
                .fetch_gql(&format!("/repos/{repo}/commits"), &query, vars, cx)
                .ready()
            {
                let commit = data.at("repository.object");
                for (i, path) in chunk.iter().enumerate() {
                    if let Some(c) = commit.list(&format!("e{i}.nodes")).first() {
                        found.insert(path.clone(), c.clone());
                    }
                }
            }
        }
        found
    }

    /// A commit's authors as overlapping avatars and "a and b", each
    /// opening that person's profile.
    pub fn commit_authors(
        &mut self,
        id: &str,
        authors: &[Value],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let authors = distinct_authors(authors);
        widgets::row()
            .gap_2()
            .flex_none()
            .child(self.author_avatars(&authors, cx))
            .child(author_names(id, &authors).font_weight(FontWeight::SEMIBOLD))
            .into_any_element()
    }

    /// Authors' avatars, overlapping.
    pub fn author_avatars(&mut self, authors: &[Value], cx: &mut Context<Self>) -> AnyElement {
        let mut avatars = div().flex().flex_row().flex_none();
        for (i, a) in authors.iter().enumerate() {
            let url = if a.has("user.avatarUrl") {
                a.s("user.avatarUrl")
            } else {
                a.s("avatarUrl")
            };
            let avatar = self.avatar(&url, 20.0, cx);
            avatars = avatars.child(div().when(i > 0, |d| d.ml(px(-6.0))).child(avatar));
        }
        avatars.into_any_element()
    }

    fn repo_about(&mut self, repo: &str, info: &Value, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let repo_s = repo.to_string();
        let stat = |id: &str, icon_name: &str, text: String, act: Act| {
            widgets::ibtn(ElementId::Name(id.to_string().into()), icon_name, text, act)
                .h(px(26.0))
                .gap_1p5()
        };
        let mut about = widgets::col()
            .gap_2()
            .child(widgets::h3("About"))
            .when(!info.s("description").is_empty(), |d| {
                d.child(div().child(info.s("description")))
            })
            .when(info.has("license"), |d| {
                d.child(widgets::icon_text(
                    "book",
                    info.s("license.name"),
                    p.text_dim,
                ))
            })
            // Bitbucket counts none of these.
            .when(!crate::forge::is_bitbucket(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .child(stat(
                            "stargazers",
                            "star",
                            format!("{} stars", json::count(info.i("stargazers_count"))),
                            Act::choose(format!("insights.view:{repo_s}"), "stargazers").then_go(
                                Route::Repo {
                                    repo: repo_s.clone(),
                                    tab: RepoTab::Insights,
                                },
                            ),
                        ))
                        // GitLab doesn't count watchers.
                        .when(!crate::forge::is_gitlab(), |d| {
                            d.child(stat(
                                "watchers",
                                "eye",
                                format!("{} watching", json::count(info.i("subscribers_count"))),
                                Act::choose(format!("insights.view:{repo_s}"), "watchers").then_go(
                                    Route::Repo {
                                        repo: repo_s.clone(),
                                        tab: RepoTab::Insights,
                                    },
                                ),
                            ))
                        })
                        .child(stat(
                            "forks",
                            "fork",
                            format!("{} forks", json::count(info.i("forks_count"))),
                            Act::choose(format!("insights.view:{repo_s}"), "forks").then_go(
                                Route::Repo {
                                    repo: repo_s.clone(),
                                    tab: RepoTab::Insights,
                                },
                            ),
                        )),
                )
            });

        // Latest release.
        if let Some(release) = self
            .fetch(&format!("/repos/{repo}/releases?per_page=1"), cx)
            .ready()
            .and_then(|v| v.list("").first().cloned())
        {
            about = about
                .child(div().h(px(1.0)).bg(rgb(p.divider)))
                .child(widgets::h3("Latest release"))
                .child(
                    widgets::ibtn(
                        "latest-release",
                        "tag",
                        format!(
                            "{}  ·  {}",
                            release.s("tag_name"),
                            time::ago(&release.s("published_at"))
                        ),
                        Act::Go(Route::Release {
                            repo: repo_s.clone(),
                            id: release.i("id") as u64,
                        }),
                    )
                    .w_full()
                    .justify_start(),
                );
        }

        // Languages, as a bar and a legend.
        if let Some(langs) = self
            .fetch(&format!("/repos/{repo}/languages"), cx)
            .ready()
            .cloned()
        {
            if let Value::Object(map) = &*langs {
                let total: i64 = map.values().filter_map(|v| v.as_i64()).sum();
                if total > 0 {
                    let mut bar = div()
                        .flex()
                        .flex_row()
                        .h(px(8.0))
                        .rounded_full()
                        .overflow_hidden();
                    let mut legend = div().flex().flex_row().flex_wrap().gap_x_3().gap_y_1();
                    let mut sorted: Vec<(&String, i64)> = map
                        .iter()
                        .map(|(k, v)| (k, v.as_i64().unwrap_or(0)))
                        .collect();
                    sorted.sort_by_key(|l| std::cmp::Reverse(l.1));
                    for (name, bytes) in sorted.iter().take(8) {
                        let color = language_color(name);
                        let pct = *bytes as f32 * 100.0 / total as f32;
                        bar =
                            bar.child(div().h_full().w(gpui::relative(pct / 100.0)).bg(rgb(color)));
                        legend = legend.child(
                            widgets::row()
                                .gap_1()
                                .child(div().size(px(8.0)).rounded_full().bg(rgb(color)))
                                .child(div().text_size(px(12.0)).child(name.to_string()))
                                .child(widgets::faint(format!("{pct:.1}%"))),
                        );
                    }
                    about = about
                        .child(div().h(px(1.0)).bg(rgb(p.divider)))
                        .child(widgets::h3("Languages"))
                        .child(bar)
                        .child(legend);
                }
            }
        }

        // Contributors.
        if let Some(people) = self
            .fetch(&format!("/repos/{repo}/contributors?per_page=24"), cx)
            .ready()
            .cloned()
        {
            let mut grid = div().flex().flex_row().flex_wrap().gap_1();
            for (i, person) in people.list("").iter().enumerate() {
                let avatar = self.avatar(&person.s("avatar_url"), 28.0, cx);
                let login = person.s("login");
                grid = grid.child(
                    div()
                        .id(("contributor", i))
                        .cursor_pointer()
                        .tooltip(crate::ui::tip(
                            format!("{login} · {} commits", person.i("contributions")),
                            None,
                        ))
                        .child(avatar)
                        .on_click(on(Act::Go(Route::User { login }))),
                );
            }
            about = about
                .child(div().h(px(1.0)).bg(rgb(p.divider)))
                .child(widgets::h3("Contributors"))
                .child(grid);
        }
        about.into_any_element()
    }

    fn repo_branches(&mut self, repo: &str, info: &Value, cx: &mut Context<Self>) -> AnyElement {
        let default_branch = info.s("default_branch");
        let admin = info.b("permissions.admin");
        let repo_s = repo.to_string();
        let spec = ListSpec::new(format!("/repos/{repo}/branches"), move |b| {
            let name = b.s("name");
            let is_default = name == default_branch;
            let inval = format!("/repos/{repo_s}/branches");
            let protected = b.b("protected");
            let mut row = Row::new(name.clone())
                .icon("branch", widgets::gray())
                .meta(format!(
                    "Last commit {}",
                    b.s("commit.sha").chars().take(7).collect::<String>()
                ))
                .open(Act::run({
                    let (repo, name) = (repo_s.clone(), name.clone());
                    move |hub, _, cx| {
                        hub.choices.insert(format!("ref:{repo}"), name.clone());
                        hub.go(
                            Route::Repo {
                                repo: repo.clone(),
                                tab: RepoTab::Code,
                            },
                            cx,
                        );
                    }
                }));
            if is_default {
                row = row.tag("default", widgets::gray());
            }
            if protected {
                row = row.tag("protected", widgets::yellow());
            }
            if !is_default {
                row = row
                    .action(
                        format!("New {}", crate::forge::pr()),
                        super::pulls::new_pull_form(&repo_s, &default_branch, &name),
                    )
                    .action(
                        "Compare",
                        Act::Go(Route::Compare {
                            repo: repo_s.clone(),
                            base: default_branch.clone(),
                            head: name.clone(),
                        }),
                    );
                if admin {
                    row = row.action(
                        "Set as default",
                        Req::rest("PATCH", format!("/repos/{repo_s}"))
                            .body(json!({ "default_branch": name }))
                            .ok(format!("{name} is now the default branch"))
                            .inval(format!("/repos/{repo_s}"))
                            .act(),
                    );
                }
            }
            // Bitbucket can't rename a branch, or protect one the way
            // GitHub does; its branch restrictions are in its settings.
            let bitbucket = crate::forge::is_bitbucket();
            if !bitbucket {
                row = row.action(
                    "Rename",
                    FormSpec::new(format!("Rename {name}"))
                        .submit("Rename branch")
                        .field(
                            Field::text("new_name", "New name")
                                .value(name.clone())
                                .required(),
                        )
                        .rest(
                            "POST",
                            format!("/repos/{repo_s}/branches/{}/rename", enc(&name)),
                        )
                        .ok("Branch renamed")
                        .inval(inval.clone())
                        .act(),
                );
            }
            if admin && !bitbucket {
                row = row.action(
                    "Protection rules…",
                    if crate::forge::is_gitlab() {
                        gitlab_protection_form(&repo_s, &name)
                    } else {
                        protection_form(&repo_s, &name)
                    },
                );
                if protected {
                    row = row.action(
                        "Remove protection",
                        Req::rest(
                            "DELETE",
                            format!("/repos/{repo_s}/branches/{}/protection", enc(&name)),
                        )
                        .ok("Protection removed")
                        .inval(inval.clone())
                        .act()
                        .confirm(
                            "Remove protection?",
                            format!("Anyone with write access will be able to push to {name}."),
                            "Remove",
                        ),
                    );
                }
            }
            if !is_default {
                row = row.danger(
                    "Delete branch",
                    Req::rest(
                        "DELETE",
                        format!("/repos/{repo_s}/git/refs/heads/{}", enc_path(&name)),
                    )
                    .ok(format!("Deleted {name}"))
                    .inval(inval)
                    .act()
                    .confirm(
                        "Delete branch?",
                        format!(
                            "{name} will be deleted. Open {} from it will be closed.",
                            crate::forge::prs()
                        ),
                        "Delete",
                    ),
                );
            }
            row
        })
        .empty("No branches.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::h2("Branches"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn(
                        "new-branch",
                        "New branch",
                        new_branch_form(repo, ""),
                    )),
            )
            .child(list)
            .into_any_element()
    }

    fn repo_tags(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let repo_s = repo.to_string();
        let spec = ListSpec::new(format!("/repos/{repo}/tags"), move |t| {
            let name = t.s("name");
            let sha = t.s("commit.sha");
            Row::new(name.clone())
                .icon("tag", widgets::gray())
                .meta(sha.chars().take(7).collect::<String>())
                .open(Act::Go(Route::Tree {
                    repo: repo_s.clone(),
                    git_ref: name.clone(),
                    path: String::new(),
                    file: false,
                }))
                .action(
                    "Create release",
                    super::releases::release_form(&repo_s, None, &name),
                )
                .action(
                    "Download ZIP",
                    Act::Url(archive_url(&repo_s, &name, "zip", true)),
                )
                .action(
                    "Download tar.gz",
                    Act::Url(archive_url(&repo_s, &name, "tar.gz", true)),
                )
                .action(
                    "Commit",
                    Act::Go(Route::Commit {
                        repo: repo_s.clone(),
                        sha,
                    }),
                )
                .danger(
                    "Delete tag",
                    Req::rest(
                        "DELETE",
                        format!("/repos/{repo_s}/git/refs/tags/{}", enc_path(&name)),
                    )
                    .ok(format!("Deleted tag {name}"))
                    .inval(format!("/repos/{repo_s}/tags"))
                    .act()
                    .confirm(
                        "Delete tag?",
                        format!(
                            "The tag {name} will be deleted. Releases that use it become drafts."
                        ),
                        "Delete",
                    ),
                )
        })
        .empty("No tags.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::h2("Tags"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-tag", "New tag", new_tag_form(repo))),
            )
            .child(list)
            .into_any_element()
    }

    fn repo_commits(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let git_ref = self.current_ref(repo, default_branch);
        let picker = self.ref_picker(repo, &git_ref, cx);
        let path_field = format!("commits.path:{repo}");
        let author_field = format!("commits.author:{repo}");
        let applied_key = format!("commits.applied:{repo}");
        let (fp, fa, ak) = (
            path_field.clone(),
            author_field.clone(),
            applied_key.clone(),
        );
        let apply = Act::run(move |hub, _, cx| {
            let value = format!(
                "{}\u{1}{}",
                hub.field_text(&fp).trim(),
                hub.field_text(&fa).trim()
            );
            hub.choices.insert(ak.clone(), value);
            cx.notify();
        });
        self.submits.insert(path_field.clone(), apply.clone());
        self.submits.insert(author_field.clone(), apply);
        let applied = self.choice(&applied_key, "\u{1}");
        let (path_filter, author_filter) = applied.split_once('\u{1}').unwrap_or(("", ""));
        let mut api = format!("/repos/{repo}/commits?sha={}", enc(&git_ref));
        if !path_filter.is_empty() {
            api.push_str(&format!("&path={}", enc(path_filter)));
        }
        if !author_filter.is_empty() {
            api.push_str(&format!("&author={}", enc(author_filter)));
        }
        let repo_s = repo.to_string();
        let spec = ListSpec::new(api, move |c| commit_row(&repo_s, c)).empty("No commits match.");
        let list = self.list(&spec, cx);
        let path_input = self
            .input(&path_field, "Path filter — Enter", cx)
            .w(px(220.0));
        let author_input = self
            .input(&author_field, "Author login or email — Enter", cx)
            .w(px(220.0));
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(picker)
                    .child(path_input)
                    .child(author_input),
            )
            .child(list)
            .into_any_element()
    }

    /// A directory or a file at a ref.
    pub fn tree(
        &mut self,
        repo: &str,
        git_ref: &str,
        path: &str,
        file: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let crumbs = self.path_crumbs(repo, git_ref, path);
        if file {
            return self.file_view(repo, git_ref, path, crumbs, cx);
        }
        let listing = self.directory(repo, git_ref, path, cx);
        let readme = self.readme(repo, git_ref, path, cx);
        widgets::page()
            .child(crumbs)
            .child(widgets::card().child(listing))
            .child(readme)
            .into_any_element()
    }

    fn path_crumbs(&self, repo: &str, git_ref: &str, path: &str) -> AnyElement {
        let p = palette();
        let mut row = widgets::row()
            .flex_wrap()
            .gap_1()
            .text_size(px(15.0))
            .child(widgets::tag(git_ref.to_string(), p.text_dim))
            .child(
                crate::ui::Link::new(
                    "crumb-root",
                    repo.rsplit('/').next().unwrap_or(repo).to_string(),
                )
                .font_weight(FontWeight::SEMIBOLD)
                .on_click(on(Act::Go(Route::Tree {
                    repo: repo.to_string(),
                    git_ref: git_ref.to_string(),
                    path: String::new(),
                    file: false,
                }))),
            );
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            row = row.child(div().text_color(rgb(p.text_faint)).child("/"));
            if i + 1 == parts.len() {
                row = row.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(part.to_string()),
                );
            } else {
                row = row.child(
                    crate::ui::Link::new(
                        ElementId::Name(format!("crumb-{i}").into()),
                        part.to_string(),
                    )
                    .on_click(on(Act::Go(Route::Tree {
                        repo: repo.to_string(),
                        git_ref: git_ref.to_string(),
                        path: parts[..=i].join("/"),
                        file: false,
                    }))),
                );
            }
        }
        row.child(widgets::spacer())
            .child(widgets::btn(
                "copy-path",
                "Copy path",
                Act::Copy(path.to_string()),
            ))
            .into_any_element()
    }

    /// A file, read-only: rendered Markdown, an image, or numbered lines.
    fn file_view(
        &mut self,
        repo: &str,
        git_ref: &str,
        path: &str,
        crumbs: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let lower = path.to_lowercase();
        let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
            .iter()
            .any(|e| lower.ends_with(e));
        let is_markdown = lower.ends_with(".md") || lower.ends_with(".markdown");
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
        let raw_url = if bitbucket {
            format!("{}/{repo}/raw/{git_ref}/{path}", crate::forge::web())
        } else if gitlab {
            format!("{}/{repo}/-/raw/{git_ref}/{path}", crate::forge::web())
        } else {
            format!("https://raw.githubusercontent.com/{repo}/{git_ref}/{path}")
        };
        // GitLab's API serves a private project's file to the token too.
        // Bitbucket's API serves a private repository's file to the token.
        let image_url = if bitbucket {
            format!(
                "{}/repositories/{repo}/src/{}/{}",
                crate::forge::BITBUCKET_API,
                enc(git_ref),
                enc_path(path)
            )
        } else if gitlab {
            format!(
                "{}{}/repository/files/{}/raw?ref={}",
                crate::forge::web(),
                crate::screens::gitlab::project_api(repo),
                enc(path),
                enc(git_ref)
            )
        } else {
            raw_url.clone()
        };
        let blame_url = if bitbucket {
            format!("{}/{repo}/annotate/{git_ref}/{path}", crate::forge::web())
        } else if gitlab {
            format!("{}/{repo}/-/blame/{git_ref}/{path}", crate::forge::web())
        } else {
            format!("{}/{repo}/blame/{git_ref}/{path}", crate::forge::web())
        };
        let history = Act::run({
            let (repo, path) = (repo.to_string(), path.to_string());
            move |hub, _, cx| {
                hub.choices
                    .insert(format!("commits.applied:{repo}"), format!("{path}\u{1}"));
                hub.set_field(&format!("commits.path:{repo}"), path.clone());
                hub.go(
                    Route::Repo {
                        repo: repo.clone(),
                        tab: RepoTab::Commits,
                    },
                    cx,
                );
            }
        });
        let view_key = format!("file.view:{repo}:{path}");
        let view = self.choice(&view_key, if is_markdown { "preview" } else { "code" });
        let api = format!(
            "/repos/{repo}/contents/{}?ref={}",
            enc_path(path),
            enc(git_ref)
        );

        let mut toolbar = widgets::row()
            .px_4()
            .py_2()
            .bg(rgb(p.deep_bg))
            .border_b_1()
            .border_color(rgb(p.divider));
        if is_markdown {
            toolbar = toolbar.child(widgets::chips(vec![
                (
                    "Preview".into(),
                    view == "preview",
                    Act::choose(&view_key, "preview"),
                ),
                (
                    "Code".into(),
                    view == "code",
                    Act::choose(&view_key, "code"),
                ),
            ]));
        }

        let body: AnyElement = if is_image {
            let image = self.image(&image_url, cx);
            div()
                .p_4()
                .flex()
                .justify_center()
                .child(image)
                .into_any_element()
        } else {
            let text = match self.fetch_text(&api, "application/vnd.github.raw", cx) {
                Load::Ready(v) => v.s(""),
                other => {
                    return widgets::page()
                        .child(crumbs)
                        .child(widgets::placeholder(&other))
                        .into_any_element()
                }
            };
            let lines = text.lines().count();
            toolbar = toolbar.child(widgets::dim(format!(
                "{lines} lines  ·  {}",
                json::bytes(text.len() as i64)
            )));
            let copy_text = text.clone();
            toolbar = toolbar.child(widgets::spacer()).child(widgets::btn(
                "copy-file",
                "Copy",
                Act::Copy(copy_text),
            ));
            if is_markdown && view == "preview" {
                let md = self.markdown(&format!("file-{path}"), &text, cx);
                div()
                    .id("file-md")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroller("file-md"))
                    .p_6()
                    .child(md)
                    .into_any_element()
            } else {
                numbered_lines(&text, path, self.list_scroller("file-lines"))
            }
        };
        if is_image {
            toolbar = toolbar.child(widgets::spacer());
        }
        toolbar = toolbar
            .child(widgets::btn("raw", "Raw", Act::Url(raw_url.clone())))
            .child(widgets::btn("blame", "Blame", Act::Url(blame_url)))
            .child(widgets::ibtn("file-history", "history", "History", history));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_3()
            .px_6()
            .py_4()
            .child(crumbs)
            .child(
                widgets::card()
                    .flex_1()
                    .min_h_0()
                    .child(toolbar)
                    .child(body),
            )
            .into_any_element()
    }
}

/// Monospace lines with numbers, virtualised so large files stay quick.
pub fn numbered_lines(text: &str, path: &str, scroll: gpui::UniformListScrollHandle) -> AnyElement {
    let p = palette();
    let lines: std::rc::Rc<Vec<String>> =
        std::rc::Rc::new(text.lines().map(|l| l.replace('\t', "    ")).collect());
    let colours = crate::highlight::lines(crate::highlight::syntax_for(path), &lines, &[]);
    let block = format!("file:{path}");
    let count = lines.len();
    let width = (count.to_string().len() as f32) * 8.0 + 24.0;
    gpui::uniform_list("file-lines", count, move |range, _window, _cx| {
        range
            .map(|i| {
                div()
                    .flex()
                    .flex_row()
                    .h(px(20.0))
                    .child(
                        div()
                            .w(px(width))
                            .flex_none()
                            .pr_3()
                            .text_right()
                            .text_color(rgb(p.text_faint))
                            .child((i + 1).to_string()),
                    )
                    .child(crate::select::line(
                        &block,
                        &lines,
                        i,
                        colours.as_ref().map(|c| c[i].as_slice()),
                    ))
            })
            .collect()
    })
    .track_scroll(scroll)
    .flex_1()
    .min_h_0()
    .py_2()
    .font_family(widgets::MONO)
    .text_size(px(12.0))
    .into_any_element()
}

/// GitHub's colour for a language, for the common ones.
pub fn language_color(name: &str) -> u32 {
    match name {
        "Rust" => 0xDEA584,
        "JavaScript" => 0xF1E05A,
        "TypeScript" => 0x3178C6,
        "Python" => 0x3572A5,
        "Go" => 0x00ADD8,
        "Java" => 0xB07219,
        "C" => 0x555555,
        "C++" => 0xF34B7D,
        "C#" => 0x178600,
        "Ruby" => 0x701516,
        "PHP" => 0x4F5D95,
        "Swift" => 0xF05138,
        "Kotlin" => 0xA97BFF,
        "Shell" => 0x89E051,
        "HTML" => 0xE34C26,
        "CSS" => 0x563D7C,
        "SCSS" => 0xC6538C,
        "Vue" => 0x41B883,
        "Dart" => 0x00B4AB,
        "Lua" => 0x000080,
        "Makefile" => 0x427819,
        "Dockerfile" => 0x384D54,
        "Nix" => 0x7E7EFF,
        "Zig" => 0xEC915C,
        "Haskell" => 0x5E5086,
        "Elixir" => 0x6E4A7E,
        "Scala" => 0xC22D40,
        "Objective-C" => 0x438EFF,
        "PowerShell" => 0x012456,
        "Jupyter Notebook" => 0xDA5B0B,
        "MDX" => 0xFCB32C,
        "WGSL" => 0x1A5E9A,
        _ => {
            // A stable colour from the name for everything else.
            let h = name
                .bytes()
                .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            0x404040 | (h & 0x9F9F9F)
        }
    }
}

trait ThenGo {
    fn then_go(self, route: Route) -> Act;
}

impl ThenGo for Act {
    /// Do this, then go somewhere.
    fn then_go(self, route: Route) -> Act {
        Act::run(move |hub, window, cx| {
            hub.perform(self.clone(), window, cx);
            hub.go(route.clone(), cx);
        })
    }
}
