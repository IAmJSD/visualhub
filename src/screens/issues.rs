//! Issues: yours across GitHub, a repository's, one issue's conversation
//! with everything its sidebar edits, and labels and milestones.

use super::common::{
    self, api_path, delete_comment, edit_comment, issue_row, post_comment, side_section,
    CommentActs,
};
use crate::picker::{PickItem, Picker};
use crate::form::{DropOption, Field, FormSpec};
use crate::hub::{on, Act, Hub, MenuEntry, Req, Route, RepoTab};
use crate::json::{enc, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, ElementId, FontWeight, IntoElement as _, ParentElement as _, Styled as _};
use crate::ui::palette;
use serde_json::{json, Value};

pub fn new_issue_form(repo: &str) -> Act {
    let repo = repo.to_string();
    let target = repo.clone();
    FormSpec::new(format!("New issue in {repo}"))
        .submit("Submit new issue")
        .width(640.0)
        .field(Field::text("title", "Title").required())
        .field(Field::multiline("body", "Description").hint("Markdown is supported."))
        .field(Field::dropdown("labels", "Labels", format!("/repos/{repo}/labels?per_page=100"), |l| {
            DropOption::new(l.s("name"), l.s("name")).color(l.s("color")).detail(l.s("description"))
        }))
        .field(Field::dropdown("assignees", "Assignees", format!("/repos/{repo}/assignees?per_page=100"), |u| {
            DropOption::new(u.s("login"), u.s("login")).avatar(u.s("avatar_url"))
        }))
        .field(Field::dropdown_one("milestone", "Milestone", format!("/repos/{repo}/milestones?state=open&per_page=100"), "No milestone", |m| {
            let due = if m.has("due_on") { format!("Due {}", time::date(&m.s("due_on"))) } else { "No due date".to_string() };
            DropOption::new(m.i("number"), m.s("title")).detail(due)
        }))
        .rest("POST", format!("/repos/{repo}/issues"))
        .ok("Issue created")
        .inval(format!("/repos/{repo}/issues"))
        .then(move |hub, value, cx| {
            hub.go(
                Route::Issue {
                    repo: target.clone(),
                    number: value.i("number") as u64,
                },
                cx,
            );
        })
        .act()
}

/// How many issues and pull requests a repository has open and closed.
const ISSUE_COUNTS: &str = "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { openIssues: issues(states: OPEN) { totalCount } closedIssues: issues(states: CLOSED) { totalCount } openPulls: pullRequests(states: OPEN) { totalCount } closedPulls: pullRequests(states: [CLOSED, MERGED]) { totalCount } } }";

/// A search query over issues and pull requests as a paged list.
pub fn search_spec(query: &str, show_repo: bool) -> ListSpec {
    ListSpec::new(
        format!("/search/issues?q={}&sort=updated", enc(query)),
        move |item| issue_row(item, show_repo),
    )
    .items("items")
    .empty("No results matched.")
}

impl Hub {
    /// A repository's open and closed counts (`openIssues`, `closedPulls`,
    /// ...), once known. Kept with its issues, so creating or closing one
    /// counts again.
    pub fn issue_counts(&mut self, repo: &str, cx: &mut Context<Self>) -> Option<Value> {
        let (owner, name) = repo.split_once('/')?;
        let vars = json!({ "o": owner, "n": name });
        self.fetch_gql(&format!("/repos/{repo}/issues"), ISSUE_COUNTS, vars, cx)
            .ready()
            .map(|v| v.at("repository").clone())
    }

    /// Issues you created, are assigned, or are mentioned in.
    pub fn my_issues(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.my_items("issue", cx)
    }

    /// The shared page behind Issues and Pull requests.
    pub fn my_items(&mut self, kind: &str, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("my.{kind}");
        let filter = self.choice(&key, "created");
        let state_key = format!("{key}.state");
        let state = self.choice(&state_key, "open");
        let mut filters = vec![
            ("created", "Created", "author:@me"),
            ("assigned", "Assigned", "assignee:@me"),
            ("mentioned", "Mentioned", "mentions:@me"),
        ];
        if kind == "pr" {
            filters.push(("review", "Review requests", "review-requested:@me"));
            filters.push(("reviewed", "Reviewed by you", "reviewed-by:@me"));
        }
        let qualifier = filters
            .iter()
            .find(|f| f.0 == filter)
            .map(|f| f.2)
            .unwrap_or("author:@me");
        let query = format!("is:{kind} is:{state} {qualifier} archived:false");
        let spec = search_spec(&query, true);
        let list = self.list(&spec, cx);
        let title = if kind == "pr" { "Pull requests" } else { "Issues" };
        widgets::page()
            .child(widgets::title(title))
            .child(
                widgets::row()
                    .justify_between()
                    .child(widgets::chips(
                        filters
                            .iter()
                            .map(|(v, l, _)| (l.to_string(), filter == *v, Act::choose(&key, *v)))
                            .collect(),
                    ))
                    .child(widgets::chips(vec![
                        ("Open".into(), state == "open", Act::choose(&state_key, "open")),
                        ("Closed".into(), state == "closed", Act::choose(&state_key, "closed")),
                    ])),
            )
            .child(widgets::faint(query))
            .child(list)
            .into_any_element()
    }

    /// A repository's Issues tab.
    pub fn repo_issues(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let view_key = format!("issues.view:{repo}");
        let view = self.choice(&view_key, "list");
        let header = widgets::row()
            .child(widgets::chips(vec![
                ("Issues".into(), view == "list", Act::choose(&view_key, "list")),
                ("Labels".into(), view == "labels", Act::choose(&view_key, "labels")),
                ("Milestones".into(), view == "milestones", Act::choose(&view_key, "milestones")),
            ]))
            .child(widgets::spacer());
        match view.as_str() {
            "labels" => {
                let body = self.labels_view(repo, cx);
                widgets::col().gap_3().child(header).child(body).into_any_element()
            }
            "milestones" => {
                let body = self.milestones_view(repo, cx);
                widgets::col().gap_3().child(header).child(body).into_any_element()
            }
            _ => {
                let list = self.issue_list(repo, "issue", cx);
                widgets::col()
                    .gap_3()
                    .child(header.child(widgets::go_btn(
                        "new-issue",
                        "New issue",
                        new_issue_form(repo),
                    )))
                    .child(list)
                    .into_any_element()
            }
        }
    }

    /// The filterable list of a repository's issues or pull requests.
    pub fn issue_list(&mut self, repo: &str, kind: &str, cx: &mut Context<Self>) -> AnyElement {
        let state_key = format!("{kind}.state:{repo}");
        let state = self.choice(&state_key, "open");
        let sort_key = format!("{kind}.sort:{repo}");
        let sort = self.choice(&sort_key, "created-desc");
        let query_field = format!("{kind}.q:{repo}");
        let applied_key = format!("{kind}.applied:{repo}");
        let applied = self.choice(&applied_key, "");
        let label_key = format!("{kind}.label:{repo}");
        let label = self.choice(&label_key, "");

        let (sort_field, direction) = sort.split_once('-').unwrap_or(("created", "desc"));
        let spec = if applied.trim().is_empty() {
            let path = if kind == "pr" {
                format!(
                    "/repos/{repo}/pulls?state={state}&sort={sort_field}&direction={direction}"
                )
            } else {
                let mut path = format!(
                    "/repos/{repo}/issues?state={state}&sort={sort_field}&direction={direction}"
                );
                if !label.is_empty() {
                    path.push_str(&format!("&labels={}", enc(&label)));
                }
                path
            };
            let is_pr = kind == "pr";
            let repo_for_rows = repo.to_string();
            ListSpec::new(path, move |item| {
                let mut row = issue_row(item, false);
                if is_pr {
                    let mut item = item.clone();
                    item["pull_request"] = json!({});
                    row = issue_row(&item, false);
                    row = row.meta(format!(
                        "#{} opened {} by {}  ·  {} → {}",
                        item.i("number"),
                        time::ago(&item.s("created_at")),
                        item.s("user.login"),
                        item.s("head.label"),
                        item.s("base.ref")
                    ));
                }
                let _ = &repo_for_rows;
                row
            })
            .filter(move |item| is_pr || !item.has("pull_request"))
            .empty(if kind == "pr" {
                "No pull requests match."
            } else {
                "No issues match."
            })
        } else {
            let mut query = format!("repo:{repo} is:{kind} {applied}");
            if state != "all" {
                query.push_str(&format!(" is:{state}"));
            }
            if !label.is_empty() {
                query.push_str(&format!(" label:\"{label}\""));
            }
            search_spec(&query, false)
        };

        let apply_key = applied_key.clone();
        let field = query_field.clone();
        self.submits.insert(
            query_field.clone(),
            Act::run(move |hub, _, cx| {
                let text = hub.field_text(&field);
                hub.choices.insert(apply_key.clone(), text);
                cx.notify();
            }),
        );
        let search = self
            .input(&query_field, "Filter, e.g. author:octocat bug in:title — Enter", cx)
            .w(px(360.0));

        let labels_menu = {
            let mut entries = vec![MenuEntry::check(
                "Any label",
                label.is_empty(),
                Act::choose(&label_key, ""),
            )];
            if let Some(labels) = self.fetch(&format!("/repos/{repo}/labels?per_page=100"), cx).ready().cloned() {
                for l in labels.list("") {
                    let name = l.s("name");
                    entries.push(MenuEntry::check(
                        name.clone(),
                        label == name,
                        Act::choose(&label_key, name),
                    ));
                }
            }
            Act::menu(entries)
        };
        let sorts = [
            ("created-desc", "Newest"),
            ("created-asc", "Oldest"),
            ("comments-desc", "Most commented"),
            ("updated-desc", "Recently updated"),
        ];
        let sort_menu = Act::menu(
            sorts
                .iter()
                .map(|(v, l)| MenuEntry::check(*l, sort == *v, Act::choose(&sort_key, *v)))
                .collect(),
        );
        let list = self.list(&spec, cx);
        // "12 Open", "34 Closed", as github.com heads the list.
        let counts = self.issue_counts(repo, cx);
        let counted = |label: &str, key: &str| match counts.as_ref().map(|c| c.i(&format!("{key}.totalCount"))) {
            Some(n) => format!("{n} {label}"),
            None => label.to_string(),
        };
        let (open_key, closed_key) = if kind == "pr" { ("openPulls", "closedPulls") } else { ("openIssues", "closedIssues") };
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .flex_wrap()
                    .child(widgets::chips(vec![
                        (counted("Open", open_key), state == "open", Act::choose(&state_key, "open")),
                        (counted("Closed", closed_key), state == "closed", Act::choose(&state_key, "closed")),
                        ("All".into(), state == "all", Act::choose(&state_key, "all")),
                    ]))
                    .child(search)
                    .child(widgets::btn(
                        ElementId::Name(format!("{kind}-labels").into()),
                        if label.is_empty() { "Label ▾".to_string() } else { format!("Label: {label} ▾") },
                        labels_menu,
                    ))
                    .when(kind == "issue" || applied.is_empty(), |d| {
                        d.child(widgets::btn(
                            ElementId::Name(format!("{kind}-sort").into()),
                            format!(
                                "Sort: {} ▾",
                                sorts.iter().find(|s| s.0 == sort).map(|s| s.1).unwrap_or("Newest")
                            ),
                            sort_menu,
                        ))
                    }),
            )
            .child(list)
            .into_any_element()
    }

    /// One issue's page.
    pub fn issue(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/issues/{number}");
        let issue = ready!(self.fetch(&path, cx));
        if issue.has("pull_request") {
            // An issue number that is really a pull request.
            let route = Route::Pull {
                repo: repo.to_string(),
                number,
                tab: crate::hub::PullTab::Conversation,
            };
            return widgets::page()
                .child(widgets::dim("This is a pull request."))
                .child(widgets::primary("open-pr", "Open the pull request", Act::Go(route)))
                .into_any_element();
        }
        let header = self.issue_header(repo, &issue, false, cx);
        let timeline = self.issue_timeline(repo, number, &issue, cx);
        let sidebar = self.issue_sidebar(repo, &issue, false, cx);
        widgets::page()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_6()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(timeline))
                    .child(div().w(px(280.0)).flex_none().child(sidebar)),
            )
            .into_any_element()
    }

    /// The title, state and byline shared by issues and pull requests.
    pub fn issue_header(
        &mut self,
        repo: &str,
        issue: &Value,
        is_pr: bool,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = issue.i("number");
        let (icon_name, _) = common::issue_icon(issue);
        let state = issue.s("state");
        let merged = issue.b("merged") || issue.has("merged_at");
        let (label, fill) = if merged {
            ("Merged", widgets::purple_fill())
        } else if state == "closed" {
            if !is_pr && issue.s("state_reason") == "not_planned" {
                ("Closed as not planned", widgets::gray_fill())
            } else if is_pr {
                ("Closed", widgets::red_fill())
            } else {
                ("Closed", widgets::purple_fill())
            }
        } else if issue.b("draft") {
            ("Draft", widgets::gray_fill())
        } else {
            ("Open", widgets::green_fill())
        };
        let kind = if is_pr { "pulls" } else { "issues" };
        let edit = FormSpec::new(if is_pr { "Edit pull request" } else { "Edit issue" })
            .width(680.0)
            .field(Field::text("title", "Title").value(issue.s("title")).required())
            .field(Field::multiline("body", "Description").value(issue.s("body")).keep_empty())
            .rest("PATCH", format!("/repos/{repo}/{kind}/{number}"))
            .ok("Saved")
            .inval(format!("/repos/{repo}/issues/{number}"))
            .inval(format!("/repos/{repo}/pulls/{number}"))
            .act();
        let byline = if is_pr {
            format!(
                "{} wants to merge {} commits into {} from {}  ·  opened {}",
                issue.s("user.login"),
                issue.i("commits"),
                issue.s("base.ref"),
                issue.s("head.label"),
                time::ago(&issue.s("created_at"))
            )
        } else {
            format!(
                "{} opened this issue {}  ·  {} comments",
                issue.s("user.login"),
                time::ago(&issue.s("created_at")),
                issue.i("comments")
            )
        };
        widgets::col()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(rgb(palette().divider))
            .child(
                widgets::row()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .text_size(px(24.0))
                            .child(issue.s("title"))
                            .child(
                                div()
                                    .text_color(rgb(palette().text_dim))
                                    .child(format!("#{number}")),
                            ),
                    )
                    .child(widgets::btn("edit-issue", "Edit", edit))
                    .child(widgets::btn("copy-issue-link", "Copy link", Act::Copy(issue.s("html_url")))),
            )
            .child(
                widgets::row()
                    .child(widgets::pill(icon_name, label, fill))
                    .child(widgets::dim(byline)),
            )
            .into_any_element()
    }

    /// The body, then every comment and event, then the composer.
    fn issue_timeline(&mut self, repo: &str, number: u64, issue: &Value, cx: &mut Context<Self>) -> AnyElement {
        let inval = format!("/repos/{repo}/issues/{number}");
        let me = self.login();
        let field = format!("comment:{repo}#{number}");
        let mut col = widgets::col().gap_3();
        let body_acts = CommentActs {
            edit: Some(
                FormSpec::new("Edit description")
                    .width(680.0)
                    .field(Field::multiline("body", "Description").value(issue.s("body")).keep_empty())
                    .rest("PATCH", format!("/repos/{repo}/issues/{number}"))
                    .ok("Saved")
                    .inval(inval.clone())
                    .act(),
            ),
            delete: None,
            extra: Vec::new(),
            reactions: Some(format!("/repos/{repo}/issues/{number}/reactions")),
            invalidate: inval.clone(),
            quote_into: Some(field.clone()),
        };
        col = col.child(self.comment_card("issue-body", issue, "opened", body_acts, cx));

        let spec = ListSpec::new(format!("/repos/{repo}/issues/{number}/timeline"), |_| Row::new(""));
        match self.fetch_list(&spec, cx) {
            crate::resource::Fetched::Items { items, loading, more } => {
                for (i, event) in items.iter().enumerate() {
                    if let Some(el) = self.timeline_item(repo, &format!("tl{i}"), event, &field, &inval, &me, cx) {
                        col = col.child(el);
                    }
                }
                if loading {
                    col = col.child(widgets::loading());
                } else if more {
                    let id = spec.id.clone();
                    col = col.child(widgets::btn(
                        "timeline-more",
                        "Load more",
                        Act::run(move |hub, _, cx| {
                            let next = hub.page(&id) + 1;
                            hub.pages.insert(id.clone(), next);
                            cx.notify();
                        }),
                    ));
                }
            }
            crate::resource::Fetched::Failed(error) => col = col.child(widgets::error_box(&error)),
        }

        let open = issue.s("state") == "open";
        let state_path = format!("/repos/{repo}/issues/{number}");
        let close = |reason: &'static str, label: &'static str| {
            let (state_path, inval, field) = (state_path.clone(), inval.clone(), field.clone());
            Act::run(move |hub, window, cx| {
                let text = hub.field_text(&field);
                let state_path = state_path.clone();
                let comment_path = format!("{state_path}/comments");
                let clear = field.clone();
                let req = Req::custom(move |client| {
                    if !text.trim().is_empty() {
                        client.json("POST", &comment_path, Some(&json!({ "body": text })))?;
                    }
                    let body = if reason == "reopened" {
                        json!({ "state": "open", "state_reason": "reopened" })
                    } else {
                        json!({ "state": "closed", "state_reason": reason })
                    };
                    client.json("PATCH", &state_path, Some(&body))
                })
                .ok(label)
                .inval(inval.clone())
                .then(move |hub, _, cx| {
                    hub.set_field(&clear, "");
                    cx.notify();
                });
                hub.perform(req.act(), window, cx);
            })
        };
        let extra = if open {
            vec![
                widgets::icon_action("close-issue", "issue-closed", widgets::purple(), "Close as completed", close("completed", "Issue closed")).into_any_element(),
                widgets::icon_action("close-np", "issue-skip", widgets::gray(), "Close as not planned", close("not_planned", "Issue closed")).into_any_element(),
            ]
        } else {
            vec![widgets::icon_action("reopen-issue", "issue", widgets::green(), "Reopen issue", close("reopened", "Issue reopened")).into_any_element()]
        };
        let submit = post_comment(&field, &format!("/repos/{repo}/issues/{number}/comments"), &inval);
        if issue.b("locked") {
            col = col.child(widgets::dim("🔒 This conversation is locked."));
        }
        col.child(self.composer(&field, submit, extra, cx)).into_any_element()
    }

    /// One entry of an issue or pull request timeline.
    #[allow(clippy::too_many_arguments)]
    pub fn timeline_item(
        &mut self,
        repo: &str,
        id: &str,
        event: &Value,
        field: &str,
        inval: &str,
        _me: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let kind = event.s("event");
        let actor = if event.has("actor") {
            event.at("actor").clone()
        } else {
            event.at("user").clone()
        };
        let when = event.s("created_at");
        Some(match kind.as_str() {
            "commented" => {
                let url = event.s("url");
                let mine = event.s("user.login") == self.login();
                let acts = CommentActs {
                    edit: mine.then(|| edit_comment(&url, &event.s("body"), inval)),
                    delete: Some(delete_comment(&url, inval)),
                    extra: Vec::new(),
                    reactions: Some(format!("{}/reactions", api_path(&url))),
                    invalidate: inval.to_string(),
                    quote_into: Some(field.to_string()),
                };
                self.comment_card(id, event, "commented", acts, cx)
            }
            "reviewed" => {
                let state = event.s("state");
                let (icon_name, color, verb) = match state.as_str() {
                    "approved" => ("check-circle", widgets::green(), "approved these changes"),
                    "changes_requested" => ("x-circle", widgets::red(), "requested changes"),
                    "dismissed" => ("skip", widgets::gray(), "review was dismissed"),
                    _ => ("eye", widgets::gray(), "reviewed"),
                };
                let when = event.s("submitted_at");
                let head = self.timeline_event(icon_name, color, &actor, verb, &when, None, cx);
                if event.s("body").trim().is_empty() {
                    head
                } else {
                    let body = self.markdown(id, &event.s("body"), cx);
                    widgets::col()
                        .child(head)
                        .child(div().ml(px(44.0)).child(widgets::card().p_3().child(body)))
                        .into_any_element()
                }
            }
            "committed" => {
                let sha = event.s("sha");
                let message = crate::json::first_line(&event.s("message"));
                let author = json!({ "login": event.s("author.name") });
                let people = self.commit_people.get(&sha).cloned().filter(|a| !a.is_empty());
                let sha_btn = widgets::btn(
                    ElementId::Name(format!("{id}-sha").into()),
                    sha.chars().take(7).collect::<String>(),
                    Act::Go(Route::Commit {
                        repo: repo.to_string(),
                        sha: sha.clone(),
                    }),
                )
                .h(px(20.0))
                .font_family(widgets::MONO)
                .into_any_element();
                if let Some(people) = people {
                    // Every author's face and name, as on the commit.
                    let who = widgets::row()
                        .gap_2()
                        .child(self.author_avatars(&people, cx))
                        .child(crate::screens::repo::author_names(id, &people).font_weight(FontWeight::SEMIBOLD))
                        .into_any_element();
                    return Some(self.timeline_event_by(
                        "commit",
                        widgets::gray(),
                        who,
                        format!("committed  {message}"),
                        &event.s("author.date"),
                        Some(sha_btn),
                    ));
                }
                self.timeline_event(
                    "commit",
                    widgets::gray(),
                    &author,
                    format!("committed  {message}"),
                    &event.s("author.date"),
                    Some(sha_btn),
                    cx,
                )
            }
            "labeled" | "unlabeled" => {
                let chip = widgets::label_chip(&event.s("label.name"), &event.s("label.color"));
                let verb = if kind == "labeled" { "added the label" } else { "removed the label" };
                self.timeline_event("label", widgets::gray(), &actor, verb, &when, Some(chip.into_any_element()), cx)
            }
            "assigned" | "unassigned" => {
                let who = event.s("assignee.login");
                let text = if kind == "assigned" {
                    format!("assigned {who}")
                } else {
                    format!("unassigned {who}")
                };
                self.timeline_event("person", widgets::gray(), &actor, text, &when, None, cx)
            }
            "closed" => {
                let reason = event.s("state_reason");
                let text = if reason == "not_planned" {
                    "closed this as not planned".to_string()
                } else {
                    "closed this".to_string()
                };
                self.timeline_event("issue-closed", widgets::purple(), &actor, text, &when, None, cx)
            }
            "reopened" => self.timeline_event("issue", widgets::green(), &actor, "reopened this", &when, None, cx),
            "merged" => self.timeline_event(
                "pr-merged",
                widgets::purple(),
                &actor,
                format!("merged commit {}", event.s("commit_id").chars().take(7).collect::<String>()),
                &when,
                None,
                cx,
            ),
            "renamed" => self.timeline_event(
                "pencil",
                widgets::gray(),
                &actor,
                format!("changed the title {} → {}", event.s("rename.from"), event.s("rename.to")),
                &when,
                None,
                cx,
            ),
            "milestoned" | "demilestoned" => self.timeline_event(
                "milestone",
                widgets::gray(),
                &actor,
                format!(
                    "{} the {} milestone",
                    if kind == "milestoned" { "added this to" } else { "removed this from" },
                    event.s("milestone.title")
                ),
                &when,
                None,
                cx,
            ),
            "locked" | "unlocked" => self.timeline_event(
                if kind == "locked" { "lock" } else { "unlock" },
                widgets::gray(),
                &actor,
                if kind == "locked" { "locked this conversation" } else { "unlocked this conversation" },
                &when,
                None,
                cx,
            ),
            "cross-referenced" => {
                let source = event.at("source.issue").clone();
                let title = source.s("title");
                let route = common::issue_route(&source);
                let link = widgets::btn(
                    ElementId::Name(format!("{id}-ref").into()),
                    crate::json::clip(&format!("{title} #{}", source.i("number")), 60),
                    Act::Go(route),
                )
                .h(px(20.0))
                .into_any_element();
                self.timeline_event("link", widgets::gray(), &actor, "mentioned this in", &when, Some(link), cx)
            }
            "referenced" => {
                let sha = event.s("commit_id");
                if sha.is_empty() {
                    return None;
                }
                let link = widgets::btn(
                    ElementId::Name(format!("{id}-commit").into()),
                    sha.chars().take(7).collect::<String>(),
                    Act::Go(Route::Commit {
                        repo: common::repo_of(&event.s("commit_url")),
                        sha,
                    }),
                )
                .h(px(20.0))
                .into_any_element();
                self.timeline_event("commit", widgets::gray(), &actor, "referenced this in commit", &when, Some(link), cx)
            }
            "review_requested" | "review_request_removed" => {
                let who = if event.has("requested_reviewer") {
                    event.s("requested_reviewer.login")
                } else {
                    event.s("requested_team.name")
                };
                let text = if kind == "review_requested" {
                    format!("requested a review from {who}")
                } else {
                    format!("removed the review request for {who}")
                };
                self.timeline_event("eye", widgets::gray(), &actor, text, &when, None, cx)
            }
            "head_ref_deleted" => self.timeline_event("branch", widgets::gray(), &actor, "deleted the head branch", &when, None, cx),
            "head_ref_restored" => self.timeline_event("branch", widgets::gray(), &actor, "restored the head branch", &when, None, cx),
            "head_ref_force_pushed" => self.timeline_event("branch", widgets::gray(), &actor, "force-pushed the head branch", &when, None, cx),
            "ready_for_review" => self.timeline_event("eye", widgets::green(), &actor, "marked this ready for review", &when, None, cx),
            "convert_to_draft" => self.timeline_event("pr-draft", widgets::gray(), &actor, "converted this to a draft", &when, None, cx),
            "pinned" | "unpinned" => self.timeline_event("pin", widgets::gray(), &actor, format!("{kind} this issue"), &when, None, cx),
            "transferred" => self.timeline_event("arrow-right", widgets::gray(), &actor, "transferred this issue", &when, None, cx),
            "connected" | "disconnected" => self.timeline_event("link", widgets::gray(), &actor, format!("{kind} a pull request"), &when, None, cx),
            "marked_as_duplicate" => self.timeline_event("copy", widgets::gray(), &actor, "marked this as a duplicate", &when, None, cx),
            "added_to_project_v2" | "project_v2_item_status_changed" | "added_to_project" | "moved_columns_in_project" => {
                self.timeline_event("project", widgets::gray(), &actor, "updated this in a project", &when, None, cx)
            }
            "deployed" => self.timeline_event("rocket", widgets::gray(), &actor, "deployed this", &when, None, cx),
            _ => return None,
        })
    }

    /// Assignees, labels, milestone, and the issue's other actions.
    pub fn issue_sidebar(&mut self, repo: &str, issue: &Value, is_pr: bool, cx: &mut Context<Self>) -> AnyElement {
        let number = issue.i("number");
        let path = format!("/repos/{repo}/issues/{number}");
        let inval_issue = path.clone();
        let inval_pull = format!("/repos/{repo}/pulls/{number}");
        let with_inval = move |req: Req| req.inval(inval_issue.clone()).inval(inval_pull.clone());

        // Assignees.
        let current: Vec<String> = issue.list("assignees").iter().map(|a| a.s("login")).collect();
        let mut assignee_picker = Picker::new("Assign up to 10 people", "Filter people…", true);
        match self.fetch(&format!("/repos/{repo}/assignees?per_page=100"), cx).ready().cloned() {
            Some(users) => {
                for user in users.list("") {
                    let login = user.s("login");
                    let on_now = current.contains(&login);
                    let req = |method: &'static str| with_inval(Req::rest(method, format!("{path}/assignees")).body(json!({ "assignees": [login] }))).act();
                    assignee_picker = assignee_picker.item(PickItem::toggle(login.clone(), on_now, req("POST"), req("DELETE")).avatar(user.s("avatar_url")));
                }
            }
            None => assignee_picker.loading = true,
        }
        let mut assignees = widgets::col().gap_1();
        if current.is_empty() {
            let me = self.login();
            assignees = assignees.child(
                widgets::row()
                    .child(widgets::dim("No one —"))
                    .child(
                        crate::ui::Link::new("assign-self", "assign yourself").on_click(on(
                            with_inval(Req::rest("POST", format!("{path}/assignees")).body(json!({ "assignees": [me] }))).act(),
                        )),
                    ),
            );
        }
        for a in issue.list("assignees").to_vec() {
            let avatar = self.avatar(&a.s("avatar_url"), 20.0, cx);
            assignees = assignees.child(widgets::row().child(avatar).child(a.s("login")));
        }

        // Labels.
        let current_labels: Vec<String> = issue.list("labels").iter().map(|l| l.s("name")).collect();
        let mut label_picker = Picker::new("Apply labels", "Filter labels…", true);
        match self.fetch(&format!("/repos/{repo}/labels?per_page=100"), cx).ready().cloned() {
            Some(labels) => {
                for l in labels.list("") {
                    let name = l.s("name");
                    let on_now = current_labels.contains(&name);
                    let add = with_inval(Req::rest("POST", format!("{path}/labels")).body(json!({ "labels": [name] }))).act();
                    let remove = with_inval(Req::rest("DELETE", format!("{path}/labels/{}", enc(&name)))).act();
                    label_picker = label_picker.item(PickItem::toggle(name, on_now, add, remove).color(l.s("color")).detail(l.s("description")));
                }
            }
            None => label_picker.loading = true,
        }
        let mut labels = div().flex().flex_row().flex_wrap().gap_1();
        if current_labels.is_empty() {
            labels = labels.child(widgets::dim("None yet"));
        }
        for l in issue.list("labels") {
            labels = labels.child(widgets::label_chip(&l.s("name"), &l.s("color")));
        }

        // Milestone.
        let mut milestone_picker = Picker::new("Set milestone", "Filter milestones…", false).item(PickItem::new(
            "No milestone",
            !issue.has("milestone"),
            with_inval(Req::rest("PATCH", path.clone()).body(json!({ "milestone": null }))).act(),
        ));
        match self.fetch(&format!("/repos/{repo}/milestones?state=open&per_page=100"), cx).ready().cloned() {
            Some(ms) => {
                for m in ms.list("") {
                    let n = m.i("number");
                    let due = if m.has("due_on") { format!("Due {}", crate::time::date(&m.s("due_on"))) } else { "No due date".to_string() };
                    milestone_picker = milestone_picker.item(
                        PickItem::new(m.s("title"), issue.i("milestone.number") == n, with_inval(Req::rest("PATCH", path.clone()).body(json!({ "milestone": n }))).act())
                            .detail(due),
                    );
                }
            }
            None => milestone_picker.loading = true,
        }
        let milestone: AnyElement = if issue.has("milestone") {
            let m = issue.at("milestone");
            let total = m.i("open_issues") + m.i("closed_issues");
            let done = if total > 0 { m.i("closed_issues") as f32 / total as f32 } else { 0.0 };
            widgets::col()
                .gap_1()
                .child(m.s("title"))
                .child(crate::ui::ProgressBar::new(done).h(px(6.0)).w_full())
                .into_any_element()
        } else {
            widgets::dim("No milestone").into_any_element()
        };

        // Everything else.
        let locked = issue.b("locked");
        let lock = if locked {
            with_inval(Req::rest("DELETE", format!("{path}/lock")).ok("Conversation unlocked")).act()
        } else {
            FormSpec::new("Lock conversation")
                .submit("Lock")
                .field(Field::choice(
                    "lock_reason",
                    "Reason",
                    &[("", "No reason"), ("off-topic", "Off-topic"), ("too heated", "Too heated"), ("resolved", "Resolved"), ("spam", "Spam")],
                ))
                .rest("PUT", format!("{path}/lock"))
                .map(|body, values| {
                    if values.s("lock_reason").is_empty() {
                        json!({})
                    } else {
                        body
                    }
                })
                .ok("Conversation locked")
                .inval(path.clone())
                .inval(format!("/repos/{repo}/pulls/{number}"))
                .act()
        };
        let node = issue.s("node_id");
        let pin = with_inval(
            Req::gql(
                "mutation($id: ID!) { pinIssue(input: {issueId: $id}) { clientMutationId } }",
                json!({ "id": node }),
            )
            .ok("Issue pinned"),
        )
        .act();
        let unpin = with_inval(
            Req::gql(
                "mutation($id: ID!) { unpinIssue(input: {issueId: $id}) { clientMutationId } }",
                json!({ "id": node }),
            )
            .ok("Issue unpinned"),
        )
        .act();
        let transfer_node = node.clone();
        let transfer = FormSpec::new("Transfer issue")
            .submit("Transfer")
            .note("Move this issue to another repository you can write to. Labels and milestones that do not exist there are dropped.")
            .field(Field::text("target", "Destination repository (owner/name)").required())
            .build_with(move |values| {
                let target = values.s("target");
                let Some((owner, name)) = target.split_once('/') else {
                    return Err("Use the owner/name form.".into());
                };
                let (owner, name, id) = (owner.trim().to_string(), name.trim().to_string(), transfer_node.clone());
                Ok(Req::custom(move |client| {
                    let repo = client.graphql(
                        "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { id } }",
                        json!({ "o": owner, "n": name }),
                    )?;
                    client.graphql(
                        "mutation($i: ID!, $r: ID!) { transferIssue(input: {issueId: $i, repositoryId: $r}) { issue { number url } } }",
                        json!({ "i": id, "r": repo.s("repository.id") }),
                    )
                })
                .ok("Issue transferred")
                .then(|hub, value, cx| {
                    let url = value.s("transferIssue.issue.url");
                    hub.open_link(&url, cx);
                })
                .act())
            })
            .act();
        let delete_repo = repo.to_string();
        let delete = Req::gql(
            "mutation($id: ID!) { deleteIssue(input: {issueId: $id}) { clientMutationId } }",
            json!({ "id": node }),
        )
        .ok("Issue deleted")
        .inval(format!("/repos/{repo}/issues"))
        .then(move |hub, _, cx| {
            hub.go(
                Route::Repo {
                    repo: delete_repo.clone(),
                    tab: RepoTab::Issues,
                },
                cx,
            )
        })
        .act()
        .confirm(
            "Delete this issue?",
            "Deleting an issue removes it and its comments for good. Only admins can do this.",
            "Delete issue",
        );
        let subscribe_path = format!("/repos/{repo}/issues/{number}");
        let mut actions = widgets::col().gap_1().child(
            widgets::ibtn("lock", if locked { "unlock" } else { "lock" }, if locked { "Unlock conversation" } else { "Lock conversation" }, lock)
                .w_full(),
        );
        if !is_pr {
            actions = actions
                .child(widgets::ibtn("pin", "pin", "Pin issue", pin).w_full())
                .child(widgets::ibtn("unpin", "pin", "Unpin issue", unpin).w_full())
                .child(widgets::ibtn("transfer", "arrow-right", "Transfer issue", transfer).w_full())
                .child(widgets::ibtn(
                    "branch-for-issue",
                    "branch",
                    "Create a branch for this issue",
                    crate::screens::repo::new_branch_form(repo, &format!("{number}-{}", slug(&issue.s("title")))),
                ).w_full())
                .child(widgets::ibtn("delete-issue", "trash", "Delete issue", delete).w_full());
        }
        actions = actions.child(
            widgets::ibtn("subscribe", "bell", "Notification settings…", Act::Url(format!("{}{}", crate::api::WEB, subscribe_path.replace("/repos", ""))))
                .w_full(),
        );

        widgets::col()
            .gap_3()
            .child(side_section("assignees", "Assignees", Some(assignee_picker.act()), assignees.into_any_element()))
            .child(side_section("labels", "Labels", Some(label_picker.act()), labels.into_any_element()))
            .child(side_section("milestone", "Milestone", Some(milestone_picker.act()), milestone))
            .child(actions)
            .into_any_element()
    }

    /// The repository's labels, with create, edit and delete.
    fn labels_view(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/labels");
        let create = FormSpec::new("New label")
            .submit("Create label")
            .field(Field::text("name", "Name").required())
            .field(Field::text("description", "Description"))
            .field(Field::text("color", "Colour (hex, no #)").value("ededed"))
            .rest("POST", path.clone())
            .ok("Label created")
            .inval(path.clone())
            .act();
        let repo_s = repo.to_string();
        let spec = ListSpec::new(path.clone(), move |label| {
            let name = label.s("name");
            let lpath = format!("/repos/{repo_s}/labels/{}", enc(&name));
            let inval = format!("/repos/{repo_s}/labels");
            let mut row = Row::new(String::new())
                .meta(label.s("description"))
                .open(Act::choose(format!("issues.view:{repo_s}"), "list"))
                .action(
                    "Edit",
                    FormSpec::new(format!("Edit label {name}"))
                        .field(Field::text("new_name", "Name").value(name.clone()).required())
                        .field(Field::text("description", "Description").value(label.s("description")).keep_empty())
                        .field(Field::text("color", "Colour (hex, no #)").value(label.s("color")))
                        .rest("PATCH", lpath.clone())
                        .ok("Label saved")
                        .inval(inval.clone())
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", lpath)
                        .ok("Label deleted")
                        .inval(inval)
                        .act()
                        .confirm("Delete label?", format!("“{name}” will be removed from every issue and pull request."), "Delete"),
                )
                .inline();
            row.labels = vec![(name, label.s("color"))];
            row
        })
        .empty("No labels.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(widgets::row().child(widgets::h2("Labels")).child(widgets::spacer()).child(widgets::go_btn("new-label", "New label", create)))
            .child(list)
            .into_any_element()
    }

    /// The repository's milestones, with progress and editing.
    fn milestones_view(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let state_key = format!("milestones.state:{repo}");
        let state = self.choice(&state_key, "open");
        let path = format!("/repos/{repo}/milestones");
        let create = FormSpec::new("New milestone")
            .submit("Create milestone")
            .field(Field::text("title", "Title").required())
            .field(Field::text("due_on", "Due date").hint("YYYY-MM-DDT00:00:00Z, optional"))
            .field(Field::multiline("description", "Description"))
            .rest("POST", path.clone())
            .ok("Milestone created")
            .inval(path.clone())
            .act();
        let repo_s = repo.to_string();
        let spec = ListSpec::new(format!("{path}?state={state}&sort=due_on"), move |m| {
            let n = m.i("number");
            let mpath = format!("/repos/{repo_s}/milestones/{n}");
            let inval = format!("/repos/{repo_s}/milestones");
            let open = m.i("open_issues");
            let closed = m.i("closed_issues");
            let pct = if open + closed > 0 { closed * 100 / (open + closed) } else { 0 };
            let due = if m.has("due_on") { format!("Due {}", time::date(&m.s("due_on"))) } else { "No due date".into() };
            let title = m.s("title");
            let state_toggle = if m.s("state") == "open" { ("Close", "closed") } else { ("Reopen", "open") };
            Row::new(title.clone())
                .icon("milestone", widgets::gray())
                .meta(format!("{due}  ·  {pct}% complete  ·  {open} open  ·  {closed} closed"))
                .body(m.s("description"))
                .open(Act::run({
                    let repo = repo_s.clone();
                    move |hub, _, cx| {
                        hub.choices.insert(format!("issues.view:{repo}"), "list".into());
                        hub.choices.insert(format!("issue.applied:{repo}"), format!("milestone:\"{title}\""));
                        cx.notify();
                    }
                }))
                .action(
                    "Edit",
                    FormSpec::new("Edit milestone")
                        .field(Field::text("title", "Title").value(m.s("title")).required())
                        .field(Field::text("due_on", "Due date").value(m.s("due_on")))
                        .field(Field::multiline("description", "Description").value(m.s("description")).keep_empty())
                        .rest("PATCH", mpath.clone())
                        .ok("Milestone saved")
                        .inval(inval.clone())
                        .act(),
                )
                .action(
                    state_toggle.0,
                    Req::rest("PATCH", mpath.clone())
                        .body(json!({ "state": state_toggle.1 }))
                        .ok("Milestone updated")
                        .inval(inval.clone())
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", mpath)
                        .ok("Milestone deleted")
                        .inval(inval)
                        .act()
                        .confirm("Delete milestone?", "Issues in it will be left without a milestone.", "Delete"),
                )
        })
        .empty("No milestones.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::chips(vec![
                        ("Open".into(), state == "open", Act::choose(&state_key, "open")),
                        ("Closed".into(), state == "closed", Act::choose(&state_key, "closed")),
                    ]))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-milestone", "New milestone", create)),
            )
            .child(list)
            .into_any_element()
    }
}

/// A title as a branch-name slug.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}
