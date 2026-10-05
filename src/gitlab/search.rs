//! GitHub's search, from GitLab's lists and search.
//!
//! GitHub searches with a query language (`is:pr is:open
//! review-requested:@me label:"bug"`); GitLab filters lists with
//! parameters. The qualifiers the app uses, and the common ones people
//! type, become those parameters; the words left over are GitLab's
//! `search`.

use super::{get, grp, me, missing, parallel, proj, project_path, shape, Ask};
use crate::api::{self, Client};
use crate::json::{enc, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

/// A search query taken apart: `key:value` qualifiers (`-key:value` for
/// "not") and the words left.
#[derive(Default, Debug)]
pub struct Parsed {
    pub quals: Vec<(String, String, bool)>,
    pub text: String,
}

impl Parsed {
    pub fn new(q: &str) -> Self {
        let mut parsed = Parsed::default();
        let mut words = Vec::new();
        for token in tokens(q) {
            let (negated, rest) = match token.strip_prefix('-') {
                Some(rest) if rest.contains(':') => (true, rest.to_string()),
                _ => (false, token.clone()),
            };
            match rest.split_once(':') {
                Some((key, value))
                    if !key.is_empty()
                        && !value.is_empty()
                        && !value.starts_with("//")
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_') =>
                {
                    parsed.quals.push((
                        key.to_lowercase(),
                        value.trim_matches('"').to_string(),
                        negated,
                    ));
                }
                _ => words.push(token),
            }
        }
        parsed.text = words.join(" ");
        parsed
    }

    /// Every value of `key`, not negated.
    pub fn all(&self, key: &str) -> Vec<&str> {
        self.quals
            .iter()
            .filter(|(k, _, n)| k == key && !n)
            .map(|(_, v, _)| v.as_str())
            .collect()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.all(key).into_iter().next()
    }

    pub fn not(&self, key: &str) -> Vec<&str> {
        self.quals
            .iter()
            .filter(|(k, _, n)| k == key && *n)
            .map(|(_, v, _)| v.as_str())
            .collect()
    }

    pub fn is(&self, value: &str) -> bool {
        self.all("is").contains(&value) || self.all("type").contains(&value)
    }
}

/// Words, keeping `"quoted phrases"` whole.
fn tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in q.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn search(a: &Ask, kind: &str) -> Result<Value> {
    let parsed = Parsed::new(a.query.get("q").unwrap_or(""));
    let items = match kind {
        "issues" => issues(a, &parsed)?,
        "repositories" => repositories(a, &parsed)?,
        "users" => shape::users(
            a.c,
            get(
                a.c,
                &format!("/users?search={}&{}", enc(&parsed.text), a.paging()),
            )?
            .list(""),
        ),
        "code" => scoped(a, &parsed, "blobs")?,
        "commits" => scoped(a, &parsed, "commits")?,
        "topics" => {
            let list = get(
                a.c,
                &format!("/topics?search={}&{}", enc(&parsed.text), a.paging()),
            )?;
            Value::Array(list.list("").iter().map(|t| json!({ "name": t.s("name"), "display_name": t.s("title"), "short_description": t.s("description") })).collect())
        }
        _ => return Err(missing("search of that kind")),
    };
    let total = items.list("").len();
    Ok(json!({ "total_count": total, "items": items }))
}

/// `@me` as the signed-in login.
fn who(c: &Client, value: &str) -> Result<String> {
    Ok(if value == "@me" {
        me(c)?.s("username")
    } else {
        value.trim_start_matches('@').to_string()
    })
}

/// Where a search looks: one project, one group, or everywhere.
fn scope(parsed: &Parsed) -> String {
    if let Some(repo) = parsed.get("repo") {
        proj(repo)
    } else if let Some(group) = parsed
        .get("org")
        .or_else(|| parsed.get("group"))
        .or_else(|| parsed.get("user"))
    {
        grp(group)
    } else {
        String::new()
    }
}

fn issues(a: &Ask, parsed: &Parsed) -> Result<Value> {
    let c = a.c;
    let want_prs = !parsed.is("issue");
    let want_issues = !parsed.is("pr")
        && !parsed.is("merged")
        && parsed.get("review-requested").is_none()
        && parsed.get("reviewed-by").is_none();
    if let Some(who_value) = parsed.get("mentions") {
        return mentions(a, parsed, &who(c, who_value)?, want_issues, want_prs);
    }
    let mut params = vec![a.paging()];
    let state = if parsed.is("open") {
        "opened"
    } else if parsed.is("merged") {
        "merged"
    } else if parsed.is("closed") {
        "closed"
    } else {
        "all"
    };
    params.push(format!("state={state}"));
    let order = match a.query.get("sort") {
        Some("created") => "created_at",
        Some("reactions") => "popularity",
        _ => "updated_at",
    };
    params.push(format!(
        "order_by={order}&sort={}",
        if a.query.get("order") == Some("asc") {
            "asc"
        } else {
            "desc"
        }
    ));
    if let Some(v) = parsed.get("author") {
        params.push(format!("author_username={}", enc(&who(c, v)?)));
    }
    if let Some(v) = parsed.get("assignee") {
        params.push(format!("assignee_username={}", enc(&who(c, v)?)));
    }
    let labels = parsed.all("label");
    if !labels.is_empty() {
        params.push(format!("labels={}", enc(&labels.join(","))));
    }
    for label in parsed.not("label") {
        params.push(format!("not[labels]={}", enc(label)));
    }
    if let Some(m) = parsed.get("milestone") {
        params.push(format!("milestone={}", enc(m)));
    }
    for none in parsed.all("no") {
        match none {
            "label" => params.push("labels=None".into()),
            "milestone" => params.push("milestone=None".into()),
            "assignee" => params.push("assignee_id=None".into()),
            _ => {}
        }
    }
    if parsed.get("archived") == Some("false") {
        params.push("non_archived=true".into());
    }
    if let Some(field) = parsed.get("in") {
        params.push(format!("in={}", enc(field)));
    }
    if !parsed.text.is_empty() {
        params.push(format!("search={}", enc(&parsed.text)));
    }
    let base = scope(parsed);
    let all = if base.is_empty() { "&scope=all" } else { "" };
    let query = params.join("&");
    let mut items: Vec<Value> = Vec::new();
    if want_issues && state != "merged" {
        let list = get(c, &format!("{base}/issues?{query}{all}"))?;
        items.extend(
            list.list("")
                .iter()
                .map(|v| shape::issue(c, &shape::repo_of(v), v, &HashMap::new())),
        );
    }
    if want_prs {
        let mut mr_query = query.clone();
        if let Some(v) = parsed.get("review-requested") {
            mr_query.push_str(&format!("&reviewer_username={}", enc(&who(c, v)?)));
        }
        let list = match parsed.get("reviewed-by") {
            Some(v) => {
                let login = who(c, v)?;
                // Approvals by name are a paid tier's filter; elsewhere the
                // reviewer is the closest there is.
                get(
                    c,
                    &format!(
                        "{base}/merge_requests?{mr_query}{all}&approved_by_usernames[]={}",
                        enc(&login)
                    ),
                )
                .or_else(|_| {
                    get(
                        c,
                        &format!(
                            "{base}/merge_requests?{mr_query}{all}&reviewer_username={}",
                            enc(&login)
                        ),
                    )
                })?
            }
            None => get(c, &format!("{base}/merge_requests?{mr_query}{all}"))?,
        };
        items.extend(
            list.list("")
                .iter()
                .map(|v| shape::mr(c, &shape::repo_of(v), v, &HashMap::new(), "")),
        );
    }
    if want_issues && want_prs {
        items.sort_by_key(|i| {
            std::cmp::Reverse(crate::time::parse(&i.s("updated_at")).unwrap_or(0))
        });
    }
    Ok(Value::Array(items))
}

/// Where `login` was mentioned: GitLab's to-do list keeps those.
fn mentions(
    a: &Ask,
    parsed: &Parsed,
    _login: &str,
    want_issues: bool,
    want_prs: bool,
) -> Result<Value> {
    let c = a.c;
    let states = ["pending", "done"];
    let lists = parallel(&states, |state| {
        get(
            c,
            &format!("/todos?action=mentioned&state={state}&{}", a.paging()),
        )
    });
    let open_only = parsed.is("open");
    let closed_only = parsed.is("closed");
    let mut items = Vec::new();
    for list in lists {
        for todo in list?.list("") {
            let target = todo.at("target");
            let repo = todo.s("project.path_with_namespace");
            let open = target.s("state") == "opened";
            if (open_only && !open) || (closed_only && open) {
                continue;
            }
            match todo.s("target_type").as_str() {
                "Issue" if want_issues => {
                    items.push(shape::issue(c, &repo, target, &HashMap::new()))
                }
                "MergeRequest" if want_prs => {
                    items.push(shape::mr(c, &repo, target, &HashMap::new(), ""))
                }
                _ => {}
            }
        }
    }
    items.dedup_by_key(|i| i.s("url"));
    Ok(Value::Array(items))
}

fn repositories(a: &Ask, parsed: &Parsed) -> Result<Value> {
    let c = a.c;
    let mut params = vec![a.paging()];
    if !parsed.text.is_empty() {
        params.push(format!("search={}", enc(&parsed.text)));
    }
    if let Some(topic) = parsed.get("topic") {
        params.push(format!("topic={}", enc(topic)));
    }
    if let Some(language) = parsed.get("language") {
        params.push(format!("with_programming_language={}", enc(language)));
    }
    for visibility in ["public", "private", "internal"] {
        if parsed.is(visibility) {
            params.push(format!("visibility={visibility}"));
        }
    }
    if parsed.get("archived") == Some("false") {
        params.push("archived=false".into());
    }
    let order = match a.query.get("sort") {
        Some("stars") | Some("forks") => "star_count",
        Some("updated") => "last_activity_at",
        _ if !parsed.text.is_empty() => "similarity",
        _ => "last_activity_at",
    };
    params.push(format!("order_by={order}&sort=desc"));
    let query = params.join("&");
    let owner = parsed
        .get("user")
        .or_else(|| parsed.get("org"))
        .or_else(|| parsed.get("group"));
    let list = match owner {
        Some(owner) => match get(
            c,
            &format!("{}/projects?include_subgroups=true&{query}", grp(owner)),
        ) {
            Ok(list) => list,
            Err(e) if api::status_of(&e) == Some(404) => {
                let id = super::user_id(c, owner)?;
                get(c, &format!("/users/{id}/projects?{query}"))?
            }
            Err(e) => return Err(e),
        },
        None => get(c, &format!("/projects?{query}"))?,
    };
    Ok(shape::projects(c, list.list("")))
}

/// Code or commits, in one project, one group, or everywhere (which
/// needs the instance's advanced search).
fn scoped(a: &Ask, parsed: &Parsed, what: &str) -> Result<Value> {
    let c = a.c;
    let base = scope(parsed);
    let list = get(
        c,
        &format!(
            "{base}/search?scope={what}&search={}&{}",
            enc(&parsed.text),
            a.paging()
        ),
    )?;
    let mut ids: Vec<i64> = list.list("").iter().map(|r| r.i("project_id")).collect();
    ids.sort();
    ids.dedup();
    let paths: HashMap<i64, String> = ids
        .iter()
        .copied()
        .zip(parallel(&ids, |id| project_path(c, *id)))
        .collect();
    Ok(Value::Array(
        list.list("")
            .iter()
            .map(|r| {
                let repo = paths.get(&r.i("project_id")).cloned().or_else(|| parsed.get("repo").map(str::to_string)).unwrap_or_default();
                if what == "blobs" {
                    let path = r.s("path");
                    json!({
                        "name": r.s("filename"),
                        "path": path,
                        "repository": { "full_name": repo },
                        "html_url": format!("{}/{repo}/-/blob/{}/{path}#L{}", c.web, r.s("ref"), r.i("startline")),
                    })
                } else {
                    let mut commit = shape::commit(&repo, r, None);
                    commit["repository"] = json!({ "full_name": repo });
                    commit
                }
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_come_apart() {
        let p = Parsed::new(
            "is:open is:pr review-requested:@me label:\"good first\" -label:wontfix speed up",
        );
        assert!(p.is("open") && p.is("pr"));
        assert_eq!(p.get("review-requested"), Some("@me"));
        assert_eq!(p.all("label"), ["good first"]);
        assert_eq!(p.not("label"), ["wontfix"]);
        assert_eq!(p.text, "speed up");
        assert_eq!(
            Parsed::new("https://example.com").text,
            "https://example.com"
        );
    }
}
