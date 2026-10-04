//! GitHub-flavoured Markdown as gpui elements: paragraphs and headings as
//! styled text runs with clickable links, lists, task lists, quotes,
//! fenced code, tables, rules and images.
//!
//! gpui's styled text wants its highlight runs sorted and disjoint, so
//! inline styles are not ranges pushed and popped around the text but a
//! set of flags each appended piece of text is tagged with.

use crate::hub::{perform, Act, Hub};
use crate::widgets::{self, rgb};
use gpui::{
    div, px, AnyElement, Context, ElementId, FontStyle, FontWeight, HighlightStyle,
    InteractiveElement as _, InteractiveText, IntoElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, UnderlineStyle,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use crate::ui::palette;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Copy, Default, PartialEq)]
struct Flags {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

#[derive(Default)]
struct Inline {
    text: String,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, String)>,
}

impl Inline {
    fn push(&mut self, s: &str, flags: Flags) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        if flags == Flags::default() {
            return;
        }
        let p = palette();
        let mut style = HighlightStyle::default();
        if flags.bold {
            style.font_weight = Some(FontWeight::SEMIBOLD);
        }
        if flags.italic {
            style.font_style = Some(FontStyle::Italic);
        }
        if flags.strike {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            });
        }
        if flags.code {
            style.background_color = Some(rgb(p.control_bg).into());
        }
        if flags.link {
            style.color = Some(rgb(p.accent_hover).into());
            style.underline = Some(UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            });
        }
        self.runs.push((start..self.text.len(), style));
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

enum Frame {
    Root,
    Quote,
    /// The next number for an ordered list.
    List(Option<u64>),
    Item(String),
    Table,
    TableRow(bool),
}

struct Builder {
    id: String,
    n: usize,
    stack: Vec<(Frame, Vec<AnyElement>)>,
    inline: Option<Inline>,
    flags: Flags,
    link_start: Vec<(usize, String)>,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
    images: Vec<(usize, String)>,
}

impl Builder {
    fn next_id(&mut self) -> ElementId {
        self.n += 1;
        ElementId::Name(format!("{}-{}", self.id, self.n).into())
    }

    fn inline(&mut self) -> &mut Inline {
        self.inline.get_or_insert_with(Inline::default)
    }

    fn push_el(&mut self, el: AnyElement) {
        if let Some((_, children)) = self.stack.last_mut() {
            children.push(el);
        }
    }

    /// The text gathered so far, as one element.
    fn take_text(&mut self) -> Option<AnyElement> {
        let inline = self.inline.take()?;
        if inline.is_blank() && inline.links.is_empty() {
            return None;
        }
        let text = SharedString::from(inline.text);
        let styled = StyledText::new(text).with_highlights(inline.runs);
        let id = self.next_id();
        if inline.links.is_empty() {
            Some(div().child(styled).into_any_element())
        } else {
            let (ranges, urls): (Vec<_>, Vec<_>) = inline.links.into_iter().unzip();
            let urls = Rc::new(urls);
            Some(
                div()
                    .child(
                        InteractiveText::new(id, styled).on_click(ranges, move |ix, window, cx| {
                            let url: String = urls[ix].clone();
                            perform(
                                Act::run(move |hub, _, cx| hub.open_link(&url, cx)),
                                window,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
            )
        }
    }

    fn flush_paragraph(&mut self) {
        if let Some(el) = self.take_text() {
            self.push_el(el);
        }
    }
}

impl Hub {
    /// Markdown as elements. `id` must be unique on the page.
    pub fn markdown(&mut self, id: &str, source: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        if source.trim().is_empty() {
            return widgets::dim("No description provided.")
                .italic()
                .into_any_element();
        }
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TASKLISTS);
        let mut b = Builder {
            id: id.to_string(),
            n: 0,
            stack: vec![(Frame::Root, Vec::new())],
            inline: None,
            flags: Flags::default(),
            link_start: Vec::new(),
            heading: None,
            code: None,
            images: Vec::new(),
        };

        for event in Parser::new_ext(source, options) {
            if let Some((_, code)) = &mut b.code {
                match event {
                    Event::Text(t) => {
                        code.push_str(&t);
                        continue;
                    }
                    Event::End(TagEnd::CodeBlock) => {}
                    _ => continue,
                }
            }
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        b.flush_paragraph();
                    }
                    Tag::Heading { level, .. } => {
                        b.flush_paragraph();
                        b.heading = Some(level);
                    }
                    Tag::BlockQuote(_) => {
                        b.flush_paragraph();
                        b.stack.push((Frame::Quote, Vec::new()));
                    }
                    Tag::CodeBlock(kind) => {
                        b.flush_paragraph();
                        let lang = match kind {
                            CodeBlockKind::Fenced(lang) => lang.to_string(),
                            CodeBlockKind::Indented => String::new(),
                        };
                        b.code = Some((lang, String::new()));
                    }
                    Tag::List(start) => {
                        b.flush_paragraph();
                        b.stack.push((Frame::List(start), Vec::new()));
                    }
                    Tag::Item => {
                        b.flush_paragraph();
                        let marker = match b.stack.last_mut() {
                            Some((Frame::List(Some(n)), _)) => {
                                let m = format!("{n}.");
                                *n += 1;
                                m
                            }
                            _ => "•".to_string(),
                        };
                        b.stack.push((Frame::Item(marker), Vec::new()));
                    }
                    Tag::Table(_) => {
                        b.flush_paragraph();
                        b.stack.push((Frame::Table, Vec::new()));
                    }
                    Tag::TableHead => b.stack.push((Frame::TableRow(true), Vec::new())),
                    Tag::TableRow => b.stack.push((Frame::TableRow(false), Vec::new())),
                    Tag::TableCell => {
                        b.inline = Some(Inline::default());
                    }
                    Tag::Emphasis => b.flags.italic = true,
                    Tag::Strong => b.flags.bold = true,
                    Tag::Strikethrough => b.flags.strike = true,
                    Tag::Link { dest_url, .. } => {
                        let at = b.inline().text.len();
                        b.link_start.push((at, dest_url.to_string()));
                        b.flags.link = true;
                    }
                    Tag::Image { dest_url, .. } => {
                        b.images.push((0, dest_url.to_string()));
                    }
                    _ => {}
                },
                Event::End(end) => match end {
                    TagEnd::Paragraph => b.flush_paragraph(),
                    TagEnd::Heading(_) => {
                        let level = b.heading.take();
                        if let Some(text) = b.take_text() {
                            let (size, rule) = match level {
                                Some(HeadingLevel::H1) => (22.0, true),
                                Some(HeadingLevel::H2) => (18.0, true),
                                Some(HeadingLevel::H3) => (15.0, false),
                                _ => (13.0, false),
                            };
                            let mut el = div()
                                .pt_2()
                                .pb_1()
                                .text_size(px(size))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(text);
                            if rule {
                                el = el.border_b_1().border_color(rgb(p.divider));
                            }
                            b.push_el(el.into_any_element());
                        }
                    }
                    TagEnd::BlockQuote(_) => {
                        b.flush_paragraph();
                        if let Some((_, children)) = b.stack.pop() {
                            b.push_el(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .pl_3()
                                    .border_l_4()
                                    .border_color(rgb(p.edge))
                                    .text_color(rgb(p.text_dim))
                                    .children(children)
                                    .into_any_element(),
                            );
                        }
                    }
                    TagEnd::CodeBlock => {
                        if let Some((lang, code)) = b.code.take() {
                            let id = b.next_id();
                            let el = code_block(id, &lang, &code);
                            b.push_el(el);
                        }
                    }
                    TagEnd::List(_) => {
                        b.flush_paragraph();
                        if let Some((_, children)) = b.stack.pop() {
                            b.push_el(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(children)
                                    .into_any_element(),
                            );
                        }
                    }
                    TagEnd::Item => {
                        b.flush_paragraph();
                        if let Some((Frame::Item(marker), children)) = b.stack.pop() {
                            b.push_el(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_none()
                                            .min_w(px(14.0))
                                            .text_color(rgb(p.text_dim))
                                            .child(marker),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .children(children),
                                    )
                                    .into_any_element(),
                            );
                        }
                    }
                    TagEnd::TableCell => {
                        let head = matches!(b.stack.last(), Some((Frame::TableRow(true), _)));
                        let text = b.take_text();
                        b.push_el(
                            div()
                                .flex_1()
                                .min_w(px(60.0))
                                .px_2()
                                .py_1()
                                .border_r_1()
                                .border_color(rgb(p.divider))
                                .when_head(head)
                                .children(text)
                                .into_any_element(),
                        );
                    }
                    TagEnd::TableHead | TagEnd::TableRow => {
                        if let Some((_, cells)) = b.stack.pop() {
                            b.push_el(
                                div()
                                    .flex()
                                    .flex_row()
                                    .border_b_1()
                                    .border_color(rgb(p.divider))
                                    .children(cells)
                                    .into_any_element(),
                            );
                        }
                    }
                    TagEnd::Table => {
                        if let Some((_, rows)) = b.stack.pop() {
                            b.push_el(
                                div()
                                    .flex()
                                    .flex_col()
                                    .border_1()
                                    .border_color(rgb(p.edge))
                                    .rounded_sm()
                                    .text_size(px(12.0))
                                    .children(rows)
                                    .into_any_element(),
                            );
                        }
                    }
                    TagEnd::Emphasis => b.flags.italic = false,
                    TagEnd::Strong => b.flags.bold = false,
                    TagEnd::Strikethrough => b.flags.strike = false,
                    TagEnd::Link => {
                        b.flags.link = false;
                        if let Some((start, url)) = b.link_start.pop() {
                            let end = b.inline().text.len();
                            if end > start {
                                b.inline().links.push((start..end, url));
                            }
                        }
                    }
                    TagEnd::Image => {
                        if let Some((_, url)) = b.images.pop() {
                            // The image goes in a block of its own, after
                            // whatever text led up to it.
                            b.flush_paragraph();
                            let image = self.image(&url, cx);
                            b.push_el(image);
                        }
                    }
                    _ => {}
                },
                Event::Text(t) => {
                    if b.images.is_empty() {
                        let flags = b.flags;
                        b.inline().push(&t, flags);
                    }
                }
                Event::Code(t) => {
                    let mut flags = b.flags;
                    flags.code = true;
                    b.inline().push(&format!("\u{2009}{t}\u{2009}"), flags);
                }
                Event::SoftBreak => {
                    let flags = b.flags;
                    b.inline().push(" ", flags);
                }
                Event::HardBreak => b.inline().push("\n", Flags::default()),
                Event::Rule => {
                    b.flush_paragraph();
                    b.push_el(
                        div()
                            .h(px(2.0))
                            .my_2()
                            .bg(rgb(p.divider))
                            .into_any_element(),
                    );
                }
                Event::TaskListMarker(done) => {
                    b.inline()
                        .push(if done { "☑ " } else { "☐ " }, Flags::default());
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    if let Some(src) = img_src(&html) {
                        b.flush_paragraph();
                        let image = self.image(&src, cx);
                        b.push_el(image);
                    } else if html.contains("<br") {
                        b.inline().push("\n", Flags::default());
                    } else {
                        let text = strip_tags(&html);
                        if !text.trim().is_empty() {
                            let flags = b.flags;
                            b.inline().push(&text, flags);
                        }
                    }
                }
                Event::FootnoteReference(name) => {
                    b.inline().push(&format!("[{name}]"), Flags::default());
                }
                _ => {}
            }
        }
        b.flush_paragraph();
        let children = b.stack.into_iter().next().map(|(_, c)| c).unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .text_size(px(widgets::TEXT))
            .line_height(px(20.0))
            .text_color(rgb(p.text))
            .children(children)
            .into_any_element()
    }
}

trait WhenHead {
    fn when_head(self, head: bool) -> Self;
}

impl WhenHead for gpui::Div {
    fn when_head(self, head: bool) -> Self {
        if head {
            self.font_weight(FontWeight::SEMIBOLD)
                .bg(rgb(palette().deep_bg))
        } else {
            self
        }
    }
}

/// A fenced block: monospace lines on the recessed fill, scrolling
/// sideways rather than wrapping.
pub fn code_block(id: ElementId, lang: &str, code: &str) -> AnyElement {
    let p = palette();
    let code = code.strip_suffix('\n').unwrap_or(code);
    let mut lines = div().flex().flex_col();
    for line in code.split('\n') {
        let line = line.replace('\t', "    ");
        lines = lines.child(
            div()
                .whitespace_nowrap()
                .min_h(px(18.0))
                .child(if line.is_empty() { " ".to_string() } else { line }),
        );
    }
    let mut block = div()
        .id(id)
        .relative()
        .flex()
        .flex_col()
        .p_3()
        .rounded_md()
        .bg(rgb(p.deep_bg))
        .border_1()
        .border_color(rgb(p.divider))
        .font_family(widgets::MONO)
        .text_size(px(12.0))
        .line_height(px(18.0))
        .overflow_x_scroll()
        .child(lines);
    if !lang.is_empty() {
        block = block.child(
            div()
                .absolute()
                .top_1()
                .right_2()
                .text_size(px(10.0))
                .text_color(rgb(p.text_faint))
                .child(lang.to_string()),
        );
    }
    block.into_any_element()
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

/// The `src` of an `<img>` tag, which is how GitHub embeds uploads.
fn img_src(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let at = lower.find("<img")?;
    let rest = &html[at..];
    let src = rest.find("src=")?;
    let rest = &rest[src + 4..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &rest[1..];
        let end = rest.find(quote)?;
        Some(rest[..end].to_string())
    } else {
        let end = rest.find([' ', '>']).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn img_tags_and_html_text() {
        assert_eq!(
            img_src(r#"<img width="300" src="https://x/y.png" alt="">"#).as_deref(),
            Some("https://x/y.png")
        );
        assert_eq!(strip_tags("<b>bold</b> &amp; more"), "bold & more");
    }
}
