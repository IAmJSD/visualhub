//! A repository's pages, asked for as GitHub's repository paths: the
//! repository itself, its files, branches and tags, commits and
//! comparisons, build statuses as checks, forks, watchers, people with
//! access and deploy keys.

use super::{
    call, get, get_all, missing, page, personal_workspace, repo_api, shape, user_by_login, values,
    Ask,
};
use crate::api::{self, Client};
use crate::gitlab::MINUTE;
use crate::json::{enc, enc_path, Json as _};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn route(a: &Ask, repo: &str, rest: &[&str]) -> Result<Value> {
    let c = a.c;
    let r = repo_api(repo);
    let m = a.method;
    match rest {
        [] => repository(a, repo),
        ["issues", ..] | ["pulls", ..] | ["pullrequests", ..] => {
            super::pulls::route(a, repo, rest)
        }
        ["assignees"] => {
            let ws = repo.split('/').next().unwrap_or("");
            let members = values(
                c,
                &format!("/workspaces/{}/members?pagelen=100", enc(ws)),
            )?;
            let people: Vec<Value> = members.iter().map(|m| m.at("user").clone()).collect();
            Ok(shape::users(c, &people))
        }
        ["labels", ..] if a.get() => Ok(json!([])),
        ["labels", ..] => Err(missing("labels")),
        ["milestones", ..] if a.get() => Ok(json!([])),
        ["milestones", ..] => Err(missing("milestones")),
        ["releases", ..] if a.get() && rest.len() == 1 => Ok(json!([])),
        ["releases", ..] => Err(missing("releases (it keeps files under Downloads)")),
        ["topics"] | ["transfer"] | ["generate"] => Err(missing(&format!(
            "way to change a repository's {} through its API",
            rest[0]
        ))),
        ["contents", path @ ..] => contents(a, repo, &path.join("/")),
        ["branches"] => {
            let list = page(c, &format!("{r}/refs/branches?sort=name"), &a.query)?;
            Ok(Value::Array(list.iter().map(|b| branch(repo, b)).collect()))
        }
        ["branches", .., "protection"] => Err(missing(
            "protection rules like GitHub's; its branch restrictions are in the repository's settings",
        )),
        ["branches", .., "rename"] => Err(missing("way to rename a branch")),
        ["branches", name @ ..] => Ok(branch(
            repo,
            &get(c, &format!("{r}/refs/branches/{}", enc(&name.join("/"))))?,
        )),
        ["tags"] => {
            let list = page(c, &format!("{r}/refs/tags?sort=-target.date"), &a.query)?;
            Ok(Value::Array(
                list.iter()
                    .map(|t| {
                        let name = t.s("name");
                        json!({
                            "name": name,
                            "commit": { "sha": t.s("target.hash"), "url": format!("/repos/{repo}/commits/{}", t.s("target.hash")) },
                            "zipball_url": archive(repo, &name, "zip"),
                            "tarball_url": archive(repo, &name, "tar.gz"),
                        })
                    })
                    .collect(),
            ))
        }
        ["git", "refs"] => {
            let full = a.field("ref").as_str().unwrap_or("");
            let sha = a.field("sha").clone();
            let (kind, name) = if let Some(name) = full.strip_prefix("refs/tags/") {
                ("tags", name)
            } else {
                ("branches", full.trim_start_matches("refs/heads/"))
            };
            let made = call(
                c,
                "POST",
                &format!("{r}/refs/{kind}"),
                Some(&json!({ "name": name, "target": { "hash": sha } })),
            )?;
            Ok(json!({ "ref": full, "object": { "sha": made.s("target.hash") } }))
        }
        ["git", "refs", "heads", name @ ..] => {
            let path = format!("{r}/refs/branches/{}", enc(&name.join("/")));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => {
                    let b = get(c, &path)?;
                    Ok(json!({ "ref": format!("refs/heads/{}", name.join("/")), "object": { "sha": b.s("target.hash") } }))
                }
            }
        }
        ["git", "refs", "tags", name @ ..] => {
            let path = format!("{r}/refs/tags/{}", enc(&name.join("/")));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => {
                    let t = get(c, &path)?;
                    Ok(json!({ "ref": format!("refs/tags/{}", name.join("/")), "object": { "sha": t.s("target.hash") } }))
                }
            }
        }
        ["git", "trees", sha] => tree(c, repo, sha),
        ["commits"] => commits(a, repo),
        ["commits", sha] => commit(c, repo, sha),
        ["commits", sha, "comments"] => commit_comments(a, repo, sha),
        ["commits", sha, "comments", id] => {
            let path = format!("{r}/commit/{}/comments/{id}", enc(sha));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                _ => call(
                    c,
                    "PUT",
                    &path,
                    Some(&json!({ "content": { "raw": a.field("body") } })),
                ),
            }
        }
        ["commits", sha, "check-runs"] => check_runs(c, repo, sha),
        ["commits", sha, "status"] => combined_status(c, repo, sha),
        ["commits", sha, "statuses"] => Ok(Value::Array(
            statuses(c, repo, sha)?
                .iter()
                .map(|s| json!({ "state": combined(&s.s("state")), "context": s.s("name"), "description": s.s("description"), "target_url": s.s("url") }))
                .collect(),
        )),
        ["check-runs", _, "annotations"] => Ok(json!([])),
        ["compare", range] => compare(c, repo, range),
        ["languages"] => {
            let info = get(c, &r)?;
            let language = info.s("language");
            Ok(if language.is_empty() {
                json!({})
            } else {
                // Bitbucket names one language; the bar shows it whole.
                let mut name = language.clone();
                if let Some(first) = name.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                let mut one = serde_json::Map::new();
                one.insert(name, json!(1));
                Value::Object(one)
            })
        }
        ["contributors"] => contributors(a, repo),
        ["forks"] if a.get() => Ok(shape::repos(&page(c, &format!("{r}/forks"), &a.query)?)),
        ["forks"] => {
            let ws = match a.field("organization").as_str() {
                Some(org) if !org.is_empty() => org.to_string(),
                _ => personal_workspace(c)?,
            };
            let mut body = json!({ "workspace": { "slug": ws } });
            if let Some(name) = a.field("name").as_str().filter(|n| !n.is_empty()) {
                body["name"] = json!(name);
            }
            Ok(shape::repo(
                &call(c, "POST", &format!("{r}/forks"), Some(&body))?,
                "admin",
            ))
        }
        ["stargazers"] => Ok(json!([])),
        ["subscribers"] => Ok(shape::users(
            c,
            &page(c, &format!("{r}/watchers"), &a.query)?,
        )),
        ["subscription"] => Err(missing("way to watch a repository through its API")),
        ["collaborators"] => {
            let list = page(c, &format!("{r}/permissions-config/users"), &a.query)?;
            Ok(Value::Array(
                list.iter()
                    .map(|p| {
                        let mut u = shape::user(c, p.at("user"));
                        let perm = p.s("permission");
                        u["permissions"] = json!({ "admin": perm == "admin", "maintain": perm == "admin", "push": perm != "read", "triage": true, "pull": true });
                        u["role_name"] = json!(perm);
                        u
                    })
                    .collect(),
            ))
        }
        ["collaborators", login] => {
            let id = user_by_login(c, login)?.s("account_id");
            let path = format!("{r}/permissions-config/users/{}", enc(&id));
            match m {
                "DELETE" => call(c, "DELETE", &path, None),
                "GET" => get(c, &path),
                _ => {
                    let permission = match a.field("permission").as_str().unwrap_or("push") {
                        "pull" | "read" | "triage" => "read",
                        "admin" | "maintain" => "admin",
                        _ => "write",
                    };
                    call(c, "PUT", &path, Some(&json!({ "permission": permission })))
                }
            }
        }
        ["keys"] if a.get() => {
            let list = page(c, &format!("{r}/deploy-keys"), &a.query)?;
            Ok(Value::Array(list.iter().map(deploy_key).collect()))
        }
        ["keys"] => Ok(deploy_key(&call(
            c,
            "POST",
            &format!("{r}/deploy-keys"),
            Some(&json!({ "key": a.field("key"), "label": a.field("title") })),
        )?)),
        ["keys", id] => call(c, "DELETE", &format!("{r}/deploy-keys/{id}"), None),
        ["actions", ..] => Err(missing(
            "GitHub Actions; its pipelines have their own page",
        )),
        _ => Err(missing(&format!(
            "equivalent of a repository's {}",
            rest.join("/")
        ))),
    }
}

/// A source archive's address on the site.
pub fn archive(repo: &str, git_ref: &str, format: &str) -> String {
    format!(
        "https://{}/{repo}/get/{}.{format}",
        crate::forge::BITBUCKET_HOST,
        enc(git_ref)
    )
}

fn deploy_key(k: &Value) -> Value {
    json!({ "id": k.i("id"), "title": k.s("label"), "key": k.s("key"), "read_only": true, "created_at": k.s("created_on"), "last_used": k.at("last_used").clone() })
}

/// The repository, with what the signed-in user may do to it.
fn repository(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let r = repo_api(repo);
    match a.method {
        "GET" => {
            let info = get(c, &r)?;
            Ok(shape::repo(&info, &permission(c, repo)))
        }
        "DELETE" => call(c, "DELETE", &r, None),
        _ => {
            let mut body = json!({});
            for (from, to) in [
                ("name", "name"),
                ("description", "description"),
                ("homepage", "website"),
            ] {
                if a.field(from).is_string() {
                    body[to] = a.field(from).clone();
                }
            }
            if let Some(private) = a.field("private").as_bool() {
                body["is_private"] = json!(private);
            }
            match a.field("visibility").as_str() {
                Some("private") => body["is_private"] = json!(true),
                Some("public") => body["is_private"] = json!(false),
                _ => {}
            }
            if let Some(branch) = a.field("default_branch").as_str() {
                body["mainbranch"] = json!({ "name": branch });
            }
            let updated = call(c, "PUT", &r, Some(&body))?;
            Ok(shape::repo(&updated, &permission(c, repo)))
        }
    }
}

/// `read`, `write` or `admin`: what the signed-in user may do here.
fn permission(c: &Client, repo: &str) -> String {
    let ws = repo.split('/').next().unwrap_or("");
    let query = enc(&format!("repository.full_name=\"{repo}\""));
    c.memo
        .get_or(&format!("permission:{repo}"), MINUTE, || {
            let found = values(
                c,
                &format!(
                    "/user/workspaces/{}/permissions/repositories?q={query}",
                    enc(ws)
                ),
            )
            .unwrap_or_default();
            Ok(json!(found
                .first()
                .map(|p| p.s("permission"))
                .unwrap_or_default()))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

fn branch(repo: &str, b: &Value) -> Value {
    let sha = b.s("target.hash");
    json!({
        "name": b.s("name"),
        "commit": {
            "sha": sha,
            "url": format!("/repos/{repo}/commits/{sha}"),
            "commit": { "message": b.s("target.message"), "author": { "date": b.s("target.date") } },
        },
        "protected": false,
    })
}

/// A ref Bitbucket can take in a path. Branch and tag names with slashes
/// in them can't go in `/src/{ref}/…`, so those become their commit.
pub fn resolve(c: &Client, repo: &str, git_ref: &str) -> Result<String> {
    if git_ref.is_empty() {
        return Ok("HEAD".into());
    }
    if !git_ref.contains('/') {
        return Ok(git_ref.to_string());
    }
    let r = repo_api(repo);
    let found = c
        .memo
        .get_or(&format!("ref:{repo}:{git_ref}"), MINUTE, || {
            let hash = get(c, &format!("{r}/refs/branches/{}", enc(git_ref)))
                .or_else(|_| get(c, &format!("{r}/refs/tags/{}", enc(git_ref))))
                .map(|v| v.s("target.hash"))?;
            Ok(json!(hash))
        })?;
    Ok(found.s(""))
}

/// The address of a path in the tree at a ref.
fn src(c: &Client, repo: &str, git_ref: &str, path: &str) -> Result<String> {
    let at = resolve(c, repo, git_ref)?;
    Ok(format!(
        "{}/src/{}/{}",
        repo_api(repo),
        enc(&at),
        enc_path(path.trim_matches('/'))
    ))
}

/// A directory's entries, in GitHub's contents shape.
fn listing(c: &Client, repo: &str, git_ref: &str, path: &str) -> Result<Vec<Value>> {
    let dir = src(c, repo, git_ref, path)?;
    let dir = if dir.ends_with('/') {
        dir
    } else {
        format!("{dir}/")
    };
    let entries = get_all(c, &dir, 40)?;
    Ok(entries.iter().map(|e| entry(repo, git_ref, e)).collect())
}

fn entry(repo: &str, git_ref: &str, e: &Value) -> Value {
    let path = e.s("path");
    let name = path.rsplit('/').next().unwrap_or("").to_string();
    let attributes: Vec<String> = e.list("attributes").iter().map(|a| a.s("")).collect();
    let kind = match e.s("type").as_str() {
        "commit_directory" => "dir",
        _ if attributes.iter().any(|a| a == "link") => "symlink",
        _ if attributes.iter().any(|a| a == "subrepository") => "submodule",
        _ => "file",
    };
    let web = format!(
        "https://{}/{repo}/src/{git_ref}/{path}",
        crate::forge::BITBUCKET_HOST
    );
    json!({
        "name": name,
        "path": path,
        "type": kind,
        "size": e.i("size"),
        "sha": e.s("commit.hash"),
        "html_url": web,
        "download_url": format!("https://{}/{repo}/raw/{git_ref}/{path}", crate::forge::BITBUCKET_HOST),
        "url": format!("/repos/{repo}/contents/{path}?ref={}", enc(git_ref)),
    })
}

/// A directory's entries, or a file with its contents.
fn contents(a: &Ask, repo: &str, path: &str) -> Result<Value> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("").to_string();
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Ok(Value::Array(listing(c, repo, &git_ref, "")?));
    }
    let meta = get(c, &format!("{}?format=meta", src(c, repo, &git_ref, path)?))?;
    if meta.s("type") == "commit_directory" {
        return Ok(Value::Array(listing(c, repo, &git_ref, path)?));
    }
    let raw = c.raw("GET", &src(c, repo, &git_ref, path)?, None, Some("*/*"))?;
    if raw.status >= 400 {
        return Err(api::failure(&raw));
    }
    use base64::Engine as _;
    let mut file = entry(repo, &git_ref, &meta);
    file["encoding"] = json!("base64");
    file["content"] = json!(base64::engine::general_purpose::STANDARD.encode(raw.body.as_bytes()));
    Ok(file)
}

/// Raw text: a file, or a directory's README.
pub fn text(a: &Ask, repo: &str, rest: &[&str]) -> Result<String> {
    let c = a.c;
    let git_ref = a.query.get("ref").unwrap_or("").to_string();
    let path = match rest {
        ["contents", path @ ..] => path.join("/"),
        ["readme", dir @ ..] => {
            let dir = dir.join("/");
            let entries = listing(c, repo, &git_ref, &dir)?;
            let readme = entries
                .iter()
                .filter(|e| e.s("type") == "file")
                .map(|e| e.s("path"))
                .filter(|p| {
                    p.rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_lowercase()
                        .starts_with("readme")
                })
                // README.md before README.txt before README.
                .min_by_key(|p| {
                    let lower = p.to_lowercase();
                    (
                        !(lower.ends_with(".md") || lower.ends_with(".markdown")),
                        lower.len(),
                    )
                })
                .ok_or_else(|| {
                    anyhow::Error::new(api::Status {
                        code: 404,
                        message: "No README here.".into(),
                    })
                })?;
            readme
        }
        _ => return Err(missing("text at that path")),
    };
    let raw = c.raw("GET", &src(c, repo, &git_ref, &path)?, None, Some("*/*"))?;
    if raw.status >= 400 {
        return Err(api::failure(&raw));
    }
    Ok(raw.body)
}

/// Every path under a ref, as GitHub's recursive tree.
fn tree(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let root = src(c, repo, sha, "")?;
    let root = format!("{}/", root.trim_end_matches('/'));
    let entries = get_all(c, &format!("{root}?max_depth=30"), 100)?;
    Ok(json!({
        "sha": sha,
        "tree": entries.iter().map(|e| json!({
            "path": e.s("path"),
            "type": if e.s("type") == "commit_directory" { "tree" } else { "blob" },
            "size": e.i("size"),
        })).collect::<Vec<_>>(),
        "truncated": entries.len() >= 5000,
    }))
}

fn commits(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    let git_ref = a.query.get("sha").unwrap_or("");
    let mut path = if git_ref.is_empty() {
        format!("{}/commits", repo_api(repo))
    } else {
        format!(
            "{}/commits/{}",
            repo_api(repo),
            enc(&resolve(c, repo, git_ref)?)
        )
    };
    if let Some(file) = a.query.get("path") {
        path = crate::hub::with_query(&path, &format!("path={}", enc(file)));
    }
    let mut list = page(c, &path, &a.query)?;
    // Bitbucket can't filter by author; this page can.
    if let Some(author) = a.query.get("author") {
        let author = author.to_lowercase();
        list.retain(|v| {
            shape::login_of(v.at("author.user")).to_lowercase() == author
                || v.s("author.raw").to_lowercase().contains(&author)
        });
    }
    Ok(Value::Array(
        list.iter().map(|v| shape::commit(c, repo, v)).collect(),
    ))
}

/// A whole diff's text, kept: a commit's never changes.
fn diff_text(c: &Client, repo: &str, spec: &str) -> String {
    c.memo
        .get_or(&format!("diff:{repo}:{spec}"), MINUTE * 10, || {
            Ok(json!(c
                .raw_text(
                    &format!("{}/diff/{}", repo_api(repo), enc(spec)),
                    "text/plain"
                )
                .unwrap_or_default()))
        })
        .map(|v| v.s(""))
        .unwrap_or_default()
}

/// What changed between two revisions, `spec` being `a..b` or one commit.
pub fn changed_files(c: &Client, repo: &str, spec: &str) -> Result<Value> {
    let stats = get_all(c, &format!("{}/diffstat/{}", repo_api(repo), enc(spec)), 20)?;
    Ok(shape::files(&stats, &diff_text(c, repo, spec)))
}

pub fn commit(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let v = get(c, &format!("{}/commit/{}", repo_api(repo), enc(sha)))?;
    let mut shaped = shape::commit(c, repo, &v);
    let files = changed_files(c, repo, &v.s("hash")).unwrap_or(json!([]));
    let (adds, dels) = files.list("").iter().fold((0, 0), |(a, d), f| {
        (a + f.i("additions"), d + f.i("deletions"))
    });
    shaped["stats"] = json!({ "additions": adds, "deletions": dels, "total": adds + dels });
    shaped["files"] = files;
    Ok(shaped)
}

fn commit_comments(a: &Ask, repo: &str, sha: &str) -> Result<Value> {
    let c = a.c;
    let base = format!("{}/commit/{}/comments", repo_api(repo), enc(sha));
    if a.get() {
        let list = get_all(c, &base, 10)?;
        return Ok(Value::Array(
            list.iter()
                .map(|cm| {
                    let url = format!("/repos/{repo}/commits/{sha}/comments/{}", cm.i("id"));
                    let mut shaped = shape::comment(c, cm, &url);
                    shaped["line"] = cm.at("inline.to").clone();
                    shaped
                })
                .collect(),
        ));
    }
    let mut body = json!({ "content": { "raw": a.field("body") } });
    if let Some(path) = a.field("path").as_str() {
        body["inline"] = json!({ "path": path, "to": a.field("line") });
    }
    call(c, "POST", &base, Some(&body))
}

pub fn statuses(c: &Client, repo: &str, sha: &str) -> Result<Vec<Value>> {
    get_all(
        c,
        &format!("{}/commit/{}/statuses", repo_api(repo), enc(sha)),
        4,
    )
}

fn check_runs(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let list = statuses(c, repo, sha)?;
    let runs: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, s)| shape::check_run(s, i))
        .collect();
    Ok(json!({ "total_count": runs.len(), "check_runs": runs }))
}

/// A build state as a commit status's.
fn combined(state: &str) -> &'static str {
    match state {
        "SUCCESSFUL" => "success",
        "FAILED" => "failure",
        "STOPPED" => "error",
        _ => "pending",
    }
}

fn combined_status(c: &Client, repo: &str, sha: &str) -> Result<Value> {
    let list = statuses(c, repo, sha)?;
    let state = match shape::rollup(&list).s("state").as_str() {
        "SUCCESS" => "success",
        "FAILURE" => "failure",
        _ => "pending",
    };
    Ok(json!({
        "state": state,
        "total_count": list.len(),
        "statuses": list.iter().map(|s| json!({ "state": combined(&s.s("state")), "context": s.s("name"), "description": s.s("description"), "target_url": s.s("url") })).collect::<Vec<_>>(),
    }))
}

/// `base...head`: the commits on head that base lacks, and the files they
/// change.
fn compare(c: &Client, repo: &str, range: &str) -> Result<Value> {
    let (base, head) = range
        .split_once("...")
        .or_else(|| range.split_once(".."))
        .ok_or_else(|| missing("comparison without two sides"))?;
    let r = repo_api(repo);
    let (base, head) = (resolve(c, repo, base)?, resolve(c, repo, head)?);
    let ahead = get_all(
        c,
        &format!("{r}/commits/{}?exclude={}", enc(&head), enc(&base)),
        5,
    )?;
    let behind = values(
        c,
        &format!(
            "{r}/commits/{}?exclude={}&pagelen=50",
            enc(&base),
            enc(&head)
        ),
    )?;
    let merge_base = get(
        c,
        &format!("{r}/merge-base/{}", enc(&format!("{head}..{base}"))),
    )
    .map(|v| v.s("hash"))
    .unwrap_or_default();
    let files = if ahead.is_empty() {
        json!([])
    } else {
        changed_files(c, repo, &format!("{head}..{base}"))?
    };
    let status = match (ahead.len(), behind.len()) {
        (0, 0) => "identical",
        (_, 0) => "ahead",
        (0, _) => "behind",
        _ => "diverged",
    };
    // GitHub lists them oldest first.
    let commits: Vec<Value> = ahead
        .iter()
        .rev()
        .map(|v| shape::commit(c, repo, v))
        .collect();
    Ok(json!({
        "status": status,
        "ahead_by": ahead.len(),
        "behind_by": behind.len(),
        "total_commits": ahead.len(),
        "commits": commits,
        "files": files,
        "merge_base_commit": { "sha": merge_base },
        "html_url": format!("https://{}/{repo}/branches/compare/{head}%0D{base}", crate::forge::BITBUCKET_HOST),
    }))
}

/// Who committed most lately, from the default branch's last commits:
/// Bitbucket doesn't count contributions.
fn contributors(a: &Ask, repo: &str) -> Result<Value> {
    let c = a.c;
    if a.query.page() > 1 {
        return Ok(json!([]));
    }
    let recent = c
        .memo
        .get_or(&format!("contributors:{repo}"), MINUTE * 10, || {
            let list = get_all(c, &format!("{}/commits", repo_api(repo)), 6)?;
            let mut counts: HashMap<String, (i64, Value)> = HashMap::new();
            for v in &list {
                let who = v.at("author.user");
                let (login, person) = if who.is_null() {
                    let (name, _) = shape::author_parts(&v.s("author.raw"));
                    (name.clone(), json!({ "login": name, "avatar_url": "" }))
                } else {
                    (shape::login_of(who), shape::user(c, who))
                };
                let slot = counts.entry(login).or_insert((0, person));
                slot.0 += 1;
            }
            let mut people: Vec<(i64, Value)> = counts.into_values().collect();
            people.sort_by_key(|p| std::cmp::Reverse(p.0));
            Ok(Value::Array(
                people
                    .into_iter()
                    .map(|(n, mut person)| {
                        person["contributions"] = json!(n);
                        person
                    })
                    .collect(),
            ))
        })?;
    let per = a.query.per_page() as usize;
    Ok(Value::Array(
        recent.list("").iter().take(per).cloned().collect(),
    ))
}
