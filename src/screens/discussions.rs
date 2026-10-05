//! Discussions, over GraphQL (REST has no discussions API): the list by
//! category, one thread with answers, replies and upvotes, and starting,
//! editing, closing or deleting one.

use super::common::CommentActs;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, RepoTab, Req, Route};
use crate::json::Json as _;
use crate::resource::Row;
use crate::time;
use crate::ui::{icon, palette};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, IntoElement as _, ParentElement as _, Styled as _,
};
use serde_json::{json, Value};

const LIST: &str = "query($o: String!, $n: String!, $cat: ID) {
  repository(owner: $o, name: $n) {
    id
    discussionCategories(first: 50) { nodes { id name emoji description isAnswerable } }
    discussions(first: 50, categoryId: $cat, orderBy: {field: UPDATED_AT, direction: DESC}) {
      totalCount
      nodes {
        number title createdAt updatedAt closed upvoteCount
        author { login avatarUrl }
        category { name emoji }
        comments { totalCount }
        answer { id }
      }
    }
  }
}";

const ONE: &str = "query($o: String!, $n: String!, $num: Int!) {
  repository(owner: $o, name: $n) {
    discussion(number: $num) {
      id title body createdAt url locked closed upvoteCount viewerHasUpvoted viewerCanDelete
      author { login avatarUrl }
      category { name emoji isAnswerable }
      answer { id }
      comments(first: 60) {
        nodes {
          id body createdAt url isAnswer upvoteCount viewerHasUpvoted viewerCanMarkAsAnswer viewerCanUnmarkAsAnswer viewerCanDelete viewerCanUpdate
          author { login avatarUrl }
          replies(first: 40) { nodes { id body createdAt url viewerCanDelete viewerCanUpdate author { login avatarUrl } } }
        }
      }
    }
  }
}";

/// A GraphQL comment node in the shape the REST comment card reads.
fn as_rest(node: &Value) -> Value {
    json!({
        "user": { "login": node.s("author.login"), "avatar_url": node.s("author.avatarUrl") },
        "body": node.s("body"),
        "created_at": node.s("createdAt"),
        "html_url": node.s("url"),
    })
}

fn gql(query: &str, vars: Value, ok: &str, scope: &str) -> Act {
    Req::gql(query, vars)
        .ok(ok.to_string())
        .inval(scope.to_string())
        .act()
}

impl Hub {
    pub fn repo_discussions(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let cat_key = format!("discussions.cat:{repo}");
        let cat = self.choice(&cat_key, "");
        let scope = format!("/repos/{repo}/discussions");
        let vars = json!({ "o": owner, "n": name, "cat": if cat.is_empty() { Value::Null } else { json!(cat) } });
        let data = match self.fetch_gql(&scope, LIST, vars, cx) {
            Load::Ready(v) => v,
            other => return widgets::placeholder(&other),
        };
        let categories: Vec<Value> = data.list("repository.discussionCategories.nodes").to_vec();
        let mut chips = vec![("All".to_string(), cat.is_empty(), Act::choose(&cat_key, ""))];
        for c in &categories {
            chips.push((
                format!("{} {}", emoji(&c.s("emoji")), c.s("name")),
                cat == c.s("id"),
                Act::choose(&cat_key, c.s("id")),
            ));
        }
        let repo_id = data.s("repository.id");
        let options: Vec<(String, String)> = categories
            .iter()
            .map(|c| (c.s("id"), c.s("name")))
            .collect();
        let target = repo.to_string();
        let new = FormSpec::new("Start a new discussion")
            .submit("Start discussion")
            .width(640.0)
            .field(Field::pick("category", "Category", options.iter().map(|o| o.1.clone()).collect()))
            .field(Field::text("title", "Title").required())
            .field(Field::multiline("body", "Body").required())
            .build_with(move |v| {
                let category = options.iter().find(|o| o.1 == v.s("category")).map(|o| o.0.clone()).unwrap_or_default();
                let target = target.clone();
                Ok(Req::gql(
                    "mutation($r: ID!, $c: ID!, $t: String!, $b: String!) { createDiscussion(input: {repositoryId: $r, categoryId: $c, title: $t, body: $b}) { discussion { number } } }",
                    json!({ "r": repo_id, "c": category, "t": v.s("title"), "b": v.s("body") }),
                )
                .ok("Discussion started")
                .inval(format!("/repos/{target}/discussions"))
                .then(move |hub, value, cx| {
                    hub.go(Route::Discussion { repo: target.clone(), number: value.i("createDiscussion.discussion.number") as u64 }, cx)
                })
                .act())
            })
            .act();
        let mut list = widgets::card();
        let nodes = data.list("repository.discussions.nodes").to_vec();
        if nodes.is_empty() {
            list = list.child(widgets::empty("No discussions yet."));
        }
        for (i, d) in nodes.iter().enumerate() {
            let mut row = Row::new(d.s("title"))
                .avatar(d.s("author.avatarUrl"))
                .meta(format!(
                    "{} {}  ·  #{} started {} by {}  ·  updated {}",
                    emoji(&d.s("category.emoji")),
                    d.s("category.name"),
                    d.i("number"),
                    time::ago(&d.s("createdAt")),
                    d.s("author.login"),
                    time::ago(&d.s("updatedAt"))
                ))
                .right(format!(
                    "▲ {}   💬 {}",
                    d.i("upvoteCount"),
                    d.i("comments.totalCount")
                ))
                .open(Act::Go(Route::Discussion {
                    repo: repo.to_string(),
                    number: d.i("number") as u64,
                }));
            if d.has("answer") {
                row = row.tag("Answered", widgets::green());
            }
            if d.b("closed") {
                row = row.tag("Closed", widgets::purple());
            }
            list = list.child(self.render_row(&format!("disc{i}"), row, cx));
        }
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::chips(chips))
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-discussion", "New discussion", new)),
            )
            .child(list)
            .into_any_element()
    }

    pub fn discussion(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) -> AnyElement {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let scope = format!("/repos/{repo}/discussions");
        let data = match self.fetch_gql(
            &scope,
            ONE,
            json!({ "o": owner, "n": name, "num": number }),
            cx,
        ) {
            Load::Ready(v) => v,
            other => {
                return widgets::page()
                    .child(widgets::placeholder(&other))
                    .into_any_element()
            }
        };
        let d = data.at("repository.discussion").clone();
        if d.is_null() {
            return widgets::page()
                .child(widgets::empty("Discussion not found."))
                .into_any_element();
        }
        let id = d.s("id");
        let answerable = d.b("category.isAnswerable");
        let upvote = |subject: &str, has: bool, count: i64, eid: String| {
            let q = if has {
                "mutation($id: ID!) { removeUpvote(input: {subjectId: $id}) { clientMutationId } }"
            } else {
                "mutation($id: ID!) { addUpvote(input: {subjectId: $id}) { clientMutationId } }"
            };
            crate::ui::Button::new(ElementId::Name(eid.into()), format!("▲ {count}"))
                .h(px(24.0))
                .px_2()
                .active(has)
                .on_click(on(gql(
                    q,
                    json!({ "id": subject }),
                    if has { "Upvote removed" } else { "Upvoted" },
                    &scope,
                )))
        };

        let edit = FormSpec::new("Edit discussion")
            .width(680.0)
            .field(Field::text("title", "Title").value(d.s("title")).required())
            .field(Field::multiline("body", "Body").value(d.s("body")).required())
            .gql(
                "mutation($id: ID!, $t: String!, $b: String!) { updateDiscussion(input: {discussionId: $id, title: $t, body: $b}) { clientMutationId } }",
                {
                    let id = id.clone();
                    move |v| json!({ "id": id, "t": v.s("title"), "b": v.s("body") })
                },
            )
            .ok("Discussion updated")
            .inval(scope.clone())
            .act();
        let close = if d.b("closed") {
            gql("mutation($id: ID!) { reopenDiscussion(input: {discussionId: $id}) { clientMutationId } }", json!({ "id": id }), "Discussion reopened", &scope)
        } else {
            Act::menu(vec![
                MenuEntry::item("Close as resolved", gql("mutation($id: ID!) { closeDiscussion(input: {discussionId: $id, reason: RESOLVED}) { clientMutationId } }", json!({ "id": id }), "Discussion closed", &scope)),
                MenuEntry::item("Close as outdated", gql("mutation($id: ID!) { closeDiscussion(input: {discussionId: $id, reason: OUTDATED}) { clientMutationId } }", json!({ "id": id }), "Discussion closed", &scope)),
                MenuEntry::item("Close as duplicate", gql("mutation($id: ID!) { closeDiscussion(input: {discussionId: $id, reason: DUPLICATE}) { clientMutationId } }", json!({ "id": id }), "Discussion closed", &scope)),
            ])
        };
        let lock = if d.b("locked") {
            gql("mutation($id: ID!) { unlockLockable(input: {lockableId: $id}) { clientMutationId } }", json!({ "id": id }), "Unlocked", &scope)
        } else {
            gql("mutation($id: ID!) { lockLockable(input: {lockableId: $id}) { clientMutationId } }", json!({ "id": id }), "Locked", &scope)
        };
        let back = repo.to_string();
        let delete = Req::gql(
            "mutation($id: ID!) { deleteDiscussion(input: {id: $id}) { clientMutationId } }",
            json!({ "id": id }),
        )
        .ok("Discussion deleted")
        .inval(scope.clone())
        .then(move |hub, _, cx| {
            hub.go(
                Route::Repo {
                    repo: back.clone(),
                    tab: RepoTab::Discussions,
                },
                cx,
            )
        })
        .act()
        .confirm(
            "Delete this discussion?",
            "It and all of its comments will be removed.",
            "Delete",
        );

        let body = as_rest(&d);
        let mut col = widgets::col().gap_3();
        let body_card = self.comment_card(
            "disc-body",
            &body,
            "started this",
            CommentActs {
                edit: None,
                delete: None,
                extra: Vec::new(),
                reactions: None,
                invalidate: scope.clone(),
                quote_into: None,
            },
            cx,
        );
        col = col.child(body_card).child(div().pl(px(44.0)).child(upvote(
            &id,
            d.b("viewerHasUpvoted"),
            d.i("upvoteCount"),
            "disc-upvote".into(),
        )));

        for (i, c) in d.list("comments.nodes").to_vec().iter().enumerate() {
            let cid = c.s("id");
            let mut extra = Vec::new();
            if answerable && c.b("viewerCanMarkAsAnswer") {
                extra.push(MenuEntry::item("Mark as answer", gql("mutation($id: ID!) { markDiscussionCommentAsAnswer(input: {id: $id}) { clientMutationId } }", json!({ "id": cid }), "Marked as answer", &scope)));
            }
            if c.b("viewerCanUnmarkAsAnswer") {
                extra.push(MenuEntry::item("Unmark as answer", gql("mutation($id: ID!) { unmarkDiscussionCommentAsAnswer(input: {id: $id}) { clientMutationId } }", json!({ "id": cid }), "Unmarked", &scope)));
            }
            let edit = c
                .b("viewerCanUpdate")
                .then(|| comment_edit(&cid, &c.s("body"), &scope));
            let delete = c.b("viewerCanDelete").then(|| comment_delete(&cid, &scope));
            let card = self.comment_card(
                &format!("dc{i}"),
                &as_rest(c),
                if c.b("isAnswer") {
                    "answered"
                } else {
                    "commented"
                },
                CommentActs {
                    edit,
                    delete,
                    extra,
                    reactions: None,
                    invalidate: scope.clone(),
                    quote_into: None,
                },
                cx,
            );
            col = col.child(if c.b("isAnswer") {
                div()
                    .border_l_4()
                    .border_color(rgb(widgets::green()))
                    .pl_2()
                    .child(card)
                    .into_any_element()
            } else {
                card
            });
            let mut replies = widgets::col().gap_2().pl(px(44.0));
            replies = replies.child(
                widgets::row()
                    .child(upvote(
                        &cid,
                        c.b("viewerHasUpvoted"),
                        c.i("upvoteCount"),
                        format!("dc{i}-up"),
                    ))
                    .when(c.b("isAnswer"), |d| {
                        d.child(widgets::icon_text(
                            "check-circle",
                            "Marked as answer",
                            widgets::green(),
                        ))
                    }),
            );
            for (j, r) in c.list("replies.nodes").to_vec().iter().enumerate() {
                let rid = r.s("id");
                let card = self.comment_card(
                    &format!("dc{i}r{j}"),
                    &as_rest(r),
                    "replied",
                    CommentActs {
                        edit: r
                            .b("viewerCanUpdate")
                            .then(|| comment_edit(&rid, &r.s("body"), &scope)),
                        delete: r.b("viewerCanDelete").then(|| comment_delete(&rid, &scope)),
                        extra: Vec::new(),
                        reactions: None,
                        invalidate: scope.clone(),
                        quote_into: None,
                    },
                    cx,
                );
                replies = replies.child(card);
            }
            let reply_field = format!("disc-reply:{cid}");
            self.submits.insert(
                reply_field.clone(),
                reply_act(&id, Some(&cid), &reply_field, &scope),
            );
            let reply_input = self
                .input(&reply_field, "Write a reply — Enter to send", cx)
                .w_full();
            replies = replies.child(reply_input);
            col = col.child(replies);
        }

        let field = format!("disc-comment:{repo}#{number}");
        let submit = reply_act(&id, None, &field, &scope);
        let composer = self.composer(&field, submit, Vec::new(), cx);
        widgets::page()
            .child(
                widgets::row()
                    .items_start()
                    .child(widgets::title(format!("{}  #{number}", d.s("title"))).flex_1())
                    .child(widgets::btn("edit-disc", "Edit", edit))
                    .child(widgets::btn(
                        "close-disc",
                        if d.b("closed") { "Reopen" } else { "Close ▾" },
                        close,
                    ))
                    .child(widgets::btn(
                        "lock-disc",
                        if d.b("locked") { "Unlock" } else { "Lock" },
                        lock,
                    ))
                    .when(d.b("viewerCanDelete"), |r| {
                        r.child(widgets::danger("delete-disc", "Delete", delete))
                    }),
            )
            .child(
                widgets::row()
                    .child(icon("discussion", 16.0, palette().text_dim))
                    .child(widgets::dim(format!(
                        "{} {}",
                        emoji(&d.s("category.emoji")),
                        d.s("category.name")
                    )))
                    .when(d.b("closed"), |r| {
                        r.child(widgets::tag("Closed", widgets::purple()))
                    })
                    .when(d.has("answer"), |r| {
                        r.child(widgets::tag("Answered", widgets::green()))
                    }),
            )
            .child(col)
            .child(composer)
            .into_any_element()
    }
}

fn reply_act(discussion: &str, reply_to: Option<&str>, field: &str, scope: &str) -> Act {
    let (discussion, reply_to, field, scope) = (
        discussion.to_string(),
        reply_to.map(str::to_string),
        field.to_string(),
        scope.to_string(),
    );
    Act::run(move |hub, window, cx| {
        let body = hub.field_text(&field);
        if body.trim().is_empty() {
            return;
        }
        let clear = field.clone();
        let req = Req::gql(
            "mutation($d: ID!, $b: String!, $r: ID) { addDiscussionComment(input: {discussionId: $d, body: $b, replyToId: $r}) { clientMutationId } }",
            json!({ "d": discussion, "b": body, "r": reply_to }),
        )
        .ok("Comment posted")
        .inval(scope.clone())
        .then(move |hub, _, cx| {
            hub.set_field(&clear, "");
            cx.notify();
        });
        hub.perform(req.act(), window, cx);
    })
}

fn comment_edit(id: &str, body: &str, scope: &str) -> Act {
    let id = id.to_string();
    FormSpec::new("Edit comment")
        .field(Field::multiline("body", "Comment").value(body).required())
        .gql(
            "mutation($id: ID!, $b: String!) { updateDiscussionComment(input: {commentId: $id, body: $b}) { clientMutationId } }",
            move |v| json!({ "id": id, "b": v.s("body") }),
        )
        .ok("Comment updated")
        .inval(scope.to_string())
        .act()
}

fn comment_delete(id: &str, scope: &str) -> Act {
    Req::gql(
        "mutation($id: ID!) { deleteDiscussionComment(input: {id: $id}) { clientMutationId } }",
        json!({ "id": id }),
    )
    .ok("Comment deleted")
    .inval(scope.to_string())
    .act()
    .confirm("Delete comment?", "This cannot be undone.", "Delete")
}

/// GitHub sends category emoji as `:shortcode:`.
fn emoji(code: &str) -> String {
    emojis::get_by_shortcode(code.trim_matches(':'))
        .map_or("•", |e| e.as_str())
        .to_string()
}
