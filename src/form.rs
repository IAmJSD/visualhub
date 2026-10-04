//! Dialog forms, described rather than hand-built.
//!
//! Most of what GitHub lets you create or edit -- a repository, a label, a
//! webhook, a release, a key -- is a handful of fields sent as one JSON
//! body. A [`FormSpec`] lists the fields and the request, and the same
//! code draws the dialog, reads it back, checks it and sends it. Field
//! keys may be dotted (`config.url`) to build nested bodies.

use crate::hub::{Act, Hub, Modal, Req, Then};
use crate::json::Json as _;
use crate::picker::{PickItem, Picker};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, InteractiveElement as _, IntoElement as _, ParentElement as _, StatefulInteractiveElement as _, SharedString,
    Styled as _,
};
use crate::ui::{palette, Button, Checkbox, DropdownButton, LineEdit, TextInput, TextPress};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone)]
pub enum Kind {
    Text,
    /// Masked while typed: tokens, webhook secrets, Actions secrets.
    Secret,
    Multiline,
    Bool,
    /// One of a few values: `(value, label)`.
    Choice(Vec<(String, String)>),
    Number,
    /// Comma- or line-separated, sent as an array of strings.
    List,
    /// Raw JSON, sent as parsed.
    Json,
    /// Picked from a list the API gives: one, or several.
    Dropdown(Rc<Dropdown>),
}

/// Where a dropdown field's choices come from and how each reads.
pub struct Dropdown {
    pub path: String,
    pub multi: bool,
    /// The choice offered for "none" in a one-pick dropdown.
    pub none: Option<String>,
    pub option: Rc<dyn Fn(&Value) -> DropOption>,
}

/// One choice: what is sent, and how it is shown.
pub struct DropOption {
    pub value: Value,
    pub label: String,
    pub detail: String,
    pub avatar: Option<String>,
    /// A label's hex colour.
    pub color: Option<String>,
}

impl DropOption {
    pub fn new(value: impl Into<Value>, label: impl Into<String>) -> Self {
        DropOption {
            value: value.into(),
            label: label.into(),
            detail: String::new(),
            avatar: None,
            color: None,
        }
    }

    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = text.into();
        self
    }

    pub fn avatar(mut self, url: impl Into<String>) -> Self {
        self.avatar = Some(url.into());
        self
    }

    pub fn color(mut self, hex: impl Into<String>) -> Self {
        self.color = Some(hex.into());
        self
    }
}

/// A dropdown's picks as they are kept: one JSON value per line.
fn picks(text: &str) -> Vec<Value> {
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[derive(Clone)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub value: String,
    pub on: bool,
    pub required: bool,
    pub hint: Option<String>,
    /// Send `""` for an empty box rather than leaving the key out (to clear
    /// a value on an edit).
    pub keep_empty: bool,
}

impl Field {
    fn new(key: &str, label: &str, kind: Kind) -> Self {
        Field {
            key: key.to_string(),
            label: label.to_string(),
            kind,
            value: String::new(),
            on: false,
            required: false,
            hint: None,
            keep_empty: false,
        }
    }

    pub fn text(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::Text)
    }

    pub fn secret(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::Secret)
    }

    pub fn multiline(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::Multiline)
    }

    pub fn number(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::Number)
    }

    pub fn list(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::List)
    }

    pub fn json(key: &str, label: &str) -> Self {
        Self::new(key, label, Kind::Json)
    }

    pub fn bool(key: &str, label: &str, on: bool) -> Self {
        let mut field = Self::new(key, label, Kind::Bool);
        field.on = on;
        field
    }

    pub fn choice(key: &str, label: &str, options: &[(&str, &str)]) -> Self {
        let options: Vec<(String, String)> = options
            .iter()
            .map(|(v, l)| (v.to_string(), l.to_string()))
            .collect();
        let mut field = Self::new(key, label, Kind::Choice(options.clone()));
        field.value = options.first().map(|o| o.0.clone()).unwrap_or_default();
        field
    }

    /// Several of the items at `path`, each read by `option`.
    pub fn dropdown(key: &str, label: &str, path: impl Into<String>, option: impl Fn(&Value) -> DropOption + 'static) -> Self {
        Self::new(
            key,
            label,
            Kind::Dropdown(Rc::new(Dropdown { path: path.into(), multi: true, none: None, option: Rc::new(option) })),
        )
    }

    /// One of the items at `path`, or `none`.
    pub fn dropdown_one(key: &str, label: &str, path: impl Into<String>, none: &str, option: impl Fn(&Value) -> DropOption + 'static) -> Self {
        Self::new(
            key,
            label,
            Kind::Dropdown(Rc::new(Dropdown { path: path.into(), multi: false, none: Some(none.to_string()), option: Rc::new(option) })),
        )
    }

    /// A choice among values that are their own labels.
    pub fn pick(key: &str, label: &str, options: Vec<String>) -> Self {
        let options: Vec<(String, String)> = options.into_iter().map(|o| (o.clone(), o)).collect();
        let mut field = Self::new(key, label, Kind::Choice(options.clone()));
        field.value = options.first().map(|o| o.0.clone()).unwrap_or_default();
        field
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn keep_empty(mut self) -> Self {
        self.keep_empty = true;
        self
    }

    fn is_text(&self) -> bool {
        matches!(
            self.kind,
            Kind::Text | Kind::Secret | Kind::Multiline | Kind::Number | Kind::List | Kind::Json
        )
    }

    fn is_paragraph(&self) -> bool {
        matches!(self.kind, Kind::Multiline | Kind::Json)
    }
}

type Build = Rc<dyn Fn(&FormValues) -> Result<Act, String>>;
type MapBody = Rc<dyn Fn(Value, &FormValues) -> Value>;

pub struct FormSpec {
    pub title: String,
    pub submit: String,
    pub fields: Vec<Field>,
    pub note: Option<String>,
    pub danger: bool,
    pub width: f32,
    method: &'static str,
    path: Option<String>,
    extra: Option<Value>,
    map: Option<MapBody>,
    gql: Option<(String, Rc<dyn Fn(&FormValues) -> Value>)>,
    ok: String,
    inval: Vec<String>,
    then: Option<Then>,
    custom: Option<Build>,
}

impl FormSpec {
    pub fn new(title: impl Into<String>) -> Self {
        FormSpec {
            title: title.into(),
            submit: "Save".into(),
            fields: Vec::new(),
            note: None,
            danger: false,
            width: 520.0,
            method: "POST",
            path: None,
            extra: None,
            map: None,
            gql: None,
            ok: String::new(),
            inval: Vec::new(),
            then: None,
            custom: None,
        }
    }

    pub fn submit(mut self, label: impl Into<String>) -> Self {
        self.submit = label.into();
        self
    }

    pub fn field(mut self, field: Field) -> Self {
        self.fields.push(field);
        self
    }

    pub fn fields(mut self, fields: impl IntoIterator<Item = Field>) -> Self {
        self.fields.extend(fields);
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Send the fields as a JSON body.
    pub fn rest(mut self, method: &'static str, path: impl Into<String>) -> Self {
        self.method = method;
        self.path = Some(path.into());
        self
    }

    /// Fixed keys merged into the body.
    pub fn extra(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }

    /// Reshape the body before it is sent.
    pub fn map(mut self, f: impl Fn(Value, &FormValues) -> Value + 'static) -> Self {
        self.map = Some(Rc::new(f));
        self
    }

    /// Send a GraphQL mutation built from the values instead.
    pub fn gql(mut self, query: &str, vars: impl Fn(&FormValues) -> Value + 'static) -> Self {
        self.gql = Some((query.to_string(), Rc::new(vars)));
        self
    }

    pub fn ok(mut self, message: impl Into<String>) -> Self {
        self.ok = message.into();
        self
    }

    pub fn inval(mut self, prefix: impl Into<String>) -> Self {
        self.inval.push(prefix.into());
        self
    }

    pub fn then(mut self, f: impl Fn(&mut Hub, &Value, &mut Context<Hub>) + 'static) -> Self {
        self.then = Some(Rc::new(f));
        self
    }

    /// Decide what submitting does from the values, for forms that are
    /// not one request.
    pub fn build_with(mut self, f: impl Fn(&FormValues) -> Result<Act, String> + 'static) -> Self {
        self.custom = Some(Rc::new(f));
        self
    }

    pub fn act(self) -> Act {
        Act::Form(Rc::new(self))
    }

    /// What submitting these values does, or why they cannot be sent.
    pub fn build(&self, values: &FormValues) -> Result<Act, String> {
        for field in &self.fields {
            if field.required && field.is_text() && values.s(&field.key).trim().is_empty() {
                return Err(format!("{} is required.", field.label));
            }
        }
        if let Some(custom) = &self.custom {
            return custom(values);
        }
        let mut req = if let Some((query, vars)) = &self.gql {
            Req::gql(query, vars(values))
        } else {
            let path = self.path.clone().ok_or("This form has nowhere to send to.")?;
            let mut body = values.json(&self.fields)?;
            if let (Some(Value::Object(extra)), Value::Object(map)) = (&self.extra, &mut body) {
                for (k, v) in extra {
                    map.insert(k.clone(), v.clone());
                }
            }
            if let Some(f) = &self.map {
                body = f(body, values);
            }
            Req::rest(self.method, path).body(body)
        };
        req.ok = self.ok.clone();
        req.invalidate = self.inval.clone();
        req.then = self.then.clone();
        Ok(req.act())
    }
}

/// A submitted form's values, by field key.
pub struct FormValues {
    text: HashMap<String, String>,
    bools: HashMap<String, bool>,
}

impl FormValues {
    pub fn read(hub: &Hub, spec: &FormSpec) -> Self {
        let mut text = HashMap::new();
        let mut bools = HashMap::new();
        for field in &spec.fields {
            let id = format!("form.{}", field.key);
            match &field.kind {
                Kind::Bool => {
                    bools.insert(field.key.clone(), hub.toggle(&id));
                }
                Kind::Choice(_) | Kind::Dropdown(_) => {
                    text.insert(field.key.clone(), hub.choice(&id, &field.value));
                }
                _ => {
                    text.insert(field.key.clone(), hub.field_text(&id));
                }
            }
        }
        FormValues { text, bools }
    }

    pub fn s(&self, key: &str) -> String {
        self.text.get(key).cloned().unwrap_or_default()
    }

    pub fn b(&self, key: &str) -> bool {
        self.bools.get(key).copied().unwrap_or(false)
    }

    /// The comma/line separated items of a list field.
    pub fn items(&self, key: &str) -> Vec<String> {
        split_list(&self.s(key))
    }

    /// The values as a JSON body.
    pub fn json(&self, fields: &[Field]) -> Result<Value, String> {
        let mut body = Value::Object(Map::new());
        for field in fields {
            let value = match &field.kind {
                Kind::Bool => Value::Bool(self.b(&field.key)),
                Kind::Number => {
                    let text = self.s(&field.key);
                    if text.trim().is_empty() {
                        continue;
                    }
                    match text.trim().parse::<i64>() {
                        Ok(n) => Value::from(n),
                        Err(_) => return Err(format!("{} must be a number.", field.label)),
                    }
                }
                Kind::List => {
                    let items = self.items(&field.key);
                    if items.is_empty() && !field.keep_empty {
                        continue;
                    }
                    Value::Array(items.into_iter().map(Value::String).collect())
                }
                Kind::Json => {
                    let text = self.s(&field.key);
                    if text.trim().is_empty() {
                        continue;
                    }
                    serde_json::from_str(&text)
                        .map_err(|e| format!("{} is not valid JSON: {e}", field.label))?
                }
                Kind::Dropdown(dropdown) => {
                    let mut chosen = picks(&self.s(&field.key));
                    if chosen.is_empty() && !field.keep_empty {
                        continue;
                    }
                    if dropdown.multi {
                        Value::Array(chosen)
                    } else {
                        chosen.pop().unwrap_or(Value::Null)
                    }
                }
                _ => {
                    let text = self.s(&field.key);
                    if text.is_empty() && !field.keep_empty {
                        continue;
                    }
                    Value::String(text)
                }
            };
            insert_dotted(&mut body, &field.key, value);
        }
        Ok(body)
    }
}

pub fn split_list(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn insert_dotted(body: &mut Value, key: &str, value: Value) {
    let mut here = body;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        let Value::Object(map) = here else {
            return;
        };
        if i == parts.len() - 1 {
            map.insert(part.to_string(), value);
            return;
        }
        here = map
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

/// Fill the dialog's boxes with the spec's starting values and give the
/// first one the keyboard.
pub fn reset(hub: &mut Hub, spec: &FormSpec) {
    let mut first = None;
    for field in &spec.fields {
        let id = format!("form.{}", field.key);
        match &field.kind {
            Kind::Bool => {
                hub.toggles.insert(id, field.on);
            }
            Kind::Choice(_) | Kind::Dropdown(_) => {
                hub.choices.insert(id, field.value.clone());
            }
            _ => {
                let mut edit = if field.is_paragraph() {
                    LineEdit::multiline()
                } else {
                    LineEdit::default()
                };
                edit.set_text(field.value.clone());
                hub.fields.insert(id.clone(), edit);
                first.get_or_insert(id);
            }
        }
    }
    if let Some(id) = first {
        hub.focus_field(&id);
    }
}

/// Tab and Shift+Tab between a dialog's boxes.
pub fn tab(hub: &mut Hub, from: &str, back: bool) {
    let Some(Modal::Form { spec, .. }) = &hub.modal else {
        return;
    };
    let ids: Vec<String> = spec
        .fields
        .iter()
        .filter(|f| f.is_text())
        .map(|f| format!("form.{}", f.key))
        .collect();
    let Some(at) = ids.iter().position(|id| id == from) else {
        return;
    };
    let next = if back {
        (at + ids.len() - 1) % ids.len()
    } else {
        (at + 1) % ids.len()
    };
    let id = ids[next].clone();
    hub.focus_field(&id);
}

// ---------------------------------------------------------------------------
// Drawing.

impl Hub {
    fn text_box(&mut self, id: &str, placeholder: &str, cx: &mut Context<Self>) -> TextInput {
        let edit = self.field(id).clone();
        let (a, b) = (id.to_string(), id.to_string());
        TextInput::edit(ElementId::Name(SharedString::from(id.to_string())), &edit)
            .placeholder(placeholder.to_string())
            .text_size(px(13.0))
            .px_2()
            .on_focus(cx.listener(move |hub, press: &TextPress, _, cx| {
                hub.press_field(&a, press);
                cx.notify();
            }))
            .on_select_to(cx.listener(move |hub, at: &usize, _, cx| {
                hub.field(&b).extend_to(*at);
                cx.notify();
            }))
    }

    /// A one-line text box bound to the field `id`.
    pub fn input(&mut self, id: &str, placeholder: &str, cx: &mut Context<Self>) -> TextInput {
        self.field(id).multiline = false;
        self.text_box(id, placeholder, cx).h(px(30.0))
    }

    /// A paragraph box bound to the field `id`; Ctrl+Enter submits it.
    pub fn textarea(&mut self, id: &str, placeholder: &str, cx: &mut Context<Self>) -> TextInput {
        self.field(id).multiline = true;
        self.text_box(id, placeholder, cx)
            .multiline()
            .min_h(px(100.0))
            .w_full()
    }

    /// A box that shows dots for what is typed.
    pub fn secret_input(&mut self, id: &str, placeholder: &str, cx: &mut Context<Self>) -> TextInput {
        let edit = self.field(id).clone();
        let masked: String = "•".repeat(edit.text.chars().count());
        let len = masked.len();
        let a = id.to_string();
        TextInput::new(ElementId::Name(SharedString::from(id.to_string())), masked)
            .cursor(len)
            .active(edit.active)
            .placeholder(placeholder.to_string())
            .text_size(px(13.0))
            .px_2()
            .h(px(30.0))
            .on_focus(cx.listener(move |hub, _press: &TextPress, _, cx| {
                hub.focus_field(&a);
                cx.notify();
            }))
    }

    pub fn render_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette();
        match self.modal.as_ref()? {
            Modal::Form { spec, error, busy } => {
                let (spec, error, busy) = (spec.clone(), error.clone(), *busy);
                let mut modal = crate::ui::Modal::new(spec.title.clone())
                    .when(spec.fields.iter().any(|f| f.is_paragraph()), |m| m.fill())
                    .width(spec.width)
                    .text_size(px(13.0))
                    .p_4()
                    .gap_3();
                if let Some(note) = &spec.note {
                    modal = modal.child(
                        div()
                            .text_color(rgb(p.text_dim))
                            .pb_1()
                            .child(note.clone()),
                    );
                }
                for field in &spec.fields {
                    modal = modal.child(self.render_field(field, cx));
                }
                if let Some(error) = error {
                    modal = modal.child(
                        div()
                            .p_2()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(widgets::red()))
                            .text_color(rgb(widgets::red()))
                            .child(error),
                    );
                }
                let submit = Button::new("form-submit", spec.submit.clone())
                    .h(px(28.0))
                    .disabled(busy)
                    .on_click(cx.listener(|hub, _, window, cx| hub.submit_modal(window, cx)));
                let submit = if spec.danger {
                    submit.colors(crate::ui::ButtonColors {
                        bg: Some(widgets::red_fill()),
                        hover: widgets::red(),
                        text: 0xFFFFFF,
                        border: None,
                    })
                } else {
                    submit.primary()
                };
                Some(
                    modal
                        .action(
                            div().flex_1().when(busy, |d| {
                                d.child(crate::ui::Spinner::new("form-busy").size(16.0))
                            }),
                        )
                        .action(
                            Button::new("form-cancel", "Cancel")
                                .h(px(28.0))
                                .on_click(cx.listener(|hub, _, _, cx| hub.close_modal(cx))),
                        )
                        .action(submit)
                        .into_any_element(),
                )
            }
            Modal::Confirm {
                title,
                message,
                label,
                busy,
                error,
                ..
            } => {
                let busy = *busy;
                Some(
                    crate::ui::Modal::new(title.clone())
                        .width(440.0)
                        .text_size(px(13.0))
                        .p_4()
                        .gap_3()
                        .child(div().text_color(rgb(p.text)).child(message.clone()))
                        .when_some(error.clone(), |m, error| {
                            m.child(div().text_color(rgb(widgets::red())).child(error))
                        })
                        .action(div().flex_1().when(busy, |d| {
                            d.child(crate::ui::Spinner::new("confirm-busy").size(16.0))
                        }))
                        .action(
                            Button::new("confirm-cancel", "Cancel")
                                .h(px(28.0))
                                .on_click(cx.listener(|hub, _, _, cx| hub.close_modal(cx))),
                        )
                        .action(
                            Button::new("confirm-ok", label.clone())
                                .h(px(28.0))
                                .disabled(busy)
                                .colors(crate::ui::ButtonColors {
                                    bg: Some(widgets::red_fill()),
                                    hover: widgets::red(),
                                    text: 0xFFFFFF,
                                    border: None,
                                })
                                .on_click(
                                    cx.listener(|hub, _, window, cx| hub.submit_modal(window, cx)),
                                ),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    /// A dropdown field: what is picked, as chips or names, in a box that
    /// opens the picker of everything there is to pick.
    fn dropdown_field(&mut self, id: &str, field: &Field, dropdown: &Dropdown, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let chosen = picks(&self.choice(id, &field.value));
        let load = self.fetch(&dropdown.path, cx);
        let options: Vec<DropOption> = load
            .ready()
            .map(|v| v.list("").iter().map(|item| (dropdown.option)(item)).collect())
            .unwrap_or_default();

        let mut picker = Picker::new(field.label.clone(), "Filter…", dropdown.multi);
        picker.loading = matches!(load, crate::hub::Load::Loading);
        if let Some(none) = &dropdown.none {
            picker = picker.item(PickItem::new(none.clone(), chosen.is_empty(), Act::choose(id.to_string(), "")));
        }
        for option in &options {
            let on = chosen.contains(&option.value);
            let line = option.value.to_string();
            let mut item = if dropdown.multi {
                let (add, remove) = (id.to_string(), id.to_string());
                let (a, r) = (line.clone(), line.clone());
                PickItem::toggle(
                    option.label.clone(),
                    on,
                    Act::run(move |hub, _, cx| {
                        let mut lines: Vec<String> = hub.choice(&add, "").lines().map(str::to_string).collect();
                        if !lines.contains(&a) {
                            lines.push(a.clone());
                        }
                        hub.choices.insert(add.clone(), lines.join("\n"));
                        cx.notify();
                    }),
                    Act::run(move |hub, _, cx| {
                        let lines: Vec<String> = hub.choice(&remove, "").lines().filter(|l| *l != r).map(str::to_string).collect();
                        hub.choices.insert(remove.clone(), lines.join("\n"));
                        cx.notify();
                    }),
                )
            } else {
                PickItem::new(option.label.clone(), on, Act::choose(id.to_string(), line))
            };
            if let Some(url) = &option.avatar {
                item = item.avatar(url.clone());
            }
            if let Some(color) = &option.color {
                item = item.color(color.clone());
            }
            if !option.detail.is_empty() {
                item = item.detail(option.detail.clone());
            }
            picker = picker.item(item);
        }

        // What is picked, in the order it was picked.
        let mut shown = div().flex().flex_row().flex_wrap().items_center().gap_1().flex_1().min_w_0();
        if chosen.is_empty() {
            shown = shown.child(widgets::dim(dropdown.none.clone().unwrap_or_else(|| "None yet".into())));
        }
        for value in &chosen {
            let option = options.iter().find(|o| o.value == *value);
            let label = option.map(|o| o.label.clone()).unwrap_or_else(|| value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()));
            shown = shown.child(match option {
                Some(DropOption { color: Some(color), .. }) => widgets::label_chip(&label, color).into_any_element(),
                Some(DropOption { avatar: Some(url), .. }) => {
                    let avatar = self.avatar(url, 18.0, cx);
                    widgets::row().gap_1().pr_1().child(avatar).child(label).into_any_element()
                }
                _ => div().pr_1().child(label).into_any_element(),
            });
        }
        div()
            .id(ElementId::Name(format!("{id}-dropdown").into()))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_h(px(30.0))
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(p.edge))
            .bg(rgb(p.field_bg))
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(p.accent)))
            .on_click(crate::hub::on(picker.act()))
            .child(shown)
            .child(crate::ui::icon("chevron-down", 14.0, p.text_dim))
            .into_any_element()
    }

    fn render_field(&mut self, field: &Field, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let id = format!("form.{}", field.key);
        let label = div()
            .flex()
            .flex_row()
            .gap_1()
            .text_size(px(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(rgb(p.text))
            .child(field.label.clone())
            .when(field.required, |d| d.child(div().text_color(rgb(widgets::red())).child("*")));
        let control: AnyElement = match &field.kind {
            Kind::Bool => {
                let on_now = self.toggle(&id);
                let key = id.clone();
                return div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        Checkbox::new(ElementId::Name(id.clone().into()), field.label.clone(), on_now)
                            .text_size(px(13.0))
                            .on_change(cx.listener(move |hub, value: &bool, _, cx| {
                                hub.toggles.insert(key.clone(), *value);
                                cx.notify();
                            })),
                    )
                    .when_some(field.hint.clone(), |d, hint| {
                        d.child(div().pl(px(22.0)).child(widgets::faint(hint)))
                    })
                    .into_any_element();
            }
            Kind::Choice(options) => {
                let current = self.choice(&id, &field.value);
                let short = options.len() <= 4
                    && options.iter().map(|o| o.1.len()).sum::<usize>() < 48;
                if short {
                    widgets::chips(
                        options
                            .iter()
                            .map(|(value, label)| {
                                (
                                    label.clone(),
                                    *value == current,
                                    Act::choose(id.clone(), value.clone()),
                                )
                            })
                            .collect(),
                    )
                    .into_any_element()
                } else {
                    let shown = options
                        .iter()
                        .find(|o| o.0 == current)
                        .map(|o| o.1.clone())
                        .unwrap_or_else(|| current.clone());
                    let entries: Vec<crate::hub::MenuEntry> = options
                        .iter()
                        .map(|(value, label)| {
                            crate::hub::MenuEntry::check(
                                label.clone(),
                                *value == current,
                                Act::choose(id.clone(), value.clone()),
                            )
                        })
                        .collect();
                    let act = Act::menu(entries);
                    DropdownButton::new(ElementId::Name(id.clone().into()), shown)
                        .h(px(30.0))
                        .px_2()
                        .text_size(px(13.0))
                        .on_press(move |_, window, cx| crate::hub::perform(act.clone(), window, cx))
                        .into_any_element()
                }
            }
            Kind::Multiline | Kind::Json => {
                let placeholder = if matches!(field.kind, Kind::Json) {
                    "{ }"
                } else {
                    ""
                };
                // Fills the dialog's spare height, and grows past it with
                // its text.
                let mut el = self.textarea(&id, placeholder, cx).flex_grow().flex_shrink_0();
                if matches!(field.kind, Kind::Json) {
                    el = el.font_family(widgets::MONO);
                }
                // The box grows with its text; this keeps it to the
                // dialog's spare height and scrolls it there.
                let scroll = self.scroller(&format!("{id}-scroll"));
                div()
                    .id(ElementId::Name(format!("{id}-scroll").into()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(100.0))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(el)
                    .into_any_element()
            }
            Kind::Dropdown(dropdown) => self.dropdown_field(&id, field, dropdown, cx),
            Kind::Secret => self.secret_input(&id, "", cx).w_full().into_any_element(),
            Kind::List => self
                .input(&id, "comma, separated, values", cx)
                .w_full()
                .into_any_element(),
            Kind::Text | Kind::Number => self.input(&id, "", cx).w_full().into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(field.is_paragraph(), |d| d.flex_1().min_h(px(130.0)))
            .child(label)
            .child(control)
            .when_some(field.hint.clone(), |d, hint| d.child(widgets::faint(hint)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dropdowns_send_what_was_picked() {
        let option = |v: &Value| DropOption::new(v.clone(), v.to_string());
        let fields = vec![
            Field::dropdown("labels", "Labels", "/l", option),
            Field::dropdown("assignees", "Assignees", "/a", option),
            Field::dropdown_one("milestone", "Milestone", "/m", "No milestone", option),
        ];
        let picked = |values: &[Value]| values.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let mut text = HashMap::new();
        text.insert("labels".to_string(), picked(&[json!("bug"), json!("good first issue")]));
        text.insert("milestone".to_string(), picked(&[json!(3)]));
        let values = FormValues { text, bools: HashMap::new() };
        assert_eq!(
            values.json(&fields).unwrap(),
            json!({ "labels": ["bug", "good first issue"], "milestone": 3 })
        );
    }
}
