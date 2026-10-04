//! Search across GitHub: repositories, issues, pull requests, users,
//! code, commits and topics.

use super::common::{issue_row, repo_row, user_row};
use super::pulls::commit_row;
use crate::hub::{Act, Hub, MenuEntry, Route};
use crate::json::{enc, first_line, Json as _};
use crate::resource::{ListSpec, Row};
use crate::widgets;
use gpui::{px, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};

const KINDS: [(&str, &str); 7] = [
    ("repositories", "Repositories"),
    ("issues", "Issues"),
    ("pulls", "Pull requests"),
    ("users", "Users"),
    ("code", "Code"),
    ("commits", "Commits"),
    ("topics", "Topics"),
];

impl Hub {
    pub fn search(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.choice("search.kind", "repositories");
        let sort = self.choice(&format!("search.sort:{kind}"), "");
        self.submits.insert(
            "search.q".into(),
            Act::run(|hub, _, cx| {
                let q = hub.field_text("search.q");
                hub.choices.insert("search.applied".into(), q);
                cx.notify();
            }),
        );
        // A query typed into the jump box arrives in the field; apply it.
        let typed = self.field_text("search.q");
        let applied = match self.choices.get("search.applied") {
            Some(q) => q.clone(),
            None => {
                self.choices.insert("search.applied".into(), typed.clone());
                typed
            }
        };
        let input = self
            .input("search.q", "Search GitHub — qualifiers like language:rust stars:>100 user:octocat work", cx)
            .w_full()
            .h(px(34.0));
        let sorts: &[(&str, &str)] = match kind.as_str() {
            "repositories" => &[("", "Best match"), ("stars", "Most stars"), ("forks", "Most forks"), ("updated", "Recently updated")],
            "issues" | "pulls" => &[("", "Best match"), ("comments", "Most commented"), ("created", "Newest"), ("updated", "Recently updated"), ("reactions", "Most reactions")],
            "users" => &[("", "Best match"), ("followers", "Most followers"), ("repositories", "Most repositories"), ("joined", "Recently joined")],
            "commits" => &[("", "Best match"), ("author-date", "Newest"), ("committer-date", "Recently committed")],
            _ => &[("", "Best match")],
        };
        let sort_key = format!("search.sort:{kind}");
        let sort_menu = Act::menu(sorts.iter().map(|(v, l)| MenuEntry::check(*l, sort == *v, Act::choose(&sort_key, *v))).collect());
        let sort_label = sorts.iter().find(|s| s.0 == sort).map(|s| s.1).unwrap_or("Best match");

        let results = if applied.trim().is_empty() {
            widgets::empty("Type a search and press Enter.")
        } else {
            let sort_q = if sort.is_empty() { String::new() } else { format!("&sort={sort}&order=desc") };
            let q = enc(applied.trim());
            let spec = match kind.as_str() {
                "issues" => ListSpec::new(format!("/search/issues?q={}{sort_q}", enc(&format!("{} is:issue", applied.trim()))), |i| issue_row(i, true)),
                "pulls" => ListSpec::new(format!("/search/issues?q={}{sort_q}", enc(&format!("{} is:pr", applied.trim()))), |i| issue_row(i, true)),
                "users" => ListSpec::new(format!("/search/users?q={q}{sort_q}"), user_row),
                "code" => ListSpec::new(format!("/search/code?q={q}"), |c| {
                    let repo = c.s("repository.full_name");
                    let path = c.s("path");
                    Row::new(path.clone())
                        .icon("file", widgets::gray())
                        .meta(repo.clone())
                        .open(Act::Go(Route::Tree { repo: repo.clone(), git_ref: default_ref(c), path, file: true }))
                }),
                "commits" => ListSpec::new(format!("/search/commits?q={q}{sort_q}"), |c| {
                    let repo = c.s("repository.full_name");
                    commit_row(&repo, c).meta(format!("{repo}  ·  {}", first_line(&c.s("commit.author.name"))))
                }),
                "topics" => ListSpec::new(format!("/search/topics?q={q}"), |t| {
                    let name = t.s("name");
                    Row::new(t.s("display_name").is_empty().then(|| name.clone()).unwrap_or_else(|| t.s("display_name")))
                        .icon("tag", widgets::gray())
                        .meta(format!("#{name}"))
                        .body(t.s("short_description"))
                        .open(Act::run(move |hub, _, cx| {
                            hub.choices.insert("search.kind".into(), "repositories".into());
                            let q = format!("topic:{name}");
                            hub.set_field("search.q", q.clone());
                            hub.choices.insert("search.applied".into(), q);
                            cx.notify();
                        }))
                }),
                _ => ListSpec::new(format!("/search/repositories?q={q}{sort_q}"), repo_row),
            }
            .items("items")
            .empty("No results.");
            self.list(&spec, cx)
        };
        widgets::page()
            .child(widgets::title("Search"))
            .child(input)
            .child(
                widgets::row()
                    .child(widgets::chips(KINDS.iter().map(|(v, l)| (l.to_string(), kind == *v, Act::choose("search.kind", *v))).collect()))
                    .child(widgets::spacer())
                    .child(widgets::btn("search-sort", format!("Sort: {sort_label} ▾"), sort_menu)),
            )
            .child(results)
            .child(widgets::faint("Search syntax: in:title, author:, label:, language:, stars:>N, created:>2024-01-01, org:, repo:, is:open …"))
            .into_any_element()
    }
}

/// Code search hits carry the blob SHA but not the branch; the default
/// branch is the right place to look at them.
fn default_ref(c: &serde_json::Value) -> String {
    let url = c.s("html_url");
    // https://github.com/o/r/blob/<ref>/path
    url.split("/blob/").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or("HEAD").to_string()
}

