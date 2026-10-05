//! Releases: the list, one release with its notes and assets, and the
//! form that drafts, publishes or edits one.

use super::common::reactions_bar;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, RepoTab, Req, Route};
use crate::json::{self, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::{json, Value};

/// Draft a release (`existing` = None) or edit one.
pub fn release_form(repo: &str, existing: Option<&Value>, tag: &str) -> Act {
    let editing = existing.is_some();
    let v = existing.cloned().unwrap_or(Value::Null);
    let target_repo = repo.to_string();
    let mut form = FormSpec::new(if editing {
        "Edit release"
    } else {
        "Draft a new release"
    })
    .submit(if editing {
        "Update release"
    } else {
        "Publish release"
    })
    .width(680.0)
    .field(
        Field::text("tag_name", "Tag")
            .value(if editing {
                v.s("tag_name")
            } else {
                tag.to_string()
            })
            .required()
            .hint("An existing tag, or a new one to create on publish."),
    )
    .field(
        Field::text("target_commitish", "Target")
            .value(v.s("target_commitish"))
            .hint("Branch or commit for a new tag. Empty: the default branch."),
    )
    .field(Field::text("name", "Release title").value(v.s("name")))
    .field(
        Field::multiline("body", "Release notes")
            .value(v.s("body"))
            .keep_empty(),
    );
    // GitLab releases have no drafts, pre-releases or "latest" to choose.
    if !crate::forge::is_gitlab() {
        form = form
            .field(Field::bool("draft", "Save as draft", v.b("draft")))
            .field(Field::bool(
                "prerelease",
                "Set as a pre-release",
                v.b("prerelease"),
            ))
            .field(Field::choice(
                "make_latest",
                "Mark as latest",
                &[("true", "Yes"), ("false", "No"), ("legacy", "By date")],
            ));
    }
    if !editing && !crate::forge::is_gitlab() {
        form = form.field(Field::bool(
            "generate_release_notes",
            "Generate release notes from pull requests",
            true,
        ));
    }
    let (method, path) = if editing {
        ("PATCH", format!("/repos/{repo}/releases/{}", v.i("id")))
    } else {
        ("POST", format!("/repos/{repo}/releases"))
    };
    form.rest(method, path)
        .ok(if editing {
            "Release updated"
        } else {
            "Release published"
        })
        .inval(format!("/repos/{repo}/releases"))
        .then(move |hub, value, cx| {
            hub.go(
                Route::Release {
                    repo: target_repo.clone(),
                    id: value.i("id") as u64,
                },
                cx,
            )
        })
        .act()
}

impl Hub {
    pub fn repo_releases(
        &mut self,
        repo: &str,
        _default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let repo_s = repo.to_string();
        let latest = self
            .fetch(&format!("/repos/{repo}/releases/latest"), cx)
            .ready()
            .map(|v| v.i("id"));
        let spec = ListSpec::new(format!("/repos/{repo}/releases"), move |r| {
            let id = r.i("id");
            let mut row = Row::new(if r.s("name").is_empty() {
                r.s("tag_name")
            } else {
                r.s("name")
            })
            .icon("tag", widgets::green())
            .meta(format!(
                "{}  ·  {} {}  ·  {} assets",
                r.s("tag_name"),
                if r.b("draft") { "drafted" } else { "published" },
                time::ago(&if r.b("draft") {
                    r.s("created_at")
                } else {
                    r.s("published_at")
                }),
                r.list("assets").len()
            ))
            .body(summary_line(&r.s("body")))
            .avatar(r.s("author.avatar_url"))
            .open(Act::Go(Route::Release {
                repo: repo_s.clone(),
                id: id as u64,
            }))
            .action("Edit", release_form(&repo_s, Some(r), ""))
            .danger(
                "Delete",
                Req::rest("DELETE", format!("/repos/{repo_s}/releases/{id}"))
                    .ok("Release deleted")
                    .inval(format!("/repos/{repo_s}/releases"))
                    .act()
                    .confirm(
                        "Delete this release?",
                        "The tag is kept; the release notes and uploaded assets are removed.",
                        "Delete",
                    ),
            );
            if Some(id) == latest {
                row = row.tag("Latest", widgets::green());
            }
            if r.b("prerelease") {
                row = row.tag("Pre-release", widgets::yellow());
            }
            if r.b("draft") {
                row = row.tag("Draft", widgets::gray());
            }
            row
        })
        .empty("There aren’t any releases here.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::h2("Releases"))
                    .child(widgets::spacer())
                    .child(widgets::go_btn(
                        "new-release",
                        "Draft a new release",
                        release_form(repo, None, ""),
                    )),
            )
            .child(list)
            .into_any_element()
    }

    pub fn release(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/releases/{id}");
        let r = ready!(self.fetch(&path, cx));
        let notes = self.markdown(&format!("release-{id}"), &r.s("body"), cx);
        let avatar = self.avatar(&r.s("author.avatar_url"), 20.0, cx);
        let me = self.login();
        let repo_s = repo.to_string();
        let assets = ListSpec::new(format!("{path}/assets"), move |a| {
            Row::new(a.s("name"))
                .icon("package", widgets::gray())
                .meta(format!(
                    "{}  ·  {} downloads  ·  {}",
                    json::bytes(a.i("size")),
                    a.i("download_count"),
                    time::ago(&a.s("updated_at"))
                ))
                .open(Act::Url(a.s("browser_download_url")))
                .action("Download", Act::Url(a.s("browser_download_url")))
                .action("Copy link", Act::Copy(a.s("browser_download_url")))
                .danger(
                    "Delete",
                    Req::rest(
                        "DELETE",
                        format!("/repos/{repo_s}/releases/assets/{}", a.i("id")),
                    )
                    .ok("Asset deleted")
                    .inval(format!("/repos/{repo_s}/releases/{id}"))
                    .act()
                    .confirm(
                        "Delete this asset?",
                        "The file will no longer be downloadable.",
                        "Delete",
                    ),
                )
        })
        .unpaged()
        .empty("No uploaded assets.");
        let assets = self.list(&assets, cx);
        let repo_back = repo.to_string();
        let tag = r.s("tag_name");
        let gitlab = crate::forge::is_gitlab();
        let archive = |format: &str| {
            let web = crate::forge::web();
            if gitlab {
                let name = crate::forge::split_repo(repo).1;
                format!("{web}/{repo}/-/archive/{tag}/{name}-{tag}.{format}")
            } else {
                format!("{web}/{repo}/archive/refs/tags/{tag}.{format}")
            }
        };
        widgets::page()
            .child(
                widgets::row()
                    .child(widgets::title(if r.s("name").is_empty() {
                        tag.clone()
                    } else {
                        r.s("name")
                    }))
                    .when(r.b("prerelease"), |d| {
                        d.child(widgets::tag("Pre-release", widgets::yellow()))
                    })
                    .when(r.b("draft"), |d| {
                        d.child(widgets::tag("Draft", widgets::gray()))
                    })
                    .child(widgets::spacer())
                    .child(widgets::btn(
                        "edit-release",
                        "Edit",
                        release_form(repo, Some(&r), ""),
                    ))
                    .when(r.b("draft"), |d| {
                        d.child(widgets::go_btn(
                            "publish",
                            "Publish",
                            Req::rest("PATCH", path.clone())
                                .body(json!({ "draft": false }))
                                .ok("Release published")
                                .inval(format!("/repos/{repo}/releases"))
                                .act(),
                        ))
                    })
                    .child(widgets::danger(
                        "delete-release",
                        "Delete",
                        Req::rest("DELETE", path.clone())
                            .ok("Release deleted")
                            .inval(format!("/repos/{repo}/releases"))
                            .then(move |hub, _, cx| {
                                hub.go(
                                    Route::Repo {
                                        repo: repo_back.clone(),
                                        tab: RepoTab::Releases,
                                    },
                                    cx,
                                )
                            })
                            .act()
                            .confirm(
                                "Delete this release?",
                                "The tag is kept; notes and assets are removed.",
                                "Delete",
                            ),
                    )),
            )
            .child(
                widgets::row()
                    .child(avatar)
                    .child(widgets::h3(r.s("author.login")))
                    .child(widgets::dim(format!(
                        "released this {}",
                        time::ago(&r.s("published_at"))
                    )))
                    .child(widgets::tag(tag.clone(), widgets::gray()))
                    .child(widgets::dim(format!("target {}", r.s("target_commitish")))),
            )
            .child(widgets::card().p_6().child(notes))
            .when(!gitlab, |d| {
                d.child(reactions_bar(
                    "release",
                    r.at("reactions"),
                    &format!("{path}/reactions"),
                    &me,
                    &path,
                ))
            })
            .child(widgets::h2("Assets"))
            .child(assets)
            .child(
                widgets::row()
                    .child(widgets::btn(
                        "zip",
                        "Source code (zip)",
                        Act::Url(archive("zip")),
                    ))
                    .child(widgets::btn(
                        "tar",
                        "Source code (tar.gz)",
                        Act::Url(archive("tar.gz")),
                    ))
                    .child(widgets::btn(
                        "release-tree",
                        "Browse files at this tag",
                        Act::Go(Route::Tree {
                            repo: repo.to_string(),
                            git_ref: tag,
                            path: String::new(),
                            file: false,
                        }),
                    )),
            )
            .child(div().h(px(8.0)))
            .into_any_element()
    }
}

/// The first line of release notes worth showing in a list, with its
/// Markdown dressing (headings, bullets, emphasis, code) taken off.
fn summary_line(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("<!--"))
        .unwrap_or("");
    let line = line.trim_start_matches(['#', '>', '-', '*', '+', ' ']);
    line.replace("**", "").replace("__", "").replace('`', "")
}
