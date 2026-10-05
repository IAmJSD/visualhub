//! GitHub-flavoured Markdown as gpui elements: paragraphs and headings as
//! styled text runs with clickable links, lists, task lists, quotes,
//! fenced code, tables, rules and images.
//!
//! gpui's styled text wants its highlight runs sorted and disjoint, so
//! inline styles are not ranges pushed and popped around the text but a
//! set of flags each appended piece of text is tagged with.

use crate::hub::{perform, Act, Hub, Route};
use crate::ui::palette;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontStyle, FontWeight, HighlightStyle,
    InteractiveElement as _, InteractiveText, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, UnderlineStyle,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Copy, Default, PartialEq)]
struct Flags {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
    /// An @mention of whoever is signed in.
    mine: bool,
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
        if flags.mine {
            let mut bg: gpui::Hsla = rgb(widgets::yellow()).into();
            bg.a = 0.22;
            style.background_color = Some(bg);
            style.color = Some(rgb(widgets::yellow()).into());
        }
        self.runs.push((start..self.text.len(), style));
    }

    /// `shown` as a link to `url`.
    fn push_link(&mut self, shown: &str, url: String, mut flags: Flags) {
        flags.link = true;
        let start = self.text.len();
        self.push(shown, flags);
        self.links.push((start..self.text.len(), url));
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
    /// A cell, which holds images as well as its text.
    TableCell,
}

struct Builder {
    id: String,
    n: usize,
    stack: Vec<(Frame, Vec<AnyElement>)>,
    inline: Option<Inline>,
    flags: Flags,
    /// Inside an HTML comment, which an HTML block hands over a line at a
    /// time.
    in_comment: bool,
    link_start: Vec<(usize, String)>,
    heading: Option<HeadingLevel>,
    code: Option<(String, String)>,
    images: Vec<(usize, String)>,
    /// The repository the page is about, which `#123` refers into.
    repo: Option<String>,
    /// Where relative image paths point: the repository's raw root at the
    /// shown ref, and the folder of the file being shown.
    raw_root: Option<String>,
    /// On GitLab, the project and ref whose files those are, which its API
    /// serves to the token (a private project's raw files want it).
    gitlab_raw: Option<(String, String)>,
    raw_dir: String,
    me: String,
    /// The document's paragraphs' text, one selectable line each.
    prose: Rc<RefCell<Vec<String>>>,
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
        let line = {
            let mut prose = self.prose.borrow_mut();
            prose.push(inline.text);
            prose.len() - 1
        };
        let block = format!("md:{}", self.id);
        let (el, styled) = crate::select::paragraph(&block, &self.prose, line, &inline.runs);
        let id = self.next_id();
        if inline.links.is_empty() {
            Some(el.child(styled).into_any_element())
        } else {
            let (ranges, urls): (Vec<_>, Vec<_>) = inline.links.into_iter().unzip();
            let urls = Rc::new(urls);
            Some(
                el.child(InteractiveText::new(id, styled).on_click(
                    ranges,
                    move |ix, window, cx| {
                        let url: String = urls[ix].clone();
                        perform(
                            Act::run(move |hub, _, cx| hub.open_link(&url, cx)),
                            window,
                            cx,
                        );
                    },
                ))
                .into_any_element(),
            )
        }
    }

    /// An image path as a URL: absolute ones as they are, relative ones
    /// against the shown file's folder, `/`-led ones against the repo.
    fn resolve(&self, src: &str) -> String {
        if src.contains("://") || src.starts_with("data:") {
            return src.to_string();
        }
        // GitLab's attachments live under the project.
        if src.starts_with("/uploads/") && crate::forge::is_gitlab() {
            if let Some(repo) = &self.repo {
                return format!("{}/{repo}{src}", crate::forge::web());
            }
        }
        if self.raw_root.is_none() && self.gitlab_raw.is_none() {
            return src.to_string();
        }
        let path = match src.strip_prefix('/') {
            Some(from_root) => from_root.to_string(),
            None if self.raw_dir.is_empty() => src.trim_start_matches("./").to_string(),
            None => format!("{}/{}", self.raw_dir, src.trim_start_matches("./")),
        };
        // Fold "a/../b" so the URL is the file's own.
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                ".." => {
                    parts.pop();
                }
                "." | "" => {}
                p => parts.push(p),
            }
        }
        let path = parts.join("/");
        match (&self.gitlab_raw, &self.raw_root) {
            (Some((repo, git_ref)), _) => format!(
                "{}{}/repository/files/{}/raw?ref={}",
                crate::forge::web(),
                crate::screens::gitlab::project_api(repo),
                crate::json::enc(&path),
                crate::json::enc(git_ref)
            ),
            (None, Some(root)) => format!("{root}/{path}"),
            (None, None) => path,
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
            in_comment: false,
            link_start: Vec::new(),
            heading: None,
            code: None,
            images: Vec::new(),
            repo: self.route.repo().map(str::to_string),
            raw_root: match &self.route {
                _ if crate::forge::is_gitlab() => None,
                // Bitbucket's API serves a private repository's images to
                // the token.
                Route::Repo { repo, .. } if crate::forge::is_bitbucket() => Some(format!(
                    "{}/repositories/{repo}/src/HEAD",
                    crate::forge::BITBUCKET_API
                )),
                Route::Tree { repo, git_ref, .. } if crate::forge::is_bitbucket() => Some(format!(
                    "{}/repositories/{repo}/src/{git_ref}",
                    crate::forge::BITBUCKET_API
                )),
                Route::Repo { repo, .. } => {
                    Some(format!("https://raw.githubusercontent.com/{repo}/HEAD"))
                }
                Route::Tree { repo, git_ref, .. } => Some(format!(
                    "https://raw.githubusercontent.com/{repo}/{git_ref}"
                )),
                _ => None,
            },
            gitlab_raw: match &self.route {
                Route::Repo { repo, .. } if crate::forge::is_gitlab() => {
                    Some((repo.clone(), "HEAD".to_string()))
                }
                Route::Tree { repo, git_ref, .. } if crate::forge::is_gitlab() => {
                    Some((repo.clone(), git_ref.clone()))
                }
                _ => None,
            },
            raw_dir: match &self.route {
                Route::Tree {
                    path, file: true, ..
                } => path
                    .rsplit_once('/')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_default(),
                Route::Tree { path, .. } => path.clone(),
                _ => String::new(),
            },
            me: self.login(),
            prose: Rc::default(),
        };

        let mut events = Parser::new_ext(source, options).peekable();
        while let Some(event) = events.next() {
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
                        b.stack.push((Frame::TableCell, Vec::new()));
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
                        b.flush_paragraph();
                        let children = match b.stack.last() {
                            Some((Frame::TableCell, _)) => b.stack.pop().map(|(_, c)| c),
                            _ => None,
                        }
                        .unwrap_or_default();
                        let head = matches!(b.stack.last(), Some((Frame::TableRow(true), _)));
                        b.push_el(
                            div()
                                .flex_1()
                                .min_w(px(60.0))
                                .px_2()
                                .py_1()
                                .border_r_1()
                                .border_color(rgb(p.divider))
                                .when_head(head)
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(children)
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
                            let url = b.resolve(&url);
                            // GitLab sizes an image with attributes after
                            // it: `![x](a.png){width=12}`.
                            let attrs = match events.peek() {
                                Some(Event::Text(t)) => image_attrs(t),
                                _ => None,
                            };
                            let (width, height) = match attrs {
                                Some((width, height, used)) => {
                                    if let Some(Event::Text(t)) = events.next() {
                                        let rest = t[used..].to_string();
                                        if !rest.is_empty() {
                                            let flags = b.flags;
                                            b.inline().push(&rest, flags);
                                        }
                                    }
                                    (width, height)
                                }
                                None => (None, None),
                            };
                            // The image goes in a block of its own, after
                            // whatever text led up to it.
                            b.flush_paragraph();
                            let image = self.image_sized(&url, width, height, cx);
                            b.push_el(image);
                        }
                    }
                    _ => {}
                },
                Event::Text(t) => {
                    if b.images.is_empty() {
                        let flags = b.flags;
                        if b.link_start.is_empty() {
                            autolink(&mut b, &t, flags);
                        } else {
                            b.inline().push(&t, flags);
                        }
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
                    // Templates leave their instructions in comments,
                    // which the forges' own pages hide.
                    let html = strip_comments(&html, &mut b.in_comment);
                    if html.trim().is_empty() {
                        continue;
                    }
                    if let Some(tag) = img_tag(&html) {
                        b.flush_paragraph();
                        // A <picture> may offer a dark-mode version.
                        let src = b.resolve(&match tag.dark_src {
                            Some(dark) if !crate::ui::is_light() => dark,
                            _ => tag.src,
                        });
                        let image = self.image_sized(&src, tag.width, tag.height, cx);
                        let image = match tag.href {
                            Some(href) => div()
                                .id(b.next_id())
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    let href = href.clone();
                                    perform(
                                        Act::run(move |hub, _, cx| hub.open_link(&href, cx)),
                                        window,
                                        cx,
                                    );
                                })
                                .child(image)
                                .into_any_element(),
                            None => image,
                        };
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
        let children = b
            .stack
            .into_iter()
            .next()
            .map(|(_, c)| c)
            .unwrap_or_default();
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
    let source: Vec<String> = code.split('\n').map(|l| l.replace('\t', "    ")).collect();
    // The fence's tag names the language: ```rust, ```ts, ```shell.
    let tag = lang.split([',', ' ', '{']).next().unwrap_or("");
    let colours = crate::highlight::lines(crate::highlight::syntax_for(tag), &source, &[]);
    let block = format!("code:{id:?}");
    let source = Rc::new(source);
    let mut lines = div().flex().flex_col();
    for i in 0..source.len() {
        lines = lines.child(
            crate::select::line(
                &block,
                &source,
                i,
                colours.as_ref().map(|c| c[i].as_slice()),
            )
            .min_h(px(18.0)),
        );
    }
    let copy_id = ElementId::Name(format!("{block}-copy").into());
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
    let all = source.join("\n");
    block = block.child(
        div()
            .absolute()
            .top_1()
            .right_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .when(!lang.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(p.text_faint))
                        .child(lang.to_string()),
                )
            })
            .child(
                crate::ui::IconButton::new(copy_id, "copy")
                    .size(22.0)
                    .icon_size(13.0)
                    .color(p.text_dim)
                    .tooltip("Copy", None)
                    .on_click(crate::hub::on(Act::Copy(all))),
            ),
    );
    block.into_any_element()
}

/// The `{width=12 height=8}` GitLab allows after an image: its width and
/// height in pixels, and how much of `text` it took.
fn image_attrs(text: &str) -> Option<(Option<f32>, Option<f32>, usize)> {
    let inner = text.strip_prefix('{')?;
    let end = inner.find('}')?;
    let (mut width, mut height) = (None, None);
    for attr in inner[..end].split_whitespace() {
        let (key, value) = attr.split_once('=')?;
        let value = value.trim_matches('"');
        // Percentages have nothing to be a share of here.
        let pixels = value
            .strip_suffix("px")
            .unwrap_or(value)
            .parse::<f32>()
            .ok();
        match key {
            "width" => width = pixels,
            "height" => height = pixels,
            _ => {}
        }
    }
    (width.is_some() || height.is_some()).then_some((width, height, end + 2))
}

/// `html` without its comments, carrying whether one is still open over
/// to the next piece.
fn strip_comments(html: &str, in_comment: &mut bool) -> String {
    let mut out = String::new();
    let mut rest = html;
    loop {
        if *in_comment {
            match rest.find("-->") {
                Some(end) => {
                    rest = &rest[end + 3..];
                    *in_comment = false;
                }
                None => return out,
            }
        } else {
            match rest.find("<!--") {
                Some(start) => {
                    out.push_str(&rest[..start]);
                    rest = &rest[start + 4..];
                    *in_comment = true;
                }
                None => {
                    out.push_str(rest);
                    return out;
                }
            }
        }
    }
}

pub fn strip_tags(html: &str) -> String {
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
/// An `<img>` as README HTML writes it: where it is, the size it asks
/// for, a `<picture>`'s dark-mode source, and the link around it.
struct ImgTag {
    src: String,
    width: Option<f32>,
    height: Option<f32>,
    dark_src: Option<String>,
    href: Option<String>,
}

/// The value of `name="…"` in the tag starting at `tag`.
fn attr(html: &str, tag: &str, name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let at = lower.find(tag)?;
    let end = lower[at..].find('>').map_or(html.len(), |e| at + e);
    let inside = &html[at..end];
    let inside_lower = &lower[at..end];
    let key = format!(" {name}=");
    let start = inside_lower.find(&key)? + key.len();
    let rest = &inside[start..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &rest[1..];
        Some(rest[..rest.find(quote)?].to_string())
    } else {
        Some(rest[..rest.find([' ', '>']).unwrap_or(rest.len())].to_string())
    }
}

fn img_tag(html: &str) -> Option<ImgTag> {
    let src = img_src(html)?;
    // "120" or "120px"; a percentage can't be honoured without the
    // column's width, so it's left to the image's own size.
    let size = |name: &str| {
        attr(html, "<img", name).and_then(|v| v.trim_end_matches("px").parse::<f32>().ok())
    };
    let dark_src = html
        .to_ascii_lowercase()
        .contains("prefers-color-scheme: dark")
        .then(|| {
            let lower = html.to_ascii_lowercase();
            let at = lower.find("prefers-color-scheme: dark")?;
            let start = lower[..at].rfind("<source")?;
            attr(&html[start..], "<source", "srcset")
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
        })
        .flatten();
    Some(ImgTag {
        src,
        width: size("width"),
        height: size("height"),
        dark_src,
        href: attr(html, "<a", "href"),
    })
}

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

/// Plain text with the forge's references made links, as its site shows
/// them: bare URLs (shortened when they point into it), `@login`, `#123`
/// in the page's repository, and on GitLab `!123` for a merge request.
fn autolink(b: &mut Builder, text: &str, flags: Flags) {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut plain = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let after_word = text[..i]
            .chars()
            .next_back()
            .is_some_and(|c| word(c) || c == '/' || c == '.');
        let token: Option<(usize, String, String, Flags)> =
            if !after_word && (rest.starts_with("https://") || rest.starts_with("http://")) {
                let mut end = rest
                    .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
                    .unwrap_or(rest.len());
                end -= rest[..end].len()
                    - rest[..end]
                        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"'])
                        .len();
                let url = &rest[..end];
                let (shown, code) = short_url(url, b.repo.as_deref());
                Some((
                    end,
                    shown,
                    url.to_string(),
                    Flags {
                        code: code || flags.code,
                        ..flags
                    },
                ))
            } else if !after_word && rest.starts_with('@') {
                let login: String = rest[1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .take(39)
                    .collect();
                (!login.is_empty() && !login.starts_with('-')).then(|| {
                    let mine = login.eq_ignore_ascii_case(&b.me);
                    (
                        1 + login.len(),
                        format!("@{login}"),
                        format!("{}/{login}", crate::forge::web()),
                        Flags { mine, ..flags },
                    )
                })
            } else if !after_word
                && (rest.starts_with('#') || (rest.starts_with('!') && crate::forge::is_gitlab()))
            {
                let mark = &rest[..1];
                let digits: String = rest[1..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                let ends_word = rest[1 + digits.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !word(c));
                let web = crate::forge::web();
                match &b.repo {
                    Some(repo) if !digits.is_empty() && ends_word => {
                        let url = match (crate::forge::is_gitlab(), mark) {
                            // Bitbucket's numbers are pull requests'.
                            _ if crate::forge::is_bitbucket() => {
                                format!("{web}/{repo}/pull-requests/{digits}")
                            }
                            (true, "!") => format!("{web}/{repo}/-/merge_requests/{digits}"),
                            (true, _) => format!("{web}/{repo}/-/issues/{digits}"),
                            _ => format!("{web}/{repo}/issues/{digits}"),
                        };
                        Some((1 + digits.len(), format!("{mark}{digits}"), url, flags))
                    }
                    _ => None,
                }
            } else {
                None
            };
        match token {
            Some((len, shown, url, link_flags)) => {
                b.inline().push(&text[plain..i], flags);
                b.inline().push_link(&shown, url, link_flags);
                i += len;
                plain = i;
            }
            None => i += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    b.inline().push(&text[plain..], flags);
}

/// How GitHub shows a link to itself: `#138` in this repository,
/// `owner/repo#138` elsewhere, a commit's short SHA, a compare's range.
/// The flag says to set it as code.
fn short_url(url: &str, here: Option<&str>) -> (String, bool) {
    if crate::forge::is_gitlab() {
        return short_gitlab_url(url, here);
    }
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return (url.to_string(), false);
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let [owner, name, kind, item, ..] = parts.as_slice() else {
        return (url.to_string(), false);
    };
    let repo = format!("{owner}/{name}");
    let prefix = if here == Some(repo.as_str()) {
        String::new()
    } else {
        repo
    };
    match *kind {
        "pull" | "issues" | "discussions" if item.parse::<u64>().is_ok() && parts.len() == 4 => {
            (format!("{prefix}#{item}"), false)
        }
        "commit" if item.len() >= 7 => {
            let sha = &item[..7];
            (
                if prefix.is_empty() {
                    sha.to_string()
                } else {
                    format!("{prefix}@{sha}")
                },
                true,
            )
        }
        "compare" => (parts[3..].join("/"), true),
        _ => (url.to_string(), false),
    }
}

/// [`short_url`] for a GitLab instance: `#12` and `!12` in this project,
/// `group/project#12` elsewhere, a commit's short SHA.
fn short_gitlab_url(url: &str, here: Option<&str>) -> (String, bool) {
    let web = format!("{}/", crate::forge::web());
    let Some(rest) = url.strip_prefix(&web) else {
        return (url.to_string(), false);
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let Some((repo, page)) = rest.split_once("/-/") else {
        return (url.to_string(), false);
    };
    let parts: Vec<&str> = page.split('/').filter(|p| !p.is_empty()).collect();
    let prefix = if here == Some(repo) {
        String::new()
    } else {
        repo.to_string()
    };
    match parts.as_slice() {
        ["issues", n] if n.parse::<u64>().is_ok() => (format!("{prefix}#{n}"), false),
        ["merge_requests", n] if n.parse::<u64>().is_ok() => (format!("{prefix}!{n}"), false),
        ["commit", sha] if sha.len() >= 8 => {
            let short = &sha[..8];
            (
                if prefix.is_empty() {
                    short.to_string()
                } else {
                    format!("{prefix}@{short}")
                },
                true,
            )
        }
        ["compare", range] => (range.to_string(), true),
        _ => (url.to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_take_gitlabs_sizes() {
        assert_eq!(image_attrs("{width=12}"), Some((Some(12.0), None, 10)));
        assert_eq!(
            image_attrs("{width=\"30px\" height=20} after"),
            Some((Some(30.0), Some(20.0), 24))
        );
        assert_eq!(image_attrs("{width=50%}"), None);
        assert_eq!(image_attrs("(UTC+1)"), None);
        assert_eq!(image_attrs("{: .class}"), None);
    }

    #[test]
    fn comments_are_hidden() {
        let mut open = false;
        assert_eq!(strip_comments("<!--", &mut open), "");
        assert!(open);
        assert_eq!(
            strip_comments("**Not ready yet?** see [x](y)", &mut open),
            ""
        );
        assert_eq!(
            strip_comments("--> <b>after</b>", &mut open),
            " <b>after</b>"
        );
        assert!(!open);
        assert_eq!(strip_comments("a <!-- b --> c <!--- d", &mut open), "a  c ");
        assert!(open);
    }

    #[test]
    fn github_links_shorten() {
        let here = Some("Infrawrench/schist");
        assert_eq!(
            short_url("https://github.com/Infrawrench/schist/pull/138", here),
            ("#138".into(), false)
        );
        assert_eq!(
            short_url("https://github.com/other/repo/issues/9", here),
            ("other/repo#9".into(), false)
        );
        assert_eq!(
            short_url(
                "https://github.com/Infrawrench/schist/compare/v0.14.0...v0.15.0",
                here
            ),
            ("v0.14.0...v0.15.0".into(), true)
        );
        assert_eq!(
            short_url(
                "https://github.com/Infrawrench/schist/commit/0123456789abcdef",
                here
            ),
            ("0123456".into(), true)
        );
        assert_eq!(
            short_url("https://github.com/Infrawrench/schist/pull/138/files", here).0,
            "https://github.com/Infrawrench/schist/pull/138/files"
        );
        assert_eq!(
            short_url("https://example.com/a", here).0,
            "https://example.com/a"
        );
    }

    #[test]
    fn picture_tags() {
        let html = r#"<a href="https://leanercloud.com"><picture><source media="(prefers-color-scheme: dark)" srcset="https://x/dark"><img src="https://x/light" alt="L" width="120"></picture></a>"#;
        let tag = img_tag(html).unwrap();
        assert_eq!(tag.src, "https://x/light");
        assert_eq!(tag.dark_src.as_deref(), Some("https://x/dark"));
        assert_eq!(tag.width, Some(120.0));
        assert_eq!(tag.height, None);
        assert_eq!(tag.href.as_deref(), Some("https://leanercloud.com"));
    }

    #[test]
    fn img_tags_and_html_text() {
        assert_eq!(
            img_src(r#"<img width="300" src="https://x/y.png" alt="">"#).as_deref(),
            Some("https://x/y.png")
        );
        assert_eq!(strip_tags("<b>bold</b> &amp; more"), "bold & more");
    }
}
