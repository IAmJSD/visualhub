//! Dialog forms, described rather than hand-built.
//!
//! Most of what GitHub lets you create or edit -- a repository, a label, a
//! webhook, a release, a key -- is a handful of fields sent as one JSON
//! body. A [`FormSpec`] lists the fields and the request, and the same
//! code draws the dialog, reads it back, checks it and sends it. Field
//! keys may be dotted (`config.url`) to build nested bodies.

use crate::hub::{Act, Hub, Modal, Req, Then};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, IntoElement as _, ParentElement as _, SharedString,
    Styled as _,
};
use schist_ui::{palette, Button, Checkbox, DropdownButton, LineEdit, TextInput, TextPress};
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
                Kind::Choice(_) => {
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
            Kind::Choice(_) => {
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
                let mut modal = schist_ui::Modal::new(spec.title.clone())
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
                    submit.colors(schist_ui::ButtonColors {
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
                                d.child(schist_ui::Spinner::new("form-busy").size(16.0))
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
                    schist_ui::Modal::new(title.clone())
                        .width(440.0)
                        .text_size(px(13.0))
                        .p_4()
                        .gap_3()
                        .child(div().text_color(rgb(p.text)).child(message.clone()))
                        .when_some(error.clone(), |m, error| {
                            m.child(div().text_color(rgb(widgets::red())).child(error))
                        })
                        .action(div().flex_1().when(busy, |d| {
                            d.child(schist_ui::Spinner::new("confirm-busy").size(16.0))
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
                                .colors(schist_ui::ButtonColors {
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
                let mut el = self.textarea(&id, placeholder, cx);
                if matches!(field.kind, Kind::Json) {
                    el = el.font_family(widgets::MONO);
                }
                el.into_any_element()
            }
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
            .child(label)
            .child(control)
            .when_some(field.hint.clone(), |d, hint| d.child(widgets::faint(hint)))
            .into_any_element()
    }
}
