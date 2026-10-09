//! Pull requests: the conversation with its merge box, reviews, commits,
//! the changed files with inline review comments, checks, and creating
//! one from a comparison.

use super::common::{post_comment, side_section, CommentActs};
use crate::diff::ReviewTarget;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, PullTab, RepoTab, Req, Route};
use crate::json::{first_line, Json as _};
use crate::picker::{PickItem, Picker};
use crate::ready;
use crate::resource::{Fetched, ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette, Button, IconButton};
use crate::widgets::{self, rgb, TabItem};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};

pub fn new_pull_form(repo: &str, base: &str, head: &str) -> Act {
    let target = repo.to_string();
    FormSpec::new(format!("Open a {} in {repo}", crate::forge::pr()))
        .submit(format!("Create {}", crate::forge::pr()))
        .width(640.0)
        .field(
            Field::text("base", "Base branch")
                .value(base)
                .required()
                .hint("The branch to merge into."),
        )
        .field(
            Field::text(
                "head",
                if crate::forge::is_gitlab() {
                    "Source branch"
                } else {
                    "Compare branch"
                },
            )
            .value(head)
            .required()
            .hint("branch, or owner:branch for a fork."),
        )
        .field(Field::text("title", "Title").required())
        .field(Field::multiline("body", "Description"))
        .field(Field::bool("draft", "Create as draft", false))
        .field(Field::bool(
            "maintainer_can_modify",
            "Allow edits by maintainers",
            true,
        ))
        .rest("POST", format!("/repos/{repo}/pulls"))
        .ok(format!("{} opened", crate::forge::pr_title()))
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
            &[
                ("COMMENT", "Comment"),
                ("APPROVE", "Approve"),
                ("REQUEST_CHANGES", "Request changes"),
            ],
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
            if conclusion.is_empty() {
                status.clone()
            } else {
                conclusion.clone()
            },
            if duration.is_empty() {
                String::new()
            } else {
                format!(" in {duration}")
            }
        ))
        .body(check.s("output.title"))
        .open(match crate::hub::route_for_url(&details) {
            Some(route) => Act::Go(route),
            None if !details.is_empty() => Act::Url(details.clone()),
            None => Act::Url(check.s("html_url")),
        });
    if matches!(check.s("app.slug").as_str(), "github-actions" | "gitlab-ci") {
        row = row.action(
            "Re-run",
            Req::rest(
                "POST",
                format!("/repos/{repo}/actions/jobs/{}/rerun", check.i("id")),
            )
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
        ("queued", _) | ("waiting", _) | ("pending", _) | ("requested", _) => {
            ("clock", widgets::yellow())
        }
        ("completed", _) => ("check-circle", widgets::gray()),
        _ => ("circle", widgets::gray()),
    }
}

/// What GraphQL is asked about a commit's checks: how they went overall,
/// and how many there are and in what state.
pub const ROLLUP: &str = "statusCheckRollup { state contexts { checkRunCount checkRunCountsByState { state count } statusContextCount statusContextCountsByState { state count } } }";

/// How a commit's checks went, and how many of them passed: github.com's
/// "✓ 5 / 5".
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Checks {
    /// `statusCheckRollup.state`, or empty for a commit without checks.
    pub state: String,
    pub passed: i64,
    pub total: i64,
    /// The repository and commit they ran on, when known, so the mark
    /// can list them.
    pub repo: String,
    pub sha: String,
}

impl Checks {
    /// From a `statusCheckRollup` asked for with [`ROLLUP`].
    pub fn from_rollup(rollup: &Value) -> Self {
        let count = |key: &str, passing: &[&str]| -> i64 {
            rollup
                .list(&format!("contexts.{key}"))
                .iter()
                .filter(|c| passing.contains(&c.s("state").as_str()))
                .map(|c| c.i("count"))
                .sum()
        };
        Checks {
            state: rollup.s("state"),
            // Skipped and neutral runs don't hold anything up, as on
            // github.com.
            passed: count("checkRunCountsByState", &["SUCCESS", "NEUTRAL", "SKIPPED"])
                + count("statusContextCountsByState", &["SUCCESS"]),
            total: rollup.i("contexts.checkRunCount") + rollup.i("contexts.statusContextCount"),
            ..Checks::default()
        }
    }

    /// The checks of commit `sha` in `repo`: a mark that lists them when
    /// clicked.
    pub fn on(mut self, repo: impl Into<String>, sha: impl Into<String>) -> Self {
        self.repo = repo.into();
        self.sha = sha.into();
        self
    }
}

/// A commit's checks as one mark, the way github.com shows it beside a
/// commit: from GraphQL's `statusCheckRollup.state`.
pub fn ci_mark(state: &str) -> Option<(&'static str, u32, &'static str)> {
    match state {
        "SUCCESS" => Some(("check", widgets::green(), "All checks have passed")),
        "FAILURE" | "ERROR" => Some(("close", widgets::red(), "Some checks were not successful")),
        "PENDING" | "EXPECTED" => Some((
            "dot",
            widgets::yellow(),
            "Some checks haven't completed yet",
        )),
        _ => None,
    }
}

/// [`ci_mark`] drawn, with what it means on hover, and with `counts` how
/// many checks passed (`5 / 5`). When the commit is known, a click lists
/// its checks, as github.com does.
pub fn ci_mark_el(
    id: impl Into<gpui::ElementId>,
    checks: &Checks,
    counts: bool,
) -> Option<AnyElement> {
    let (mark, color, tip) = ci_mark(&checks.state)?;
    let tip = if checks.total > 0 {
        format!("{tip}: {} of {} passed", checks.passed, checks.total)
    } else {
        tip.to_string()
    };
    let open = (!checks.repo.is_empty() && !checks.sha.is_empty()).then(|| {
        let (repo, sha, state) = (
            checks.repo.clone(),
            checks.sha.clone(),
            checks.state.clone(),
        );
        Act::run(move |hub, _, cx| {
            hub.modal = Some(crate::hub::Modal::Checks {
                repo: repo.clone(),
                sha: sha.clone(),
                state: state.clone(),
            });
            cx.notify();
        })
    });
    Some(
        div()
            .id(id.into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .tooltip(crate::ui::tip(tip, None))
            .child(icon(mark, 14.0, color))
            .when(counts && checks.total > 0, |d| {
                d.child(widgets::dim(format!(
                    "{} / {}",
                    checks.passed, checks.total
                )))
            })
            .when_some(open, |d, act| {
                // The press stops here, so a row the mark sits in
                // doesn't open too.
                d.cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(crate::hub::on(act))
            })
            .into_any_element(),
    )
}

/// How a commit's check runs went: the mark, its colour, a title and a
/// count of each kind, as github.com heads them.
fn check_summary(runs: &[Value]) -> (&'static str, u32, String, String) {
    let ok = runs
        .iter()
        .filter(|r| {
            matches!(
                r.s("conclusion").as_str(),
                "success" | "skipped" | "neutral"
            )
        })
        .count();
    let bad = runs
        .iter()
        .filter(|r| {
            matches!(
                r.s("conclusion").as_str(),
                "failure" | "timed_out" | "cancelled" | "action_required"
            )
        })
        .count();
    let pending = runs.len() - ok - bad;
    if runs.is_empty() {
        (
            "dot",
            widgets::gray(),
            "No checks reported".to_string(),
            "This commit has no status checks.".to_string(),
        )
    } else if bad > 0 {
        (
            "close",
            widgets::red(),
            "Some checks were not successful".to_string(),
            format!("{bad} failing, {pending} pending, {ok} successful checks"),
        )
    } else if pending > 0 {
        (
            "clock",
            widgets::yellow(),
            "Some checks haven't completed yet".to_string(),
            format!("{pending} pending, {ok} successful checks"),
        )
    } else {
        (
            "check",
            widgets::green(),
            "All checks have passed".to_string(),
            format!("{ok} successful check{}", if ok == 1 { "" } else { "s" }),
        )
    }
}

/// A commit status (the older API some CI posts to) as a check run, so
/// it lists with them.
fn status_as_run(s: &Value) -> Value {
    let (status, conclusion) = match s.s("state").as_str() {
        "success" => ("completed", "success"),
        "failure" | "error" => ("completed", "failure"),
        _ => ("pending", ""),
    };
    json!({
        "name": s.s("context"),
        "status": status,
        "conclusion": conclusion,
        "details_url": s.s("target_url"),
        "app": { "name": s.s("description") },
    })
}

impl Hub {
    pub fn my_pulls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.my_items("pr", cx)
    }

    /// A repository's Pull requests tab.
    pub fn repo_pulls(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let list = self.issue_list(repo, "pr", cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::btn(
                        "compare",
                        "Compare branches",
                        compare_form(repo, default_branch),
                    ))
                    .child(widgets::go_btn(
                        "new-pr",
                        format!("New {}", crate::forge::pr()),
                        new_pull_form(repo, default_branch, ""),
                    )),
            )
            .child(list)
            .into_any_element()
    }

    pub fn pull(
        &mut self,
        repo: &str,
        number: u64,
        tab: PullTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                TabItem::new(
                    "Conversation",
                    "comment",
                    tab == PullTab::Conversation,
                    go(PullTab::Conversation),
                )
                .count(Some(pr.i("comments") + pr.i("review_comments"))),
                TabItem::new(
                    "Commits",
                    "commit",
                    tab == PullTab::Commits,
                    go(PullTab::Commits),
                )
                .count(Some(pr.i("commits"))),
                TabItem::new(
                    "Checks",
                    "check-circle",
                    tab == PullTab::Checks,
                    go(PullTab::Checks),
                ),
                TabItem::new(
                    "Files changed",
                    "file",
                    tab == PullTab::Files,
                    go(PullTab::Files),
                )
                .count(Some(pr.i("changed_files"))),
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
                widgets::row().child(div().flex_1().child(tabs)).child(
                    div().text_size(px(12.0)).child(
                        widgets::row()
                            .child(
                                div()
                                    .text_color(rgb(widgets::green()))
                                    .child(format!("+{}", pr.i("additions"))),
                            )
                            .child(
                                div()
                                    .text_color(rgb(widgets::red()))
                                    .child(format!("−{}", pr.i("deletions"))),
                            ),
                    ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(fill, |d| d.flex_1().min_h_0())
                    .child(body),
            )
            .into_any_element()
    }

    fn pull_conversation(
        &mut self,
        repo: &str,
        number: u64,
        pr: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let inval = crate::forge::issue_api(repo, number, true);
        let field = format!("comment:{repo}#{number}");
        let mut col = widgets::col().gap_3();
        let body_acts = CommentActs {
            edit: Some(
                FormSpec::new("Edit description")
                    .width(680.0)
                    .field(
                        Field::multiline("body", "Description")
                            .value(pr.s("body"))
                            .keep_empty(),
                    )
                    .rest("PATCH", format!("/repos/{repo}/pulls/{number}"))
                    .ok("Saved")
                    .inval(format!("/repos/{repo}/pulls/{number}"))
                    .act(),
            ),
            delete: None,
            extra: Vec::new(),
            reactions: (!crate::forge::is_bitbucket()).then(|| format!("{inval}/reactions")),
            invalidate: inval.clone(),
            quote_into: Some(field.clone()),
        };
        // The PR object has no reaction counts; the issue view of it does.
        let mut body_value = pr.clone();
        if let Some(issue) = self.fetch(&inval, cx).ready() {
            body_value["reactions"] = issue.at("reactions").clone();
        }
        col = col.child(self.comment_card("pr-body", &body_value, "opened", body_acts, cx));

        let me = self.login();
        let spec = ListSpec::new(format!("{inval}/timeline"), |_| Row::new(""));
        match self.fetch_list(&spec, cx) {
            Fetched::Items {
                items,
                loading,
                more,
            } => {
                // Commit events carry only the git author's name; ask who
                // the accounts are, co-authors included, in one go.
                let shas: Vec<String> = items
                    .iter()
                    .filter(|e| e.s("event") == "committed")
                    .map(|e| e.s("sha"))
                    .collect();
                let found = self.commit_batch(repo, &shas, cx);
                self.commit_info.extend(found);
                for (i, event) in items.iter().enumerate() {
                    if let Some(el) =
                        self.timeline_item(repo, &format!("tl{i}"), event, &field, &inval, &me, cx)
                    {
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
        let (pr_word, pr_title) = (crate::forge::pr(), crate::forge::pr_title());
        let extra = vec![
            widgets::icon_action(
                "review",
                "eye",
                palette().text_dim,
                "Review changes",
                review_form(repo, number),
            )
            .into_any_element(),
            if open {
                widgets::icon_action(
                    "close-pr",
                    "pr-closed",
                    widgets::red(),
                    format!("Close {pr_word}"),
                    state_req("closed", &format!("{pr_title} closed")),
                )
                .into_any_element()
            } else if !pr.b("merged") {
                widgets::icon_action(
                    "reopen-pr",
                    "pr",
                    widgets::green(),
                    format!("Reopen {pr_word}"),
                    state_req("open", &format!("{pr_title} reopened")),
                )
                .into_any_element()
            } else {
                div().into_any_element()
            },
        ];
        let submit = post_comment(&field, &format!("{inval}/comments"), &inval);
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
    fn merge_box(
        &mut self,
        repo: &str,
        number: u64,
        pr: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let pr_path = format!("/repos/{repo}/pulls/{number}");
        let inval_issue = crate::forge::issue_api(repo, number, true);
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
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
                        .child(widgets::h3(format!(
                            "{} successfully merged and closed",
                            crate::forge::pr_title()
                        )))
                        .child(widgets::dim(format!(
                            "Merged by {} {}",
                            pr.s("merged_by.login"),
                            time::ago(&pr.s("merged_at"))
                        ))),
                );
            if !head_repo.is_empty() {
                let exists =
                    self.fetch_check(&format!("/repos/{head_repo}/branches/{head_ref}"), cx);
                if exists.ready().map(|v| v.b("")).unwrap_or(false) {
                    row = row.child(widgets::btn(
                        "delete-head",
                        "Delete branch",
                        Req::rest(
                            "DELETE",
                            format!("/repos/{head_repo}/git/refs/heads/{head_ref}"),
                        )
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
        let checks = self.fetch(
            &format!("/repos/{repo}/commits/{sha}/check-runs?per_page=100"),
            cx,
        );
        let runs: Vec<Value> = checks
            .ready()
            .map(|v| v.list("check_runs").to_vec())
            .unwrap_or_default();
        let bad = runs
            .iter()
            .filter(|r| {
                matches!(
                    r.s("conclusion").as_str(),
                    "failure" | "timed_out" | "cancelled" | "action_required"
                )
            })
            .count();
        let (check_icon, check_color, check_title, check_sub) = check_summary(&runs);

        let state = pr.s("mergeable_state");
        // GitLab says why it can't merge in its own words.
        let blocker = pr.s("merge_blocker");
        let (merge_icon, merge_color, merge_title, merge_sub) = match state.as_str() {
            "blocked" if !blocker.is_empty() => (
                "alert",
                widgets::yellow(),
                "Merging is blocked",
                blocker.as_str(),
            ),
            "dirty" if gitlab => (
                "close",
                widgets::red(),
                "This branch has conflicts that must be resolved",
                "Resolve them on the command line or in GitLab's editor.",
            ),
            "dirty" if bitbucket => (
                "close",
                widgets::red(),
                "This branch has conflicts that must be resolved",
                "Resolve them on the command line, then push.",
            ),
            "draft" if gitlab => (
                "pr-draft",
                widgets::gray(),
                "This merge request is still a draft",
                "Draft merge requests can't be merged.",
            ),
            "clean" | "has_hooks" => (
                "check",
                widgets::green(),
                "No conflicts with base branch",
                "Merging can be performed automatically.",
            ),
            "unstable" => (
                "check",
                widgets::green(),
                "No conflicts with base branch",
                "Merging can be performed automatically, though some checks failed.",
            ),
            "dirty" => (
                "close",
                widgets::red(),
                "This branch has conflicts that must be resolved",
                "Resolve them on the command line or in GitHub's web editor.",
            ),
            "blocked" => (
                "alert",
                widgets::yellow(),
                "Merging is blocked",
                "Branch protection requires approving reviews or passing checks.",
            ),
            "behind" => (
                "alert",
                widgets::yellow(),
                "This branch is out-of-date with the base branch",
                "Update it to bring in the latest changes from the base.",
            ),
            "draft" => (
                "pr-draft",
                widgets::gray(),
                "This pull request is still a work in progress",
                "Draft pull requests can't be merged.",
            ),
            _ => ("clock", widgets::gray(), "Checking mergeability…", ""),
        };
        let mergeable = matches!(state.as_str(), "clean" | "has_hooks" | "unstable");
        // GitHub works mergeability out after the first ask, answering
        // "unknown" until then; ask again until it has.
        if matches!(state.as_str(), "unknown" | "") || pr.at("mergeable").is_null() {
            self.poll(std::slice::from_ref(&pr_path), 3, cx);
        }

        let method_key = format!("merge.method:{repo}");
        // GitLab merges or squashes; how it merges is the project's setting.
        let method = match self.choice(&method_key, "merge") {
            m if gitlab && m == "rebase" => "merge".to_string(),
            m => m,
        };
        let method_label = match method.as_str() {
            "rebase" if bitbucket => "Fast-forward",
            "squash" => "Squash and merge",
            "rebase" => "Rebase and merge",
            _ => "Create a merge commit",
        };
        let title_default = match method.as_str() {
            "squash" => format!("{} ({})", pr.s("title"), crate::forge::pr_ref(number)),
            _ if bitbucket => format!("Merged in {} (pull request #{number})", pr.s("head.ref")),
            _ if gitlab => format!(
                "Merge branch '{}' into '{}'",
                pr.s("head.ref"),
                pr.s("base.ref")
            ),
            _ => format!("Merge pull request #{number} from {}", pr.s("head.label")),
        };
        let repo_path = format!("/repos/{repo}");
        let merge_form = FormSpec::new(method_label)
            .submit("Confirm merge")
            .width(600.0)
            .field(Field::text("commit_title", "Commit title").value(title_default))
            .field(Field::multiline("commit_message", "Commit message").value(
                if method == "squash" {
                    pr.s("body")
                } else {
                    pr.s("title")
                },
            ))
            .field(Field::bool(
                "delete_branch",
                if gitlab {
                    "Delete the source branch afterwards"
                } else {
                    "Delete the head branch afterwards"
                },
                (gitlab && pr.b("force_remove_source_branch"))
                    || (bitbucket && pr.b("close_source_branch")),
            ))
            .build_with({
                let (pr_path, repo_path, method) =
                    (pr_path.clone(), repo_path.clone(), method.clone());
                let (head_repo, head_ref, sha) = (head_repo.clone(), head_ref.clone(), sha.clone());
                move |values| {
                    let mut body = json!({ "merge_method": method, "sha": sha });
                    if method != "rebase" {
                        body["commit_title"] = json!(values.s("commit_title"));
                        body["commit_message"] = json!(values.s("commit_message"));
                    }
                    let delete = values.b("delete_branch");
                    let (path, head_repo, head_ref) = (
                        format!("{pr_path}/merge"),
                        head_repo.clone(),
                        head_ref.clone(),
                    );
                    Ok(Req::custom(move |client| {
                        let merged = client.json("PUT", &path, Some(&body))?;
                        if delete && !head_repo.is_empty() {
                            client.json(
                                "DELETE",
                                &format!("/repos/{head_repo}/git/refs/heads/{head_ref}"),
                                None,
                            )?;
                        }
                        Ok(merged)
                    })
                    .ok(format!("{} merged", crate::forge::pr_title()))
                    // A merge moves the base branch: its commits, files
                    // and the repository page are all stale now.
                    .inval(repo_path.clone())
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
        let mut methods = vec![
            MenuEntry::check(
                if gitlab {
                    "Merge — as the project merges (a merge commit, or fast-forward)"
                } else {
                    "Create a merge commit — all commits are added to the base branch"
                },
                method == "merge",
                pick("merge"),
            ),
            MenuEntry::check(
                "Squash and merge — the commits are combined into one",
                method == "squash",
                pick("squash"),
            ),
        ];
        if bitbucket {
            methods.push(MenuEntry::check(
                "Fast-forward — the base branch moves up to the head, if it can",
                method == "rebase",
                pick("rebase"),
            ));
        } else if !gitlab {
            methods.push(MenuEntry::check(
                "Rebase and merge — the commits are rebased onto the base branch",
                method == "rebase",
                pick("rebase"),
            ));
        }
        let methods = Act::menu(methods);
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
        } else if state == "dirty" {
            // Conflicts: no merging until they're settled, here (GitLab's
            // editor is on its site).
            let resolve = if gitlab || bitbucket {
                Act::Url(
                    Route::Conflicts {
                        repo: repo.to_string(),
                        number,
                    }
                    .web_url(),
                )
            } else {
                Act::Go(Route::Conflicts {
                    repo: repo.to_string(),
                    number,
                })
            };
            widgets::btn("resolve-conflicts", "Resolve conflicts", resolve)
                .h(px(32.0))
                .px_4()
                .text_size(px(13.0))
                .into_any_element()
        } else if matches!(state.as_str(), "unknown" | "") {
            // Still working out whether it can merge.
            widgets::row()
                .gap_2()
                .child(crate::ui::Spinner::new("merge-wait").size(14.0))
                .child(widgets::dim("Checking whether this can merge…"))
                .into_any_element()
        } else {
            widgets::split_btn("merge", method_label, merge_form, methods)
        };
        let flag = match method.as_str() {
            "squash" => "--squash",
            "rebase" => "--rebase",
            _ if gitlab => "",
            _ => "--merge",
        };
        let cli = if gitlab {
            format!("glab mr merge {number} {flag} --repo {repo}").replace("  ", " ")
        } else {
            format!("gh pr merge {number} {flag} --repo {repo}")
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
                    // Bitbucket can do neither through its API.
                    .when(!bitbucket, |d| {
                        d.child(widgets::btn(
                            "update-branch",
                            if gitlab {
                                "Rebase source branch"
                            } else {
                                "Update branch"
                            },
                            Req::rest("PUT", format!("{pr_path}/update-branch"))
                                .body(json!({ "expected_head_sha": sha }))
                                .ok("Branch update queued")
                                .inval(pr_path.clone())
                                .act(),
                        ))
                        .child(widgets::btn(
                            "auto-merge",
                            match (gitlab, auto_merge_on) {
                                (true, true) => "Cancel auto-merge",
                                (true, false) => "Merge when pipeline succeeds",
                                (false, true) => "Disable auto-merge",
                                (false, false) => "Enable auto-merge",
                            },
                            auto,
                        ))
                    }),
            )
            .when(!bitbucket, |d| {
                d.child(
                    widgets::row()
                        .gap_1()
                        .child(widgets::dim(
                            "You can also merge this with the command line.",
                        ))
                        .child(
                            crate::ui::Link::new("merge-cli", "Copy the command")
                                .on_click(crate::hub::on(Act::Copy(cli))),
                        ),
                )
            });

        // A status: a filled circle with its mark, a title and a line.
        // `open` is what clicking does and the chevron that says so.
        let status = |id: &str,
                      mark: &str,
                      color: u32,
                      title: String,
                      sub: String,
                      open: Option<(Act, &'static str)>| {
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
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .when(!sub.is_empty(), |d| d.child(widgets::dim(sub))),
                )
                .when_some(chevron, |d, c| d.child(icon(c, 14.0, p.text_dim)))
                .when_some(open, |d, (act, _)| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(rgb(p.hover)))
                        .on_click(crate::hub::on(act))
                })
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
        card = card
            .border_color(rgb(if mergeable && bad == 0 {
                widgets::green()
            } else {
                p.edge
            }))
            .child(status(
                "merge-checks",
                check_icon,
                check_color,
                check_title,
                check_sub,
                (!runs.is_empty()).then_some((
                    toggle,
                    if expanded {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                )),
            ));
        if expanded {
            let list = self
                .check_list("merge-check", repo, runs, false)
                .max_h(px(360.0))
                .bg(rgb(p.deep_bg))
                .border_b_1()
                .border_color(rgb(p.divider));
            card = card.child(list);
        }
        card = card.child(status(
            "merge-state",
            merge_icon,
            merge_color,
            merge_title.to_string(),
            merge_sub.to_string(),
            None,
        ));
        if auto_merge_on {
            card = card.child(status(
                "merge-auto",
                "zap",
                widgets::green(),
                "Auto-merge is enabled".into(),
                format!(
                    "{} will merge this once its requirements are met.",
                    pr.s("auto_merge.enabled_by.login")
                ),
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
                        .child(
                            crate::ui::Link::new("to-draft", "Convert to draft")
                                .on_click(crate::hub::on(to_draft)),
                        ),
                )
            })
            .into_any_element()
    }

    fn pull_sidebar(
        &mut self,
        repo: &str,
        number: u64,
        pr: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pr_path = format!("/repos/{repo}/pulls/{number}");
        let requested: Vec<String> = pr
            .list("requested_reviewers")
            .iter()
            .map(|u| u.s("login"))
            .collect();
        let mut reviewer_picker = Picker::new("Request up to 15 reviewers", "Filter people…", true);
        match self
            .fetch(&format!("/repos/{repo}/assignees?per_page=100"), cx)
            .ready()
            .cloned()
        {
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
                    reviewer_picker = reviewer_picker.item(
                        PickItem::toggle(login.clone(), on_now, req("POST"), req("DELETE"))
                            .avatar(user.s("avatar_url")),
                    );
                }
            }
            None => reviewer_picker.loading = true,
        }
        // Latest review state per reviewer.
        let mut states: Vec<(String, String, String)> = Vec::new();
        if let Some(reviews) = self
            .fetch(&format!("{pr_path}/reviews?per_page=100"), cx)
            .ready()
            .cloned()
        {
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
            .field(
                Field::text("base", "Base branch")
                    .value(pr.s("base.ref"))
                    .required(),
            )
            .rest("PATCH", pr_path.clone())
            .ok("Base branch changed")
            .inval(pr_path.clone())
            .act();
        let issue_side = self.issue_sidebar(repo, pr, true, cx);
        widgets::col()
            .gap_3()
            .child(side_section(
                "reviewers",
                "Reviewers",
                Some(reviewer_picker.act()),
                reviewers.into_any_element(),
            ))
            .child(issue_side)
            .child(widgets::ibtn("change-base", "branch", "Change base branch", base_form).w_full())
            .child(
                widgets::ibtn(
                    "checkout",
                    "copy",
                    "Copy checkout command",
                    Act::Copy(if crate::forge::is_bitbucket() {
                        let branch = pr.s("head.ref");
                        format!("git fetch origin {branch} && git switch {branch}")
                    } else if crate::forge::is_gitlab() {
                        format!("glab mr checkout {number} --repo {repo}")
                    } else {
                        format!("gh pr checkout {number} --repo {repo}")
                    }),
                )
                .w_full(),
            )
            .into_any_element()
    }

    fn pull_commits(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) -> AnyElement {
        // Files changed, showing one commit's changes.
        let review = move |repo: &str, sha: String| {
            let key = format!("pr.commit:{repo}#{number}");
            let route = Route::Pull {
                repo: repo.to_string(),
                number,
                tab: PullTab::Files,
            };
            Act::run(move |hub, _, cx| {
                hub.choices.insert(key.clone(), sha.clone());
                hub.go(route.clone(), cx);
            })
        };
        let repo_s = repo.to_string();
        let spec = ListSpec::new(format!("/repos/{repo}/pulls/{number}/commits"), move |c| {
            commit_row(&repo_s, c).action("Review its changes", review(&repo_s, c.s("sha")))
        });
        let list = self.list(&spec, cx);
        let first = self
            .pr_commits(repo, number, cx)
            .0
            .first()
            .map(|c| c.s("sha"));
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::dim(
                        "Step through the changes one commit at a time, rather than all at once.",
                    ))
                    .child(widgets::spacer())
                    .child(
                        widgets::btn(
                            "review-by-commit",
                            "Review commit by commit",
                            first
                                .clone()
                                .map(|sha| review(repo, sha))
                                .unwrap_or(Act::None),
                        )
                        .disabled(first.is_none()),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    /// A pull request's commits, oldest first, as far as they've loaded;
    /// whether a page is loading; and, when there's a further page, the
    /// list whose page to turn for it. GitHub lists the oldest first, so
    /// pages load as they're wanted. GitLab and Bitbucket list the newest
    /// first, so every page loads before any shows.
    fn pr_commits(
        &mut self,
        repo: &str,
        number: u64,
        cx: &mut Context<Self>,
    ) -> (Vec<Value>, bool, Option<String>) {
        // The Commits tab's own list, so the two share their pages.
        let spec = ListSpec::new(format!("/repos/{repo}/pulls/{number}/commits"), |_| {
            Row::new("")
        });
        let github = crate::forge::is_github();
        loop {
            match self.fetch_list(&spec, cx) {
                Fetched::Items {
                    more: true,
                    loading: false,
                    ..
                } if !github => {
                    let next = self.page(&spec.id) + 1;
                    self.pages.insert(spec.id.clone(), next);
                }
                Fetched::Items { loading: true, .. } if !github => return (Vec::new(), true, None),
                Fetched::Items {
                    mut items,
                    loading,
                    more,
                } => {
                    if !github {
                        items.reverse();
                    }
                    return (items, loading, more.then(|| spec.id.clone()));
                }
                Fetched::Failed(_) => return (Vec::new(), false, None),
            }
        }
    }

    /// The Files tab's commit picker: all commits, or any one, growing as
    /// it scrolls.
    fn commit_picker(
        &mut self,
        repo: &str,
        number: u64,
        key: &str,
        n: i64,
        cx: &mut Context<Self>,
    ) -> Picker {
        let (commits, loading, more) = self.pr_commits(repo, number, cx);
        let sha = self.choice(key, "");
        let choose = |sha: String| Act::choose(key.to_string(), sha);
        let mut picker = Picker::new("Show changes from", "Filter commits…", false).item(
            PickItem::new("All commits", sha.is_empty(), choose(String::new()))
                .detail(format!("{n} commits, as one diff")),
        );
        for (i, c) in commits.iter().enumerate() {
            let short: String = c.s("sha").chars().take(7).collect();
            picker = picker.item(
                PickItem::new(
                    first_line(&c.s("commit.message")),
                    c.s("sha") == sha,
                    choose(c.s("sha")),
                )
                .detail(format!("{} of {n} · {short}", i + 1)),
            );
        }
        picker.loading = loading;
        picker.more = more;
        picker
    }

    fn pull_files(
        &mut self,
        repo: &str,
        number: u64,
        pr: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The whole pull request's changes, or one commit's: "" or its SHA.
        let commit_key = format!("pr.commit:{repo}#{number}");
        let sha = self.choice(&commit_key, "");
        let (mut commits, _, more) = self.pr_commits(repo, number, cx);
        let mut at = commits.iter().position(|c| c.s("sha") == sha);
        // Page on until the chosen commit has loaded, and the one after it,
        // so the next is a click away.
        if let Some(id) = more.filter(|_| !sha.is_empty()) {
            if at.is_none_or(|i| i + 1 == commits.len()) {
                let next = self.page(&id) + 1;
                self.pages.insert(id, next);
                commits = self.pr_commits(repo, number, cx).0;
                at = commits.iter().position(|c| c.s("sha") == sha);
            }
        }
        let total = pr.i("commits").max(commits.len() as i64);
        let picker = {
            let (repo, key) = (repo.to_string(), commit_key.clone());
            Picker::live(move |hub, cx| hub.commit_picker(&repo, number, &key, total, cx))
        };
        let comments = self
            .fetch(
                &format!("/repos/{repo}/pulls/{number}/comments?per_page=100"),
                cx,
            )
            .ready()
            .map(|v| v.list("").to_vec())
            .unwrap_or_default();
        // On one commit, the comments left on its own diff. Only GitHub's
        // say which commit they were left on, or can be left on one.
        let comments = if sha.is_empty() {
            comments
        } else {
            commit_comments(&comments, &sha)
        };
        let target = ReviewTarget {
            repo: repo.to_string(),
            number,
            commit: if sha.is_empty() {
                pr.s("head.sha")
            } else {
                sha.clone()
            },
            comments: sha.is_empty() || crate::forge::is_github(),
        };
        let hint = if target.comments {
            "Click a line to leave a review comment on it, or shift-click two lines to comment on the lines between."
        } else {
            "Comments go on the whole diff here: pick All commits to leave one."
        };
        let header = widgets::row()
            .child(commit_bar(
                repo,
                &commit_key,
                &commits,
                at,
                !sha.is_empty(),
                total,
                picker,
            ))
            .child(div().flex_1().min_w_0().child(widgets::dim(hint)))
            .child(widgets::primary(
                "review-top",
                "Review changes",
                review_form(repo, number),
            ));
        let (files, loading) = if sha.is_empty() {
            let spec = ListSpec::new(format!("/repos/{repo}/pulls/{number}/files"), |_| {
                Row::new("")
            });
            // Every page, without asking: the tree needs the whole list.
            loop {
                match self.fetch_list(&spec, cx) {
                    Fetched::Items {
                        more: true,
                        loading: false,
                        ..
                    } => {
                        let next = self.page(&spec.id) + 1;
                        self.pages.insert(spec.id.clone(), next);
                    }
                    Fetched::Items { items, loading, .. } => break (items, loading),
                    Fetched::Failed(error) => {
                        return widgets::col()
                            .gap_3()
                            .child(header)
                            .child(widgets::error_box(&error))
                            .into_any_element()
                    }
                }
            }
        } else {
            match self.fetch(&format!("/repos/{repo}/commits/{sha}"), cx) {
                Load::Ready(c) => (c.list("files").to_vec(), false),
                Load::Loading => (Vec::new(), true),
                Load::Failed(error) => {
                    return widgets::col()
                        .gap_3()
                        .child(header)
                        .child(widgets::error_box(&error))
                        .into_any_element()
                }
            }
        };
        if files.is_empty() {
            let body = if loading {
                widgets::loading()
            } else {
                widgets::card()
                    .child(widgets::empty(if sha.is_empty() {
                        "No files changed."
                    } else {
                        "This commit changes no files."
                    }))
                    .into_any_element()
            };
            return widgets::col()
                .gap_3()
                .child(header)
                .child(body)
                .into_any_element();
        }

        // One file at a time on the right, picked from a tree on the left.
        let paths: Vec<String> = files.iter().map(|f| f.s("filename")).collect();
        let order = tree_order(&FileTree::build(&paths, |_| true));
        let key = format!("pr.file:{repo}#{number}");
        let chosen = self.choice(&key, "");
        let selected = paths
            .iter()
            .position(|f| *f == chosen)
            .or_else(|| order.first().copied())
            .unwrap_or(0);
        let comments_on = |name: &str| -> Vec<Value> {
            comments
                .iter()
                .filter(|c| c.s("path") == name && c.has("line"))
                .cloned()
                .collect()
        };

        let p = palette();
        let search_id = format!("pr-file-search:{repo}#{number}");
        let query = self.field_text(&search_id).trim().to_lowercase();
        let search = self.input(&search_id, "Filter files…", cx).w_full();
        let tree = FileTree::build(&paths, |path| {
            query.is_empty() || path.to_lowercase().contains(&query)
        });
        let fold_prefix = format!("pr.dir:{repo}#{number}:");
        let mut entries = Vec::new();
        // While filtering every folder is open, so matches are never hidden.
        tree_entries(&tree, "", 0, &mut entries, &|dir| {
            query.is_empty() && self.is_open(&format!("{fold_prefix}{dir}"))
        });

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
                        .child(icon(
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            12.0,
                            p.text_dim,
                        ))
                        .child(icon("folder", 13.0, p.text_dim))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.0))
                                .text_color(rgb(p.text_dim))
                                .text_ellipsis()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(label),
                        )
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
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.0))
                                .text_ellipsis()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .when(noted > 0, |d| {
                            d.child(
                                widgets::row()
                                    .gap_0p5()
                                    .child(icon("comment", 11.0, p.text_dim))
                                    .child(widgets::faint(noted.to_string())),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(widgets::green()))
                                .child(format!("+{}", file.i("additions"))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(widgets::red()))
                                .child(format!("−{}", file.i("deletions"))),
                        )
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
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if sha.is_empty() {
                                format!("{} files", pr.i("changed_files"))
                            } else {
                                format!("{} files", files.len())
                            }),
                    )
                    .child(widgets::spacer())
                    .child(widgets::faint(format!(
                        "{} of {}",
                        position + 1,
                        files.len()
                    ))),
            )
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .child(search),
            )
            .child(picker);

        let file = &files[selected];
        let name = file.s("filename");
        let step =
            |id: &'static str, icon_name: &'static str, label: &'static str, to: Option<&Value>| {
                let act = to
                    .map(|f| Act::choose(key.clone(), f.s("filename")))
                    .unwrap_or(Act::None);
                widgets::ibtn(id, icon_name, label, act).disabled(to.is_none())
            };
        let nav = widgets::row()
            .child(step(
                "file-prev",
                "chevron-left",
                "Previous file",
                position
                    .checked_sub(1)
                    .and_then(|n| order.get(n))
                    .map(|&i| &files[i]),
            ))
            .child(widgets::spacer())
            .child(step(
                "file-next",
                "chevron-right",
                "Next file",
                order.get(position + 1).map(|&i| &files[i]),
            ));
        let short: String = sha.chars().take(12).collect();
        let diff = self.diff_file(
            &format!("prf{number}@{short}-{selected}"),
            file,
            Some(&target),
            &comments_on(&name),
            cx,
        );

        // A fresh handle per file, so each starts at its top.
        let diff_scroll = self.scroller(&format!("pr-diff:{short}:{name}"));
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

    /// Check runs one to a line, failing first, then running, then the
    /// rest, by name: mark, name, where it ran and how long, and the
    /// links. In a dialog (`in_modal`), following a link closes it.
    fn check_list(
        &mut self,
        id: &str,
        repo: &str,
        mut runs: Vec<Value>,
        in_modal: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let p = palette();
        let rank = |c: &Value| match (c.s("status").as_str(), c.s("conclusion").as_str()) {
            (_, "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure") => 0,
            ("completed", _) => 2,
            _ => 1,
        };
        runs.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.s("name").cmp(&b.s("name")))
        });
        let list_id = format!("{id}-list");
        let mut list = div()
            .id(gpui::ElementId::Name(list_id.clone().into()))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroller(&list_id));
        let leave = |act: Act| {
            if !in_modal {
                return act;
            }
            Act::run(move |hub, window, cx| {
                hub.close_modal(cx);
                hub.perform(act.clone(), window, cx);
            })
        };
        for (i, run) in runs.iter().enumerate() {
            let (mark, color) = status_icon(&run.s("status"), &run.s("conclusion"));
            let took = time::span(&run.s("started_at"), &run.s("completed_at"));
            let summary = match (run.s("conclusion").as_str(), took.is_empty()) {
                ("", _) => run.s("status").replace('_', " "),
                (c, true) => c.to_string(),
                (c, false) => format!("{c} in {took}"),
            };
            let app = run.s("app.name");
            let row = check_row(repo, run);
            let rerun = row.actions.into_iter().find(|a| a.label == "Re-run");
            let name = |part: &str| gpui::ElementId::Name(format!("{id}-{part}-{i}").into());
            list = list.child(
                div()
                    .id(name("row"))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(32.0))
                    .px_4()
                    .text_size(px(12.0))
                    .hover(|s| s.bg(rgb(p.hover)))
                    .child(icon(mark, 14.0, color))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(run.s("name")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(p.text_dim))
                            .text_ellipsis()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(if app.is_empty() {
                                summary
                            } else {
                                format!("{app}  ·  {summary}")
                            }),
                    )
                    .when_some(rerun, |d, a| {
                        d.child(
                            crate::ui::Link::new(name("rerun"), "Re-run")
                                .on_click(crate::hub::on(a.act)),
                        )
                    })
                    .when(!matches!(row.open, Act::Url(ref u) if u.is_empty()), |d| {
                        d.child(
                            crate::ui::Link::new(name("details"), "Details")
                                .on_click(crate::hub::on(leave(row.open))),
                        )
                    }),
            );
        }
        list
    }

    /// What the dialog a commit's checks mark opens says: how they went
    /// (its title), and each one, as github.com shows it. `state` is the
    /// mark's, for the title until the runs are in.
    pub fn checks_dialog(
        &mut self,
        repo: &str,
        sha: &str,
        state: &str,
        cx: &mut Context<Self>,
    ) -> (String, AnyElement) {
        let p = palette();
        let runs_path = format!("/repos/{repo}/commits/{sha}/check-runs?per_page=100");
        let runs = self.fetch(&runs_path, cx).ready().cloned();
        // GitHub counts commit statuses among the checks too; elsewhere
        // they are the same jobs again.
        let statuses = crate::forge::is_github()
            .then(|| {
                self.fetch(&format!("/repos/{repo}/commits/{sha}/status"), cx)
                    .ready()
                    .cloned()
            })
            .flatten();
        let mut all: Vec<Value> = runs
            .as_ref()
            .map(|v| v.list("check_runs").to_vec())
            .unwrap_or_default();
        if let Some(statuses) = &statuses {
            all.extend(statuses.list("statuses").iter().map(status_as_run));
        }
        let loaded = runs.is_some() && (statuses.is_some() || !crate::forge::is_github());
        let (title, sub) = if loaded {
            let (_, _, title, sub) = check_summary(&all);
            (title, sub)
        } else {
            (
                ci_mark(state)
                    .map(|(_, _, t)| t.to_string())
                    .unwrap_or_else(|| "Checks".to_string()),
                String::new(),
            )
        };
        // Keep running checks moving while they show.
        if all.iter().any(|r| r.s("status") != "completed") {
            self.poll(
                &[
                    format!("/repos/{repo}/commits/{sha}/check-runs"),
                    format!("/repos/{repo}/commits/{sha}/status"),
                ],
                10,
                cx,
            );
        }
        let body = if !loaded {
            div()
                .p_4()
                .flex()
                .justify_center()
                .child(crate::ui::Spinner::new("checks-loading").size(16.0))
                .into_any_element()
        } else {
            self.check_list("checks-dialog", repo, all, true)
                .max_h(px(420.0))
                .border_t_1()
                .border_b_1()
                .border_color(rgb(p.divider))
                .bg(rgb(p.deep_bg))
                .into_any_element()
        };
        let short: String = sha.chars().take(7).collect();
        let head = widgets::dim(if sub.is_empty() {
            format!("On {short}")
        } else {
            format!("{sub} on {short}")
        });
        (
            title,
            widgets::col()
                .gap_3()
                .child(head)
                .child(body)
                .into_any_element(),
        )
    }

    pub fn pull_checks(&mut self, repo: &str, sha: &str, cx: &mut Context<Self>) -> AnyElement {
        let repo_s = repo.to_string();
        let runs = ListSpec::new(
            format!("/repos/{repo}/commits/{sha}/check-runs"),
            move |c| check_row(&repo_s, c),
        )
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

    pub fn compare(
        &mut self,
        repo: &str,
        base: &str,
        head: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let mut rows: Vec<Row> = cmp
            .list("commits")
            .iter()
            .map(|c| commit_row(repo, c))
            .collect();
        self.resolve_authors(&mut rows, cx);
        self.resolve_pull_checks(&mut rows, cx);
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
                    .child(widgets::go_btn(
                        "create-pr",
                        format!("Create {}", crate::forge::pr()),
                        new_pull_form(repo, base, head),
                    )),
            )
            .child(
                widgets::row()
                    .child(widgets::tag(status, widgets::gray()))
                    .child(widgets::dim(summary)),
            )
            .child(widgets::h2(format!(
                "{} commits",
                cmp.list("commits").len()
            )))
            .child(commits)
            .child(widgets::h2(format!(
                "{} files changed",
                cmp.list("files").len()
            )))
            .child(files)
            .into_any_element()
    }
}

/// The Files tab's choice of what to show: every commit's changes, or
/// one commit's, with buttons to step through them in order.
fn commit_bar(
    repo: &str,
    key: &str,
    commits: &[Value],
    at: Option<usize>,
    one: bool,
    n: i64,
    picker: Act,
) -> AnyElement {
    let p = palette();
    let choose = |sha: String| Act::choose(key.to_string(), sha);
    let short = |c: &Value| c.s("sha").chars().take(7).collect::<String>();
    let label = match at {
        Some(i) => format!("Commit {} of {n}", i + 1),
        None if one => "One commit".to_string(),
        None => "All commits".to_string(),
    };
    let step =
        |id: &'static str, icon_name: &'static str, tip: &'static str, to: Option<&Value>| {
            IconButton::new(id, icon_name)
                .size(28.0)
                .icon_size(14.0)
                .color(p.text)
                .tooltip(tip, None)
                .disabled(to.is_none())
                .on_click(on(to.map(|c| choose(c.s("sha"))).unwrap_or(Act::None)))
        };
    // From all commits, "next" starts at the first.
    let (prev, next) = match at {
        Some(i) => (
            i.checked_sub(1).and_then(|i| commits.get(i)),
            commits.get(i + 1),
        ),
        None => (None, if one { None } else { commits.first() }),
    };
    let mut bar = widgets::row()
        .gap_1()
        .flex_none()
        .child(widgets::dropdown_btn(
            "pr-commit",
            Some(icon("commit", 14.0, p.text_dim).into_any_element()),
            label,
            picker,
        ))
        .child(step(
            "pr-commit-prev",
            "chevron-left",
            "Previous commit",
            prev,
        ))
        .child(step("pr-commit-next", "chevron-right", "Next commit", next));
    if let Some(c) = at.map(|i| &commits[i]) {
        let headline = first_line(&c.s("commit.message"));
        bar = bar.child(
            Button::new("pr-commit-open", "")
                .h(px(28.0))
                .px_2()
                .gap_2()
                .max_w(px(360.0))
                .tooltip(headline.clone(), Some("Open this commit".into()))
                .child(
                    div()
                        .font_family(widgets::MONO)
                        .text_size(px(12.0))
                        .child(short(c)),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(headline),
                )
                .on_click(on(Act::Go(Route::Commit {
                    repo: repo.to_string(),
                    sha: c.s("sha"),
                }))),
        );
    }
    bar.into_any_element()
}

/// The review comments left on commit `sha`'s own diff, placed by the
/// lines they were left on rather than where those lines are now.
fn commit_comments(comments: &[Value], sha: &str) -> Vec<Value> {
    comments
        .iter()
        .filter(|c| c.s("original_commit_id") == sha && c.has("original_line"))
        .map(|c| {
            let mut c = c.clone();
            c["line"] = c["original_line"].clone();
            c["start_line"] = c["original_start_line"].clone();
            c
        })
        .collect()
}

/// A commit as a list row.
pub fn commit_row(repo: &str, c: &Value) -> Row {
    let sha = c.s("sha");
    let who = if c.has("author.login") {
        c.s("author.login")
    } else {
        c.s("commit.author.name")
    };
    let verified = c.b("commit.verification.verified");
    let mut row = Row::new(first_line(&c.s("commit.message")))
        .avatar(c.s("author.avatar_url"))
        .meta(format!(
            "{who} committed {}",
            time::ago(&c.s("commit.author.date"))
        ))
        .commit(
            repo,
            sha.clone(),
            format!("committed {}", time::ago(&c.s("commit.author.date"))),
        )
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
        let head = self.commit_head(repo, sha, cx);
        let checks = head
            .as_ref()
            .map(|h| Checks::from_rollup(h.at("statusCheckRollup")).on(repo, sha))
            .unwrap_or_default();
        let authors = match head {
            Some(head) => self.commit_authors("commit", head.list("authors.nodes"), cx),
            None => {
                let login = c.s("author.login");
                let name = if login.is_empty() {
                    c.s("commit.author.name")
                } else {
                    login.clone()
                };
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
                    edit: Some(super::common::edit_comment(
                        &url,
                        &cm.s("body"),
                        &comments_path,
                    )),
                    delete: Some(super::common::delete_comment(&url, &comments_path)),
                    extra: Vec::new(),
                    reactions: crate::forge::is_github()
                        .then(|| format!("{}/reactions", super::common::api_path(&url))),
                    invalidate: comments_path.clone(),
                    quote_into: Some(field.clone()),
                };
                comments =
                    comments.child(self.comment_card(&format!("cc{i}"), cm, "commented", acts, cx));
            }
        }
        let submit = post_comment(&field, &comments_path, &comments_path);
        let composer = self.composer(&field, submit, Vec::new(), cx);
        let parents = c
            .list("parents")
            .iter()
            .map(|p| p.s("sha").chars().take(7).collect::<String>())
            .collect::<Vec<_>>()
            .join(", ");
        widgets::page()
            .child(
                widgets::card()
                    .child(
                        div()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(headline.to_string()),
                            )
                            .when(!rest.trim().is_empty(), |d| {
                                d.child(
                                    widgets::mono(rest.trim().to_string())
                                        .text_color(rgb(p.text_dim)),
                                )
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
                            .child(widgets::dim(format!(
                                "committed {}",
                                time::ago(&c.s("commit.author.date"))
                            )))
                            .children(ci_mark_el("commit-checks", &checks, true))
                            .when(c.b("commit.verification.verified"), |d| {
                                d.child(widgets::tag("Verified", widgets::green()))
                            })
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
    fn squash(mut label: String, mut node: &FileTree) -> (String, &FileTree) {
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (name, only) = node.dirs.iter().next().unwrap();
            label = format!("{label}/{name}");
            node = only;
        }
        (label, node)
    }
}

enum Entry {
    Dir {
        path: String,
        label: String,
        open: bool,
    },
    File {
        index: usize,
        label: String,
    },
}

struct TreeEntry {
    depth: usize,
    kind: Entry,
}

/// The rows to draw, folders first; `closed` says which folders are shut.
fn tree_entries(
    node: &FileTree,
    prefix: &str,
    depth: usize,
    out: &mut Vec<TreeEntry>,
    closed: &dyn Fn(&str) -> bool,
) {
    for (name, child) in &node.dirs {
        let (label, child) = FileTree::squash(name.clone(), child);
        let path = if prefix.is_empty() {
            label.clone()
        } else {
            format!("{prefix}/{label}")
        };
        let open = !closed(&path);
        out.push(TreeEntry {
            depth,
            kind: Entry::Dir {
                path: path.clone(),
                label,
                open,
            },
        });
        if open {
            tree_entries(child, &path, depth + 1, out, closed);
        }
    }
    for (name, index) in &node.files {
        out.push(TreeEntry {
            depth,
            kind: Entry::File {
                index: *index,
                label: name.clone(),
            },
        });
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

#[cfg(test)]
mod tests {
    use super::{commit_comments, Checks};
    use crate::json::Json as _;
    use serde_json::json;

    #[test]
    fn commit_comments_sit_where_they_were_left() {
        let comments = [
            json!({ "id": 1, "original_commit_id": "aaa", "original_line": 4, "original_start_line": 2, "line": 9 }),
            json!({ "id": 2, "original_commit_id": "bbb", "original_line": 7, "line": 7 }),
            json!({ "id": 3, "original_commit_id": "aaa", "original_line": null, "line": null }),
        ];
        let on_a = commit_comments(&comments, "aaa");
        assert_eq!(on_a.len(), 1);
        assert_eq!(
            (on_a[0].i("id"), on_a[0].i("line"), on_a[0].i("start_line")),
            (1, 4, 2)
        );
        assert!(commit_comments(&comments, "ccc").is_empty());
    }

    #[test]
    fn checks_count_what_passed() {
        let rollup = json!({
            "state": "FAILURE",
            "contexts": {
                "checkRunCount": 5,
                "checkRunCountsByState": [
                    { "state": "SUCCESS", "count": 2 },
                    { "state": "SKIPPED", "count": 1 },
                    { "state": "FAILURE", "count": 2 },
                ],
                "statusContextCount": 1,
                "statusContextCountsByState": [{ "state": "SUCCESS", "count": 1 }],
            },
        });
        let checks = Checks::from_rollup(&rollup);
        assert_eq!(
            (checks.state.as_str(), checks.passed, checks.total),
            ("FAILURE", 4, 6)
        );
        assert_eq!(
            Checks::from_rollup(&serde_json::Value::Null),
            Checks::default()
        );
    }
}
