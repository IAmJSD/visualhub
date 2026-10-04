//! Pull requests: the conversation with its merge box, reviews, commits,
//! the changed files with inline review comments, checks, and creating
//! one from a comparison.

use super::common::{post_comment, side_section, CommentActs};
use crate::diff::ReviewTarget;
use crate::picker::{PickItem, Picker};
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, MenuEntry, PullTab, Req, Route, RepoTab};
use crate::json::{first_line, Json as _};
use crate::ready;
use crate::resource::{Fetched, ListSpec, Row};
use crate::time;
use crate::widgets::{self, rgb, TabItem};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _};
use crate::ui::{icon, palette};
use serde_json::{json, Value};

pub fn new_pull_form(repo: &str, base: &str, head: &str) -> Act {
    let target = repo.to_string();
    FormSpec::new(format!("Open a pull request in {repo}"))
        .submit("Create pull request")
        .width(640.0)
        .field(Field::text("base", "Base branch").value(base).required().hint("The branch to merge into."))
        .field(Field::text("head", "Compare branch").value(head).required().hint("branch, or owner:branch for a fork."))
        .field(Field::text("title", "Title").required())
        .field(Field::multiline("body", "Description"))
        .field(Field::bool("draft", "Create as draft", false))
        .field(Field::bool("maintainer_can_modify", "Allow edits by maintainers", true))
        .rest("POST", format!("/repos/{repo}/pulls"))
        .ok("Pull request opened")
        .inval(format!("/repos/{repo}/pulls"))
        .then(move |hub, value, cx| {
            hub.go(
                Route::Pull {
                    repo: target.clone(),
                    number: value.i("number") as u64,
                    tab: PullTab::Conversation,
                },
                cx,
            )
        })
        .act()
}

fn compare_form(repo: &str, default_branch: &str) -> Act {
    let repo = repo.to_string();
    FormSpec::new("Compare changes")
        .submit("Compare")
        .field(Field::text("base", "Base").value(default_branch).required())
        .field(Field::text("head", "Compare").required())
        .build_with(move |values| {
            Ok(Act::Go(Route::Compare {
                repo: repo.clone(),
                base: values.s("base"),
                head: values.s("head"),
            }))
        })
        .act()
}

/// A review: comment, approve, or request changes.
pub fn review_form(repo: &str, number: u64) -> Act {
    FormSpec::new("Finish your review")
        .submit("Submit review")
        .width(600.0)
        .field(Field::multiline("body", "Summary"))
        .field(Field::choice(
            "event",
            "Verdict",
            &[("COMMENT", "Comment"), ("APPROVE", "Approve"), ("REQUEST_CHANGES", "Request changes")],
        ))
        .rest("POST", format!("/repos/{repo}/pulls/{number}/reviews"))
        .ok("Review submitted")
        .inval(format!("/repos/{repo}/issues/{number}"))
        .inval(format!("/repos/{repo}/pulls/{number}"))
        .act()
}

/// A check run or commit status as a row.
fn check_row(repo: &str, check: &Value) -> Row {
    let status = check.s("status");
    let conclusion = check.s("conclusion");
    let (icon_name, color) = status_icon(&status, &conclusion);
    let duration = time::span(&check.s("started_at"), &check.s("completed_at"));
    let details = check.s("details_url");
    let mut row = Row::new(check.s("name"))
        .icon(icon_name, color)
        .meta(format!(
            "{}  ·  {}{}",
            check.s("app.name"),
            if conclusion.is_empty() { status.clone() } else { conclusion.clone() },
            if duration.is_empty() { String::new() } else { format!(" in {duration}") }
        ))
        .body(check.s("output.title"))
        .open(match crate::hub::route_for_url(&details) {
            Some(route) => Act::Go(route),
            None if !details.is_empty() => Act::Url(details.clone()),
            None => Act::Url(check.s("html_url")),
        });
    if check.s("app.slug") == "github-actions" {
        row = row.action(
            "Re-run",
            Req::rest("POST", format!("/repos/{repo}/actions/jobs/{}/rerun", check.i("id")))
                .ok("Job re-run requested")
                .inval(format!("/repos/{repo}/commits"))
                .act(),
        );
    }
    row
}

/// The icon and colour for a run's or check's status and conclusion.
pub fn status_icon(status: &str, conclusion: &str) -> (&'static str, u32) {
    match (status, conclusion) {
        (_, "success") => ("check-circle", widgets::green()),
        (_, "failure") | (_, "timed_out") | (_, "startup_failure") => ("x-circle", widgets::red()),
        (_, "cancelled") => ("stop", widgets::gray()),
        (_, "skipped") | (_, "neutral") => ("skip", widgets::gray()),
        (_, "action_required") => ("alert", widgets::yellow()),
        ("in_progress", _) => ("dot", widgets::yellow()),
        ("queued", _) | ("waiting", _) | ("pending", _) | ("requested", _) => ("clock", widgets::yellow()),
        ("completed", _) => ("check-circle", widgets::gray()),
        _ => ("circle", widgets::gray()),
    }
}

impl Hub {
    pub fn my_pulls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.my_items("pr", cx)
    }

    /// A repository's Pull requests tab.
    pub fn repo_pulls(&mut self, repo: &str, default_branch: &str, cx: &mut Context<Self>) -> AnyElement {
        let list = self.issue_list(repo, "pr", cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::btn("compare", "Compare branches", compare_form(repo, default_branch)))
                    .child(widgets::go_btn("new-pr", "New pull request", new_pull_form(repo, default_branch, ""))),
            )
            .child(list)
            .into_any_element()
    }

    pub fn pull(&mut self, repo: &str, number: u64, tab: PullTab, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/pulls/{number}");
        let pr = ready!(self.fetch(&path, cx));
        let header = self.issue_header(repo, &pr, true, cx);
        let go = |tab: PullTab| {
            Act::Go(Route::Pull {
                repo: repo.to_string(),
                number,
                tab,
            })
        };
        let tabs = widgets::tabs(
            "pr",
            vec![
                TabItem::new("Conversation", "comment", tab == PullTab::Conversation, go(PullTab::Conversation))
                    .count(Some(pr.i("comments") + pr.i("review_comments"))),
                TabItem::new("Commits", "commit", tab == PullTab::Commits, go(PullTab::Commits)).count(Some(pr.i("commits"))),
                TabItem::new("Checks", "check-circle", tab == PullTab::Checks, go(PullTab::Checks)),
                TabItem::new("Files changed", "file", tab == PullTab::Files, go(PullTab::Files)).count(Some(pr.i("changed_files"))),
            ],
        );
        let body = match tab {
            PullTab::Conversation => self.pull_conversation(repo, number, &pr, cx),
            PullTab::Commits => self.pull_commits(repo, number, cx),
            PullTab::Files => self.pull_files(repo, number, &pr, cx),
            PullTab::Checks => self.pull_checks(repo, &pr.s("head.sha"), cx),
        };
        // Files fills the window, its file tree and diff scrolling apart.
        let fill = tab == PullTab::Files;
        widgets::page()
            .when(fill, |d| d.flex_1().min_h_0())
            .child(header)
            .child(
                widgets::row()
                    .child(div().flex_1().child(tabs))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(
                                widgets::row()
                                    .child(div().text_color(rgb(widgets::green())).child(format!("+{}", pr.i("additions"))))
                                    .child(div().text_color(rgb(widgets::red())).child(format!("−{}", pr.i("deletions")))),
                            ),
                    ),
            )
            .child(div().flex().flex_col().when(fill, |d| d.flex_1().min_h_0()).child(body))
            .into_any_element()
    }

    fn pull_conversation(&mut self, repo: &str, number: u64, pr: &Value, cx: &mut Context<Self>) -> AnyElement {
        let inval = format!("/repos/{repo}/issues/{number}");
        let field = format!("comment:{repo}#{number}");
        let mut col = widgets::col().gap_3();
        let body_acts = CommentActs {
            edit: Some(
                FormSpec::new("Edit description")
                    .width(680.0)
                    .field(Field::multiline("body", "Description").value(pr.s("body")).keep_empty())
                    .rest("PATCH", format!("/repos/{repo}/pulls/{number}"))
                    .ok("Saved")
                    .inval(format!("/repos/{repo}/pulls/{number}"))
                    .act(),
            ),
            delete: None,
            extra: Vec::new(),
            reactions: Some(format!("/repos/{repo}/issues/{number}/reactions")),
            invalidate: inval.clone(),
            quote_into: Some(field.clone()),
        };
        // The PR object has no reaction counts; the issue view of it does.
        let mut body_value = pr.clone();
        if let Some(issue) = self.fetch(&format!("/repos/{repo}/issues/{number}"), cx).ready() {
            body_value["reactions"] = issue.at("reactions").clone();
        }
        col = col.child(self.comment_card("pr-body", &body_value, "opened", body_acts, cx));

        let me = self.login();
        let spec = ListSpec::new(format!("/repos/{repo}/issues/{number}/timeline"), |_| Row::new(""));
        match self.fetch_list(&spec, cx) {
            Fetched::Items { items, loading, more } => {
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
            Fetched::Failed(error) => col = col.child(widgets::error_box(&error)),
        }

        col = col.child(self.merge_box(repo, number, pr, cx));

        let open = pr.s("state") == "open";
        let state_req = |state: &str, ok: &str| {
            Req::rest("PATCH", format!("/repos/{repo}/pulls/{number}"))
                .body(json!({ "state": state }))
                .ok(ok.to_string())
                .inval(format!("/repos/{repo}/pulls/{number}"))
                .inval(inval.clone())
                .act()
        };
        let extra = vec![
            widgets::icon_action("review", "eye", palette().text_dim, "Review changes", review_form(repo, number)).into_any_element(),
            if open {
                widgets::icon_action("close-pr", "pr-closed", widgets::red(), "Close pull request", state_req("closed", "Pull request closed")).into_any_element()
            } else if !pr.b("merged") {
                widgets::icon_action("reopen-pr", "pr", widgets::green(), "Reopen pull request", state_req("open", "Pull request reopened")).into_any_element()
            } else {
                div().into_any_element()
            },
        ];
        let submit = post_comment(&field, &format!("/repos/{repo}/issues/{number}/comments"), &inval);
        let composer = self.composer(&field, submit, extra, cx);
        let sidebar = self.pull_sidebar(repo, number, pr, cx);
        div()
            .flex()
            .flex_row()
            .gap_6()
            .items_start()
            .child(div().flex_1().min_w_0().child(col.child(composer)))
            .child(div().w(px(280.0)).flex_none().child(sidebar))
            .into_any_element()
    }

    /// Whether it can merge, and the buttons that do.
    fn merge_box(&mut self, repo: &str, number: u64, pr: &Value, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let pr_path = format!("/repos/{repo}/pulls/{number}");
        let inval_issue = format!("/repos/{repo}/issues/{number}");
        let head_ref = pr.s("head.ref");
        let head_repo = pr.s("head.repo.full_name");
        let mut card = widgets::card();
        if pr.b("merged") {
            let mut row = widgets::row()
                .p_4()
                .child(icon("pr-merged", 20.0, widgets::purple()))
                .child(
                    widgets::col()
                        .gap_0()
                        .flex_1()
                        .child(widgets::h3("Pull request successfully merged and closed"))
                        .child(widgets::dim(format!(
                            "Merged by {} {}",
                            pr.s("merged_by.login"),
                            time::ago(&pr.s("merged_at"))
                        ))),
                );
            if !head_repo.is_empty() {
                let exists = self.fetch_check(&format!("/repos/{head_repo}/branches/{head_ref}"), cx);
                if exists.ready().map(|v| v.b("")).unwrap_or(false) {
                    row = row.child(widgets::btn(
                        "delete-head",
                        "Delete branch",
                        Req::rest("DELETE", format!("/repos/{head_repo}/git/refs/heads/{head_ref}"))
                            .ok(format!("Deleted {head_ref}"))
                            .inval(format!("/repos/{head_repo}/branches"))
                            .act(),
                    ));
                }
            }
            return card.child(row).into_any_element();
        }
        if pr.s("state") == "closed" {
            return card
                .child(
                    widgets::row()
                        .p_4()
                        .child(icon("pr-closed", 20.0, widgets::red()))
                        .child(widgets::h3("Closed with unmerged commits")),
                )
                .into_any_element();
        }

        // Checks on the head commit.
        let sha = pr.s("head.sha");
        let checks = self.fetch(&format!("/repos/{repo}/commits/{sha}/check-runs?per_page=100"), cx);
        let (ok, bad, pending) = checks
            .ready()
            .map(|v| {
                let runs = v.list("check_runs");
                let ok = runs.iter().filter(|r| matches!(r.s("conclusion").as_str(), "success" | "skipped" | "neutral")).count();
                let bad = runs.iter().filter(|r| matches!(r.s("conclusion").as_str(), "failure" | "timed_out" | "cancelled" | "action_required")).count();
                (ok, bad, runs.len() - ok - bad)
            })
            .unwrap_or((0, 0, 0));
        let (check_icon, check_color, check_title, check_sub) = if ok + bad + pending == 0 {
            ("dot", widgets::gray(), "No checks reported".to_string(), "This commit has no status checks.".to_string())
        } else if bad > 0 {
            ("close", widgets::red(), "Some checks were not successful".to_string(), format!("{bad} failing, {pending} pending, {ok} successful checks"))
        } else if pending > 0 {
            ("clock", widgets::yellow(), "Some checks haven't completed yet".to_string(), format!("{pending} pending, {ok} successful checks"))
        } else {
            ("check", widgets::green(), "All checks have passed".to_string(), format!("{ok} successful check{}", if ok == 1 { "" } else { "s" }))
        };

        let state = pr.s("mergeable_state");
        let (merge_icon, merge_color, merge_title, merge_sub) = match state.as_str() {
            "clean" | "has_hooks" => ("check", widgets::green(), "No conflicts with base branch", "Merging can be performed automatically."),
            "unstable" => ("check", widgets::green(), "No conflicts with base branch", "Merging can be performed automatically, though some checks failed."),
            "dirty" => ("close", widgets::red(), "This branch has conflicts that must be resolved", "Resolve them on the command line or in GitHub's web editor."),
            "blocked" => ("alert", widgets::yellow(), "Merging is blocked", "Branch protection requires approving reviews or passing checks."),
            "behind" => ("alert", widgets::yellow(), "This branch is out-of-date with the base branch", "Update it to bring in the latest changes from the base."),
            "draft" => ("pr-draft", widgets::gray(), "This pull request is still a work in progress", "Draft pull requests can't be merged."),
            _ => ("clock", widgets::gray(), "Checking mergeability…", ""),
        };
        let mergeable = matches!(state.as_str(), "clean" | "has_hooks" | "unstable");

        let method_key = format!("merge.method:{repo}");
        let method = self.choice(&method_key, "merge");
        let method_label = match method.as_str() {
            "squash" => "Squash and merge",
            "rebase" => "Rebase and merge",
            _ => "Create a merge commit",
        };
        let title_default = match method.as_str() {
            "squash" => format!("{} (#{number})", pr.s("title")),
            _ => format!("Merge pull request #{number} from {}", pr.s("head.label")),
        };
        let merge_form = FormSpec::new(method_label)
            .submit("Confirm merge")
            .width(600.0)
            .field(Field::text("commit_title", "Commit title").value(title_default))
            .field(Field::multiline("commit_message", "Commit message").value(if method == "squash" { pr.s("body") } else { pr.s("title") }))
            .field(Field::bool("delete_branch", "Delete the head branch afterwards", false))
            .build_with({
                let (pr_path, inval_issue, method) = (pr_path.clone(), inval_issue.clone(), method.clone());
                let (head_repo, head_ref, sha) = (head_repo.clone(), head_ref.clone(), sha.clone());
                move |values| {
                    let mut body = json!({ "merge_method": method, "sha": sha });
                    if method != "rebase" {
                        body["commit_title"] = json!(values.s("commit_title"));
                        body["commit_message"] = json!(values.s("commit_message"));
                    }
                    let delete = values.b("delete_branch");
                    let (path, head_repo, head_ref) = (format!("{pr_path}/merge"), head_repo.clone(), head_ref.clone());
                    Ok(Req::custom(move |client| {
                        let merged = client.json("PUT", &path, Some(&body))?;
                        if delete && !head_repo.is_empty() {
                            client.json("DELETE", &format!("/repos/{head_repo}/git/refs/heads/{head_ref}"), None)?;
                        }
                        Ok(merged)
                    })
                    .ok("Pull request merged")
                    .inval(pr_path.clone())
                    .inval(inval_issue.clone())
                    .act())
                }
            })
            .act();
        // The method sticks per repository, across restarts too.
        let pick = |m: &'static str| {
            let key = method_key.clone();
            Act::run(move |hub, _, cx| {
                hub.choices.insert(key.clone(), m.to_string());
                crate::api::save_merge_methods(&hub.choices);
                cx.notify();
            })
        };
        let methods = Act::menu(vec![
            MenuEntry::check("Create a merge commit — all commits are added to the base branch", method == "merge", pick("merge")),
            MenuEntry::check("Squash and merge — the commits are combined into one", method == "squash", pick("squash")),
            MenuEntry::check("Rebase and merge — the commits are rebased onto the base branch", method == "rebase", pick("rebase")),
        ]);
        let node = pr.s("node_id");
        let auto_merge_on = pr.has("auto_merge");
        let auto = if auto_merge_on {
            Req::gql(
                "mutation($id: ID!) { disablePullRequestAutoMerge(input: {pullRequestId: $id}) { clientMutationId } }",
                json!({ "id": node }),
            )
            .ok("Auto-merge disabled")
            .inval(pr_path.clone())
            .act()
        } else {
            Req::gql(
                "mutation($id: ID!, $m: PullRequestMergeMethod!) { enablePullRequestAutoMerge(input: {pullRequestId: $id, mergeMethod: $m}) { clientMutationId } }",
                json!({ "id": node, "m": method.to_uppercase() }),
            )
            .ok("Auto-merge enabled")
            .inval(pr_path.clone())
            .act()
        };
        let draft = pr.b("draft");
        let main: AnyElement = if draft {
            widgets::go_btn(
                "ready",
                "Ready for review",
                Req::gql(
                    "mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { clientMutationId } }",
                    json!({ "id": node }),
                )
                .ok("Marked ready for review")
                .inval(pr_path.clone())
                .inval(inval_issue.clone())
                .act(),
            )
            .h(px(32.0))
            .px_4()
            .text_size(px(13.0))
            .into_any_element()
        } else {
            widgets::split_btn("merge", method_label, merge_form, methods)
        };
        let flag = match method.as_str() {
            "squash" => "--squash",
            "rebase" => "--rebase",
            _ => "--merge",
        };
        // Fixed places: the merge button left, the branch buttons right,
        // the command-line hint on its own line, so nothing reflows.
        let footer = widgets::col()
            .gap_3()
            .p_4()
            .bg(rgb(p.deep_bg))
            .child(
                widgets::row()
                    .gap_2()
                    .child(main)
                    .child(widgets::spacer())
                    .child(widgets::btn(
                        "update-branch",
                        "Update branch",
                        Req::rest("PUT", format!("{pr_path}/update-branch"))
                            .body(json!({ "expected_head_sha": sha }))
                            .ok("Branch update queued")
                            .inval(pr_path.clone())
                            .act(),
                    ))
                    .child(widgets::btn(
                        "auto-merge",
                        if auto_merge_on { "Disable auto-merge" } else { "Enable auto-merge" },
                        auto,
                    )),
            )
            .child(
                widgets::row()
                    .gap_1()
                    .child(widgets::dim("You can also merge this with the command line."))
                    .child(crate::ui::Link::new("merge-cli", "Copy the command").on_click(crate::hub::on(Act::Copy(format!("gh pr merge {number} {flag} --repo {repo}"))))),
            );

        // A status: a filled circle with its mark, a title and a line.
        // `open` is what clicking does and the chevron that says so.
        let status = |id: &str, mark: &str, color: u32, title: String, sub: String, open: Option<(Act, &'static str)>| {
            let chevron = open.as_ref().map(|(_, c)| *c);
            div()
                .id(gpui::ElementId::Name(id.to_string().into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(p.divider))
                .child(
                    div()
                        .size(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(color))
                        .child(icon(mark, 14.0, 0xFFFFFF)),
                )
                .child(
                    widgets::col()
                        .flex_1()
                        .gap_0p5()
                        .child(div().text_size(px(15.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                        .when(!sub.is_empty(), |d| d.child(widgets::dim(sub))),
                )
                .when_some(chevron, |d, c| d.child(icon(c, 14.0, p.text_dim)))
                .when_some(open, |d, (act, _)| d.cursor_pointer().hover(|s| s.bg(rgb(p.hover))).on_click(crate::hub::on(act)))
        };
        // The checks open in place, as on github.com.
        let open_key = format!("merge.checks:{repo}#{number}");
        let expanded = self.is_open(&open_key);
        let toggle = Act::run(move |hub, _, cx| {
            if !hub.open.remove(&open_key) {
                hub.open.insert(open_key.clone());
            }
            cx.notify();
        });
        let runs: Vec<Value> = checks.ready().map(|v| v.list("check_runs").to_vec()).unwrap_or_default();
        card = card
            .border_color(rgb(if mergeable && bad == 0 { widgets::green() } else { p.edge }))
            .child(status(
                "merge-checks",
                check_icon,
                check_color,
                check_title,
                check_sub,
                (!runs.is_empty()).then_some((toggle, if expanded { "chevron-down" } else { "chevron-right" })),
            ));
        if expanded {
            // Failing first, then running, then the rest, by name.
            let rank = |c: &Value| match (c.s("status").as_str(), c.s("conclusion").as_str()) {
                (_, "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure") => 0,
                ("completed", _) => 2,
                _ => 1,
            };
            let mut runs = runs;
            runs.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.s("name").cmp(&b.s("name"))));
            let mut list = div()
                .id("merge-check-list")
                .flex()
                .flex_col()
                .max_h(px(360.0))
                .overflow_y_scroll()
                .track_scroll(&self.scroller("merge-check-list"))
                .bg(rgb(p.deep_bg))
                .border_b_1()
                .border_color(rgb(p.divider));
            for (i, run) in runs.iter().enumerate() {
                let row = check_row(repo, run);
                list = list.child(self.render_row(&format!("merge-check-{i}"), row, cx));
            }
            card = card.child(list);
        }
        card = card.child(status("merge-state", merge_icon, merge_color, merge_title.to_string(), merge_sub.to_string(), None));
        if auto_merge_on {
            card = card.child(status(
                "merge-auto",
                "zap",
                widgets::green(),
                "Auto-merge is enabled".into(),
                format!("{} will merge this once its requirements are met.", pr.s("auto_merge.enabled_by.login")),
                None,
            ));
        }
        let card = card.child(footer);

        let badge_color = if draft {
            widgets::gray()
        } else if state == "dirty" {
            widgets::red()
        } else if mergeable {
            widgets::green_fill()
        } else {
            widgets::gray()
        };
        let to_draft = Req::gql(
            "mutation($id: ID!) { convertPullRequestToDraft(input: {pullRequestId: $id}) { clientMutationId } }",
            json!({ "id": node }),
        )
        .ok("Converted to draft")
        .inval(pr_path.clone())
        .act();
        widgets::col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .size(px(40.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgb(badge_color))
                            .child(icon("pr-merged", 20.0, 0xFFFFFF)),
                    )
                    .child(card.flex_1().min_w_0()),
            )
            .when(!draft, |d| {
                d.child(
                    widgets::row()
                        .gap_1()
                        .child(widgets::spacer())
                        .child(widgets::dim("Still in progress?"))
                        .child(crate::ui::Link::new("to-draft", "Convert to draft").on_click(crate::hub::on(to_draft))),
                )
            })
            .into_any_element()
    }

    fn pull_sidebar(&mut self, repo: &str, number: u64, pr: &Value, cx: &mut Context<Self>) -> AnyElement {
        let pr_path = format!("/repos/{repo}/pulls/{number}");
        let requested: Vec<String> = pr.list("requested_reviewers").iter().map(|u| u.s("login")).collect();
        let mut reviewer_picker = Picker::new("Request up to 15 reviewers", "Filter people…", true);
        match self.fetch(&format!("/repos/{repo}/assignees?per_page=100"), cx).ready().cloned() {
            Some(users) => {
                let author = pr.s("user.login");
                for user in users.list("") {
                    let login = user.s("login");
                    if login == author {
                        continue;
                    }
                    let on_now = requested.contains(&login);
                    let req = |method: &'static str| {
                        Req::rest(method, format!("{pr_path}/requested_reviewers"))
                            .body(json!({ "reviewers": [login] }))
                            .inval(pr_path.clone())
                            .act()
                    };
                    reviewer_picker = reviewer_picker.item(PickItem::toggle(login.clone(), on_now, req("POST"), req("DELETE")).avatar(user.s("avatar_url")));
                }
            }
            None => reviewer_picker.loading = true,
        }
        // Latest review state per reviewer.
        let mut states: Vec<(String, String, String)> = Vec::new();
        if let Some(reviews) = self.fetch(&format!("{pr_path}/reviews?per_page=100"), cx).ready().cloned() {
            for r in reviews.list("") {
                let login = r.s("user.login");
                let state = r.s("state");
                if state == "COMMENTED" && states.iter().any(|s| s.0 == login) {
                    continue;
                }
                states.retain(|s| s.0 != login);
                states.push((login, state, r.s("user.avatar_url")));
            }
        }
        let mut reviewers = widgets::col().gap_1();
        for login in &requested {
            if !states.iter().any(|s| &s.0 == login) {
                states.push((login.clone(), "PENDING".into(), String::new()));
            }
        }
        if states.is_empty() {
            reviewers = reviewers.child(widgets::dim("No reviews"));
        }
        for (login, state, avatar_url) in states {
            let avatar = self.avatar(&avatar_url, 20.0, cx);
            let (icon_name, color) = match state.as_str() {
                "APPROVED" => ("check", widgets::green()),
                "CHANGES_REQUESTED" => ("x-circle", widgets::red()),
                "PENDING" => ("dot", widgets::yellow()),
                _ => ("comment", widgets::gray()),
            };
            reviewers = reviewers.child(
                widgets::row()
                    .child(avatar)
                    .child(div().flex_1().child(login))
                    .child(icon(icon_name, 14.0, color)),
            );
        }
        let base_form = FormSpec::new("Change base branch")
            .field(Field::text("base", "Base branch").value(pr.s("base.ref")).required())
            .rest("PATCH", pr_path.clone())
            .ok("Base branch changed")
            .inval(pr_path.clone())
            .act();
        let issue_side = self.issue_sidebar(repo, pr, true, cx);
        widgets::col()
            .gap_3()
            .child(side_section("reviewers", "Reviewers", Some(reviewer_picker.act()), reviewers.into_any_element()))
            .child(issue_side)
            .child(widgets::ibtn("change-base", "branch", "Change base branch", base_form).w_full())
            .child(
                widgets::ibtn(
                    "checkout",
                    "copy",
                    "Copy checkout command",
                    Act::Copy(format!("gh pr checkout {number} --repo {repo}")),
                )
                .w_full(),
            )
            .into_any_element()
    }

    fn pull_commits(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) -> AnyElement {
        let repo_s = repo.to_string();
        let spec = ListSpec::new(format!("/repos/{repo}/pulls/{number}/commits"), move |c| commit_row(&repo_s, c));
        self.list(&spec, cx)
    }

    fn pull_files(&mut self, repo: &str, number: u64, pr: &Value, cx: &mut Context<Self>) -> AnyElement {
        let spec = ListSpec::new(format!("/repos/{repo}/pulls/{number}/files"), |_| Row::new(""));
        let comments = self
            .fetch(&format!("/repos/{repo}/pulls/{number}/comments?per_page=100"), cx)
            .ready()
            .map(|v| v.list("").to_vec())
            .unwrap_or_default();
        let target = ReviewTarget {
            repo: repo.to_string(),
            number,
            commit: pr.s("head.sha"),
        };
        let header = widgets::row()
            .child(widgets::dim("Click any line to leave a review comment on it."))
            .child(widgets::spacer())
            .child(widgets::primary("review-top", "Review changes", review_form(repo, number)));
        // Every page, without asking: the tree needs the whole list.
        let (files, loading) = loop {
            match self.fetch_list(&spec, cx) {
                Fetched::Items { more: true, loading: false, .. } => {
                    let next = self.page(&spec.id) + 1;
                    self.pages.insert(spec.id.clone(), next);
                }
                Fetched::Items { items, loading, .. } => break (items, loading),
                Fetched::Failed(error) => return widgets::col().gap_3().child(header).child(widgets::error_box(&error)).into_any_element(),
            }
        };
        if files.is_empty() {
            let body = if loading { widgets::loading() } else { widgets::card().child(widgets::empty("No files changed.")).into_any_element() };
            return widgets::col().gap_3().child(header).child(body).into_any_element();
        }

        // One file at a time on the right, picked from a tree on the left.
        let paths: Vec<String> = files.iter().map(|f| f.s("filename")).collect();
        let order = tree_order(&FileTree::build(&paths, |_| true));
        let key = format!("pr.file:{repo}#{number}");
        let chosen = self.choice(&key, "");
        let selected = paths.iter().position(|f| *f == chosen).or_else(|| order.first().copied()).unwrap_or(0);
        let comments_on = |name: &str| -> Vec<Value> {
            comments.iter().filter(|c| c.s("path") == name && c.has("line")).cloned().collect()
        };

        let p = palette();
        let search_id = format!("pr-file-search:{repo}#{number}");
        let query = self.field_text(&search_id).trim().to_lowercase();
        let search = self.input(&search_id, "Filter files…", cx).w_full();
        let tree = FileTree::build(&paths, |path| query.is_empty() || path.to_lowercase().contains(&query));
        let fold_prefix = format!("pr.dir:{repo}#{number}:");
        let mut entries = Vec::new();
        // While filtering every folder is open, so matches are never hidden.
        tree_entries(&tree, "", 0, &mut entries, &|dir| query.is_empty() && self.is_open(&format!("{fold_prefix}{dir}")));

        let mut picker = div()
            .id("pr-file-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroller("pr-file-list"))
            .py_1();
        if entries.is_empty() && !loading {
            picker = picker.child(div().p_3().child(widgets::faint("No files match.")));
        }
        for (n, entry) in entries.into_iter().enumerate() {
            let indent = px(10.0 + entry.depth as f32 * 14.0);
            let row = div()
                .id(("pr-tree", n))
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .h(px(26.0))
                .pl(indent)
                .pr_3()
                .cursor_pointer();
            picker = picker.child(match entry.kind {
                Entry::Dir { path, label, open } => {
                    let fold_key = format!("{fold_prefix}{path}");
                    row.hover(|s| s.bg(rgb(p.hover)))
                        .on_click(crate::hub::on(Act::run(move |hub, _, cx| {
                            // Folders start open; the set holds the closed ones.
                            if !hub.open.remove(&fold_key) {
                                hub.open.insert(fold_key.clone());
                            }
                            cx.notify();
                        })))
                        .child(icon(if open { "chevron-down" } else { "chevron-right" }, 12.0, p.text_dim))
                        .child(icon("folder", 13.0, p.text_dim))
                        .child(div().flex_1().min_w_0().text_size(px(12.0)).text_color(rgb(p.text_dim)).text_ellipsis().overflow_hidden().whitespace_nowrap().child(label))
                }
                Entry::File { index, label } => {
                    let file = &files[index];
                    let name = paths[index].clone();
                    let (mark, color) = match file.s("status").as_str() {
                        "added" => ("plus", widgets::green()),
                        "removed" => ("minus", widgets::red()),
                        "renamed" => ("arrow-right", widgets::yellow()),
                        _ => ("dot", widgets::yellow()),
                    };
                    let noted = comments_on(&name).len();
                    row.when(index == selected, |d| d.bg(rgb(p.selection_bg)))
                        .when(index != selected, |d| d.hover(|s| s.bg(rgb(p.hover))))
                        .tooltip(crate::ui::tip(name.clone(), None))
                        .on_click(crate::hub::on(Act::choose(key.clone(), name)))
                        // Lines up with a folder's chevron.
                        .child(div().w(px(12.0)).flex_none())
                        .child(icon(mark, 12.0, color))
                        .child(div().flex_1().min_w_0().text_size(px(12.0)).text_ellipsis().overflow_hidden().whitespace_nowrap().child(label))
                        .when(noted > 0, |d| d.child(widgets::row().gap_0p5().child(icon("comment", 11.0, p.text_dim)).child(widgets::faint(noted.to_string()))))
                        .child(div().text_size(px(11.0)).text_color(rgb(widgets::green())).child(format!("+{}", file.i("additions"))))
                        .child(div().text_size(px(11.0)).text_color(rgb(widgets::red())).child(format!("−{}", file.i("deletions"))))
                }
            });
        }
        if loading {
            picker = picker.child(widgets::loading());
        }
        let position = order.iter().position(|&i| i == selected).unwrap_or(0);
        let picker = widgets::card()
            .w(px(300.0))
            .flex_none()
            .h_full()
            .child(
                widgets::row()
                    .px_3()
                    .h(px(34.0))
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} files", pr.i("changed_files"))))
                    .child(widgets::spacer())
                    .child(widgets::faint(format!("{} of {}", position + 1, files.len()))),
            )
            .child(div().p_2().border_b_1().border_color(rgb(p.divider)).child(search))
            .child(picker);

        let file = &files[selected];
        let name = file.s("filename");
        let step = |id: &'static str, icon_name: &'static str, label: &'static str, to: Option<&Value>| {
            let act = to.map(|f| Act::choose(key.clone(), f.s("filename"))).unwrap_or(Act::None);
            widgets::ibtn(id, icon_name, label, act).disabled(to.is_none())
        };
        let nav = widgets::row()
            .child(step("file-prev", "chevron-left", "Previous file", position.checked_sub(1).and_then(|n| order.get(n)).map(|&i| &files[i])))
            .child(widgets::spacer())
            .child(step("file-next", "chevron-right", "Next file", order.get(position + 1).map(|&i| &files[i])));
        let diff = self.diff_file(&format!("prf{number}-{selected}"), file, Some(&target), &comments_on(&name), cx);

        // A fresh handle per file, so each starts at its top.
        let diff_scroll = self.scroller(&format!("pr-diff:{name}"));
        widgets::col()
            .gap_3()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(240.0))
                    .gap_3()
                    .child(picker)
                    .child(
                        div()
                            .id("pr-diff")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .track_scroll(&diff_scroll)
                            .child(widgets::col().gap_2().pb_4().child(diff).child(nav)),
                    ),
            )
            .into_any_element()
    }

    pub fn pull_checks(&mut self, repo: &str, sha: &str, cx: &mut Context<Self>) -> AnyElement {
        let repo_s = repo.to_string();
        let runs = ListSpec::new(format!("/repos/{repo}/commits/{sha}/check-runs"), move |c| check_row(&repo_s, c))
            .items("check_runs")
            .empty("No check runs for this commit.");
        let statuses = ListSpec::new(format!("/repos/{repo}/commits/{sha}/status"), |s| {
            let (icon_name, color) = match s.s("state").as_str() {
                "success" => ("check-circle", widgets::green()),
                "failure" | "error" => ("x-circle", widgets::red()),
                _ => ("dot", widgets::yellow()),
            };
            Row::new(s.s("context"))
                .icon(icon_name, color)
                .meta(s.s("description"))
                .open(Act::Url(s.s("target_url")))
        })
        .items("statuses")
        .unpaged()
        .empty("No commit statuses.");
        let runs = self.list(&runs, cx);
        let statuses = self.list(&statuses, cx);
        widgets::col()
            .gap_3()
            .child(widgets::h2("Check runs"))
            .child(runs)
            .child(widgets::h2("Commit statuses"))
            .child(statuses)
            .into_any_element()
    }

    pub fn compare(&mut self, repo: &str, base: &str, head: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/compare/{base}...{head}");
        let cmp = ready!(self.fetch(&path, cx));
        let status = cmp.s("status");
        let summary = format!(
            "{} is {} commits ahead and {} behind {}",
            head,
            cmp.i("ahead_by"),
            cmp.i("behind_by"),
            base
        );
        let mut col = widgets::col().gap_3();
        let mut rows: Vec<Row> = cmp.list("commits").iter().map(|c| commit_row(repo, c)).collect();
        self.resolve_authors(&mut rows, cx);
        for (i, row) in rows.into_iter().enumerate() {
            col = col.child(self.render_row(&format!("cmp-c{i}"), row, cx));
        }
        let commits = widgets::card().child(col);
        let mut files = widgets::col().gap_3();
        for (i, f) in cmp.list("files").to_vec().iter().enumerate() {
            files = files.child(self.diff_file(&format!("cmp-f{i}"), f, None, &[], cx));
        }
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title(format!("Comparing {base}...{head}")))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("create-pr", "Create pull request", new_pull_form(repo, base, head))),
            )
            .child(widgets::row().child(widgets::tag(status, widgets::gray())).child(widgets::dim(summary)))
            .child(widgets::h2(format!("{} commits", cmp.list("commits").len())))
            .child(commits)
            .child(widgets::h2(format!("{} files changed", cmp.list("files").len())))
            .child(files)
            .into_any_element()
    }
}

/// A commit as a list row.
pub fn commit_row(repo: &str, c: &Value) -> Row {
    let sha = c.s("sha");
    let who = if c.has("author.login") { c.s("author.login") } else { c.s("commit.author.name") };
    let verified = c.b("commit.verification.verified");
    let mut row = Row::new(first_line(&c.s("commit.message")))
        .avatar(c.s("author.avatar_url"))
        .meta(format!("{who} committed {}", time::ago(&c.s("commit.author.date"))))
        .commit(repo, sha.clone(), format!("committed {}", time::ago(&c.s("commit.author.date"))))
        .right(sha.chars().take(7).collect::<String>())
        .open(Act::Go(Route::Commit {
            repo: repo.to_string(),
            sha: sha.clone(),
        }))
        .action("Copy SHA", Act::Copy(sha.clone()))
        .action(
            "Browse files",
            Act::Go(Route::Tree {
                repo: repo.to_string(),
                git_ref: sha,
                path: String::new(),
                file: false,
            }),
        );
    if verified {
        row = row.tag("Verified", widgets::green());
    }
    row
}

impl Hub {
    /// One commit: message, author, stats, comments, and its diff.
    pub fn commit(&mut self, repo: &str, sha: &str, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/commits/{sha}");
        let c = ready!(self.fetch(&path, cx));
        let p = palette();
        let message = c.s("commit.message");
        let (headline, rest) = message.split_once('\n').unwrap_or((&message, ""));
        // GraphQL resolves co-authors to accounts; until it answers (or if
        // it can't), show the REST author.
        let authors = match self.commit_head(repo, sha, cx) {
            Some(head) => self.commit_authors("commit", head.list("authors.nodes"), cx),
            None => {
                let login = c.s("author.login");
                let name = if login.is_empty() { c.s("commit.author.name") } else { login.clone() };
                let author = json!([{ "name": name, "avatarUrl": c.s("author.avatar_url"), "user": if login.is_empty() { Value::Null } else { json!({ "login": login }) } }]);
                self.commit_authors("commit", author.list(""), cx)
            }
        };
        let mut files = widgets::col().gap_3();
        for (i, f) in c.list("files").to_vec().iter().enumerate() {
            files = files.child(self.diff_file(&format!("cf-{i}"), f, None, &[], cx));
        }
        let comments_path = format!("/repos/{repo}/commits/{sha}/comments");
        let field = format!("commit-comment:{repo}@{sha}");
        let mut comments = widgets::col().gap_3();
        if let Some(list) = self.fetch(&comments_path, cx).ready().cloned() {
            for (i, cm) in list.list("").iter().enumerate() {
                let url = cm.s("url");
                let acts = CommentActs {
                    edit: Some(super::common::edit_comment(&url, &cm.s("body"), &comments_path)),
                    delete: Some(super::common::delete_comment(&url, &comments_path)),
                    extra: Vec::new(),
                    reactions: Some(format!("{}/reactions", super::common::api_path(&url))),
                    invalidate: comments_path.clone(),
                    quote_into: Some(field.clone()),
                };
                comments = comments.child(self.comment_card(&format!("cc{i}"), cm, "commented", acts, cx));
            }
        }
        let submit = post_comment(&field, &comments_path, &comments_path);
        let composer = self.composer(&field, submit, Vec::new(), cx);
        let parents = c.list("parents").iter().map(|p| p.s("sha").chars().take(7).collect::<String>()).collect::<Vec<_>>().join(", ");
        widgets::page()
            .child(
                widgets::card()
                    .child(
                        div()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).child(headline.to_string()))
                            .when(!rest.trim().is_empty(), |d| {
                                d.child(widgets::mono(rest.trim().to_string()).text_color(rgb(p.text_dim)))
                            }),
                    )
                    .child(
                        widgets::row()
                            .px_4()
                            .py_2()
                            .bg(rgb(p.deep_bg))
                            .border_t_1()
                            .border_color(rgb(p.divider))
                            .child(authors)
                            .child(widgets::dim(format!("committed {}", time::ago(&c.s("commit.author.date")))))
                            .when(c.b("commit.verification.verified"), |d| d.child(widgets::tag("Verified", widgets::green())))
                            .child(widgets::spacer())
                            .child(widgets::dim(format!("parents {parents}")))
                            .child(widgets::mono(sha.to_string()))
                            .child(widgets::btn("copy-sha", "Copy", Act::Copy(sha.to_string())))
                            .child(widgets::btn(
                                "browse",
                                "Browse files",
                                Act::Go(Route::Tree {
                                    repo: repo.to_string(),
                                    git_ref: sha.to_string(),
                                    path: String::new(),
                                    file: false,
                                }),
                            )),
                    ),
            )
            .child(widgets::dim(format!(
                "Showing {} changed files with {} additions and {} deletions",
                c.list("files").len(),
                c.i("stats.additions"),
                c.i("stats.deletions")
            )))
            .child(files)
            .child(widgets::h2("Comments"))
            .child(comments)
            .child(composer)
            .child(div().h(px(8.0)))
            .child(widgets::btn(
                "back-to-repo",
                "Back to repository",
                Act::Go(Route::Repo {
                    repo: repo.to_string(),
                    tab: RepoTab::Commits,
                }),
            ))
            .into_any_element()
    }
}

/// Changed files as folders and files, for the PR file picker.
#[derive(Default)]
struct FileTree {
    dirs: std::collections::BTreeMap<String, FileTree>,
    /// (name, index into the file list)
    files: Vec<(String, usize)>,
}

impl FileTree {
    /// The tree of the paths `keep` accepts.
    fn build(paths: &[String], keep: impl Fn(&str) -> bool) -> FileTree {
        let mut root = FileTree::default();
        for (i, path) in paths.iter().enumerate() {
            if !keep(path) {
                continue;
            }
            let mut node = &mut root;
            let mut parts: Vec<&str> = path.split('/').collect();
            let name = parts.pop().unwrap_or_default();
            for part in parts {
                node = node.dirs.entry(part.to_string()).or_default();
            }
            node.files.push((name.to_string(), i));
        }
        root.sort();
        root
    }

    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.0.cmp(&b.0));
        self.dirs.values_mut().for_each(FileTree::sort);
    }

    /// A folder with nothing but one folder in it reads as one row:
    /// `app/packages/status-page-edge`.
    fn squash<'a>(mut label: String, mut node: &'a FileTree) -> (String, &'a FileTree) {
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (name, only) = node.dirs.iter().next().unwrap();
            label = format!("{label}/{name}");
            node = only;
        }
        (label, node)
    }
}

enum Entry {
    Dir { path: String, label: String, open: bool },
    File { index: usize, label: String },
}

struct TreeEntry {
    depth: usize,
    kind: Entry,
}

/// The rows to draw, folders first; `closed` says which folders are shut.
fn tree_entries(node: &FileTree, prefix: &str, depth: usize, out: &mut Vec<TreeEntry>, closed: &dyn Fn(&str) -> bool) {
    for (name, child) in &node.dirs {
        let (label, child) = FileTree::squash(name.clone(), child);
        let path = if prefix.is_empty() { label.clone() } else { format!("{prefix}/{label}") };
        let open = !closed(&path);
        out.push(TreeEntry { depth, kind: Entry::Dir { path: path.clone(), label, open } });
        if open {
            tree_entries(child, &path, depth + 1, out, closed);
        }
    }
    for (name, index) in &node.files {
        out.push(TreeEntry { depth, kind: Entry::File { index: *index, label: name.clone() } });
    }
}

/// Every file in the order the tree shows them, for previous/next.
fn tree_order(tree: &FileTree) -> Vec<usize> {
    let mut entries = Vec::new();
    tree_entries(tree, "", 0, &mut entries, &|_| false);
    entries
        .into_iter()
        .filter_map(|e| match e.kind {
            Entry::File { index, .. } => Some(index),
            Entry::Dir { .. } => None,
        })
        .collect()
}
