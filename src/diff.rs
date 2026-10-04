//! Unified diffs, read-only: a commit's or a pull request's changed files
//! with line numbers, and -- on a pull request -- review comments threaded
//! under the lines they are about, and a click on any line to start one.

use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub};
use crate::json::Json as _;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use crate::ui::{is_light, palette, IconButton};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Hunk,
    Add,
    Del,
    Context,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// The lines of a unified diff, numbered from its hunk headers.
pub fn parse(patch: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    for raw in patch.lines() {
        if let Some(rest) = raw.strip_prefix("@@") {
            // @@ -a,b +c,d @@ context
            let mut parts = rest.split_whitespace();
            let o = parts.next().unwrap_or("-0");
            let n = parts.next().unwrap_or("+0");
            old = o
                .trim_start_matches('-')
                .split(',')
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            new = n
                .trim_start_matches('+')
                .split(',')
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            out.push(Line {
                kind: Kind::Hunk,
                old: None,
                new: None,
                text: raw.to_string(),
            });
        } else if let Some(text) = raw.strip_prefix('+') {
            out.push(Line {
                kind: Kind::Add,
                old: None,
                new: Some(new),
                text: text.to_string(),
            });
            new += 1;
        } else if let Some(text) = raw.strip_prefix('-') {
            out.push(Line {
                kind: Kind::Del,
                old: Some(old),
                new: None,
                text: text.to_string(),
            });
            old += 1;
        } else if raw.starts_with('\\') {
            // "\ No newline at end of file"
            continue;
        } else {
            let text = raw.strip_prefix(' ').unwrap_or(raw);
            out.push(Line {
                kind: Kind::Context,
                old: Some(old),
                new: Some(new),
                text: text.to_string(),
            });
            old += 1;
            new += 1;
        }
    }
    out
}

/// Where a click on a line sends its review comment.
#[derive(Clone)]
pub struct ReviewTarget {
    pub repo: String,
    pub number: u64,
    pub commit: String,
}

fn colors(kind: Kind) -> (Option<u32>, Option<u32>) {
    // (row fill, gutter fill)
    let light = is_light();
    match kind {
        Kind::Add if light => (Some(0xDAFBE1), Some(0xACEEBB)),
        Kind::Add => (Some(0x12261E), Some(0x1B4721)),
        Kind::Del if light => (Some(0xFFEBE9), Some(0xFFCECB)),
        Kind::Del => (Some(0x25171C), Some(0x542426)),
        Kind::Hunk if light => (Some(0xDDF4FF), Some(0xDDF4FF)),
        Kind::Hunk => (Some(0x121D2F), Some(0x121D2F)),
        Kind::Context => (None, None),
    }
}

impl Hub {
    /// One changed file: a header with its name and counts, and its diff.
    /// `comments` are the pull request's review comments on this file.
    pub fn diff_file(
        &mut self,
        id: &str,
        file: &Value,
        review: Option<&ReviewTarget>,
        comments: &[Value],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let name = file.s("filename");
        let collapsed_key = format!("{id}:collapsed");
        let collapsed = self.is_open(&collapsed_key);
        let status = file.s("status");
        let status_color = match status.as_str() {
            "added" => widgets::green(),
            "removed" => widgets::red(),
            "renamed" => widgets::purple(),
            _ => widgets::yellow(),
        };
        let toggle_key = collapsed_key.clone();
        let header = widgets::card_header()
            .child(
                IconButton::new(
                    ElementId::Name(format!("{id}-fold").into()),
                    if collapsed { "chevron-right" } else { "chevron-down" },
                )
                .size(22.0)
                .on_click(on(Act::run(move |hub, _, cx| {
                    if !hub.open.remove(&toggle_key) {
                        hub.open.insert(toggle_key.clone());
                    }
                    cx.notify();
                }))),
            )
            .child(widgets::tag(status.clone(), status_color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(widgets::MONO)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if file.has("previous_filename") && status == "renamed" {
                        format!("{} → {name}", file.s("previous_filename"))
                    } else {
                        name.clone()
                    }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(widgets::green()))
                    .child(format!("+{}", file.i("additions"))),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(widgets::red()))
                    .child(format!("−{}", file.i("deletions"))),
            );
        let mut card = widgets::card().child(header);
        if collapsed {
            return card.into_any_element();
        }
        let patch = file.s("patch");
        if patch.is_empty() {
            return card
                .child(
                    div()
                        .p_3()
                        .text_color(rgb(p.text_dim))
                        .child("Binary file, or a diff too large to show here."),
                )
                .into_any_element();
        }
        let lines = parse(&patch);
        // Coloured as the file's language, restarting at each hunk.
        let texts: Vec<String> = lines.iter().map(|l| if l.kind == Kind::Hunk { String::new() } else { l.text.replace('\t', "    ") }).collect();
        let breaks: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.kind == Kind::Hunk).map(|(i, _)| i).collect();
        let colours = crate::highlight::lines(crate::highlight::syntax_for(&name), &texts, &breaks);
        let mut body = div()
            .id(ElementId::Name(format!("{id}-body").into()))
            .flex()
            .flex_col()
            .font_family(widgets::MONO)
            .text_size(px(12.0))
            .line_height(px(20.0));
        for (i, line) in lines.iter().enumerate() {
            let (fill, gutter) = colors(line.kind);
            let number = |n: Option<u32>| {
                div()
                    .w(px(48.0))
                    .flex_none()
                    .pr_2()
                    .text_right()
                    .text_color(rgb(p.text_faint))
                    .when_some(gutter, |d, g| d.bg(rgb(g)))
                    .child(n.map(|n| n.to_string()).unwrap_or_default())
            };
            let sign = match line.kind {
                Kind::Add => "+",
                Kind::Del => "-",
                _ => " ",
            };
            let mut row = div()
                .id(ElementId::Name(format!("{id}-l{i}").into()))
                .flex()
                .flex_row()
                .w_full()
                .when_some(fill, |d, f| d.bg(rgb(f)));
            if line.kind == Kind::Hunk {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(px(104.0))
                        .pr_4()
                        .text_color(rgb(p.text_dim))
                        .child(line.text.clone()),
                );
            } else {
                row = row
                    .child(number(line.old))
                    .child(number(line.new))
                    .child(div().w(px(16.0)).flex_none().pl_1().child(sign))
                    // Long lines wrap rather than scroll; the gutters
                    // stretch down beside them.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr_4()
                            .child(crate::highlight::styled(&texts[i], colours.as_ref().map(|c| c[i].as_slice()))),
                    );
                if let Some(target) = review {
                    let (side, at) = match line.kind {
                        Kind::Del => ("LEFT", line.old),
                        _ => ("RIGHT", line.new),
                    };
                    if let Some(at) = at {
                        let form = line_comment_form(target, &name, side, at);
                        row = row
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(p.hover)))
                            .on_click(on(form));
                    }
                }
            }
            body = body.child(row);
            // Review threads anchored here.
            let here: Vec<&Value> = comments
                .iter()
                .filter(|c| match line.kind {
                    Kind::Del => c.s("side") == "LEFT" && c.i("line") as u32 == line.old.unwrap_or(0),
                    Kind::Hunk => false,
                    _ => c.s("side") != "LEFT" && c.i("line") as u32 == line.new.unwrap_or(0),
                })
                .collect();
            for (j, comment) in here.into_iter().enumerate() {
                let el = self.review_comment(&format!("{id}-c{i}-{j}"), comment, cx);
                body = body.child(
                    div()
                        .font_family(".SystemUIFont")
                        .p_2()
                        .bg(rgb(p.deep_bg))
                        .border_y_1()
                        .border_color(rgb(p.divider))
                        .child(el),
                );
            }
        }
        card = card.child(body);
        card.into_any_element()
    }

    /// A review comment under its line: author, time, body, reply.
    fn review_comment(&mut self, id: &str, comment: &Value, cx: &mut Context<Self>) -> AnyElement {
        let avatar = self.avatar(&comment.s("user.avatar_url"), 20.0, cx);
        let body = self.markdown(id, &comment.s("body"), cx);
        let url = comment.s("url");
        let pulls_url = comment.s("pull_request_url");
        let reply = FormSpec::new("Reply")
            .submit("Reply")
            .field(Field::multiline("body", "Comment").required())
            .rest(
                "POST",
                format!(
                    "{}/comments/{}/replies",
                    pulls_url.trim_start_matches(crate::api::API),
                    comment.i("id")
                ),
            )
            .ok("Reply posted")
            .inval(pulls_url.trim_start_matches(crate::api::API).to_string())
            .act();
        let edit = FormSpec::new("Edit comment")
            .field(Field::multiline("body", "Comment").value(comment.s("body")).required())
            .rest("PATCH", url.trim_start_matches(crate::api::API).to_string())
            .ok("Comment updated")
            .inval(pulls_url.trim_start_matches(crate::api::API).to_string())
            .act();
        let delete = crate::hub::Req::rest("DELETE", url.trim_start_matches(crate::api::API).to_string())
            .ok("Comment deleted")
            .inval(pulls_url.trim_start_matches(crate::api::API).to_string())
            .act()
            .confirm("Delete comment?", "This review comment will be removed.", "Delete");
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(avatar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_1()
                    .child(
                        widgets::row()
                            .child(widgets::h3(comment.s("user.login")))
                            .child(widgets::dim(crate::time::ago(&comment.s("created_at"))))
                            .child(widgets::spacer())
                            .child(widgets::btn(ElementId::Name(format!("{id}-reply").into()), "Reply", reply).h(px(22.0)))
                            .child(
                                IconButton::new(ElementId::Name(format!("{id}-more").into()), "kebab")
                                    .on_click(on(Act::menu(vec![
                                        crate::hub::MenuEntry::item("Edit", edit),
                                        crate::hub::MenuEntry::item("Copy link", Act::Copy(comment.s("html_url"))),
                                        crate::hub::MenuEntry::Sep,
                                        crate::hub::MenuEntry::item("Delete", delete),
                                    ]))),
                            ),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}

/// The dialog a click on a diff line opens.
fn line_comment_form(target: &ReviewTarget, path: &str, side: &str, line: u32) -> Act {
    let (repo, number, commit) = (target.repo.clone(), target.number, target.commit.clone());
    let path = path.to_string();
    let side = side.to_string();
    FormSpec::new(format!("Comment on {path}:{line}"))
        .submit("Add single comment")
        .field(Field::multiline("body", "Comment").required())
        .rest("POST", format!("/repos/{repo}/pulls/{number}/comments"))
        .map(move |mut body, _| {
            body["commit_id"] = json!(commit);
            body["path"] = json!(path);
            body["side"] = json!(side);
            body["line"] = json!(line);
            body
        })
        .ok("Review comment added")
        .inval(format!("/repos/{repo}/pulls/{number}"))
        .act()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_the_hunk_header() {
        let lines = parse("@@ -10,3 +20,4 @@ fn x()\n a\n-b\n+c\n+d\n e");
        assert_eq!(lines[0].kind, Kind::Hunk);
        assert_eq!((lines[1].old, lines[1].new), (Some(10), Some(20)));
        assert_eq!((lines[2].kind, lines[2].old), (Kind::Del, Some(11)));
        assert_eq!((lines[3].kind, lines[3].new), (Kind::Add, Some(21)));
        assert_eq!((lines[5].old, lines[5].new), (Some(12), Some(23)));
    }
}
