//! Projects (the current, GraphQL-only kind): yours, your organizations',
//! a repository's; and one project as a board grouped by its Status field
//! or as a table, with items moved, added, archived and removed.

use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, Req, Route};
use crate::json::Json as _;
use crate::resource::Row;
use crate::time;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _};
use crate::ui::{icon, palette, IconButton};
use serde_json::{json, Value};

const SUMMARY: &str = "id title number shortDescription closed updatedAt url public items { totalCount } owner { ... on User { login } ... on Organization { login } }";

const PROJECT: &str = "query($id: ID!) {
  node(id: $id) {
    ... on ProjectV2 {
      id title shortDescription readme url closed public number
      fields(first: 40) {
        nodes {
          ... on ProjectV2SingleSelectField { id name dataType options { id name color } }
          ... on ProjectV2Field { id name dataType }
          ... on ProjectV2IterationField { id name dataType }
        }
      }
      items(first: 100) {
        totalCount
        nodes {
          id type isArchived
          fieldValues(first: 20) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue { name optionId field { ... on ProjectV2SingleSelectField { id name } } }
              ... on ProjectV2ItemFieldTextValue { text field { ... on ProjectV2Field { id name } } }
              ... on ProjectV2ItemFieldNumberValue { number field { ... on ProjectV2Field { id name } } }
              ... on ProjectV2ItemFieldDateValue { date field { ... on ProjectV2Field { id name } } }
            }
          }
          content {
            ... on Issue { title number url state repository { nameWithOwner } assignees(first: 3) { nodes { login } } }
            ... on PullRequest { title number url state merged repository { nameWithOwner } assignees(first: 3) { nodes { login } } }
            ... on DraftIssue { id title body }
          }
        }
      }
    }
  }
}";

fn project_row(p: &Value) -> Row {
    let mut row = Row::new(p.s("title"))
        .icon("project", if p.b("closed") { widgets::gray() } else { widgets::green() })
        .meta(format!(
            "{} #{}  ·  {} items  ·  updated {}",
            p.s("owner.login"),
            p.i("number"),
            p.i("items.totalCount"),
            time::ago(&p.s("updatedAt"))
        ))
        .body(p.s("shortDescription"))
        .open(Act::Go(Route::Project { id: p.s("id") }));
    if p.b("closed") {
        row = row.tag("Closed", widgets::gray());
    }
    if p.b("public") {
        row = row.tag("Public", widgets::gray());
    }
    row
}

fn colour(name: &str) -> u32 {
    match name {
        "GRAY" => 0x8B949E,
        "BLUE" => 0x388BFD,
        "GREEN" => 0x3FB950,
        "YELLOW" => 0xD29922,
        "ORANGE" => 0xDB6D28,
        "RED" => 0xF85149,
        "PINK" => 0xDB61A2,
        "PURPLE" => 0xA371F7,
        _ => 0x8B949E,
    }
}

impl Hub {
    pub fn projects(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let query = format!(
            "query {{ viewer {{ id login projectsV2(first: 50, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ {SUMMARY} }} }}
              organizations(first: 30) {{ nodes {{ id login projectsV2(first: 30, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ {SUMMARY} }} }} }} }} }} }}"
        );
        let data = match self.fetch_gql("/projects", &query, json!({}), cx) {
            Load::Ready(v) => v,
            other => return widgets::page().child(widgets::title("Projects")).child(widgets::placeholder(&other)).into_any_element(),
        };
        let mut owners: Vec<(String, String)> = vec![(data.s("viewer.id"), data.s("viewer.login"))];
        for org in data.list("viewer.organizations.nodes") {
            owners.push((org.s("id"), org.s("login")));
        }
        let names: Vec<String> = owners.iter().map(|o| o.1.clone()).collect();
        let create = FormSpec::new("New project")
            .submit("Create project")
            .field(Field::pick("owner", "Owner", names))
            .field(Field::text("title", "Project name").required())
            .build_with(move |v| {
                let owner = owners.iter().find(|o| o.1 == v.s("owner")).map(|o| o.0.clone()).unwrap_or_default();
                Ok(Req::gql(
                    "mutation($o: ID!, $t: String!) { createProjectV2(input: {ownerId: $o, title: $t}) { projectV2 { id } } }",
                    json!({ "o": owner, "t": v.s("title") }),
                )
                .ok("Project created")
                .inval("/projects")
                .then(|hub, value, cx| hub.go(Route::Project { id: value.s("createProjectV2.projectV2.id") }, cx))
                .act())
            })
            .act();
        let mut page = widgets::page().child(
            widgets::row().child(widgets::title("Projects")).child(widgets::spacer()).child(widgets::go_btn("new-project", "New project", create)),
        );
        let mut sections: Vec<(String, Vec<Value>)> = vec![("Your projects".into(), data.list("viewer.projectsV2.nodes").to_vec())];
        for org in data.list("viewer.organizations.nodes") {
            let list = org.list("projectsV2.nodes").to_vec();
            if !list.is_empty() {
                sections.push((org.s("login"), list));
            }
        }
        for (s, (title, list)) in sections.into_iter().enumerate() {
            let mut card = widgets::card();
            if list.is_empty() {
                card = card.child(widgets::empty("No projects."));
            }
            for (i, p) in list.iter().enumerate() {
                card = card.child(self.render_row(&format!("proj{s}-{i}"), project_row(p), cx));
            }
            page = page.child(widgets::h2(title)).child(card);
        }
        page.into_any_element()
    }

    pub fn repo_projects(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let query = format!("query($o: String!, $n: String!) {{ repository(owner: $o, name: $n) {{ id projectsV2(first: 50) {{ nodes {{ {SUMMARY} }} }} }} }}");
        let data = match self.fetch_gql(&format!("/repos/{repo}/projects"), &query, json!({ "o": owner, "n": name }), cx) {
            Load::Ready(v) => v,
            other => return widgets::placeholder(&other),
        };
        let mut card = widgets::card();
        let list = data.list("repository.projectsV2.nodes").to_vec();
        if list.is_empty() {
            card = card.child(widgets::empty("No projects are linked to this repository."));
        }
        for (i, p) in list.iter().enumerate() {
            card = card.child(self.render_row(&format!("rproj{i}"), project_row(p), cx));
        }
        widgets::col()
            .gap_3()
            .child(widgets::row().child(widgets::spacer()).child(widgets::btn("all-projects", "All your projects", Act::Go(Route::Projects))))
            .child(card)
            .into_any_element()
    }

    pub fn project(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let scope = format!("/projects/{id}");
        let data = match self.fetch_gql(&scope, PROJECT, json!({ "id": id }), cx) {
            Load::Ready(v) => v,
            other => return widgets::page().child(widgets::placeholder(&other)).into_any_element(),
        };
        let project = data.at("node").clone();
        let pid = project.s("id");
        let fields: Vec<Value> = project.list("fields.nodes").to_vec();
        // Group by "Status", or the first single-select field there is.
        let group = fields
            .iter()
            .find(|f| f.s("name") == "Status" && f.has("options"))
            .or_else(|| fields.iter().find(|f| f.has("options")))
            .cloned();
        let items: Vec<Value> = project.list("items.nodes").iter().filter(|i| !i.b("isArchived")).cloned().collect();
        let view_key = format!("project.view:{id}");
        let view = self.choice(&view_key, if group.is_some() { "board" } else { "table" });

        let edit = FormSpec::new("Project settings")
            .width(640.0)
            .field(Field::text("title", "Name").value(project.s("title")).required())
            .field(Field::text("desc", "Short description").value(project.s("shortDescription")).keep_empty())
            .field(Field::multiline("readme", "README").value(project.s("readme")).keep_empty())
            .field(Field::bool("public", "Public", project.b("public")))
            .gql(
                "mutation($p: ID!, $t: String!, $d: String, $r: String, $pub: Boolean) { updateProjectV2(input: {projectId: $p, title: $t, shortDescription: $d, readme: $r, public: $pub}) { clientMutationId } }",
                {
                    let pid = pid.clone();
                    move |v| json!({ "p": pid, "t": v.s("title"), "d": v.s("desc"), "r": v.s("readme"), "pub": v.b("public") })
                },
            )
            .ok("Project saved")
            .inval(scope.clone())
            .inval("/projects")
            .act();
        let closed = project.b("closed");
        let close = Req::gql(
            "mutation($p: ID!, $c: Boolean!) { updateProjectV2(input: {projectId: $p, closed: $c}) { clientMutationId } }",
            json!({ "p": pid, "c": !closed }),
        )
        .ok(if closed { "Project reopened" } else { "Project closed" })
        .inval(scope.clone())
        .inval("/projects")
        .act();
        let delete = Req::gql("mutation($p: ID!) { deleteProjectV2(input: {projectId: $p}) { clientMutationId } }", json!({ "p": pid }))
            .ok("Project deleted")
            .inval("/projects")
            .then(|hub, _, cx| hub.go(Route::Projects, cx))
            .act()
            .confirm("Delete this project?", "Its items, views and fields are removed. Issues and pull requests stay where they are.", "Delete project");
        let add_draft = FormSpec::new("Add a draft item")
            .field(Field::text("title", "Title").required())
            .field(Field::multiline("body", "Body"))
            .gql(
                "mutation($p: ID!, $t: String!, $b: String) { addProjectV2DraftIssue(input: {projectId: $p, title: $t, body: $b}) { projectItem { id } } }",
                {
                    let pid = pid.clone();
                    move |v| json!({ "p": pid, "t": v.s("title"), "b": v.s("body") })
                },
            )
            .ok("Item added")
            .inval(scope.clone())
            .act();
        let add_issue = FormSpec::new("Add an issue or pull request")
            .field(Field::text("url", "Issue or pull request").required().hint("A github.com URL, or owner/repo#123"))
            .build_with({
                let (pid, scope) = (pid.clone(), scope.clone());
                move |v| {
                    let text = v.s("url");
                    let (repo, number) = match crate::hub::route_for_url(text.trim()) {
                        Some(Route::Issue { repo, number }) | Some(Route::Pull { repo, number, .. }) => (repo, number),
                        _ => match text.trim().split_once('#') {
                            Some((r, n)) if r.contains('/') => (r.to_string(), n.parse::<u64>().map_err(|_| "Not a number after #.".to_string())?),
                            _ => return Err("Use a URL or owner/repo#123.".into()),
                        },
                    };
                    let pid = pid.clone();
                    Ok(Req::custom(move |client| {
                        let issue = client.get(&format!("/repos/{repo}/issues/{number}"))?;
                        client.graphql(
                            "mutation($p: ID!, $c: ID!) { addProjectV2ItemById(input: {projectId: $p, contentId: $c}) { item { id } } }",
                            json!({ "p": pid, "c": issue.s("node_id") }),
                        )
                    })
                    .ok("Added to the project")
                    .inval(scope.clone())
                    .act())
                }
            })
            .act();

        let mut header_chips = vec![("Table".to_string(), view == "table", Act::choose(&view_key, "table"))];
        if group.is_some() {
            header_chips.insert(0, ("Board".to_string(), view == "board", Act::choose(&view_key, "board")));
        }
        let body = if view == "board" {
            self.project_board(&pid, &scope, group.as_ref().unwrap(), &items, cx)
        } else {
            self.project_table(&pid, &scope, group.as_ref(), &items, cx)
        };
        widgets::page()
            .max_w(px(1600.0))
            .child(
                widgets::row()
                    .child(icon("project", 20.0, palette().text_dim))
                    .child(widgets::title(project.s("title")))
                    .when(closed, |d| d.child(widgets::tag("Closed", widgets::gray())))
                    .child(widgets::spacer())
                    .child(widgets::btn("add-draft", "Add draft", add_draft))
                    .child(widgets::btn("add-issue", "Add issue / PR", add_issue))
                    .child(widgets::btn("project-settings", "Settings", edit))
                    .child(widgets::btn("close-project", if closed { "Reopen" } else { "Close" }, close))
                    .child(widgets::danger("delete-project", "Delete", delete)),
            )
            .when(!project.s("shortDescription").is_empty(), |d| d.child(widgets::dim(project.s("shortDescription"))))
            .child(widgets::row().child(widgets::chips(header_chips)).child(widgets::spacer()).child(widgets::dim(format!("{} items", items.len()))))
            .child(body)
            .into_any_element()
    }

    fn project_board(&mut self, pid: &str, scope: &str, group: &Value, items: &[Value], _cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let field_id = group.s("id");
        let field_name = group.s("name");
        let options: Vec<Value> = group.list("options").to_vec();
        let mut columns: Vec<(String, String, u32, Vec<&Value>)> = options
            .iter()
            .map(|o| (o.s("id"), o.s("name"), colour(&o.s("color")), Vec::new()))
            .collect();
        columns.push((String::new(), format!("No {field_name}"), 0x8B949E, Vec::new()));
        for item in items {
            let value = item
                .list("fieldValues.nodes")
                .iter()
                .find(|v| v.s("field.id") == field_id)
                .map(|v| v.s("optionId"))
                .unwrap_or_default();
            let col = columns.iter_mut().find(|c| c.0 == value).or(None);
            match col {
                Some(c) => c.3.push(item),
                None => columns.last_mut().unwrap().3.push(item),
            }
        }
        let mut board = div().id("board").flex().flex_row().gap_3().items_start().overflow_x_scroll().track_scroll(&self.scroller("board")).pb_2();
        for (ci, (_, name, color, cards)) in columns.iter().enumerate() {
            let mut column = div()
                .flex()
                .flex_col()
                .gap_2()
                .w(px(300.0))
                .flex_none()
                .p_2()
                .rounded_md()
                .bg(rgb(p.deep_bg))
                .border_1()
                .border_color(rgb(p.divider))
                .child(
                    widgets::row()
                        .px_1()
                        .child(div().size(px(10.0)).rounded_full().border_2().border_color(rgb(*color)))
                        .child(widgets::h3(name.clone()))
                        .child(widgets::counter(cards.len() as i64)),
                );
            for (ii, item) in cards.iter().enumerate() {
                column = column.child(project_card(&format!("pc{ci}-{ii}"), pid, scope, &field_id, &options, item));
            }
            board = board.child(column);
        }
        board.into_any_element()
    }

    fn project_table(&mut self, pid: &str, scope: &str, group: Option<&Value>, items: &[Value], cx: &mut Context<Self>) -> AnyElement {
        let mut card = widgets::card();
        if items.is_empty() {
            card = card.child(widgets::empty("This project has no items yet."));
        }
        let field_id = group.map(|g| g.s("id")).unwrap_or_default();
        let options = group.map(|g| g.list("options").to_vec()).unwrap_or_default();
        for (i, item) in items.iter().enumerate() {
            let status = item
                .list("fieldValues.nodes")
                .iter()
                .find(|v| v.s("field.id") == field_id)
                .map(|v| v.s("name"))
                .unwrap_or_default();
            let mut row = Row::new(item_title(item))
                .icon(item_icon(item).0, item_icon(item).1)
                .meta(item_meta(item))
                .right(status)
                .open(item_open(item));
            for entry in item_menu(pid, scope, &field_id, &options, item) {
                if let MenuEntry::Item { label, act, .. } = entry {
                    row = row.action(label, act);
                }
            }
            card = card.child(self.render_row(&format!("pt{i}"), row, cx));
        }
        card.into_any_element()
    }
}

fn item_title(item: &Value) -> String {
    item.s("content.title")
}

fn item_icon(item: &Value) -> (&'static str, u32) {
    match item.s("type").as_str() {
        "PULL_REQUEST" if item.b("content.merged") => ("pr-merged", widgets::purple()),
        "PULL_REQUEST" if item.s("content.state") == "CLOSED" => ("pr-closed", widgets::red()),
        "PULL_REQUEST" => ("pr", widgets::green()),
        "ISSUE" if item.s("content.state") == "CLOSED" => ("issue-closed", widgets::purple()),
        "ISSUE" => ("issue", widgets::green()),
        _ => ("pencil", widgets::gray()),
    }
}

fn item_meta(item: &Value) -> String {
    if item.s("type") == "DRAFT_ISSUE" {
        "Draft".into()
    } else {
        let assignees: Vec<String> = item.list("content.assignees.nodes").iter().map(|a| a.s("login")).collect();
        let mut meta = format!("{}#{}", item.s("content.repository.nameWithOwner"), item.i("content.number"));
        if !assignees.is_empty() {
            meta.push_str(&format!("  ·  {}", assignees.join(", ")));
        }
        meta
    }
}

fn item_open(item: &Value) -> Act {
    let url = item.s("content.url");
    if url.is_empty() {
        Act::None
    } else {
        match crate::hub::route_for_url(&url) {
            Some(route) => Act::Go(route),
            None => Act::Url(url),
        }
    }
}

fn item_menu(pid: &str, scope: &str, field_id: &str, options: &[Value], item: &Value) -> Vec<MenuEntry> {
    let item_id = item.s("id");
    let mut entries = Vec::new();
    for o in options {
        entries.push(MenuEntry::item(
            format!("Move to {}", o.s("name")),
            Req::gql(
                "mutation($p: ID!, $i: ID!, $f: ID!, $o: String!) { updateProjectV2ItemFieldValue(input: {projectId: $p, itemId: $i, fieldId: $f, value: {singleSelectOptionId: $o}}) { clientMutationId } }",
                json!({ "p": pid, "i": item_id, "f": field_id, "o": o.s("id") }),
            )
            .ok(format!("Moved to {}", o.s("name")))
            .inval(scope.to_string())
            .act(),
        ));
    }
    if !options.is_empty() {
        entries.push(MenuEntry::Sep);
    }
    if item.s("type") == "DRAFT_ISSUE" {
        let draft = item.s("content.id");
        entries.push(MenuEntry::item(
            "Edit draft",
            FormSpec::new("Edit draft")
                .field(Field::text("title", "Title").value(item.s("content.title")).required())
                .field(Field::multiline("body", "Body").value(item.s("content.body")).keep_empty())
                .gql(
                    "mutation($d: ID!, $t: String!, $b: String) { updateProjectV2DraftIssue(input: {draftIssueId: $d, title: $t, body: $b}) { clientMutationId } }",
                    move |v| json!({ "d": draft, "t": v.s("title"), "b": v.s("body") }),
                )
                .ok("Draft saved")
                .inval(scope.to_string())
                .act(),
        ));
    }
    entries.push(MenuEntry::item(
        "Archive",
        Req::gql(
            "mutation($p: ID!, $i: ID!) { archiveProjectV2Item(input: {projectId: $p, itemId: $i}) { clientMutationId } }",
            json!({ "p": pid, "i": item_id }),
        )
        .ok("Item archived")
        .inval(scope.to_string())
        .act(),
    ));
    entries.push(MenuEntry::item(
        "Remove from project",
        Req::gql(
            "mutation($p: ID!, $i: ID!) { deleteProjectV2Item(input: {projectId: $p, itemId: $i}) { deletedItemId } }",
            json!({ "p": pid, "i": item_id }),
        )
        .ok("Item removed")
        .inval(scope.to_string())
        .act(),
    ));
    entries
}

fn project_card(id: &str, pid: &str, scope: &str, field_id: &str, options: &[Value], item: &Value) -> AnyElement {
    let p = palette();
    let (icon_name, color) = item_icon(item);
    let menu = Act::menu(item_menu(pid, scope, field_id, options, item));
    div()
        .id(ElementId::Name(id.to_string().into()))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(rgb(p.panel_bg))
        .border_1()
        .border_color(rgb(p.edge))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(p.accent)))
        .on_click(on(item_open(item)))
        .child(
            widgets::row()
                .child(icon(icon_name, 14.0, color))
                .child(widgets::faint(item_meta(item)).flex_1())
                .child(
                    IconButton::new(ElementId::Name(format!("{id}-menu").into()), "kebab")
                        .size(20.0)
                        .consume_press()
                        .on_click(on(menu)),
                ),
        )
        .child(div().font_weight(FontWeight::MEDIUM).child(item_title(item)))
        .into_any_element()
}
