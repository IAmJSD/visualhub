//! Unified diffs, read-only: a commit's or a pull request's changed files
//! with line numbers, and -- on a pull request -- review comments threaded
//! under the lines they are about, and a click on any line to start one.

use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load};
use crate::json::Json as _;
use crate::ui::{is_light, palette, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
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
    /// Where the line sits counting both sides, whichever it is on: the
    /// old and new line numbers it has or would have. A hunk header's
    /// are where its hunk starts. GitLab names lines by these.
    pub pos: (u32, u32),
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
                pos: (old, new),
                text: raw.to_string(),
            });
        } else if let Some(text) = raw.strip_prefix('+') {
            out.push(Line {
                kind: Kind::Add,
                old: None,
                new: Some(new),
                pos: (old, new),
                text: text.to_string(),
            });
            new += 1;
        } else if let Some(text) = raw.strip_prefix('-') {
            out.push(Line {
                kind: Kind::Del,
                old: Some(old),
                new: None,
                pos: (old, new),
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
                pos: (old, new),
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

/// How many lines a fold's button shows.
const STEP: i64 = 20;

/// A stretch of the file the diff leaves out: new-side lines `from` to
/// `to` (below the last hunk, `to` is the file's end, known once it is
/// read), whose old-side numbers are `shift` fewer.
#[derive(Debug, PartialEq)]
pub struct Gap {
    pub from: i64,
    pub to: Option<i64>,
    pub shift: i64,
}

/// A hunk header's starts and lengths: `@@ -a,b +c,d @@`. A missing
/// length is one.
fn header(text: &str) -> (i64, i64, i64, i64) {
    let mut parts = text.trim_start_matches('@').split_whitespace();
    let side = |part: Option<&str>, sign: char| {
        let mut nums = part
            .unwrap_or("")
            .trim_start_matches(sign)
            .split(',')
            .map(|v| v.parse::<i64>().unwrap_or(0));
        let start = nums.next().unwrap_or(0);
        (start, nums.next().unwrap_or(1))
    };
    let (a, b) = side(parts.next(), '-');
    let (c, d) = side(parts.next(), '+');
    (a, b, c, d)
}

/// The gaps of a parsed diff: one before each hunk, and one after the
/// last.
pub fn gaps(lines: &[Line]) -> Vec<Gap> {
    let mut out = Vec::new();
    // The next line on each side after what has been seen.
    let (mut old, mut new) = (1i64, 1i64);
    for line in lines.iter().filter(|l| l.kind == Kind::Hunk) {
        let (a, b, c, d) = header(&line.text);
        // An empty side's start is the line before it.
        let a = if b == 0 { a + 1 } else { a };
        let c = if d == 0 { c + 1 } else { c };
        out.push(Gap {
            from: new,
            to: Some(c - 1),
            shift: new - old,
        });
        old = a + b;
        new = c + d;
    }
    out.push(Gap {
        from: new,
        to: None,
        shift: new - old,
    });
    out
}

/// A row of a file's diff as shown.
enum Shown {
    /// A line of the diff, by its index.
    Diff(usize),
    /// An unchanged line opened around the diff.
    Extra(Line),
    /// Where `left` lines (unknown below the last hunk until the file is
    /// read) are still hidden, under a hunk's header if there is one.
    Fold {
        gap: usize,
        header: Option<String>,
        left: Option<i64>,
    },
}

/// A gap as the diff is drawn: opened `opened` lines from its top and
/// bottom.
struct Fold<'a> {
    gap: &'a Gap,
    index: usize,
    opened: (i64, i64),
    header: Option<&'a String>,
}

impl Fold<'_> {
    /// The gap's rows: lines opened from its top, the fold if lines are
    /// still hidden (or a header with nothing to hide), lines opened from
    /// its bottom.
    fn push(&self, shown: &mut Vec<Shown>, file: Option<&[String]>, expandable: bool) {
        let g = self.gap;
        let to = g.to.or(file.map(|f| f.len() as i64));
        let fold = |left| Shown::Fold {
            gap: self.index,
            header: self.header.cloned(),
            left,
        };
        let (Some(file), Some(to), true) = (file, to, expandable) else {
            let left = g.to.map(|to| (to - g.from + 1).max(0));
            if self.header.is_some() || expandable {
                shown.push(fold(left));
            }
            return;
        };
        let size = (to - g.from + 1).max(0);
        let top = self.opened.0.clamp(0, size);
        let bottom = self.opened.1.clamp(0, size - top);
        let extra = |n: i64| {
            file.get((n - 1) as usize).map(|text| {
                let old = (n - g.shift) as u32;
                Shown::Extra(Line {
                    kind: Kind::Context,
                    old: Some(old),
                    new: Some(n as u32),
                    pos: (old, n as u32),
                    text: text.clone(),
                })
            })
        };
        shown.extend((g.from..g.from + top).filter_map(extra));
        let left = size - top - bottom;
        if left > 0 || (size == 0 && self.header.is_some()) {
            shown.push(fold(Some(left)));
        }
        shown.extend((to - bottom + 1..=to).filter_map(extra));
    }
}

/// A fold: buttons to show more of the file, and the hunk's header.
#[allow(clippy::too_many_arguments)]
fn fold_row(
    id: &str,
    key: &str,
    opened: (i64, i64),
    header: Option<&str>,
    left: Option<i64>,
    expandable: bool,
    note: Option<&str>,
) -> AnyElement {
    let p = palette();
    let (fill, _) = colors(Kind::Hunk);
    let open = |top: i64, bottom: i64| {
        let key = key.to_string();
        let (t, b) = (opened.0 + top, opened.1 + bottom);
        on(Act::run(move |hub, _, cx| {
            hub.choices.insert(key.clone(), format!("{t},{b}"));
            cx.notify();
        }))
    };
    let button = |name: &str, icon: &'static str, tip: String| {
        IconButton::new(ElementId::Name(format!("{id}-{name}").into()), icon)
            .size(20.0)
            .tooltip(tip, None)
    };
    let mut buttons = div()
        .w(px(104.0))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_1();
    if expandable && note.is_none() && left != Some(0) {
        buttons = match (header, left) {
            // Below the last hunk: down to the file's end.
            (None, _) => buttons.child(
                button("down", "chevron-down", "Show more lines".into()).on_click(open(STEP, 0)),
            ),
            (Some(_), Some(n)) if n <= STEP => buttons.child(
                button(
                    "all",
                    "chevron-up",
                    format!("Show {n} hidden line{}", if n == 1 { "" } else { "s" }),
                )
                .on_click(open(0, n)),
            ),
            (Some(_), _) => {
                // Above the first hunk there is no hunk to open down from.
                let below_hunk = !key.ends_with(":gap0");
                buttons
                    .when(below_hunk, |b| {
                        b.child(
                            button(
                                "down",
                                "chevron-down",
                                format!("Show {STEP} lines below the hunk before"),
                            )
                            .on_click(open(STEP, 0)),
                        )
                    })
                    .child(
                        button(
                            "up",
                            "chevron-up",
                            format!("Show {STEP} lines above this hunk"),
                        )
                        .on_click(open(0, STEP)),
                    )
            }
        };
    }
    let text = match (note, header) {
        (Some(note), Some(header)) => format!("{header}  ·  {note}"),
        (Some(note), None) => note.to_string(),
        (None, Some(header)) => header.to_string(),
        (None, None) => String::new(),
    };
    div()
        .id(ElementId::Name(id.to_string().into()))
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_h(px(20.0))
        .when_some(fill, |d, f| d.bg(rgb(f)))
        .child(buttons)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr_4()
                .text_color(rgb(p.text_dim))
                .child(text),
        )
        .into_any_element()
}

/// The side and line number a review comment on `line` names.
fn point(line: &Line) -> (&'static str, u32) {
    match line.kind {
        Kind::Del => ("LEFT", line.old.unwrap_or(0)),
        _ => ("RIGHT", line.new.unwrap_or(0)),
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
                    if collapsed {
                        "chevron-right"
                    } else {
                        "chevron-down"
                    },
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
        let hunk_of: Vec<usize> = lines
            .iter()
            .scan(0usize, |k, l| {
                if l.kind == Kind::Hunk {
                    *k += 1;
                }
                Some(*k)
            })
            .collect();
        // What the diff leaves out opens from the file at the head, read
        // once something is opened. A removed file is all in its diff.
        let source = review.filter(|_| status != "removed");
        let gaps = gaps(&lines);
        let opened: Vec<(i64, i64)> = (0..gaps.len())
            .map(|k| {
                let v = self.choice(&format!("{id}:gap{k}"), "0,0");
                let (top, bottom) = v.split_once(',').unwrap_or(("0", "0"));
                (top.parse().unwrap_or(0), bottom.parse().unwrap_or(0))
            })
            .collect();
        let content = match source {
            Some(target) if opened.iter().any(|&(t, b)| t + b > 0) => Some(self.fetch_text(
                &format!(
                    "/repos/{}/contents/{}?ref={}",
                    target.repo,
                    crate::json::enc_path(&name),
                    crate::json::enc(&target.commit)
                ),
                "application/vnd.github.raw",
                cx,
            )),
            _ => None,
        };
        let file: Option<Vec<String>> = match &content {
            Some(Load::Ready(v)) => Some(v.s("").lines().map(str::to_string).collect()),
            _ => None,
        };
        let note = match &content {
            Some(Load::Loading) => Some("Loading…"),
            Some(Load::Failed(_)) => Some("Couldn't read the file to show more."),
            _ => None,
        };

        // The rows in order: the diff's own lines, lines opened around
        // them, and folds where lines are still hidden.
        let mut shown = Vec::new();
        let mut k = 0;
        for (i, line) in lines.iter().enumerate() {
            if line.kind == Kind::Hunk {
                let fold_at = Fold {
                    gap: &gaps[k],
                    index: k,
                    opened: opened[k],
                    header: Some(&line.text),
                };
                fold_at.push(&mut shown, file.as_deref(), source.is_some());
                k += 1;
            } else {
                shown.push(Shown::Diff(i));
            }
        }
        if source.is_some() {
            let fold_at = Fold {
                gap: &gaps[k],
                index: k,
                opened: opened[k],
                header: None,
            };
            fold_at.push(&mut shown, file.as_deref(), true);
        }

        // Coloured as the file's language, restarting at each fold.
        let texts: Vec<String> = shown
            .iter()
            .map(|s| match s {
                Shown::Diff(i) => lines[*i].text.replace('\t', "    "),
                Shown::Extra(l) => l.text.replace('\t', "    "),
                Shown::Fold { .. } => String::new(),
            })
            .collect();
        let breaks: Vec<usize> = shown
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Shown::Fold { .. }))
            .map(|(i, _)| i)
            .collect();
        let colours = crate::highlight::lines(crate::highlight::syntax_for(&name), &texts, &breaks);
        let anchor_key = format!("{id}:anchor");
        let anchor = self
            .choices
            .get(&anchor_key)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&a| a < lines.len());
        let mut body = div()
            .id(ElementId::Name(format!("{id}-body").into()))
            .flex()
            .flex_col()
            .font_family(widgets::MONO)
            .text_size(px(12.0))
            .line_height(px(20.0));
        for (r, row_of) in shown.iter().enumerate() {
            let (line, index) = match row_of {
                Shown::Diff(i) => (&lines[*i], Some(*i)),
                Shown::Extra(l) => (l, None),
                Shown::Fold { gap, header, left } => {
                    let waiting = opened[*gap] != (0, 0) || header.is_none();
                    body = body.child(fold_row(
                        &format!("{id}-r{r}"),
                        &format!("{id}:gap{}", gap),
                        opened[*gap],
                        header.as_deref(),
                        *left,
                        source.is_some(),
                        note.filter(|_| waiting),
                    ));
                    continue;
                }
            };
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
                .id(ElementId::Name(format!("{id}-r{r}").into()))
                .flex()
                .flex_row()
                .w_full()
                .when_some(fill, |d, f| d.bg(rgb(f)))
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
                        .child(crate::highlight::styled(
                            &texts[r],
                            colours.as_ref().map(|c| c[r].as_slice()),
                        )),
                );
            // Only the diff's own lines take comments: the forges place
            // review comments on lines of the diff.
            if let (Some(target), Some(i)) = (review, index) {
                let end = point(line);
                if end.1 > 0 {
                    // A click comments on the line and marks it; a
                    // shift-click marks one end of a range, or with a
                    // mark in the same hunk (the forges keep a range to
                    // one) comments on the lines from the mark to it.
                    let single = line_comment_form(target, &name, None, end);
                    let range = anchor
                        .filter(|&a| a != i && hunk_of[a] == hunk_of[i])
                        .map(|a| {
                            let (s, e) = if a < i { (a, i) } else { (i, a) };
                            line_comment_form(
                                target,
                                &name,
                                Some(point(&lines[s])),
                                point(&lines[e]),
                            )
                        });
                    let key = anchor_key.clone();
                    row = row
                        .cursor_pointer()
                        .when(anchor == Some(i), |d| d.bg(rgb(p.selection_bg)))
                        .hover(|s| s.bg(rgb(p.hover)))
                        .on_click(on(Act::run(move |hub, window, cx| {
                            let shift = window.modifiers().shift;
                            match &range {
                                Some(range) if shift => {
                                    hub.choices.remove(&key);
                                    hub.perform(range.clone(), window, cx);
                                }
                                _ => {
                                    hub.choices.insert(key.clone(), i.to_string());
                                    if shift {
                                        cx.notify();
                                    } else {
                                        hub.perform(single.clone(), window, cx);
                                    }
                                }
                            }
                        })));
                }
            }
            body = body.child(row);
            // Review threads anchored here.
            let here: Vec<&Value> = comments
                .iter()
                .filter(|c| match line.kind {
                    Kind::Del => {
                        c.s("side") == "LEFT" && c.i("line") as u32 == line.old.unwrap_or(0)
                    }
                    Kind::Hunk => false,
                    _ => c.s("side") != "LEFT" && c.i("line") as u32 == line.new.unwrap_or(0),
                })
                .collect();
            for (j, comment) in here.into_iter().enumerate() {
                let el = self.review_comment(&format!("{id}-c{r}-{j}"), comment, cx);
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
            .field(
                Field::multiline("body", "Comment")
                    .value(comment.s("body"))
                    .required(),
            )
            .rest("PATCH", url.trim_start_matches(crate::api::API).to_string())
            .ok("Comment updated")
            .inval(pulls_url.trim_start_matches(crate::api::API).to_string())
            .act();
        let delete = crate::hub::Req::rest(
            "DELETE",
            url.trim_start_matches(crate::api::API).to_string(),
        )
        .ok("Comment deleted")
        .inval(pulls_url.trim_start_matches(crate::api::API).to_string())
        .act()
        .confirm(
            "Delete comment?",
            "This review comment will be removed.",
            "Delete",
        );
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
                            .child(widgets::h3(crate::screens::common::shown_name(
                                comment.at("user"),
                            )))
                            .when(
                                comment.has("start_line")
                                    && comment.i("start_line") != comment.i("line"),
                                |r| {
                                    r.child(widgets::dim(format!(
                                        "lines {}–{}",
                                        comment.i("start_line"),
                                        comment.i("line")
                                    )))
                                },
                            )
                            .child(widgets::dim(crate::time::ago(&comment.s("created_at"))))
                            .child(widgets::spacer())
                            .child(
                                widgets::btn(
                                    ElementId::Name(format!("{id}-reply").into()),
                                    "Reply",
                                    reply,
                                )
                                .h(px(22.0)),
                            )
                            .child(
                                IconButton::new(
                                    ElementId::Name(format!("{id}-more").into()),
                                    "kebab",
                                )
                                .on_click(on(Act::menu(vec![
                                    crate::hub::MenuEntry::item("Edit", edit),
                                    crate::hub::MenuEntry::item(
                                        "Copy link",
                                        Act::Copy(comment.s("html_url")),
                                    ),
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

/// The dialog a click on a diff line opens: a comment on the line at
/// `end`, or on the lines from `start` to it.
fn line_comment_form(
    target: &ReviewTarget,
    path: &str,
    start: Option<(&str, u32)>,
    end: (&str, u32),
) -> Act {
    let (repo, number, commit) = (target.repo.clone(), target.number, target.commit.clone());
    let path = path.to_string();
    let (side, line) = (end.0.to_string(), end.1);
    let start = start.map(|(side, line)| (side.to_string(), line));
    let title = match &start {
        Some((_, from)) => format!("Comment on {path}:{from}–{line}"),
        None => format!("Comment on {path}:{line}"),
    };
    FormSpec::new(title)
        .submit("Add single comment")
        .field(Field::multiline("body", "Comment").required())
        .rest("POST", format!("/repos/{repo}/pulls/{number}/comments"))
        .map(move |mut body, _| {
            body["commit_id"] = json!(commit);
            body["path"] = json!(path);
            body["side"] = json!(side);
            body["line"] = json!(line);
            if let Some((start_side, start_line)) = &start {
                body["start_side"] = json!(start_side);
                body["start_line"] = json!(start_line);
            }
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
        // An added line still has a place on the old side.
        assert_eq!(lines[3].pos, (12, 21));
    }

    #[test]
    fn gaps_lie_between_hunks() {
        // Lines 1-9 come before the first hunk; it adds a line, so after
        // it the new side runs one ahead. The second adds after old line
        // 30 without taking any away.
        let lines = parse("@@ -10,2 +10,3 @@\n a\n+b\n c\n@@ -30,0 +32,2 @@\n+x\n+y\n");
        let g = gaps(&lines);
        assert_eq!(
            g[0],
            Gap {
                from: 1,
                to: Some(9),
                shift: 0
            }
        );
        assert_eq!(
            g[1],
            Gap {
                from: 13,
                to: Some(31),
                shift: 1
            }
        );
        assert_eq!(
            g[2],
            Gap {
                from: 34,
                to: None,
                shift: 3
            }
        );
    }
}
