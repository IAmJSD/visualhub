//! Gists: yours and starred, one gist with its files (read-only),
//! comments, stars and forks, and making one.

use super::common::{delete_comment, edit_comment, post_comment, CommentActs};
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Req, Route};
use crate::json::{self, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use crate::ui::{icon, palette};
use serde_json::Value;

pub fn gist_row(g: &Value) -> Row {
    let files: Vec<String> = match g.at("files") {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let title = files.first().cloned().unwrap_or_else(|| g.s("id"));
    let mut row = Row::new(format!("{} / {title}", g.s("owner.login")))
        .icon("gist", widgets::gray())
        .meta(format!("{} files  ·  {} comments  ·  updated {}", files.len(), g.i("comments"), time::ago(&g.s("updated_at"))))
        .body(g.s("description"))
        .open(Act::Go(Route::Gist { id: g.s("id") }));
    if !g.b("public") {
        row = row.tag("Secret", widgets::yellow());
    }
    row
}

impl Hub {
    pub fn gists(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.choice("gists.tab", "mine");
        let path = match tab.as_str() {
            "starred" => "/gists/starred",
            "public" => "/gists/public",
            _ => "/gists",
        };
        let spec = ListSpec::new(path, gist_row).empty("No gists.");
        let list = self.list(&spec, cx);
        widgets::page()
            .child(widgets::row().child(widgets::title("Gists")).child(widgets::spacer()).child(widgets::go_btn("new-gist", "New gist", Act::Go(Route::NewGist))))
            .child(widgets::chips(vec![
                ("Your gists".into(), tab == "mine", Act::choose("gists.tab", "mine")),
                ("Starred".into(), tab == "starred", Act::choose("gists.tab", "starred")),
                ("Discover".into(), tab == "public", Act::choose("gists.tab", "public")),
            ]))
            .child(list)
            .into_any_element()
    }

    pub fn gist(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let path = format!("/gists/{id}");
        let g = ready!(self.fetch(&path, cx));
        let mine = g.s("owner.login") == self.login();
        let starred = self.fetch_check(&format!("/gists/{id}/star"), cx).ready().map(|v| v.b("")).unwrap_or(false);
        let avatar = self.avatar(&g.s("owner.avatar_url"), 24.0, cx);
        let mut files = widgets::col().gap_3();
        if let Value::Object(map) = g.at("files") {
            for (i, (name, f)) in map.iter().enumerate() {
                let content = f.s("content");
                let body = if name.to_lowercase().ends_with(".md") {
                    let md = self.markdown(&format!("gist-{i}"), &content, cx);
                    div().p_4().child(md).into_any_element()
                } else {
                    crate::markdown::code_block(gpui::ElementId::Name(format!("gist-code-{i}").into()), &f.s("language"), &content)
                };
                files = files.child(
                    widgets::card()
                        .child(
                            widgets::card_header()
                                .child(icon("file", 14.0, p.text_dim))
                                .child(div().flex_1().font_family(widgets::MONO).child(name.clone()))
                                .child(widgets::faint(json::bytes(f.i("size"))))
                                .child(widgets::btn(gpui::ElementId::Name(format!("gist-copy-{i}").into()), "Copy", Act::Copy(content.clone())))
                                .child(widgets::btn(gpui::ElementId::Name(format!("gist-raw-{i}").into()), "Raw", Act::Url(f.s("raw_url")))),
                        )
                        .child(body),
                );
            }
        }
        let comments_path = format!("/gists/{id}/comments");
        let field = format!("gist-comment:{id}");
        let mut comments = widgets::col().gap_3();
        if let Some(list) = self.fetch(&comments_path, cx).ready().cloned() {
            for (i, c) in list.list("").iter().enumerate() {
                let url = c.s("url");
                let acts = CommentActs {
                    edit: Some(edit_comment(&url, &c.s("body"), &comments_path)),
                    delete: Some(delete_comment(&url, &comments_path)),
                    extra: Vec::new(),
                    reactions: None,
                    invalidate: comments_path.clone(),
                    quote_into: Some(field.clone()),
                };
                comments = comments.child(self.comment_card(&format!("gc{i}"), c, "commented", acts, cx));
            }
        }
        let submit = post_comment(&field, &comments_path, &comments_path);
        let composer = self.composer(&field, submit, Vec::new(), cx);
        let edit = FormSpec::new("Edit gist description")
            .field(Field::text("description", "Description").value(g.s("description")).keep_empty())
            .rest("PATCH", path.clone())
            .ok("Gist updated")
            .inval(path.clone())
            .act();
        let delete = Req::rest("DELETE", path.clone())
            .ok("Gist deleted")
            .inval("/gists")
            .then(|hub, _, cx| hub.go(Route::Gists, cx))
            .act()
            .confirm("Delete this gist?", "It and its revisions are deleted for good.", "Delete");
        widgets::page()
            .child(
                widgets::row()
                    .child(avatar)
                    .child(widgets::title(format!("{} / {}", g.s("owner.login"), g.s("description").lines().next().unwrap_or(id))).flex_1())
                    .child(widgets::ibtn(
                        "gist-star",
                        if starred { "star-fill" } else { "star" },
                        if starred { "Unstar" } else { "Star" },
                        Req::rest(if starred { "DELETE" } else { "PUT" }, format!("/gists/{id}/star")).ok(if starred { "Unstarred" } else { "Starred" }).inval(format!("/gists/{id}")).inval("/gists/starred").act(),
                    ))
                    .when(!mine, |d| {
                        d.child(widgets::ibtn(
                            "gist-fork",
                            "fork",
                            "Fork",
                            Req::rest("POST", format!("/gists/{id}/forks")).ok("Gist forked").inval("/gists").then(|hub, v, cx| hub.go(Route::Gist { id: v.s("id") }, cx)).act(),
                        ))
                    })
                    .child(widgets::btn("gist-clone", "Copy clone URL", Act::Copy(g.s("git_pull_url"))))
                    .when(mine, |d| d.child(widgets::btn("gist-edit", "Edit description", edit)).child(widgets::danger("gist-delete", "Delete", delete))),
            )
            .child(widgets::dim(format!(
                "{}  ·  created {}  ·  updated {}  ·  {} forks",
                if g.b("public") { "Public" } else { "Secret" },
                time::ago(&g.s("created_at")),
                time::ago(&g.s("updated_at")),
                g.list("forks").len()
            )))
            .child(files)
            .child(widgets::h2("Comments"))
            .child(comments)
            .child(composer)
            .child(div().h(px(8.0)))
            .into_any_element()
    }
}
