//! The pages only GitLab has, or has differently enough to want its own:
//! CI/CD pipelines and jobs, the to-do list, groups, a project's settings
//! and packages. They ask GitLab's API directly (`/api/v4/…`) and are
//! drawn with the same pieces as everything else.

pub mod ci;
pub mod groups;
pub mod packages;
pub mod settings;
pub mod todos;

use crate::hub::{Act, Hub, Route};
use crate::json::enc;
use crate::widgets;
use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _};

/// A project's root in GitLab's API, as the client sends it untranslated.
pub fn project_api(repo: &str) -> String {
    format!("/api/v4/projects/{}", enc(repo))
}

pub fn group_api(group: &str) -> String {
    format!("/api/v4/groups/{}", enc(group))
}

/// GitLab's access levels, for role pickers.
pub const ROLES: [(&str, &str); 5] = [
    ("10", "Guest"),
    ("20", "Reporter"),
    ("30", "Developer"),
    ("40", "Maintainer"),
    ("50", "Owner"),
];

/// An access level's name.
pub fn role(level: i64) -> &'static str {
    crate::gitlab::shape::role_name(level)
}

/// A CI status's icon and colour.
pub fn ci_icon(status: &str) -> (&'static str, u32) {
    let (status, conclusion) = crate::gitlab::shape::ci_status(status);
    crate::screens::pulls::status_icon(status, conclusion)
}

/// A CI status as GitLab words it.
pub fn ci_word(status: &str) -> String {
    match status {
        "success" => "Passed".into(),
        "failed" => "Failed".into(),
        "canceled" => "Canceled".into(),
        "skipped" => "Skipped".into(),
        "manual" => "Manual".into(),
        "running" => "Running".into(),
        "pending" => "Pending".into(),
        "created" => "Created".into(),
        "scheduled" => "Scheduled".into(),
        "waiting_for_resource" => "Waiting for resource".into(),
        "preparing" => "Preparing".into(),
        other => other.replace('_', " "),
    }
}

impl Hub {
    /// A page GitLab has no part of the app for: what it is, and a way to
    /// it on the site where there is one.
    pub fn gl_elsewhere(&mut self, route: &Route, what: &str) -> AnyElement {
        widgets::page()
            .child(widgets::title(route.title()))
            .child(
                widgets::card()
                    .p_6()
                    .gap_3()
                    .child(widgets::dim(format!(
                        "{what} isn't something GitLab has, or it's only on the site."
                    )))
                    .child(gpui::div().child(widgets::btn(
                        "elsewhere-open",
                        crate::forge::open_on(),
                        Act::Url(route.web_url()),
                    ))),
            )
            .into_any_element()
    }
}
