//! Lists of GitHub things, described rather than hand-built.
//!
//! Collaborators, webhooks, deploy keys, secrets, releases, branches,
//! followers, notifications... most of GitHub is a paged list of objects,
//! each with a title, a line of detail, somewhere to go and a few things
//! to do. A [`ListSpec`] says where the list comes from and how one item
//! reads as a [`Row`]; [`Hub::list`] does the fetching, paging, drawing
//! and the row actions.

use crate::hub::{on, with_query, Act, Hub, Load, MenuEntry};
use crate::json::{self, Json as _};
use crate::time;
use crate::ui::{icon, palette, Button, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::Value;
use std::collections::HashMap;
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
    /// A commit whose authors (co-authors too) should lead the row.
    pub commit: Option<CommitRef>,
    /// Those authors, once GraphQL has resolved them.
    pub authors: Vec<Value>,
    /// How the commit's checks went (`SUCCESS`, `FAILURE`, `PENDING`),
    /// once GraphQL has said.
    pub checks: String,
}

/// What GraphQL tells about a commit in a list: who made it, co-authors
/// too, and how its checks went.
#[derive(Clone, Default)]
pub struct CommitInfo {
    pub authors: Vec<Value>,
    /// `statusCheckRollup.state`, or empty for a commit without checks.
    pub checks: String,
}

/// A commit a row is about, and what the meta line says after its
/// authors' names ("committed 2 hours ago").
pub struct CommitRef {
    pub repo: String,
    pub sha: String,
    pub after: String,
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
            commit: None,
            authors: Vec::new(),
            checks: String::new(),
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
        self.labels = labels.iter().map(|l| (l.s("name"), l.s("color"))).collect();
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

    /// Lead with the authors of `sha` in `repo` once they're known, then
    /// `after`; `meta` is what shows until then.
    pub fn commit(
        mut self,
        repo: impl Into<String>,
        sha: impl Into<String>,
        after: impl Into<String>,
    ) -> Self {
        self.commit = Some(CommitRef {
            repo: repo.into(),
            sha: sha.into(),
            after: after.into(),
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
    /// People, drawn as a grid of profile cards.
    pub people: bool,
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
            people: false,
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

    /// Draw the items, which are users, as profile cards.
    pub fn people(mut self) -> Self {
        self.people = true;
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
            return card
                .child(widgets::empty(spec.empty.clone()))
                .into_any_element();
        }
        if spec.people {
            return self.people_grid(spec, &items, loading, more, cx);
        }
        let mut rows: Vec<Row> = items.iter().map(|item| (spec.row)(item)).collect();
        self.resolve_authors(&mut rows, cx);
        for (i, row) in rows.into_iter().enumerate() {
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

    /// Fill in the authors and checks of rows about commits, asking
    /// GraphQL for each repository's commits in batches. The batches
    /// follow list order, so loading another page reuses the batches
    /// already answered.
    pub fn resolve_authors(&mut self, rows: &mut [Row], cx: &mut Context<Self>) {
        let mut by_repo: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let Some(commit) = &row.commit else { continue };
            match by_repo.iter_mut().find(|(repo, _)| *repo == commit.repo) {
                Some((_, list)) => list.push(i),
                None => by_repo.push((commit.repo.clone(), vec![i])),
            }
        }
        for (repo, indices) in by_repo {
            let shas: Vec<String> = indices
                .iter()
                .filter_map(|&i| rows[i].commit.as_ref().map(|c| c.sha.clone()))
                .collect();
            let found = self.commit_batch(&repo, &shas, cx);
            for &i in &indices {
                if let Some(info) = rows[i].commit.as_ref().and_then(|c| found.get(&c.sha)) {
                    rows[i].authors = info.authors.clone();
                    rows[i].checks = info.checks.clone();
                }
            }
        }
    }

    /// The accounts behind each of `shas` in `repo` (co-authors too) and
    /// how its checks went, as far as GraphQL has answered, asked in
    /// batches that follow the list's order so a longer list reuses the
    /// batches already in.
    pub fn commit_batch(
        &mut self,
        repo: &str,
        shas: &[String],
        cx: &mut Context<Self>,
    ) -> HashMap<String, CommitInfo> {
        let mut found = HashMap::new();
        let Some((owner, name)) = repo.split_once('/') else {
            return found;
        };
        for chunk in shas.chunks(PER_PAGE) {
            let fields: String = chunk
                .iter()
                .enumerate()
                .map(|(n, sha)| {
                    let sha = Value::String(sha.clone());
                    format!("c{n}: object(oid: {sha}) {{ ... on Commit {{ authors(first: 10) {{ nodes {{ name avatarUrl user {{ login avatarUrl }} }} }} statusCheckRollup {{ state }} }} }} ")
                })
                .collect();
            let query = format!("query($o: String!, $n: String!) {{ repository(owner: $o, name: $n) {{ {fields}}} }}");
            let vars = serde_json::json!({ "o": owner, "n": name });
            let Some(data) = self
                .fetch_gql(&format!("/repos/{repo}/commits"), &query, vars, cx)
                .ready()
                .cloned()
            else {
                continue;
            };
            for (n, sha) in chunk.iter().enumerate() {
                let commit = data.at(&format!("repository.c{n}"));
                found.insert(
                    sha.clone(),
                    CommitInfo {
                        authors: crate::screens::repo::distinct_authors(
                            commit.list("authors.nodes"),
                        ),
                        checks: commit.s("statusCheckRollup.state"),
                    },
                );
            }
        }
        found
    }

    pub fn render_row(&mut self, id: &str, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let (leading, meta): (Option<AnyElement>, Option<AnyElement>) = match &row.commit {
            Some(commit) if !row.authors.is_empty() => (
                Some(self.author_avatars(&row.authors, cx)),
                Some(
                    crate::screens::repo::author_names(id, &row.authors)
                        .text_size(px(12.0))
                        .child(widgets::dim(commit.after.clone()))
                        .into_any_element(),
                ),
            ),
            _ => (None, None),
        };
        let meta = meta.or_else(|| {
            (!row.meta.is_empty()).then(|| widgets::dim(row.meta.clone()).into_any_element())
        });
        let leading: Option<AnyElement> = if leading.is_some() {
            leading
        } else if let Some(url) = &row.avatar {
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
            .children(crate::screens::pulls::ci_mark_el(
                ElementId::Name(format!("{id}-checks").into()),
                &row.checks,
            ))
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
            .children(meta)
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
        let mut el = widgets::list_row(
            ElementId::Name(SharedString::from(id.to_string())),
            row.open,
        )
        .items_center();
        if !clickable {
            el = el.cursor_default();
        }
        el.children(leading)
            .child(middle)
            .child(trailing)
            .into_any_element()
    }

    /// Users as GitHub's profile cards, three across: avatar, login and
    /// name, where they are or work or when they joined, Follow, and any
    /// actions the list gives them.
    fn people_grid(
        &mut self,
        spec: &ListSpec,
        items: &[Value],
        loading: bool,
        more: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let me = self.login();
        let mut grid = div().grid().grid_cols(3).gap_x_8();
        for (i, user) in items.iter().enumerate() {
            let row = (spec.row)(user);
            let login = user.s("login");
            let is_user = user.s("type") != "Organization";
            // The list only carries the login and avatar; the rest is
            // the profile, fetched once and kept.
            let profile = self.fetch(&format!("/users/{login}"), cx).ready().cloned();
            let detail: Option<(&str, String)> = profile.as_ref().and_then(|u| {
                if !u.s("location").is_empty() {
                    Some(("globe", u.s("location")))
                } else if !u.s("company").is_empty() {
                    Some(("org", u.s("company")))
                } else if !u.s("created_at").is_empty() {
                    Some((
                        "clock",
                        format!("Joined on {}", time::date(&u.s("created_at"))),
                    ))
                } else {
                    None
                }
            });
            let name = profile.as_ref().map(|u| u.s("name")).unwrap_or_default();
            // What the list says about them beyond their name: "120
            // commits", a role.
            let note = if row.meta != name && row.meta != user.s("name") {
                row.meta.clone()
            } else {
                String::new()
            };

            let mut buttons = div().flex().flex_row().flex_wrap().gap_2();
            if is_user && login != me {
                let following = self
                    .fetch_check(&format!("/user/following/{login}"), cx)
                    .ready()
                    .map(|v| v.b(""));
                if let Some(following) = following {
                    let path = format!("/user/following/{login}");
                    let act = crate::hub::Req::rest(
                        if following { "DELETE" } else { "PUT" },
                        path.clone(),
                    )
                    .ok(if following {
                        format!("Unfollowed {login}")
                    } else {
                        format!("Following {login}")
                    })
                    .inval(path)
                    .inval(format!("/users/{login}"))
                    .act();
                    buttons = buttons.child(
                        Button::new(
                            ElementId::Name(format!("{}#{i}-follow", spec.id).into()),
                            if following { "Unfollow" } else { "Follow" },
                        )
                        .h(px(28.0))
                        .px_3()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .consume_press()
                        .on_click(on(act)),
                    );
                }
            }
            for (j, action) in row.actions.into_iter().enumerate() {
                let mut button = Button::new(
                    ElementId::Name(format!("{}#{i}-act-{j}", spec.id).into()),
                    action.label,
                )
                .h(px(28.0))
                .px_3()
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
                buttons = buttons.child(button);
            }

            let avatar = self.avatar(&user.s("avatar_url"), 48.0, cx);
            grid = grid.child(
                div()
                    .id(ElementId::Name(format!("{}#{i}", spec.id).into()))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_3()
                    .py_4()
                    .min_w_0()
                    .border_b_1()
                    .border_color(rgb(p.divider))
                    .cursor_pointer()
                    .on_click(on(row.open))
                    .child(div().flex_none().child(avatar))
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_baseline()
                                    .gap_x_2()
                                    .child(
                                        div()
                                            .text_size(px(15.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(p.accent_hover))
                                            .child(login.clone()),
                                    )
                                    .when(!name.is_empty(), |d| {
                                        d.child(widgets::dim(name.clone()))
                                    }),
                            )
                            .when_some(detail, |d, (icon_name, text)| {
                                d.child(
                                    widgets::row()
                                        .gap_1p5()
                                        .child(icon(icon_name, 13.0, p.text_dim))
                                        .child(
                                            div()
                                                .text_ellipsis()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .child(text),
                                        ),
                                )
                            })
                            .when(!note.is_empty(), |d| d.child(widgets::dim(note.clone())))
                            .child(div().pt_1().child(buttons)),
                    ),
            );
        }
        let mut col = widgets::col().child(grid);
        if loading {
            col = col.child(widgets::loading());
        } else if more {
            let id = spec.id.clone();
            col = col.child(
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
        col.into_any_element()
    }
}
