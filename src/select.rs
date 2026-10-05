//! Selecting and copying text: Markdown's paragraphs and code blocks, and
//! the file viewer. gpui's text doesn't select on its own, so each line
//! asks its laid-out text which character the pointer is over, and the
//! selection lives here, one at a time, across the lines of one block.
//!
//! Drag to select, double-click for a word, triple-click for a line;
//! Cmd/Ctrl+C copies. Any other press clears it.

use gpui::{
    div, Div, HighlightStyle, InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Styled as _, StyledText, TextLayout,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

/// A place in a block: (line, byte offset).
type Pos = (usize, usize);

struct Selection {
    block: String,
    lines: Rc<Vec<String>>,
    anchor: Pos,
    head: Pos,
    dragging: bool,
}

thread_local! {
    static SELECTION: RefCell<Option<Selection>> = const { RefCell::new(None) };
}

impl Selection {
    fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// The selected part of line `line` in `block`, if any.
fn range_in(block: &str, line: usize, len: usize) -> Option<Range<usize>> {
    SELECTION.with(|s| {
        let s = s.borrow();
        let s = s.as_ref().filter(|s| s.block == block)?;
        let (start, end) = s.ordered();
        if line < start.0 || line > end.0 || start == end {
            return None;
        }
        let from = if line == start.0 { start.1.min(len) } else { 0 };
        let to = if line == end.0 { end.1.min(len) } else { len };
        Some(from..to)
    })
}

/// What's selected, as text.
pub fn selected_text() -> Option<String> {
    SELECTION.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        let (start, end) = s.ordered();
        if start == end {
            return None;
        }
        let mut out = String::new();
        for line in start.0..=end.0 {
            let text = s.lines.get(line)?;
            let from = if line == start.0 {
                start.1.min(text.len())
            } else {
                0
            };
            let to = if line == end.0 {
                end.1.min(text.len())
            } else {
                text.len()
            };
            out.push_str(&text[from..to]);
            if line != end.0 {
                out.push('\n');
            }
        }
        Some(out)
    })
}

/// Drop the selection; true if there was one.
pub fn clear() -> bool {
    SELECTION.with(|s| s.borrow_mut().take().is_some())
}

/// The word around `at` in `text`, as a byte range.
fn word(text: &str, at: usize) -> Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = at.min(text.len());
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// Syntax colours with the selection laid over them. gpui wants the runs
/// sorted and disjoint, so both are cut at every edge.
fn merge(
    syntax: &[(Range<usize>, HighlightStyle)],
    selected: Option<Range<usize>>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(sel) = selected.filter(|r| !r.is_empty()) else {
        return syntax.to_vec();
    };
    let p = crate::ui::palette();
    let mut edges: Vec<usize> = vec![sel.start, sel.end];
    for (r, _) in syntax {
        edges.push(r.start);
        edges.push(r.end);
    }
    edges.sort_unstable();
    edges.dedup();
    let mut out = Vec::new();
    for pair in edges.windows(2) {
        let piece = pair[0]..pair[1];
        let base = syntax
            .iter()
            .find(|(r, _)| r.start <= piece.start && piece.end <= r.end)
            .map(|(_, s)| *s);
        let in_sel = sel.start <= piece.start && piece.end <= sel.end;
        let style = match (base, in_sel) {
            (base, true) => HighlightStyle {
                background_color: Some(gpui::rgb(p.selection_bg).into()),
                ..base.unwrap_or_default()
            },
            (Some(base), false) => base,
            (None, false) => continue,
        };
        out.push((piece, style));
    }
    out
}

/// Line `line` of `block` as selectable text, coloured by `syntax`.
pub fn line(
    block: &str,
    lines: &Rc<Vec<String>>,
    line: usize,
    syntax: Option<&[(Range<usize>, HighlightStyle)]>,
) -> Div {
    let text = lines.get(line).cloned().unwrap_or_default();
    let selected = range_in(block, line, text.len());
    // An empty line still needs a glyph to be laid out and hit.
    let shown = if text.is_empty() {
        " ".to_string()
    } else {
        text.clone()
    };
    let runs = merge(syntax.unwrap_or(&[]), selected.filter(|_| !text.is_empty()));
    let styled = StyledText::new(shown).with_highlights(runs);
    let lines = lines.clone();
    selectable(
        div().whitespace_nowrap(),
        block,
        Rc::new(move || lines.clone()),
        line,
        styled.layout().clone(),
    )
    .child(styled)
}

/// Paragraph `line` of `block`: wrapping text (a Markdown paragraph,
/// heading or cell) that selects like a code line, styled by `runs`. A
/// document's paragraphs are still being gathered as each is built, so
/// the list is read when a press starts a selection. The caller puts the
/// returned text (or a clickable wrapper of it) in the returned element.
pub fn paragraph(
    block: &str,
    lines: &Rc<RefCell<Vec<String>>>,
    line: usize,
    runs: &[(Range<usize>, HighlightStyle)],
) -> (Div, StyledText) {
    let text = lines.borrow().get(line).cloned().unwrap_or_default();
    let selected = range_in(block, line, text.len());
    let styled = StyledText::new(text).with_highlights(merge(runs, selected));
    let lines = lines.clone();
    let el = selectable(
        div(),
        block,
        Rc::new(move || Rc::new(lines.borrow().clone())),
        line,
        styled.layout().clone(),
    );
    (el, styled)
}

/// `el` holding line `line` of `block`, laid out as `layout`, made to
/// start and drag a selection. `lines` gives the block's text.
fn selectable(
    el: Div,
    block: &str,
    lines: Rc<dyn Fn() -> Rc<Vec<String>>>,
    line: usize,
    layout: TextLayout,
) -> Div {
    let hit = move |pos| match layout.index_for_position(pos) {
        Ok(i) | Err(i) => i,
    };
    let (block_down, hit_down) = (block.to_string(), hit.clone());
    let block_move = block.to_string();
    el.cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, _| {
                let lines_down = lines();
                let Some(text) = lines_down.get(line) else {
                    return;
                };
                let at = hit_down(event.position).min(text.len());
                let (anchor, head) = match event.click_count {
                    2 => {
                        let w = word(text, at);
                        ((line, w.start), (line, w.end))
                    }
                    n if n >= 3 => ((line, 0), (line, text.len())),
                    _ => ((line, at), (line, at)),
                };
                SELECTION.with(|s| {
                    *s.borrow_mut() = Some(Selection {
                        block: block_down.clone(),
                        lines: lines_down.clone(),
                        anchor,
                        head,
                        dragging: event.click_count < 2,
                    })
                });
                window.refresh();
            },
        )
        .on_mouse_move(move |event: &MouseMoveEvent, window, _| {
            if event.pressed_button != Some(MouseButton::Left) {
                SELECTION.with(|s| {
                    if let Some(s) = s.borrow_mut().as_mut() {
                        s.dragging = false;
                    }
                });
                return;
            }
            let moved = SELECTION.with(|s| {
                let mut s = s.borrow_mut();
                let Some(s) = s.as_mut().filter(|s| s.dragging && s.block == block_move) else {
                    return false;
                };
                let Some(text) = s.lines.get(line) else {
                    return false;
                };
                let at = hit(event.position).min(text.len());
                let changed = s.head != (line, at);
                s.head = (line, at);
                changed
            });
            if moved {
                window.refresh();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(word("let foo_bar = 1;", 6), 4..11);
        assert_eq!(word("a b", 1), 0..1);
        assert_eq!(word("a  b", 2), 2..2);
    }

    #[test]
    fn copies_across_lines() {
        let lines = Rc::new(vec![
            "fn main() {".to_string(),
            "    hi();".to_string(),
            "}".to_string(),
        ]);
        SELECTION.with(|s| {
            *s.borrow_mut() = Some(Selection {
                block: "b".into(),
                lines,
                anchor: (1, 4),
                head: (0, 3),
                dragging: false,
            })
        });
        assert_eq!(selected_text().as_deref(), Some("main() {\n    "));
        assert!(clear());
    }

    #[test]
    fn paragraphs_highlight_over_their_styles() {
        let bold = HighlightStyle {
            font_weight: Some(gpui::FontWeight::BOLD),
            ..Default::default()
        };
        let runs = merge(&[(0..4, bold)], Some(2..8));
        let pieces: Vec<_> = runs.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(pieces, vec![0..2, 2..4, 4..8]);
        assert!(runs[1].1.font_weight.is_some() && runs[1].1.background_color.is_some());
        assert!(runs[2].1.font_weight.is_none() && runs[2].1.background_color.is_some());
    }
}
