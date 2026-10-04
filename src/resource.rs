//! Lists of GitHub things, described rather than hand-built.
//!
//! Collaborators, webhooks, deploy keys, secrets, releases, branches,
//! followers, notifications... most of GitHub is a paged list of objects,
//! each with a title, a line of detail, somewhere to go and a few things
//! to do. A [`ListSpec`] says where the list comes from and how one item
//! reads as a [`Row`]; [`Hub::list`] does the fetching, paging, drawing
//! and the row actions.

use crate::hub::{on, with_query, Act, Hub, Load, MenuEntry};
use crate::json;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, IntoElement as _, ParentElement as _,
    SharedString, Styled as _,
};
use crate::ui::{icon, palette, Button, IconButton};
use serde_json::Value;
use std::rc::Rc;

pub const PER_PAGE: usize = 30;

pub struct RowAct {
    pub label: String,
    pub act: Act,
    pub danger: bool,
}

/// How one item reads in a list.
pub struct Row {
    pub icon: Option<(&'static str, u32)>,
    pub avatar: Option<String>,
    pub title: String,
    pub suffix: String,
    pub labels: Vec<(String, String)>,
    pub tags: Vec<(String, u32)>,
    pub meta: String,
    pub body: String,
    pub right: String,
    pub open: Act,
    pub actions: Vec<RowAct>,
    /// Inline buttons rather than a ⋯ menu, for one or two actions.
    pub inline: bool,
}

impl Row {
    pub fn new(title: impl Into<String>) -> Self {
        Row {
            icon: None,
            avatar: None,
            title: title.into(),
            suffix: String::new(),
            labels: Vec::new(),
            tags: Vec::new(),
            meta: String::new(),
            body: String::new(),
            right: String::new(),
            open: Act::None,
            actions: Vec::new(),
            inline: false,
        }
    }

    pub fn icon(mut self, name: &'static str, color: u32) -> Self {
        self.icon = Some((name, color));
        self
    }

    pub fn avatar(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.is_empty() {
            self.avatar = Some(url);
        }
        self
    }

    /// The item's GitHub labels (`[{name, color}]`).
    pub fn labels(mut self, labels: &[Value]) -> Self {
        use crate::json::Json as _;
        self.labels = labels
            .iter()
            .map(|l| (l.s("name"), l.s("color")))
            .collect();
        self
    }

    pub fn tag(mut self, text: impl Into<String>, color: u32) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.tags.push((text, color));
        }
        self
    }

    pub fn meta(mut self, s: impl Into<String>) -> Self {
        self.meta = s.into();
        self
    }

    pub fn body(mut self, s: impl Into<String>) -> Self {
        self.body = s.into();
        self
    }

    pub fn right(mut self, s: impl Into<String>) -> Self {
        self.right = s.into();
        self
    }

    pub fn open(mut self, act: Act) -> Self {
        self.open = act;
        self
    }

    pub fn action(mut self, label: impl Into<String>, act: Act) -> Self {
        self.actions.push(RowAct {
            label: label.into(),
            act,
            danger: false,
        });
        self
    }

    pub fn danger(mut self, label: impl Into<String>, act: Act) -> Self {
        self.actions.push(RowAct {
            label: label.into(),
            act,
            danger: true,
        });
        self
    }

    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }
}

pub type RowFn = Rc<dyn Fn(&Value) -> Row>;

/// Where a list comes from and how its items read.
#[derive(Clone)]
pub struct ListSpec {
    pub id: String,
    pub path: String,
    pub items: Option<&'static str>,
    pub paged: bool,
    pub empty: String,
    pub row: RowFn,
    pub filter: Option<Rc<dyn Fn(&Value) -> bool>>,
}

impl ListSpec {
    pub fn new(path: impl Into<String>, row: impl Fn(&Value) -> Row + 'static) -> Self {
        let path = path.into();
        ListSpec {
            id: path.clone(),
            path,
            items: None,
            paged: true,
            empty: "Nothing here yet.".into(),
            row: Rc::new(row),
            filter: None,
        }
    }

    /// The array is under this key (`items`, `workflow_runs`, ...).
    pub fn items(mut self, key: &'static str) -> Self {
        self.items = Some(key);
        self
    }

    /// One request, no paging (the endpoint does not page).
    pub fn unpaged(mut self) -> Self {
        self.paged = false;
        self
    }

    pub fn empty(mut self, message: impl Into<String>) -> Self {
        self.empty = message.into();
        self
    }

    pub fn filter(mut self, f: impl Fn(&Value) -> bool + 'static) -> Self {
        self.filter = Some(Rc::new(f));
        self
    }
}

pub enum Fetched {
    /// Everything loaded so far, whether another page is loading, and
    /// whether there is a further page to ask for.
    Items {
        items: Vec<Value>,
        loading: bool,
        more: bool,
    },
    Failed(Rc<str>),
}

impl Hub {
    /// The items of every page of a list showing so far.
    pub fn fetch_list(&mut self, spec: &ListSpec, cx: &mut Context<Self>) -> Fetched {
        let pages = if spec.paged { self.page(&spec.id) } else { 1 };
        let mut items = Vec::new();
        let mut more = false;
        for page in 1..=pages {
            let path = if spec.paged {
                with_query(&spec.path, &format!("per_page={PER_PAGE}&page={page}"))
            } else {
                spec.path.clone()
            };
            match self.fetch(&path, cx) {
                Load::Ready(value) => {
                    let page_items = json::items(&value, spec.items);
                    more = spec.paged && page_items.len() >= PER_PAGE;
                    items.extend(
                        page_items
                            .iter()
                            .filter(|v| spec.filter.as_ref().is_none_or(|f| f(v)))
                            .cloned(),
                    );
                }
                Load::Loading => {
                    return Fetched::Items {
                        items,
                        loading: true,
                        more: false,
                    }
                }
                Load::Failed(error) if page == 1 => return Fetched::Failed(error),
                Load::Failed(_) => break,
            }
        }
        Fetched::Items {
            items,
            loading: false,
            more,
        }
    }

    /// A list in a card, with paging and row actions.
    pub fn list(&mut self, spec: &ListSpec, cx: &mut Context<Self>) -> AnyElement {
        let (items, loading, more) = match self.fetch_list(spec, cx) {
            Fetched::Items {
                items,
                loading,
                more,
            } => (items, loading, more),
            Fetched::Failed(error) => return widgets::error_box(&error),
        };
        let mut card = widgets::card();
        if items.is_empty() && !loading {
            return card.child(widgets::empty(spec.empty.clone())).into_any_element();
        }
        for (i, item) in items.iter().enumerate() {
            let row = (spec.row)(item);
            card = card.child(self.render_row(&format!("{}#{i}", spec.id), row, cx));
        }
        if loading {
            card = card.child(widgets::loading());
        } else if more {
            let id = spec.id.clone();
            card = card.child(
                div().flex().justify_center().p_2().child(
                    Button::new(ElementId::Name(format!("{id}-more").into()), "Load more")
                        .h(px(28.0))
                        .on_click(cx.listener(move |hub, _, _, cx| {
                            let next = hub.page(&id) + 1;
                            hub.pages.insert(id.clone(), next);
                            cx.notify();
                        })),
                ),
            );
        }
        card.into_any_element()
    }

    pub fn render_row(&mut self, id: &str, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let leading: Option<AnyElement> = if let Some(url) = &row.avatar {
            Some(self.avatar(url, 20.0, cx))
        } else {
            row.icon.map(|(name, color)| {
                div()
                    .pt(px(2.0))
                    .child(icon(name, 16.0, color))
                    .into_any_element()
            })
        };
        let title = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(widgets::TEXT))
                    .text_color(rgb(p.text))
                    .child(row.title.clone()),
            )
            .when(!row.suffix.is_empty(), |d| {
                d.child(widgets::dim(row.suffix.clone()))
            })
            .children(
                row.labels
                    .iter()
                    .map(|(name, color)| widgets::label_chip(name, color)),
            )
            .children(row.tags.iter().map(|(t, c)| widgets::tag(t.clone(), *c)));
        let middle = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(title)
            .when(!row.meta.is_empty(), |d| d.child(widgets::dim(row.meta.clone())))
            .when(!row.body.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(p.text))
                        .child(json::clip(&row.body, 240)),
                )
            });
        let mut trailing = div().flex().flex_row().flex_none().items_center().gap_1();
        if !row.right.is_empty() {
            trailing = trailing.child(widgets::dim(row.right.clone()).pr_1());
        }
        if row.inline || row.actions.len() <= 1 {
            for (i, action) in row.actions.into_iter().enumerate() {
                let mut button = Button::new(
                    ElementId::Name(format!("{id}-act-{i}").into()),
                    action.label,
                )
                .h(px(24.0))
                .text_size(px(12.0))
                .consume_press()
                .on_click(on(action.act));
                if action.danger {
                    button = button.colors(crate::ui::ButtonColors {
                        bg: Some(p.button_bg),
                        hover: widgets::red_fill(),
                        text: widgets::red(),
                        border: None,
                    });
                }
                trailing = trailing.child(button);
            }
        } else {
            let mut entries = Vec::new();
            for action in row.actions {
                if action.danger && !entries.is_empty() {
                    entries.push(MenuEntry::Sep);
                }
                entries.push(MenuEntry::item(action.label, action.act));
            }
            trailing = trailing.child(
                IconButton::new(ElementId::Name(format!("{id}-menu").into()), "kebab")
                    .size(26.0)
                    .icon_size(16.0)
                    .consume_press()
                    .on_click(on(Act::menu(entries))),
            );
        }
        let clickable = !matches!(row.open, Act::None);
        let mut el = widgets::list_row(ElementId::Name(SharedString::from(id.to_string())), row.open)
            .items_center();
        if !clickable {
            el = el.cursor_default();
        }
        el.children(leading)
            .child(middle)
            .child(trailing)
            .into_any_element()
    }
}
