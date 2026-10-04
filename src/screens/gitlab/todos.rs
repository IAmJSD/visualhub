//! GitLab's to-do list, where GitHub has notifications: what needs you,
//! with what put it there, until it's done.

use crate::hub::{Act, Hub, MenuEntry, PullTab, Req, Route};
use crate::json::Json as _;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _};
use serde_json::Value;

/// Why it's on the list, as GitLab says it.
fn why(action: &str) -> &'static str {
    match action {
        "assigned" => "assigned you",
        "mentioned" => "mentioned you",
        "directly_addressed" => "addressed you",
        "build_failed" => "pipeline failed",
        "marked" => "added a to-do",
        "approval_required" => "set you as an approver",
        "unmergeable" => "could not merge",
        "review_requested" => "requested your review",
        "review_submitted" => "reviewed your merge request",
        "merge_train_removed" => "removed from the merge train",
        "member_access_requested" => "requested access",
        "added_approver" => "made you an approver",
        "ssh_key_expired" => "SSH key expired",
        "ssh_key_expiring_soon" => "SSH key expires soon",
        _ => "",
    }
}

/// Where a to-do's target is in the app, or on the site.
fn open(t: &Value) -> Act {
    let repo = t.s("project.path_with_namespace");
    let target = t.at("target");
    let number = target.i("iid") as u64;
    match t.s("target_type").as_str() {
        "Issue" | "WorkItem" if !repo.is_empty() => Act::Go(Route::Issue { repo, number }),
        "MergeRequest" if !repo.is_empty() => Act::Go(Route::Pull { repo, number, tab: PullTab::Conversation }),
        "Commit" if !repo.is_empty() => Act::Go(Route::Commit { repo, sha: target.s("id") }),
        _ => Act::Url(t.s("target_url")),
    }
}

impl Hub {
    pub fn gl_todos(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.choice("todos.state", "pending");
        let kind = self.choice("todos.type", "");
        let mut path = format!("/api/v4/todos?state={state}");
        if !kind.is_empty() {
            path.push_str(&format!("&type={kind}"));
        }
        let pending = state == "pending";
        let spec = ListSpec::new(path, move |t| {
            let target = t.at("target");
            let open_now = target.s("state") == "opened";
            let (icon, color) = match t.s("target_type").as_str() {
                "MergeRequest" => (if target.s("state") == "merged" { "pr-merged" } else { "pr" }, if open_now { widgets::green() } else { widgets::purple() }),
                "Issue" | "WorkItem" => ("issue", if open_now { widgets::green() } else { widgets::purple() }),
                "Commit" => ("commit", widgets::gray()),
                _ => ("bell", widgets::gray()),
            };
            let title = if target.s("title").is_empty() { t.s("body") } else { target.s("title") };
            let reference = target.s("references.short");
            let id = t.i("id");
            let mut row = Row::new(title)
                .icon(icon, color)
                .meta(format!(
                    "{}{}  ·  {} {}  ·  {}",
                    t.s("project.path_with_namespace"),
                    if reference.is_empty() { String::new() } else { format!(" {reference}") },
                    t.s("author.name"),
                    why(&t.s("action_name")),
                    time::ago(&t.s("created_at"))
                ))
                .body(if target.s("title").is_empty() { String::new() } else { t.s("body") })
                .open(open(t));
            if pending {
                row = row.action("Done", Req::rest("POST", format!("/api/v4/todos/{id}/mark_as_done")).ok("Marked as done").inval("/api/v4/todos").act()).inline();
            }
            row
        })
        .empty(if pending { "You're all done!" } else { "Nothing done yet." });
        let list = self.list(&spec, cx);
        let kinds = [("", "Everything"), ("Issue", "Issues"), ("MergeRequest", "Merge requests"), ("Commit", "Commits"), ("Epic", "Epics")];
        let kind_menu = Act::menu(kinds.iter().map(|(v, l)| MenuEntry::check(*l, kind == *v, Act::choose("todos.type", *v))).collect());
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title("To-Do List"))
                    .child(widgets::spacer())
                    .child(widgets::btn("todo-settings", "Notification settings", Act::Url(format!("{}/-/profile/notifications", crate::forge::web()))))
                    .child(widgets::primary(
                        "todos-done",
                        "Mark all as done",
                        Req::rest("POST", "/api/v4/todos/mark_as_done").ok("All done").inval("/api/v4/todos").act(),
                    )),
            )
            .child(
                widgets::row()
                    .child(widgets::chips(vec![
                        ("To Do".into(), pending, Act::choose("todos.state", "pending")),
                        ("Done".into(), !pending, Act::choose("todos.state", "done")),
                    ]))
                    .child(widgets::spacer())
                    .child(widgets::btn("todo-type", format!("{} ▾", kinds.iter().find(|k| k.0 == kind).map(|k| k.1).unwrap_or("Everything")), kind_menu)),
            )
            .child(list)
            .into_any_element()
    }
}
