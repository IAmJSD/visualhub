//! Resolving a pull request's merge conflicts in the app, as GitHub's web
//! editor does, but through the public Git Data API: three-way merge every
//! file both branches changed, let each conflict be settled by picking a
//! side (or both, or by editing the file), then commit the merge of the
//! base branch into the head branch and move the branch to it.

use crate::api::Client;
use crate::hub::{on, Act, Hub, PullTab, Route};
use crate::json::{enc_path, Json as _};
use crate::ui::{icon, palette};
use crate::widgets::{self, rgb};
use anyhow::{anyhow, bail, Result};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Long enough that no file's own text looks like one of our markers.
const MARK: usize = 13;

#[derive(Clone, Copy, PartialEq)]
pub enum Pick {
    /// The head branch's lines: "current", the branch being merged into.
    Current,
    /// The base branch's lines: "incoming".
    Incoming,
    Both,
}

#[derive(Clone)]
pub enum Chunk {
    Clean(Vec<String>),
    Conflict { current: Vec<String>, incoming: Vec<String>, pick: Option<Pick> },
}

pub struct ConflictFile {
    pub path: String,
    pub chunks: Vec<Chunk>,
    /// The text as edited by hand, which replaces the chunks once set.
    pub edited: Option<String>,
    /// One side deleted it; an empty result deletes it.
    pub deletable: bool,
    /// Not text (or too big to fetch); settled by keeping one side whole.
    pub binary: Option<(Option<String>, Option<String>)>,
    pub binary_pick: Option<Pick>,
}

impl ConflictFile {
    fn conflicts(&self) -> usize {
        self.chunks.iter().filter(|c| matches!(c, Chunk::Conflict { .. })).count()
    }

    fn unresolved(&self) -> usize {
        if self.binary.is_some() {
            return usize::from(self.binary_pick.is_none());
        }
        match &self.edited {
            Some(text) => text.lines().filter(|l| l.starts_with(&"<".repeat(MARK))).count(),
            None => self.chunks.iter().filter(|c| matches!(c, Chunk::Conflict { pick: None, .. })).count(),
        }
    }

    /// The file as it stands, unresolved conflicts written out with
    /// markers (so editing by hand starts from what git would show).
    fn text(&self, current: &str, incoming: &str) -> String {
        if let Some(text) = &self.edited {
            return text.clone();
        }
        let mut out: Vec<String> = Vec::new();
        for chunk in &self.chunks {
            match chunk {
                Chunk::Clean(lines) => out.extend(lines.iter().cloned()),
                Chunk::Conflict { current: c, incoming: i, pick } => match pick {
                    Some(Pick::Current) => out.extend(c.iter().cloned()),
                    Some(Pick::Incoming) => out.extend(i.iter().cloned()),
                    Some(Pick::Both) => {
                        out.extend(c.iter().cloned());
                        out.extend(i.iter().cloned());
                    }
                    None => {
                        out.push(format!("{} {current}", "<".repeat(MARK)));
                        out.extend(c.iter().cloned());
                        out.push("=".repeat(MARK));
                        out.extend(i.iter().cloned());
                        out.push(format!("{} {incoming}", ">".repeat(MARK)));
                    }
                },
            }
        }
        out.join("")
    }
}

/// A change only the base branch made, to carry into the merge as is.
pub struct BaseChange {
    path: String,
    /// The blob to use, or `None` to delete the path.
    sha: Option<String>,
    mode: String,
}

pub struct Session {
    head_repo: String,
    head_ref: String,
    base_ref: String,
    head_sha: String,
    base_sha: String,
    pub files: Vec<ConflictFile>,
    /// Both sides changed these, but they merged cleanly: (path, text,
    /// mode, deleted).
    merged: Vec<(String, String, String)>,
    base_only: Vec<BaseChange>,
    modes: HashMap<String, String>,
    pub selected: usize,
    pub committing: bool,
}

pub enum SessionLoad {
    Loading,
    Ready(Box<Session>),
    Failed(String),
}

/// Lines with their endings kept, so a file's last newline survives.
fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// diffy's output (with our marker length) as clean runs and conflicts.
fn chunks(merged: &str) -> Vec<Chunk> {
    let (open, mid, close) = ("<".repeat(MARK), "=".repeat(MARK), ">".repeat(MARK));
    let mut out = Vec::new();
    let mut clean = Vec::new();
    let mut side: Option<(Vec<String>, Vec<String>, bool)> = None;
    for line in lines(merged) {
        let bare = line.trim_end_matches(['\n', '\r']);
        match &mut side {
            None if bare.starts_with(&open) => {
                if !clean.is_empty() {
                    out.push(Chunk::Clean(std::mem::take(&mut clean)));
                }
                side = Some((Vec::new(), Vec::new(), false));
            }
            None => clean.push(line),
            Some((_, _, in_theirs)) if bare == mid => *in_theirs = true,
            Some(_) if bare.starts_with(&close) => {
                let (current, incoming, _) = side.take().unwrap();
                out.push(Chunk::Conflict { current, incoming, pick: None });
            }
            Some((c, i, in_theirs)) => {
                if *in_theirs { i.push(line) } else { c.push(line) }
            }
        }
    }
    if !clean.is_empty() {
        out.push(Chunk::Clean(clean));
    }
    out
}

/// A file at a commit: `Ok(None)` when it isn't there, `Err` when it
/// isn't text we can merge.
fn file_at(client: &Client, repo: &str, path: &str, sha: &str) -> Result<Option<String>, ()> {
    use base64::Engine as _;
    let reply = client.send("GET", &format!("/repos/{repo}/contents/{}?ref={sha}", enc_path(path)), None, None).map_err(|_| ())?;
    if reply.status == 404 {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&reply.body).map_err(|_| ())?;
    if v.s("encoding") != "base64" {
        return Err(());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(v.s("content").replace('\n', "")).map_err(|_| ())?;
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

/// Every path a commit's tree holds, with its mode.
fn modes(client: &Client, repo: &str, sha: &str) -> HashMap<String, String> {
    client
        .get(&format!("/repos/{repo}/git/trees/{sha}?recursive=1"))
        .map(|t| t.list("tree").iter().map(|e| (e.s("path"), e.s("mode"))).collect())
        .unwrap_or_default()
}

pub fn load(client: &Client, repo: &str, number: u64) -> Result<Session> {
    let pr = client.get(&format!("/repos/{repo}/pulls/{number}"))?;
    let (base_sha, head_sha) = (pr.s("base.sha"), pr.s("head.sha"));
    let head_repo = if pr.s("head.repo.full_name").is_empty() { repo.to_string() } else { pr.s("head.repo.full_name") };
    // What each side changed since they parted.
    let head_side = client.get(&format!("/repos/{repo}/compare/{base_sha}...{head_sha}"))?;
    let base_side = client.get(&format!("/repos/{repo}/compare/{head_sha}...{base_sha}"))?;
    let ancestor = head_side.s("merge_base_commit.sha");
    if ancestor.is_empty() {
        bail!("GitHub couldn't find where the branches parted.");
    }
    let touched = |cmp: &Value| -> HashSet<String> {
        cmp.list("files")
            .iter()
            .flat_map(|f| [f.s("filename"), f.s("previous_filename")])
            .filter(|p| !p.is_empty())
            .collect()
    };
    let head_paths = touched(&head_side);
    let base_modes = modes(client, repo, &base_sha);
    let head_modes = modes(client, repo, &head_sha);

    let mut files = Vec::new();
    let mut merged = Vec::new();
    let mut base_only = Vec::new();
    let mut done = HashSet::new();
    for f in base_side.list("files") {
        let path = f.s("filename");
        let moved_from = f.s("previous_filename");
        for (p, gone) in [(path.clone(), f.s("status") == "removed"), (moved_from.clone(), true)] {
            if p.is_empty() || !done.insert(p.clone()) {
                continue;
            }
            if !head_paths.contains(&p) {
                base_only.push(BaseChange {
                    sha: (!gone).then(|| f.s("sha")),
                    mode: base_modes.get(&p).cloned().unwrap_or_else(|| "100644".into()),
                    path: p,
                });
                continue;
            }
            // Both changed it: merge the three versions.
            let mode = head_modes.get(&p).or(base_modes.get(&p)).cloned().unwrap_or_else(|| "100644".into());
            let sides = (file_at(client, repo, &p, &ancestor), file_at(client, repo, &p, &head_sha), file_at(client, repo, &p, &base_sha));
            match sides {
                (Ok(a), Ok(cur), Ok(inc)) => {
                    let deletable = cur.is_none() || inc.is_none();
                    let (a, cur, inc) = (a.unwrap_or_default(), cur.unwrap_or_default(), inc.unwrap_or_default());
                    let mut opts = diffy::MergeOptions::new();
                    opts.set_conflict_style(diffy::ConflictStyle::Merge).set_conflict_marker_length(MARK);
                    match opts.merge(&a, &cur, &inc) {
                        Ok(text) => merged.push((p, text, mode)),
                        Err(text) => files.push(ConflictFile { path: p, chunks: chunks(&text), edited: None, deletable, binary: None, binary_pick: None }),
                    }
                }
                _ => files.push(ConflictFile {
                    path: p.clone(),
                    chunks: Vec::new(),
                    edited: None,
                    deletable: false,
                    binary: Some((head_modes.get(&p).cloned(), base_modes.get(&p).cloned())),
                    binary_pick: None,
                }),
            }
        }
    }
    let mut modes = head_modes;
    modes.extend(base_modes);
    Ok(Session {
        head_repo,
        head_ref: pr.s("head.ref"),
        base_ref: pr.s("base.ref"),
        head_sha,
        base_sha,
        files,
        merged,
        base_only,
        modes,
        selected: 0,
        committing: false,
    })
}

/// What committing needs, copied out of the session so it can run on
/// the background executor.
struct Plan {
    repo: String,
    head_repo: String,
    head_ref: String,
    base_ref: String,
    head_sha: String,
    base_sha: String,
    /// (path, text or None to delete, mode)
    writes: Vec<(String, Option<String>, String)>,
    base_only: Vec<(String, Option<String>, String)>,
}

fn commit(client: &Client, plan: &Plan) -> Result<String> {
    let same_repo = plan.repo == plan.head_repo;
    let head_tree = client.get(&format!("/repos/{}/git/commits/{}", plan.head_repo, plan.head_sha))?.s("tree.sha");
    let blob = |text: &str| -> Result<String> {
        Ok(client.json("POST", &format!("/repos/{}/git/blobs", plan.head_repo), Some(&json!({ "content": text, "encoding": "utf-8" })))?.s("sha"))
    };
    let mut tree = Vec::new();
    for (path, text, mode) in &plan.writes {
        let sha = match text {
            Some(text) => Value::String(blob(text)?),
            None => Value::Null,
        };
        tree.push(json!({ "path": path, "mode": mode, "type": "blob", "sha": sha }));
    }
    for (path, sha, mode) in &plan.base_only {
        let sha = match sha {
            // A fork doesn't have the base's blobs; copy them across.
            Some(sha) if !same_repo => {
                let raw = client.get(&format!("/repos/{}/git/blobs/{sha}", plan.repo))?;
                let content = raw.s("content").replace('\n', "");
                Value::String(client.json("POST", &format!("/repos/{}/git/blobs", plan.head_repo), Some(&json!({ "content": content, "encoding": "base64" })))?.s("sha"))
            }
            Some(sha) => Value::String(sha.clone()),
            None => Value::Null,
        };
        tree.push(json!({ "path": path, "mode": mode, "type": "blob", "sha": sha }));
    }
    let new_tree = client.json("POST", &format!("/repos/{}/git/trees", plan.head_repo), Some(&json!({ "base_tree": head_tree, "tree": tree })))?.s("sha");
    let message = format!("Merge branch '{}' into {}", plan.base_ref, plan.head_ref);
    let commit = client
        .json("POST", &format!("/repos/{}/git/commits", plan.head_repo), Some(&json!({ "message": message, "tree": new_tree, "parents": [plan.head_sha, plan.base_sha] })))?
        .s("sha");
    client
        .json("PATCH", &format!("/repos/{}/git/refs/heads/{}", plan.head_repo, enc_path(&plan.head_ref)), Some(&json!({ "sha": commit, "force": false })))
        .map_err(|e| anyhow!("Couldn't move {} to the merge: {e:#}", plan.head_ref))?;
    Ok(commit)
}

fn key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

impl Hub {
    fn session(&mut self, repo: &str, number: u64) -> Option<&mut Session> {
        match self.conflicts.get_mut(&key(repo, number)) {
            Some(SessionLoad::Ready(s)) => Some(s),
            _ => None,
        }
    }

    fn start_session(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let k = key(repo, number);
        self.conflicts.insert(k.clone(), SessionLoad::Loading);
        let repo = repo.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { load(&client, &repo, number) }).await;
            this.update(cx, |hub, cx| {
                hub.conflicts.insert(
                    k,
                    match result {
                        Ok(s) => SessionLoad::Ready(Box::new(s)),
                        Err(e) => SessionLoad::Failed(format!("{e:#}")),
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn commit_resolution(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let edits: HashMap<String, String> = self.fields.iter().filter_map(|(k, f)| k.strip_prefix(&format!("conflict:{repo}#{number}:")).map(|p| (p.to_string(), f.text.clone()))).collect();
        let Some(s) = self.session(repo, number) else { return };
        let (current, incoming) = (s.head_ref.clone(), s.base_ref.clone());
        let mut writes: Vec<(String, Option<String>, String)> = s.merged.iter().map(|(p, t, m)| (p.clone(), Some(t.clone()), m.clone())).collect();
        for f in &s.files {
            let mode = s.modes.get(&f.path).cloned().unwrap_or_else(|| "100644".into());
            if f.binary.is_some() {
                // Kept whole from one side; see `binaries` below.
                continue;
            }
            let text = match (&f.edited, edits.get(&f.path)) {
                (Some(_), Some(field)) => field.clone(),
                _ => f.text(&current, &incoming),
            };
            let delete = f.deletable && text.trim().is_empty();
            writes.push((f.path.clone(), (!delete).then_some(text), mode));
        }
        let binaries: Vec<(String, Pick)> = s.files.iter().filter_map(|f| Some((f.path.clone(), f.binary_pick?))).collect();
        let base_only = s.base_only.iter().map(|c| (c.path.clone(), c.sha.clone(), c.mode.clone())).collect();
        let plan = Plan {
            repo: repo.to_string(),
            head_repo: s.head_repo.clone(),
            head_ref: s.head_ref.clone(),
            base_ref: s.base_ref.clone(),
            head_sha: s.head_sha.clone(),
            base_sha: s.base_sha.clone(),
            writes,
            base_only,
        };
        s.committing = true;
        let repo = repo.to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut plan = plan;
                    // A binary keeps the picked side's blob; the head's is
                    // already in the tree, so only "incoming" needs a write.
                    for (path, pick) in binaries {
                        if pick == Pick::Incoming {
                            let sha = client
                                .get(&format!("/repos/{}/contents/{}?ref={}", plan.repo, enc_path(&path), plan.base_sha))
                                .map(|v| v.s("sha"))
                                .ok()
                                .filter(|s| !s.is_empty());
                            plan.base_only.push((path, sha, "100644".into()));
                        }
                    }
                    commit(&client, &plan)
                })
                .await;
            this.update(cx, |hub, cx| {
                match result {
                    Ok(_) => {
                        hub.conflicts.remove(&key(&repo, number));
                        hub.invalidate(&format!("/repos/{repo}"));
                        hub.toast("Merge committed — the conflicts are resolved", false, cx);
                        hub.go(Route::Pull { repo: repo.clone(), number, tab: PullTab::Conversation }, cx);
                    }
                    Err(e) => {
                        if let Some(s) = hub.session(&repo, number) {
                            s.committing = false;
                        }
                        hub.toast(format!("{e:#}"), true, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn conflicts_page(&mut self, repo: &str, number: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let k = key(repo, number);
        match self.conflicts.get(&k) {
            None => {
                self.start_session(repo, number, cx);
                return widgets::page().child(widgets::loading()).into_any_element();
            }
            Some(SessionLoad::Loading) => {
                return widgets::page()
                    .child(widgets::title("Resolving conflicts"))
                    .child(widgets::dim("Merging every file both branches changed…"))
                    .child(widgets::loading())
                    .into_any_element()
            }
            Some(SessionLoad::Failed(e)) => {
                let e = e.clone();
                let (r, n) = (repo.to_string(), number);
                return widgets::page()
                    .child(widgets::title("Resolving conflicts"))
                    .child(widgets::error_box(&e))
                    .child(widgets::btn("conflicts-retry", "Try again", Act::run(move |hub, _, cx| {
                        hub.conflicts.remove(&key(&r, n));
                        cx.notify();
                    })))
                    .into_any_element();
            }
            Some(SessionLoad::Ready(_)) => {}
        }
        let Some(s) = self.session(repo, number) else { return div().into_any_element() };
        let (current, incoming) = (s.head_ref.clone(), s.base_ref.clone());
        let left: usize = s.files.iter().map(|f| f.unresolved()).sum();
        let committing = s.committing;
        let selected = s.selected.min(s.files.len().saturating_sub(1));
        let auto = s.merged.len();
        let file_rows: Vec<(String, usize, usize)> = s.files.iter().map(|f| (f.path.clone(), f.conflicts().max(1), f.unresolved())).collect();

        let (r, n) = (repo.to_string(), number);
        let header = widgets::row()
            .gap_3()
            .child(
                widgets::col()
                    .flex_1()
                    .gap_1()
                    .child(widgets::title("Resolving conflicts"))
                    .child(
                        widgets::row()
                            .gap_1()
                            .child(widgets::dim("between"))
                            .child(widgets::tag(current.clone(), widgets::gray()))
                            .child(widgets::dim("and"))
                            .child(widgets::tag(incoming.clone(), widgets::gray()))
                            .when(auto > 0, |d| d.child(widgets::faint(format!("  ·  {auto} more file{} merged cleanly", if auto == 1 { "" } else { "s" })))),
                    ),
            )
            .child(widgets::faint(if left == 0 { "All conflicts resolved".to_string() } else { format!("{left} conflict{} left", if left == 1 { "" } else { "s" }) }))
            .child(
                widgets::go_btn("conflicts-commit", if committing { "Committing…" } else { "Commit merge" }, Act::run(move |hub, _, cx| hub.commit_resolution(&r, n, cx)))
                    .h(px(32.0))
                    .px_4()
                    .disabled(left > 0 || committing),
            );

        let mut list = widgets::card().w(px(280.0)).flex_none();
        for (i, (path, total, open)) in file_rows.iter().enumerate() {
            let (r, n) = (repo.to_string(), number);
            list = list.child(
                div()
                    .id(("conflict-file", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .when(i == selected, |d| d.bg(rgb(p.selection_bg)))
                    .when(i != selected, |d| d.hover(|s| s.bg(rgb(p.hover))))
                    .on_click(on(Act::run(move |hub, _, cx| {
                        if let Some(s) = hub.session(&r, n) {
                            s.selected = i;
                        }
                        cx.notify();
                    })))
                    .child(icon(if *open == 0 { "check-circle" } else { "alert" }, 14.0, if *open == 0 { widgets::green() } else { widgets::yellow() }))
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_ellipsis().overflow_hidden().whitespace_nowrap().font_family(widgets::MONO).text_size(px(12.0)).child(path.rsplit('/').next().unwrap_or(path).to_string()))
                            .child(widgets::faint(if *open == 0 { "Resolved".to_string() } else { format!("{open} of {total} conflict{} left", if *total == 1 { "" } else { "s" }) })),
                    ),
            );
        }

        let body = self.conflict_file(repo, number, selected, &current, &incoming, cx);
        widgets::page()
            .max_w(px(1400.0))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_4()
                    .child(list)
                    .child(div().flex_1().min_w_0().child(body)),
            )
            .into_any_element()
    }

    /// One conflicted file: its clean stretches folded, each conflict with
    /// its two sides and the choices, or the whole file to edit by hand.
    fn conflict_file(&mut self, repo: &str, number: u64, index: usize, current: &str, incoming: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let field = {
            let Some(s) = self.session(repo, number) else { return div().into_any_element() };
            let Some(f) = s.files.get(index) else {
                return widgets::card().child(widgets::empty("No files with conflicts — commit the merge to finish.")).into_any_element();
            };
            format!("conflict:{repo}#{number}:{}", f.path)
        };
        let (file_path, chunks, editing, binary, binary_pick) = {
            let s = self.session(repo, number).unwrap();
            let f = &s.files[index];
            (f.path.clone(), f.chunks.clone(), f.edited.is_some(), f.binary.is_some(), f.binary_pick)
        };
        let (r, n) = (repo.to_string(), number);
        let set = move |f: Box<dyn Fn(&mut ConflictFile)>| {
            let (r, n) = (r.clone(), n);
            let f = std::rc::Rc::new(f);
            Act::run(move |hub, _, cx| {
                if let Some(s) = hub.session(&r, n) {
                    if let Some(file) = s.files.get_mut(index) {
                        f(file);
                    }
                }
                cx.notify();
            })
        };

        let card = widgets::card().child(
            widgets::card_header()
                .child(icon("file", 14.0, p.text_dim))
                .child(div().flex_1().font_family(widgets::MONO).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).child(file_path.clone()))
                .when(!binary, |d| {
                    let (cur, inc) = (current.to_string(), incoming.to_string());
                    let field = field.clone();
                    d.child(widgets::btn(
                        "conflict-edit",
                        if editing { "Back to choices" } else { "Edit file" },
                        Act::run({
                            let (r, n) = (repo.to_string(), number);
                            move |hub, _, cx| {
                                let text = hub.session(&r, n).and_then(|s| {
                                    let f = s.files.get_mut(index)?;
                                    if f.edited.take().is_some() {
                                        None
                                    } else {
                                        let t = f.text(&cur, &inc);
                                        f.edited = Some(t.clone());
                                        Some(t)
                                    }
                                });
                                if let Some(t) = text {
                                    hub.set_field(&field, t);
                                }
                                cx.notify();
                            }
                        }),
                    ))
                }),
        );

        if binary {
            let pick = |label: &'static str, which: Pick| {
                let chosen = binary_pick == Some(which);
                widgets::btn(ElementId::Name(format!("bin-{label}").into()), if chosen { format!("✓ Keep {label}") } else { format!("Keep {label}") }, set(Box::new(move |f| f.binary_pick = Some(which))))
            };
            return card
                .child(
                    widgets::col()
                        .p_4()
                        .gap_3()
                        .child(widgets::dim("This file isn't text the editor can merge. Keep one branch's version whole."))
                        .child(widgets::row().child(pick("current", Pick::Current)).child(pick("incoming", Pick::Incoming))),
                )
                .into_any_element();
        }

        if editing {
            // Keep the session's copy in step with the box.
            let text = self.field_text(&field);
            if let Some(f) = self.session(repo, number).and_then(|s| s.files.get_mut(index)) {
                f.edited = Some(text);
            }
            let editor = self.textarea(&field, "", cx).min_h(px(480.0)).font_family(widgets::MONO).text_size(px(12.0));
            return card
                .child(div().p_2().child(widgets::faint(format!("Remove every {}… / {} / {}… block to resolve this file.", "<".repeat(7), "=".repeat(7), ">".repeat(7)))))
                .child(div().p_2().child(editor))
                .into_any_element();
        }

        let mut body = div().flex().flex_col().font_family(widgets::MONO).text_size(px(12.0)).line_height(px(20.0)).py_1();
        let line_el = |text: &str, bg: Option<u32>| {
            div()
                .px_4()
                .whitespace_nowrap()
                .when_some(bg, |d, c| d.bg(rgb(c)))
                .child(if text.trim_end().is_empty() { " ".to_string() } else { text.trim_end_matches(['\n', '\r']).replace('\t', "    ") })
        };
        let light = crate::ui::is_light();
        let (cur_bg, inc_bg, cur_head, inc_head) = if light {
            (0xE6FFEC, 0xDDF4FF, 0xCCFFD8, 0xB6E3FF)
        } else {
            (0x12261E, 0x121D2F, 0x1B4721, 0x1F3A5F)
        };
        for (ci, chunk) in chunks.iter().enumerate() {
            match chunk {
                Chunk::Clean(lines) => {
                    // Long unchanged stretches fold to their ends.
                    let fold_key = format!("conflict-fold:{repo}#{number}:{index}:{ci}");
                    let open = self.is_open(&fold_key);
                    if lines.len() > 8 && !open {
                        for l in &lines[..3] {
                            body = body.child(line_el(l, None));
                        }
                        body = body.child(
                            div()
                                .id(("conflict-fold", ci))
                                .px_4()
                                .text_color(rgb(p.accent_hover))
                                .cursor_pointer()
                                .bg(rgb(p.deep_bg))
                                .on_click(on(Act::run(move |hub, _, cx| {
                                    hub.open.insert(fold_key.clone());
                                    cx.notify();
                                })))
                                .child(format!("⋯ {} unchanged lines", lines.len() - 6)),
                        );
                        for l in &lines[lines.len() - 3..] {
                            body = body.child(line_el(l, None));
                        }
                    } else {
                        for l in lines {
                            body = body.child(line_el(l, None));
                        }
                    }
                }
                Chunk::Conflict { current: c, incoming: i, pick } => {
                    let choose = |which: Option<Pick>| {
                        set(Box::new(move |f| {
                            if let Some(Chunk::Conflict { pick, .. }) = f.chunks.get_mut(ci) {
                                *pick = which;
                            }
                        }))
                    };
                    match pick {
                        None => {
                            body = body
                                .child(
                                    widgets::row()
                                        .gap_1()
                                        .px_4()
                                        .pt_2()
                                        .font_family(".SystemUIFont")
                                        .child(crate::ui::Link::new(("accept-current", ci), "Accept current change").on_click(on(choose(Some(Pick::Current)))))
                                        .child(widgets::faint("|"))
                                        .child(crate::ui::Link::new(("accept-incoming", ci), "Accept incoming change").on_click(on(choose(Some(Pick::Incoming)))))
                                        .child(widgets::faint("|"))
                                        .child(crate::ui::Link::new(("accept-both", ci), "Accept both changes").on_click(on(choose(Some(Pick::Both))))),
                                )
                                .child(line_el(&format!("{} {current} (current change)", "<".repeat(7)), Some(cur_head)));
                            for l in c {
                                body = body.child(line_el(l, Some(cur_bg)));
                            }
                            body = body.child(line_el(&"=".repeat(7), Some(p.deep_bg)));
                            for l in i {
                                body = body.child(line_el(l, Some(inc_bg)));
                            }
                            body = body.child(line_el(&format!("{} {incoming} (incoming change)", ">".repeat(7)), Some(inc_head)));
                        }
                        Some(which) => {
                            let label = match which {
                                Pick::Current => "Kept the current change",
                                Pick::Incoming => "Kept the incoming change",
                                Pick::Both => "Kept both changes",
                            };
                            body = body.child(
                                widgets::row()
                                    .gap_2()
                                    .px_4()
                                    .pt_2()
                                    .font_family(".SystemUIFont")
                                    .child(icon("check", 12.0, widgets::green()))
                                    .child(widgets::faint(label))
                                    .child(crate::ui::Link::new(("undo-pick", ci), "Undo").on_click(on(choose(None)))),
                            );
                            let kept: Vec<&String> = match which {
                                Pick::Current => c.iter().collect(),
                                Pick::Incoming => i.iter().collect(),
                                Pick::Both => c.iter().chain(i.iter()).collect(),
                            };
                            for l in kept {
                                body = body.child(div().border_l_2().border_color(rgb(widgets::green())).child(line_el(l, None)));
                            }
                        }
                    }
                }
            }
        }
        card.child(
            div()
                .id("conflict-body")
                .overflow_x_scroll()
                .track_scroll(&self.scroller(&format!("conflict-body:{index}")))
                .child(body),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_way_merge_finds_conflicts() {
        let base = "a\nb\nc\n";
        let ours = "a\nB\nc\n";
        let theirs = "a\nX\nc\n";
        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge).set_conflict_marker_length(MARK);
        let merged = opts.merge(base, ours, theirs).unwrap_err();
        let parts = chunks(&merged);
        assert_eq!(parts.len(), 3);
        let Chunk::Conflict { current, incoming, .. } = &parts[1] else { panic!() };
        assert_eq!(current, &vec!["B\n".to_string()]);
        assert_eq!(incoming, &vec!["X\n".to_string()]);
        let mut file = ConflictFile { path: "f".into(), chunks: parts, edited: None, deletable: false, binary: None, binary_pick: None };
        assert_eq!(file.unresolved(), 1);
        if let Chunk::Conflict { pick, .. } = &mut file.chunks[1] {
            *pick = Some(Pick::Both);
        }
        assert_eq!(file.unresolved(), 0);
        assert_eq!(file.text("head", "main"), "a\nB\nX\nc\n");
    }
}
