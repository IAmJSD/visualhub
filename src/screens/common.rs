//! Pieces several pages share: how an issue, a repository or a user reads
//! in a list, a comment with its reactions, and the comment composer.

use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, MenuEntry, PullTab, RepoTab, Req, Route};
use crate::json::{self, Json as _};
use crate::resource::Row;
use crate::time;
use crate::ui::{icon, palette, Button, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};

/// "owner/name" (on GitLab, "group/sub/project") from any API URL or
/// page for a repository or something in one.
pub fn repo_of(url: &str) -> String {
    crate::forge::repo_of(url)
}

/// The icon and colour for an issue or pull request in its state.
pub fn issue_icon(item: &Value) -> (&'static str, u32) {
    let is_pr = item.has("pull_request") || item.has("merged_at") || item.has("head");
    let state = item.s("state");
    if is_pr {
        let merged =
            item.has("pull_request.merged_at") || item.has("merged_at") || item.b("merged");
        if merged {
            ("pr-merged", widgets::purple())
        } else if state == "closed" {
            ("pr-closed", widgets::red())
        } else if item.b("draft") {
            ("pr-draft", widgets::gray())
        } else {
            ("pr", widgets::green())
        }
    } else if state == "closed" {
        if item.s("state_reason") == "not_planned" {
            ("issue-skip", widgets::gray())
        } else {
            ("issue-closed", widgets::purple())
        }
    } else {
        ("issue", widgets::green())
    }
}

/// The page an issue or pull request opens.
pub fn issue_route(item: &Value) -> Route {
    let repo = if item.has("repository_url") {
        repo_of(&item.s("repository_url"))
    } else {
        repo_of(&item.s("url"))
    };
    let number = item.i("number") as u64;
    if item.has("pull_request") || item.has("head") {
        Route::Pull {
            repo,
            number,
            tab: PullTab::Conversation,
        }
    } else {
        Route::Issue { repo, number }
    }
}

/// An issue or pull request as a list row.
pub fn issue_row(item: &Value, show_repo: bool) -> Row {
    let (icon_name, color) = issue_icon(item);
    let repo = repo_of(&item.s("repository_url"));
    let is_pr = item.has("pull_request") || item.has("head");
    let mut meta = format!(
        "{} opened {} by {}",
        if is_pr {
            crate::forge::pr_ref(item.i("number"))
        } else {
            format!("#{}", item.i("number"))
        },
        time::ago(&item.s("created_at")),
        item.s("user.login")
    );
    if show_repo && !repo.is_empty() {
        meta = format!("{repo}  ·  {meta}");
    }
    if item.has("milestone") {
        meta.push_str(&format!("  ·  {}", item.s("milestone.title")));
    }
    let comments = item.i("comments");
    let mut row = Row::new(item.s("title"))
        .icon(icon_name, color)
        .labels(item.list("labels"))
        .meta(meta)
        .open(Act::Go(issue_route(item)));
    if item.b("draft") {
        row = row.tag("Draft", widgets::gray());
    }
    if comments > 0 {
        row = row.right(format!("💬 {comments}"));
    }
    row
}

/// A repository as a list row.
pub fn repo_row(repo: &Value) -> Row {
    let full = repo.s("full_name");
    let mut meta = Vec::new();
    if repo.has("language") {
        meta.push(repo.s("language"));
    }
    meta.push(format!("★ {}", json::count(repo.i("stargazers_count"))));
    meta.push(format!("⑂ {}", json::count(repo.i("forks_count"))));
    if repo.has("license.spdx_id") && repo.s("license.spdx_id") != "NOASSERTION" {
        meta.push(repo.s("license.spdx_id"));
    }
    meta.push(format!("Updated {}", time::ago(&repo.s("pushed_at"))));
    let mut row = Row::new(full.clone())
        .icon(
            if repo.b("fork") { "fork" } else { "repo" },
            palette().text_dim,
        )
        .meta(meta.join("  ·  "))
        .body(repo.s("description"))
        .open(Act::Go(Route::Repo {
            repo: full,
            tab: RepoTab::Code,
        }));
    if repo.b("private") {
        row = row.tag("Private", widgets::yellow());
    }
    if repo.b("archived") {
        row = row.tag("Archived", widgets::yellow());
    }
    if repo.b("is_template") {
        row = row.tag("Template", widgets::gray());
    }
    row
}

/// A user or organization as a list row.
pub fn user_row(user: &Value) -> Row {
    let login = user.s("login");
    let route = if user.s("type") == "Organization" {
        Route::Org {
            login: login.clone(),
        }
    } else {
        Route::User {
            login: login.clone(),
        }
    };
    Row::new(login)
        .avatar(user.s("avatar_url"))
        .meta(user.s("name"))
        .body(user.s("bio"))
        .open(Act::Go(route))
}

pub const REACTIONS: [(&str, &str); 8] = [
    ("+1", "👍"),
    ("-1", "👎"),
    ("laugh", "😄"),
    ("hooray", "🎉"),
    ("confused", "😕"),
    ("heart", "❤️"),
    ("rocket", "🚀"),
    ("eyes", "👀"),
];

/// Add `content` to the reactions at `base`, or take it back if this user
/// already reacted with it.
pub fn toggle_reaction(base: &str, content: &str, login: &str, invalidate: &str) -> Act {
    let (base, content, login) = (base.to_string(), content.to_string(), login.to_string());
    Req::custom(move |client| {
        let existing = client.get(&format!("{base}?per_page=100&content={content}"))?;
        let mine = existing
            .list("")
            .iter()
            .find(|r| r.s("user.login") == login && r.s("content") == content)
            .map(|r| r.i("id"));
        match mine {
            Some(id) => client.json("DELETE", &format!("{base}/{id}"), None),
            None => client.json("POST", &base, Some(&json!({ "content": content }))),
        }
    })
    .inval(invalidate.to_string())
    .act()
}

/// The reaction counts under a comment, each a toggle, and a picker.
pub fn reactions_bar(
    id: &str,
    reactions: &Value,
    base: &str,
    login: &str,
    invalidate: &str,
) -> AnyElement {
    let p = palette();
    let mut bar = div().flex().flex_row().flex_wrap().gap_1().items_center();
    for (content, emoji) in REACTIONS {
        let count = reactions.i(content);
        if count > 0 {
            bar = bar.child(
                Button::new(
                    ElementId::Name(format!("{id}-r-{content}").into()),
                    format!("{emoji} {count}"),
                )
                .h(px(22.0))
                .px_2()
                .rounded_full()
                .text_size(px(12.0))
                .colors(crate::ui::ButtonColors {
                    bg: Some(p.control_bg),
                    hover: p.hover,
                    text: p.text,
                    border: Some(p.edge),
                })
                .on_click(on(toggle_reaction(base, content, login, invalidate))),
            );
        }
    }
    let picker: Vec<MenuEntry> = REACTIONS
        .iter()
        .map(|(content, emoji)| {
            MenuEntry::item(
                format!("{emoji}  {content}"),
                toggle_reaction(base, content, login, invalidate),
            )
        })
        .collect();
    bar.child(
        IconButton::new(ElementId::Name(format!("{id}-react").into()), "smile")
            .size(22.0)
            .icon_size(14.0)
            .color(p.text_dim)
            .tooltip("Add reaction", None)
            .on_click(on(Act::menu(picker))),
    )
    .into_any_element()
}

/// What a comment card can do beyond showing itself.
pub struct CommentActs {
    pub edit: Option<Act>,
    pub delete: Option<Act>,
    pub extra: Vec<MenuEntry>,
    /// The reactions endpoint, if it takes reactions.
    pub reactions: Option<String>,
    /// What a reaction or edit makes stale.
    pub invalidate: String,
    /// Where "Quote reply" puts the quote.
    pub quote_into: Option<String>,
}

impl Hub {
    /// A comment: who, when, the Markdown body, reactions, and a menu.
    pub fn comment_card(
        &mut self,
        id: &str,
        comment: &Value,
        verb: &str,
        acts: CommentActs,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let login = comment.s("user.login");
        let avatar = self.avatar(&comment.s("user.avatar_url"), 32.0, cx);
        let body_text = comment.s("body");
        let body = self.markdown(id, &body_text, cx);
        let association = comment.s("author_association");
        let me = self.login();

        let mut menu = Vec::new();
        menu.push(MenuEntry::item(
            "Copy link",
            Act::Copy(comment.s("html_url")),
        ));
        if let Some(target) = acts.quote_into.clone() {
            let quoted: String = body_text
                .lines()
                .map(|l| format!("> {l}\n"))
                .collect::<String>()
                + "\n";
            menu.push(MenuEntry::item(
                "Quote reply",
                Act::run(move |hub, _, cx| {
                    let current = hub.field_text(&target);
                    hub.set_field(&target, format!("{current}{quoted}"));
                    hub.focus_field(&target);
                    cx.notify();
                }),
            ));
        }
        menu.push(MenuEntry::item(
            crate::forge::open_on(),
            Act::Url(comment.s("html_url")),
        ));
        menu.extend(acts.extra);
        if let Some(edit) = acts.edit {
            menu.push(MenuEntry::Sep);
            menu.push(MenuEntry::item("Edit", edit));
        }
        if let Some(delete) = acts.delete {
            menu.push(MenuEntry::item("Delete", delete));
        }

        let header = widgets::card_header()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(login.clone()))
            .child(widgets::dim(format!(
                "{verb} {}",
                time::ago(&comment.s("created_at"))
            )))
            .when(
                comment.s("updated_at") != comment.s("created_at") && comment.has("updated_at"),
                |d| d.child(widgets::faint("· edited")),
            )
            .child(widgets::spacer())
            .when(!association.is_empty() && association != "NONE", |d| {
                d.child(widgets::tag(association.to_lowercase(), p.text_dim))
            })
            .child(
                IconButton::new(ElementId::Name(format!("{id}-menu").into()), "kebab")
                    .size(24.0)
                    .icon_size(16.0)
                    .on_click(on(Act::menu(menu))),
            );

        let mut card = widgets::card()
            .flex_1()
            .min_w_0()
            .child(header)
            .child(div().p_4().child(body));
        if let Some(base) = &acts.reactions {
            card = card.child(div().px_4().pb_3().child(reactions_bar(
                id,
                comment.at("reactions"),
                base,
                &me,
                &acts.invalidate,
            )));
        }
        div()
            .flex()
            .flex_row()
            .gap_3()
            .child(div().pt_1().child(avatar))
            .child(card)
            .into_any_element()
    }

    /// A small line in a timeline: an icon, and what happened.
    pub fn timeline_event(
        &mut self,
        icon_name: &str,
        color: u32,
        actor: &Value,
        text: impl Into<String>,
        when: &str,
        extra: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let avatar = self.avatar(&actor.s("avatar_url"), 18.0, cx);
        let who = widgets::row()
            .gap_2()
            .child(avatar)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(actor.s("login")),
            )
            .into_any_element();
        self.timeline_event_by(icon_name, color, who, text, when, extra)
    }

    /// [`Hub::timeline_event`] with whoever did it already drawn: several
    /// commit authors, say.
    pub fn timeline_event_by(
        &mut self,
        icon_name: &str,
        color: u32,
        who: AnyElement,
        text: impl Into<String>,
        when: &str,
        extra: Option<AnyElement>,
    ) -> AnyElement {
        let p = palette();
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_2()
            .pl(px(44.0))
            .py_1()
            .child(
                div()
                    .size(px(26.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(p.control_bg))
                    .child(icon(icon_name, 14.0, color)),
            )
            .child(who)
            .child(div().text_color(rgb(p.text_dim)).child(text.into()))
            .children(extra)
            .child(widgets::faint(time::ago(when)))
            .into_any_element()
    }

    /// The comment box under a conversation: Write and Preview tabs, and
    /// its buttons beneath, as on github.com.
    pub fn composer(
        &mut self,
        field: &str,
        submit: Act,
        extra: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let avatar_url = self.me.s("avatar_url");
        let avatar = self.avatar(&avatar_url, 40.0, cx);
        self.submits.insert(field.to_string(), submit.clone());
        let tab_key = format!("{field}:tab");
        let preview = self.choice(&tab_key, "write") == "preview";
        let body: AnyElement = if preview {
            let text = self.field_text(field);
            let md = if text.trim().is_empty() {
                widgets::dim("Nothing to preview").into_any_element()
            } else {
                self.markdown(&format!("{field}-preview"), &text, cx)
            };
            div().min_h(px(130.0)).p_2().child(md).into_any_element()
        } else {
            self.textarea(field, "Add your comment here…", cx)
                .min_h(px(130.0))
                .into_any_element()
        };
        let tab = |id: &str, label: &str, on_now: bool, value: &'static str| {
            div()
                .id(ElementId::Name(format!("{field}-{id}").into()))
                .px_4()
                .py_2()
                .cursor_pointer()
                .when(on_now, |d| {
                    d.bg(rgb(p.panel_bg))
                        .border_r_1()
                        .border_l_1()
                        .border_color(rgb(p.edge))
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!on_now, |d| {
                    d.text_color(rgb(p.text_dim))
                        .hover(|s| s.text_color(rgb(p.text)))
                })
                .on_click(on(Act::choose(tab_key.clone(), value)))
                .child(label.to_string())
        };
        let tabs = div()
            .flex()
            .flex_row()
            .bg(rgb(p.deep_bg))
            .border_b_1()
            .border_color(rgb(p.edge))
            .child(tab("write", "Write", !preview, "write"))
            .child(tab("preview", "Preview", preview, "preview"));
        div()
            .flex()
            .flex_row()
            .gap_3()
            .child(div().pt(px(30.0)).child(avatar))
            .child(
                widgets::col()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(widgets::h3("Add a comment"))
                    .child(
                        widgets::card()
                            .child(tabs)
                            .child(div().p_2().child(body))
                            .child(
                                widgets::row()
                                    .px_3()
                                    .pb_2()
                                    .gap_1()
                                    .child(icon("code", 13.0, p.text_dim))
                                    .child(widgets::faint(
                                        "Markdown is supported  ·  Ctrl+Enter to send",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .children(extra)
                            .child(
                                widgets::go_btn(
                                    ElementId::Name(format!("{field}-send").into()),
                                    "Comment",
                                    submit,
                                )
                                .h(px(32.0))
                                .px_4()
                                .text_size(px(13.0)),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Post the text in `field` as a comment to `path`, clearing the box
/// when it lands.
pub fn post_comment(field: &str, path: &str, invalidate: &str) -> Act {
    let (field, path, invalidate) = (field.to_string(), path.to_string(), invalidate.to_string());
    Act::run(move |hub, window, cx| {
        let body = hub.field_text(&field);
        if body.trim().is_empty() {
            hub.toast("Write something first.", true, cx);
            return;
        }
        let clear = field.clone();
        let req = Req::rest("POST", path.clone())
            .body(json!({ "body": body }))
            .ok("Comment posted")
            .inval(invalidate.clone())
            .then(move |hub, _, cx| {
                hub.set_field(&clear, "");
                cx.notify();
            });
        hub.perform(req.act(), window, cx);
    })
}

/// The edit dialog for a comment at `url` (an API URL).
pub fn edit_comment(url: &str, body: &str, invalidate: &str) -> Act {
    FormSpec::new("Edit comment")
        .field(Field::multiline("body", "Comment").value(body).required())
        .rest("PATCH", api_path(url))
        .ok("Comment updated")
        .inval(invalidate.to_string())
        .act()
}

pub fn delete_comment(url: &str, invalidate: &str) -> Act {
    Req::rest("DELETE", api_path(url))
        .ok("Comment deleted")
        .inval(invalidate.to_string())
        .act()
        .confirm(
            "Delete this comment?",
            "The comment will be removed for everyone. This cannot be undone.",
            "Delete",
        )
}

/// An API URL as a path the client accepts.
pub fn api_path(url: &str) -> String {
    url.trim_start_matches(crate::api::API).to_string()
}

/// A sidebar section whose heading (title and gear) opens a picker.
pub fn side_section(id: &str, title: &str, picker: Option<Act>, content: AnyElement) -> AnyElement {
    let p = palette();
    let clickable = picker.is_some();
    let heading = div()
        .id(ElementId::Name(format!("{id}-heading").into()))
        .flex()
        .flex_row()
        .items_center()
        .h(px(24.0))
        .text_color(rgb(p.text_dim))
        .child(
            div()
                .flex_1()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .when(clickable, |d| d.child(icon("settings", 14.0, p.text_dim)))
        .when_some(picker, |d, act| {
            d.cursor_pointer()
                .hover(|s| s.text_color(rgb(p.accent)))
                .on_click(on(act))
        });
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pb_3()
        .border_b_1()
        .border_color(rgb(p.divider))
        .child(heading)
        .child(content)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_from_urls() {
        assert_eq!(repo_of("https://api.github.com/repos/a/b/issues/1"), "a/b");
        assert_eq!(repo_of("https://github.com/a/b"), "a/b");
    }
}
