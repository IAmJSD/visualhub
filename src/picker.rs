//! The filterable pickers behind the issue and pull request sidebars
//! (reviewers, assignees, labels, milestone): a title, a filter box, and
//! rows with avatars or label colours, ticked when chosen.
//!
//! A multi-choice picker stays open as things are ticked, the way
//! GitHub's does, and shows the new ticks straight away while the
//! requests land.

use crate::hub::{Act, Hub};
use crate::ui::{icon, palette};
use crate::widgets::{self, rgb};
use gpui::{
    anchored, deferred, div, prelude::FluentBuilder as _, px, AnyElement, Context, FontWeight,
    InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, Point,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use std::rc::Rc;

const FILTER: &str = "picker-filter";

pub struct PickItem {
    pub label: String,
    pub detail: String,
    pub avatar: Option<String>,
    /// A label's hex colour, drawn as a dot.
    pub color: Option<String>,
    pub checked: bool,
    /// What choosing it does, and (for a tick box) what unticking does.
    pub act: Act,
    pub undo: Option<Act>,
}

impl PickItem {
    pub fn new(label: impl Into<String>, checked: bool, act: Act) -> Self {
        PickItem {
            label: label.into(),
            detail: String::new(),
            avatar: None,
            color: None,
            checked,
            act,
            undo: None,
        }
    }

    /// A tick box: `add` when ticked, `remove` when unticked.
    pub fn toggle(label: impl Into<String>, checked: bool, add: Act, remove: Act) -> Self {
        let (act, undo) = if checked { (remove, add) } else { (add, remove) };
        PickItem { undo: Some(undo), ..PickItem::new(label, checked, act) }
    }

    pub fn avatar(mut self, url: impl Into<String>) -> Self {
        self.avatar = Some(url.into());
        self
    }

    pub fn color(mut self, hex: impl Into<String>) -> Self {
        self.color = Some(hex.into());
        self
    }

    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = text.into();
        self
    }
}

pub struct Picker {
    pub title: String,
    pub placeholder: String,
    pub items: Vec<PickItem>,
    /// More than one can be chosen; the picker stays open.
    pub multi: bool,
    /// Shown while the items are still loading.
    pub loading: bool,
}

impl Picker {
    pub fn new(title: impl Into<String>, placeholder: impl Into<String>, multi: bool) -> Self {
        Picker {
            title: title.into(),
            placeholder: placeholder.into(),
            items: Vec::new(),
            multi,
            loading: false,
        }
    }

    pub fn item(mut self, item: PickItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn act(self) -> Act {
        Act::Picker(Rc::new(self))
    }
}

pub struct PickerState {
    at: Point<Pixels>,
    picker: Rc<Picker>,
    /// Ticks as they stand now, by item.
    checked: Vec<bool>,
}

impl Hub {
    pub fn open_picker(&mut self, picker: Rc<Picker>, at: Point<Pixels>) {
        self.menu = None;
        self.set_field(FILTER, "");
        self.focus_field(FILTER);
        self.picker = Some(PickerState {
            at,
            checked: picker.items.iter().map(|i| i.checked).collect(),
            picker,
        });
    }

    pub fn close_picker(&mut self, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            self.blur_fields();
            cx.notify();
        }
    }

    /// The items the filter lets through, by index.
    fn picker_matches(&self) -> Vec<usize> {
        let Some(state) = &self.picker else { return Vec::new() };
        let query = self.field_text(FILTER).trim().to_lowercase();
        state
            .picker
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                query.is_empty() || item.label.to_lowercase().contains(&query) || item.detail.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.picker else { return };
        let Some(item) = state.picker.items.get(index) else { return };
        // Clicked again after a change: undo it.
        let act = match (&item.undo, state.checked[index] != item.checked) {
            (Some(undo), true) => undo.clone(),
            _ => item.act.clone(),
        };
        if state.picker.multi {
            state.checked[index] = !state.checked[index];
        } else {
            self.picker = None;
            self.blur_fields();
        }
        // `perform` closes menus, not pickers.
        self.perform(act, window, cx);
    }

    pub fn render_picker(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette();
        let (at, picker, checked) = {
            let state = self.picker.as_ref()?;
            (state.at, state.picker.clone(), state.checked.clone())
        };
        // Enter picks the first match.
        self.submits.insert(
            FILTER.into(),
            Act::run(|hub, window, cx| {
                if let Some(&first) = hub.picker_matches().first() {
                    hub.pick(first, window, cx);
                }
            }),
        );
        let matches = self.picker_matches();
        let filter = self.input(FILTER, &picker.placeholder, cx).w_full();

        let mut list = div().id("picker-list").flex().flex_col().max_h(px(320.0)).overflow_y_scroll().py_1();
        if picker.loading {
            list = list.child(widgets::loading());
        } else if matches.is_empty() {
            list = list.child(div().px_3().py_2().child(widgets::faint("Nothing matches.")));
        }
        for i in matches {
            let item = &picker.items[i];
            let leading: AnyElement = if let Some(url) = &item.avatar {
                self.avatar(url, 20.0, cx)
            } else if let Some(color) = &item.color {
                div().size(px(12.0)).mx(px(4.0)).rounded_full().bg(rgb(widgets::parse_hex(color))).into_any_element()
            } else {
                div().into_any_element()
            };
            list = list.child(
                div()
                    .id(("pick", i))
                    .flex()
                    .flex_row()
                    // A long list scrolls rather than squeezing its rows.
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .min_h(px(34.0))
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.hover)))
                    .on_click(cx.listener(move |hub, _, window, cx| hub.pick(i, window, cx)))
                    .child(
                        div()
                            .w(px(16.0))
                            .flex_none()
                            .when(checked[i], |d| d.child(icon("check", 14.0, p.accent))),
                    )
                    .child(leading)
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::SEMIBOLD).text_ellipsis().overflow_hidden().whitespace_nowrap().child(item.label.clone()))
                            .when(!item.detail.is_empty(), |d| {
                                d.child(div().text_size(px(12.0)).text_color(rgb(p.text_dim)).text_ellipsis().overflow_hidden().whitespace_nowrap().child(item.detail.clone()))
                            }),
                    ),
            );
        }

        let popover = crate::ui::Popover::new("picker")
            .in_flow()
            .w(px(300.0))
            .text_size(px(13.0))
            .on_dismiss(cx.listener(|hub, _, _, cx| hub.close_picker(cx)))
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(picker.title.clone()),
            )
            .child(div().px_2().pb_2().border_b_1().border_color(rgb(p.divider)).child(filter))
            .child(list);
        Some(
            deferred(anchored().position(at).snap_to_window_with_margin(px(8.0)).child(popover))
                .with_priority(2)
                .into_any_element(),
        )
    }
}
