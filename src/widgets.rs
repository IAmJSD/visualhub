//! Small pieces of GitHub-flavoured chrome built on `schist-ui`: state
//! pills, label chips, underline tabs, cards, and the placeholders a page
//! shows while its data is on the way.
//!
//! Anything Schist's kit already has (buttons, fields, chips, menus,
//! modals, spinners) is used from there; these are the things a code host
//! needs that an image editor does not.

use crate::hub::{on, Act, Load};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
};
use schist_ui::{icon, is_light, palette, Button, Spinner};

#[cfg(target_os = "windows")]
pub const MONO: &str = "Consolas";
#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const MONO: &str = "DejaVu Sans Mono";

/// Body text size. A touch larger than Schist's panel text: these pages
/// are read, not glanced at.
pub const TEXT: f32 = 13.0;

pub fn rgb(hex: u32) -> gpui::Rgba {
    gpui::rgb(hex)
}

pub fn green() -> u32 {
    if is_light() {
        0x1A7F37
    } else {
        0x3FB950
    }
}

pub fn red() -> u32 {
    if is_light() {
        0xCF222E
    } else {
        0xF85149
    }
}

pub fn purple() -> u32 {
    if is_light() {
        0x8250DF
    } else {
        0xA371F7
    }
}

pub fn yellow() -> u32 {
    if is_light() {
        0x9A6700
    } else {
        0xD29922
    }
}

pub fn gray() -> u32 {
    palette().text_dim
}

/// Fills behind white text for the state pills.
pub fn green_fill() -> u32 {
    if is_light() {
        0x1F883D
    } else {
        0x238636
    }
}

pub fn red_fill() -> u32 {
    if is_light() {
        0xCF222E
    } else {
        0xDA3633
    }
}

pub fn purple_fill() -> u32 {
    if is_light() {
        0x8250DF
    } else {
        0x8957E5
    }
}

pub fn gray_fill() -> u32 {
    0x6E7681
}

/// A hex colour string ("d73a4a", "#d73a4a") as a number.
pub fn parse_hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0x888888)
}

/// Black or white, whichever reads on `bg`.
pub fn contrast(bg: u32) -> u32 {
    let (r, g, b) = ((bg >> 16) & 0xFF, (bg >> 8) & 0xFF, bg & 0xFF);
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 150.0 {
        0x1F2328
    } else {
        0xFFFFFF
    }
}

// -- text ----------------------------------------------------------------

pub fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(20.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette().text))
        .child(text.into())
}

pub fn h2(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(15.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette().text))
        .child(text.into())
}

pub fn h3(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette().text))
        .child(text.into())
}

pub fn dim(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(rgb(palette().text_dim))
        .child(text.into())
}

pub fn faint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.0))
        .text_color(rgb(palette().text_faint))
        .child(text.into())
}

pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_size(px(12.0)).child(text.into())
}

/// An icon with text after it, both in `color`.
pub fn icon_text(name: &str, text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .text_color(rgb(color))
        .child(icon(name, 14.0, color))
        .child(text.into())
}

// -- containers ----------------------------------------------------------

/// A bordered panel.
pub fn card() -> Div {
    let p = palette();
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.edge))
        .bg(rgb(p.panel_bg))
        .overflow_hidden()
}

/// A card's header strip.
pub fn card_header() -> Div {
    let p = palette();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(rgb(p.deep_bg))
        .border_b_1()
        .border_color(rgb(p.divider))
}

/// A page's column: centred, with a readable maximum width.
pub fn page() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .w_full()
        .max_w(px(1180.0))
        .mx_auto()
        .px_6()
        .py_5()
}

pub fn row() -> Div {
    div().flex().flex_row().items_center().gap_2()
}

pub fn col() -> Div {
    div().flex().flex_col().gap_2()
}

pub fn spacer() -> Div {
    div().flex_1()
}

// -- marks ---------------------------------------------------------------

/// A rounded pill with an icon: "Open", "Merged", "Draft".
pub fn pill(icon_name: &str, text: impl Into<SharedString>, fill: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap_1()
        .px_3()
        .py(px(5.0))
        .rounded_full()
        .bg(rgb(fill))
        .text_color(rgb(0xFFFFFF))
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .child(icon(icon_name, 14.0, 0xFFFFFF))
        .child(text.into())
}

/// A repository label, in its own colour.
pub fn label_chip(name: &str, color: &str) -> Div {
    let bg = parse_hex(color);
    div()
        .flex_none()
        .px_2()
        .py(px(1.0))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(contrast(bg)))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .child(name.to_string())
}

/// A small outlined tag: "Public", "Archived", "Latest".
pub fn tag(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .border_1()
        .border_color(rgb(color))
        .text_color(rgb(color))
        .text_size(px(11.0))
        .child(text.into())
}

/// A count bubble beside a tab or a heading.
pub fn counter(n: i64) -> Div {
    let p = palette();
    div()
        .flex_none()
        .px(px(6.0))
        .rounded_full()
        .bg(rgb(p.control_bg))
        .text_size(px(11.0))
        .text_color(rgb(p.text))
        .child(crate::json::count(n))
}

// -- controls ------------------------------------------------------------

/// A Schist button that performs an act.
pub fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>, act: Act) -> Button {
    Button::new(id, label).h(px(28.0)).on_click(on(act))
}

/// A button with an icon before its label.
pub fn ibtn(
    id: impl Into<ElementId>,
    icon_name: &str,
    label: impl Into<SharedString>,
    act: Act,
) -> Button {
    let p = palette();
    Button::bare(id)
        .h(px(28.0))
        .gap_2()
        .child(icon(icon_name, 14.0, p.text))
        .child(label.into())
        .on_click(on(act))
}

/// A primary (accent) button.
pub fn primary(id: impl Into<ElementId>, label: impl Into<SharedString>, act: Act) -> Button {
    Button::new(id, label).primary().h(px(28.0)).on_click(on(act))
}

/// A red-text button for destructive acts.
pub fn danger(id: impl Into<ElementId>, label: impl Into<SharedString>, act: Act) -> Button {
    let p = palette();
    Button::new(id, label)
        .colors(schist_ui::ButtonColors {
            bg: Some(p.button_bg),
            hover: red_fill(),
            text: red(),
            border: None,
        })
        .h(px(28.0))
        .on_click(on(act))
}

/// A green "positive" button: Merge, New.
pub fn go_btn(id: impl Into<ElementId>, label: impl Into<SharedString>, act: Act) -> Button {
    Button::new(id, label)
        .colors(schist_ui::ButtonColors {
            bg: Some(green_fill()),
            hover: if is_light() { 0x1A7F37 } else { 0x2EA043 },
            text: 0xFFFFFF,
            border: None,
        })
        .h(px(28.0))
        .on_click(on(act))
}

/// A row of filter chips, one selected.
pub fn chips(items: Vec<(String, bool, Act)>) -> Div {
    let mut el = div().flex().flex_row().flex_wrap().gap_1();
    for (i, (label, selected, act)) in items.into_iter().enumerate() {
        el = el.child(
            schist_ui::Chip::new(("chip", i), label)
                .selected(selected)
                .h(px(24.0))
                .px_3()
                .text_size(px(12.0))
                .on_click(on(act)),
        );
    }
    el
}

/// One underline tab.
pub struct TabItem {
    pub label: String,
    pub icon: &'static str,
    pub count: Option<i64>,
    pub selected: bool,
    pub act: Act,
}

impl TabItem {
    pub fn new(label: impl Into<String>, icon: &'static str, selected: bool, act: Act) -> Self {
        TabItem {
            label: label.into(),
            icon,
            count: None,
            selected,
            act,
        }
    }

    pub fn count(mut self, n: Option<i64>) -> Self {
        self.count = n;
        self
    }
}

/// GitHub's underline tab strip.
pub fn tabs(id: &str, items: Vec<TabItem>) -> AnyElement {
    let p = palette();
    let mut strip = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_1()
        .border_b_1()
        .border_color(rgb(p.divider));
    for (i, item) in items.into_iter().enumerate() {
        let color = if item.selected { p.text } else { p.text_dim };
        strip = strip.child(
            div()
                .id(ElementId::Name(format!("{id}-tab-{i}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .pt_1()
                .pb(px(7.0))
                .border_b_2()
                .border_color(if item.selected {
                    rgb(0xFD8C73)
                } else {
                    gpui::transparent_black().into()
                })
                .text_size(px(TEXT))
                .text_color(rgb(color))
                .when(item.selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                .cursor_pointer()
                .hover(|s| s.text_color(rgb(p.text)))
                .child(icon(item.icon, 14.0, color))
                .child(item.label)
                .when_some(item.count.filter(|n| *n > 0), |d, n| d.child(counter(n)))
                .on_click(on(item.act)),
        );
    }
    strip.into_any_element()
}

/// A clickable list row: hover fill, pointer, performs `act`.
pub fn list_row(id: impl Into<ElementId>, act: Act) -> gpui::Stateful<Div> {
    let p = palette();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_start()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(p.divider))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(p.hover)))
        .on_click(on(act))
}

// -- placeholders --------------------------------------------------------

pub fn loading() -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .text_color(rgb(palette().text_dim))
        .child(Spinner::new("loading").size(16.0))
        .child("Loading…")
        .into_any_element()
}

pub fn error_box(message: &str) -> AnyElement {
    let p = palette();
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_2()
        .m_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(red()))
        .text_color(rgb(p.text))
        .child(icon("alert", 16.0, red()))
        .child(div().flex_1().child(message.to_string()))
        .into_any_element()
}

pub fn empty(message: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .p_8()
        .text_color(rgb(palette().text_dim))
        .child(icon("inbox", 24.0, palette().text_faint))
        .child(message.into())
        .into_any_element()
}

/// What a page shows in place of data that is not here yet.
pub fn placeholder(load: &Load) -> AnyElement {
    match load {
        Load::Loading => loading(),
        Load::Failed(error) => error_box(error),
        Load::Ready(_) => div().into_any_element(),
    }
}

/// A single-series bar chart: one hue, thin bars with a 2px gap and
/// rounded tops on a shared baseline, a tooltip per bar, and the peak and
/// total written in text colours beside it (so nothing rests on colour).
pub fn bar_chart(id: &str, title: &str, bars: Vec<(String, i64)>, height: f32) -> AnyElement {
    let p = palette();
    let max = bars.iter().map(|b| b.1).max().unwrap_or(0).max(1);
    let total: i64 = bars.iter().map(|b| b.1).sum();
    let mut plot = div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(2.0))
        .h(px(height))
        .border_b_1()
        .border_color(rgb(p.divider));
    for (i, (label, value)) in bars.iter().enumerate() {
        let frac = (*value as f32 / max as f32).max(if *value > 0 { 0.02 } else { 0.0 });
        let tip = format!("{label}: {value}");
        plot = plot.child(
            div()
                .id(ElementId::Name(format!("{id}-bar-{i}").into()))
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .hover(|s| s.bg(rgb(p.hover)))
                .tooltip(schist_ui::tip(tip, None))
                .child(
                    div()
                        .w_full()
                        .h(gpui::relative(frac))
                        .rounded_t(px(4.0))
                        .bg(rgb(p.accent)),
                ),
        );
    }
    let first = bars.first().map(|b| b.0.clone()).unwrap_or_default();
    let last = bars.last().map(|b| b.0.clone()).unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            row()
                .child(h3(title.to_string()))
                .child(spacer())
                .child(dim(format!("total {total}  ·  peak {max}"))),
        )
        .child(plot)
        .child(row().child(faint(first)).child(spacer()).child(faint(last)))
        .into_any_element()
}
