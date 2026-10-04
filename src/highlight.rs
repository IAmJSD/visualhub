//! Syntax highlighting with syntect, using bat's grammars (via two-face)
//! so the languages syntect lacks out of the box -- TypeScript, TOML,
//! Kotlin, Swift, Dockerfile and the rest -- are covered too.
//!
//! Only a theme's text colours are used; backgrounds stay the app's own,
//! so code reads the same in a file, a diff, or a Markdown block.
//! Results are kept per text, since screens render every frame.

use crate::ui::is_light;
use gpui::{rgb, HighlightStyle};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static THEMES: LazyLock<EmbeddedLazyThemeSet> = LazyLock::new(two_face::theme::extra);

/// Past this much text, highlighting costs more than it's worth.
const MAX_BYTES: usize = 1 << 20;
/// Minified lines make the grammars crawl.
const MAX_LINE: usize = 4000;

/// Each line's coloured runs, by byte range within the line.
pub type Lines = Rc<Vec<Vec<(Range<usize>, HighlightStyle)>>>;

/// The grammar for a file path ("src/main.rs", "Dockerfile") or a fence
/// tag ("rust", "ts", "shell").
pub fn syntax_for(name: &str) -> Option<&'static SyntaxReference> {
    let set = &*SYNTAXES;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let file = name.rsplit('/').next().unwrap_or(name);
    let ext = file.rsplit_once('.').map(|(_, e)| e);
    ext.and_then(|e| set.find_syntax_by_extension(e))
        .or_else(|| set.find_syntax_by_extension(file))
        .or_else(|| set.find_syntax_by_token(&name.to_lowercase()))
        .filter(|s| s.name != "Plain Text")
}

fn theme() -> &'static Theme {
    THEMES.get(if is_light() { EmbeddedThemeName::Github } else { EmbeddedThemeName::OneHalfDark })
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Lines>> = RefCell::new(HashMap::new());
}

/// Highlight `lines` (no newlines) as one run of `syntax`; `None` when the
/// text is too big or has no known grammar. `breaks` are line indices to
/// restart the parse at (a diff's hunks).
pub fn lines(syntax: Option<&'static SyntaxReference>, lines: &[String], breaks: &[usize]) -> Option<Lines> {
    let syntax = syntax?;
    let bytes: usize = lines.iter().map(String::len).sum();
    if bytes > MAX_BYTES || lines.iter().any(|l| l.len() > MAX_LINE) {
        return None;
    }
    let light = is_light();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (syntax.name.as_str(), light, lines, breaks).hash(&mut hasher);
    let key = hasher.finish();
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }

    let theme = theme();
    let plain = theme.settings.foreground;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if breaks.contains(&i) {
            highlighter = HighlightLines::new(syntax, theme);
        }
        let text = format!("{line}\n");
        let Ok(regions) = highlighter.highlight_line(&text, &SYNTAXES) else {
            return None;
        };
        let mut runs = Vec::new();
        let mut at = 0;
        for (style, piece) in regions {
            let end = (at + piece.len()).min(line.len());
            if end > at && Some(style.foreground) != plain {
                runs.push((at..end, color(style.foreground)));
            }
            at += piece.len();
        }
        out.push(runs);
    }
    let out: Lines = Rc::new(out);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    Some(out)
}

fn color(c: Color) -> HighlightStyle {
    HighlightStyle {
        color: Some(rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()),
        ..Default::default()
    }
}

/// One line as text with its colours.
pub fn styled(line: &str, runs: Option<&[(Range<usize>, HighlightStyle)]>) -> gpui::StyledText {
    let text = gpui::StyledText::new(line.to_string());
    match runs {
        Some(runs) if !runs.is_empty() => text.with_highlights(runs.iter().cloned()),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_grammars() {
        for name in ["src/main.rs", "a.ts", "b.tsx", "c.py", "Cargo.toml", "Dockerfile", "x.go", "y.kt", "z.swift", "rust", "js", "sh"] {
            assert!(syntax_for(name).is_some(), "{name}");
        }
        assert!(syntax_for("notes.unknownext").is_none());
    }

    #[test]
    fn colours_keywords() {
        let lines = lines(syntax_for("rust"), &["fn main() {}".to_string()], &[]).unwrap();
        assert!(!lines[0].is_empty());
    }
}
