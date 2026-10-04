//! A workflow run and a job's log, laid out as GitHub Actions does: a
//! sidebar of the run's jobs beside either the run's summary (trigger,
//! status, the workflow graph, annotations, artifacts) or a job's steps,
//! each opening onto its own stretch of the log.

use super::actions::strip_ansi;
use super::pulls::status_icon;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, Req, Route, RepoTab};
use crate::json::{self, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::rc::Rc;

/// A job in the workflow file: its key, `name:`, and `needs:`.
#[derive(Debug, PartialEq)]
struct FileJob {
    key: String,
    name: String,
    needs: Vec<String>,
}

/// The jobs of a workflow file, read just far enough for the graph: the
/// keys under `jobs:`, and each one's `name:` and `needs:`.
fn file_jobs(yaml: &str) -> Vec<FileJob> {
    let mut jobs: Vec<FileJob> = Vec::new();
    let mut in_jobs = false;
    let mut job_indent = None;
    let mut in_needs_list = false;
    let unquote = |s: &str| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    for raw in yaml.lines() {
        let line = raw.split(" #").next().unwrap_or(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let text = line.trim();
        if indent == 0 {
            in_jobs = text == "jobs:";
            job_indent = None;
            continue;
        }
        if !in_jobs {
            continue;
        }
        let ji = *job_indent.get_or_insert(indent);
        if indent == ji {
            in_needs_list = false;
            if let Some(key) = text.strip_suffix(':') {
                jobs.push(FileJob { key: unquote(key), name: String::new(), needs: Vec::new() });
            }
            continue;
        }
        let Some(job) = jobs.last_mut() else { continue };
        if in_needs_list {
            match text.strip_prefix("- ") {
                Some(item) => {
                    job.needs.push(unquote(item));
                    continue;
                }
                None => in_needs_list = false,
            }
        }
        if let Some(name) = text.strip_prefix("name:") {
            if job.name.is_empty() && indent > ji {
                job.name = unquote(name);
            }
        } else if let Some(needs) = text.strip_prefix("needs:") {
            let needs = needs.trim();
            if needs.is_empty() {
                in_needs_list = true;
            } else {
                job.needs = needs.trim_matches(['[', ']']).split(',').map(unquote).filter(|s| !s.is_empty()).collect();
            }
        }
    }
    jobs
}

/// Which file job an API job came from: by name, by key, or as one leg
/// of a matrix ("test (ubuntu-latest)").
fn file_job_for<'a>(api_name: &str, jobs: &'a [FileJob]) -> Option<&'a FileJob> {
    let plain = |s: &str| s.split("${{").next().unwrap_or(s).trim().to_string();
    jobs.iter().find(|j| {
        let name = plain(&j.name);
        [name.as_str(), j.key.as_str()]
            .iter()
            .filter(|n| !n.is_empty())
            .any(|n| api_name == *n || api_name.starts_with(&format!("{n} (")) || (j.name.contains("${{") && api_name.starts_with(*n)))
    })
}

/// How deep in the `needs:` chain each job sits; the graph's columns.
fn stages(jobs: &[FileJob]) -> HashMap<String, usize> {
    fn depth(key: &str, jobs: &[FileJob], memo: &mut HashMap<String, usize>, seen: &mut Vec<String>) -> usize {
        if let Some(d) = memo.get(key) {
            return *d;
        }
        if seen.iter().any(|s| s == key) {
            return 0;
        }
        seen.push(key.to_string());
        let d = jobs
            .iter()
            .find(|j| j.key == key)
            .map(|j| j.needs.iter().map(|n| depth(n, jobs, memo, seen) + 1).max().unwrap_or(0))
            .unwrap_or(0);
        memo.insert(key.to_string(), d);
        d
    }
    let mut memo = HashMap::new();
    for j in jobs {
        depth(&j.key, jobs, &mut memo, &mut Vec::new());
    }
    memo
}

/// "succeeded", "failed", "in progress" — as GitHub words a status.
fn verdict(status: &str, conclusion: &str) -> String {
    match (status, conclusion) {
        (_, "success") => "Success".into(),
        (_, "failure") => "Failure".into(),
        (_, "cancelled") => "Cancelled".into(),
        (_, "skipped") => "Skipped".into(),
        (_, "timed_out") => "Timed out".into(),
        (_, "action_required") => "Action required".into(),
        ("in_progress", _) => "In progress".into(),
        ("queued", _) | ("waiting", _) | ("pending", _) => "Queued".into(),
        (s, _) => s.replace('_', " "),
    }
}

impl Hub {
    /// The jobs column beside a run or a job.
    fn run_sidebar(&mut self, repo: &str, run: &Value, jobs: &[Value], selected: Option<u64>) -> Div {
        let p = palette();
        let run_id = run.i("id") as u64;
        let item = |id: String, active: bool, act: Act| {
            div()
                .id(ElementId::Name(id.into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_h(px(32.0))
                .px_3()
                .rounded_md()
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(p.selection_bg)).font_weight(FontWeight::SEMIBOLD))
                .when(!active, |d| d.hover(|s| s.bg(rgb(p.hover))))
                .on_click(on(act))
        };
        let mut col = widgets::col()
            .gap_1()
            .w(px(260.0))
            .flex_none()
            .child(
                item("run-summary".into(), selected.is_none(), Act::Go(Route::Run { repo: repo.to_string(), id: run_id }))
                    .child(icon("home", 14.0, p.text_dim))
                    .child("Summary"),
            )
            .child(div().pt_3().pb_1().px_3().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.text_dim)).child("Jobs"));
        for job in jobs {
            let (mark, color) = status_icon(&job.s("status"), &job.s("conclusion"));
            let id = job.i("id") as u64;
            col = col.child(
                item(format!("run-job-{id}"), selected == Some(id), Act::Go(Route::Job { repo: repo.to_string(), id, name: job.s("name") }))
                    .child(icon(mark, 14.0, color))
                    .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().whitespace_nowrap().child(job.s("name"))),
            );
        }
        col.child(div().pt_3().pb_1().px_3().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(p.text_dim)).child("Run details"))
            .child(
                item(
                    "run-workflow-file".into(),
                    false,
                    Act::Go(Route::Tree { repo: repo.to_string(), git_ref: run.s("head_sha"), path: run.s("path"), file: true }),
                )
                .child(icon("file", 14.0, p.text_dim))
                .child("Workflow file"),
            )
    }

    /// The run's title, number, and what can be done to it.
    fn run_header(&mut self, repo: &str, run: &Value) -> AnyElement {
        let p = palette();
        let id = run.i("id") as u64;
        let base = format!("/repos/{repo}/actions/runs/{id}");
        let inval = format!("/repos/{repo}/actions");
        let (mark, color) = status_icon(&run.s("status"), &run.s("conclusion"));
        let running = run.s("status") != "completed";
        let action: AnyElement = if running {
            widgets::btn("cancel-run", "Cancel workflow", Req::rest("POST", format!("{base}/cancel")).ok("Cancelling").inval(inval.clone()).act())
                .h(px(32.0))
                .px_3()
                .into_any_element()
        } else {
            widgets::dropdown_btn(
                "rerun-menu",
                None,
                "Re-run jobs",
                Act::menu(vec![
                    MenuEntry::item("Re-run failed jobs", Req::rest("POST", format!("{base}/rerun-failed-jobs")).ok("Re-run requested").inval(inval.clone()).act()),
                    MenuEntry::item("Re-run all jobs", Req::rest("POST", format!("{base}/rerun")).ok("Re-run requested").inval(inval.clone()).act()),
                ]),
            )
            .into_any_element()
        };
        let repo_back = repo.to_string();
        let more = Act::menu(vec![
            MenuEntry::item("View workflow file", Act::Go(Route::Tree { repo: repo.to_string(), git_ref: run.s("head_sha"), path: run.s("path"), file: true })),
            MenuEntry::item("Open on GitHub", Act::Url(run.s("html_url"))),
            MenuEntry::item("Force cancel", Req::rest("POST", format!("{base}/force-cancel")).ok("Force-cancelling").inval(inval.clone()).act()),
            MenuEntry::Sep,
            MenuEntry::item(
                "Delete workflow run",
                Req::rest("DELETE", base.clone())
                    .ok("Run deleted")
                    .inval(inval.clone())
                    .then(move |hub, _, cx| hub.go(Route::Repo { repo: repo_back.clone(), tab: RepoTab::Actions }, cx))
                    .act()
                    .confirm("Delete this run?", "Its logs and artifacts are deleted too.", "Delete"),
            ),
        ]);
        widgets::col()
            .gap_1()
            .child(
                crate::ui::Link::new("run-back", format!("← {}", run.s("name")))
                    .text_size(px(12.0))
                    .on_click(on(Act::Go(Route::Repo { repo: repo.to_string(), tab: RepoTab::Actions }))),
            )
            .child(
                widgets::row()
                    .gap_3()
                    .child(icon(mark, 22.0, color))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_baseline()
                            .gap_2()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::title(run.s("display_title")))
                            .child(div().text_size(px(20.0)).text_color(rgb(p.text_dim)).child(format!("#{}", run.i("run_number")))),
                    )
                    .child(action)
                    .child(IconButton::new("run-more", "kebab").size(32.0).icon_size(16.0).on_click(on(more))),
            )
            .into_any_element()
    }

    pub fn run(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let base = format!("/repos/{repo}/actions/runs/{id}");
        let run = ready!(self.fetch(&base, cx));
        if run.s("status") != "completed" {
            self.poll(std::slice::from_ref(&base), 5, cx);
        }
        let jobs: Vec<Value> = match self.fetch(&format!("{base}/jobs?filter=latest&per_page=100"), cx) {
            Load::Ready(list) => list.list("jobs").to_vec(),
            _ => Vec::new(),
        };
        let header = self.run_header(repo, &run);
        let sidebar = self.run_sidebar(repo, &run, &jobs, None);

        // Summary: how it started, how it went, how long, what it made.
        let artifacts_count = self.fetch(&format!("{base}/artifacts"), cx).ready().map(|v| v.i("total_count"));
        let actor_avatar = self.avatar(&run.s("triggering_actor.avatar_url"), 20.0, cx);
        let block = |label: &str, body: AnyElement| {
            widgets::col()
                .gap_1()
                .child(widgets::faint(label.to_string()))
                .child(body)
        };
        let sha = run.s("head_sha");
        let trigger = widgets::row()
            .gap_2()
            .child(actor_avatar)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(run.s("triggering_actor.login")))
            .child(widgets::dim(match run.s("event").as_str() {
                "push" => "pushed".to_string(),
                "pull_request" => "opened or updated a pull request".to_string(),
                "schedule" => "scheduled".to_string(),
                "workflow_dispatch" => "ran this manually".to_string(),
                other => other.replace('_', " "),
            }))
            .child(crate::ui::Link::new("run-sha", sha.chars().take(7).collect::<String>()).on_click(on(Act::Go(Route::Commit { repo: repo.to_string(), sha: sha.clone() }))))
            .child(widgets::tag(run.s("head_branch"), widgets::gray()));
        let (smark, scolor) = status_icon(&run.s("status"), &run.s("conclusion"));
        let summary = widgets::card().child(
            widgets::row()
                .p_4()
                .gap_8()
                .flex_wrap()
                .items_start()
                .child(block(&format!("Triggered via {} {}", run.s("event").replace('_', " "), time::ago(&run.s("created_at"))), trigger.into_any_element()))
                .child(block(
                    "Status",
                    widgets::row().gap_1p5().child(icon(smark, 14.0, scolor)).child(div().font_weight(FontWeight::SEMIBOLD).child(verdict(&run.s("status"), &run.s("conclusion")))).into_any_element(),
                ))
                .child(block("Total duration", div().font_weight(FontWeight::SEMIBOLD).child(time::span(&run.s("run_started_at"), &run.s("updated_at"))).into_any_element()))
                .child(block("Artifacts", div().font_weight(FontWeight::SEMIBOLD).child(artifacts_count.map_or("–".to_string(), |n| n.to_string())).into_any_element())),
        );

        let approvals = self.run_approvals(&base, cx);
        let graph = self.run_graph(repo, &run, &jobs, cx);
        let annotations = self.run_annotations(repo, &jobs, cx);

        let repo_s = repo.to_string();
        let artifacts = ListSpec::new(format!("{base}/artifacts"), move |a| {
            Row::new(a.s("name"))
                .icon("package", widgets::gray())
                .right(json::bytes(a.i("size_in_bytes")))
                .action("Download on GitHub", Act::Url(format!("{}/{repo_s}/actions/runs/{id}", crate::api::WEB)))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("/repos/{repo_s}/actions/artifacts/{}", a.i("id")))
                        .ok("Artifact deleted")
                        .inval(format!("/repos/{repo_s}/actions"))
                        .act(),
                )
                .inline()
        })
        .items("artifacts")
        .unpaged()
        .empty("No artifacts were produced by this run.");
        let artifacts = self.list(&artifacts, cx);

        widgets::page()
            .max_w(px(1400.0))
            .child(header)
            .child(div().h(px(1.0)).bg(rgb(p.divider)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_6()
                    .child(sidebar)
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .gap_4()
                            .child(summary)
                            .child(approvals)
                            .child(graph)
                            .children(annotations)
                            .child(widgets::h3("Artifacts"))
                            .child(artifacts),
                    ),
            )
            .into_any_element()
    }

    /// Deployments waiting on a reviewer.
    fn run_approvals(&mut self, base: &str, cx: &mut Context<Self>) -> Div {
        let mut approvals = widgets::col().gap_2();
        if let Some(list) = self.fetch(&format!("{base}/pending_deployments"), cx).ready() {
            for (i, d) in list.list("").iter().enumerate() {
                let env_id = d.i("environment.id");
                let env = d.s("environment.name");
                let base = base.to_string();
                let review = |state: &'static str| {
                    FormSpec::new(format!("{} deployment to {env}", if state == "approved" { "Approve" } else { "Reject" }))
                        .submit(if state == "approved" { "Approve and deploy" } else { "Reject" })
                        .field(Field::multiline("comment", "Comment"))
                        .rest("POST", format!("{base}/pending_deployments"))
                        .map(move |body, _| json!({ "environment_ids": [env_id], "state": state, "comment": body.s("comment") }))
                        .ok("Review submitted")
                        .inval(base.clone())
                        .act()
                };
                approvals = approvals.child(
                    widgets::card().border_color(rgb(widgets::yellow())).child(
                        widgets::row()
                            .p_3()
                            .child(icon("alert", 16.0, widgets::yellow()))
                            .child(div().flex_1().child(format!("Waiting for review to deploy to {env}")))
                            .child(widgets::go_btn(ElementId::Name(format!("approve-{i}").into()), "Approve", review("approved")))
                            .child(widgets::danger(ElementId::Name(format!("reject-{i}").into()), "Reject", review("rejected"))),
                    ),
                );
            }
        }
        approvals
    }

    /// The workflow as GitHub draws it: jobs in columns by what they
    /// need, a matrix's legs grouped, a connector between columns.
    fn run_graph(&mut self, repo: &str, run: &Value, jobs: &[Value], cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let path = run.s("path");
        let file = self
            .fetch_text(&format!("/repos/{repo}/contents/{path}?ref={}", run.s("head_sha")), "application/vnd.github.raw", cx)
            .ready()
            .map(|v| v.s(""))
            .unwrap_or_default();
        let file_jobs = file_jobs(&file);
        let depth = stages(&file_jobs);

        // Columns of groups: (stage, group key, title, jobs).
        let mut groups: Vec<(usize, String, String, Vec<&Value>)> = Vec::new();
        for job in jobs {
            let fj = file_job_for(&job.s("name"), &file_jobs);
            let key = fj.map(|j| j.key.clone()).unwrap_or_else(|| job.s("name"));
            let title = fj.map(|j| if j.name.is_empty() || j.name.contains("${{") { j.key.clone() } else { j.name.clone() }).unwrap_or_else(|| job.s("name"));
            let stage = fj.and_then(|j| depth.get(&j.key).copied()).unwrap_or(0);
            match groups.iter_mut().find(|g| g.1 == key) {
                Some(g) => g.3.push(job),
                None => groups.push((stage, key, title, vec![job])),
            }
        }
        let columns = groups.iter().map(|g| g.0).max().map_or(0, |m| m + 1);

        let node = |job: &Value, inner: bool| {
            let (mark, color) = status_icon(&job.s("status"), &job.s("conclusion"));
            let id = job.i("id") as u64;
            let took = time::span(&job.s("started_at"), &job.s("completed_at"));
            div()
                .id(ElementId::Name(format!("graph-job-{id}").into()))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(36.0))
                .px_3()
                .cursor_pointer()
                .when(!inner, |d| d.rounded_md().border_1().border_color(rgb(p.edge)).bg(rgb(p.panel_bg)))
                .when(inner, |d| d.rounded_sm())
                .hover(|s| s.bg(rgb(p.hover)))
                .on_click(on(Act::Go(Route::Job { repo: repo.to_string(), id, name: job.s("name") })))
                .child(icon(mark, 14.0, color))
                .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().whitespace_nowrap().child(job.s("name")))
                .child(widgets::faint(took))
        };

        let mut lanes = div().flex().flex_row().items_center().gap_0().p_6();
        for c in 0..columns {
            if c > 0 {
                lanes = lanes.child(div().w(px(36.0)).h(px(2.0)).flex_none().bg(rgb(p.edge)));
            }
            let mut column = widgets::col().gap_3().w(px(260.0)).flex_none();
            for (_, _, title, members) in groups.iter().filter(|g| g.0 == c) {
                if members.len() == 1 {
                    column = column.child(node(members[0], false));
                } else {
                    let done = members.iter().filter(|j| j.s("status") == "completed").count();
                    let mut group = widgets::col()
                        .gap_0p5()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(p.edge))
                        .bg(rgb(p.panel_bg))
                        .child(
                            widgets::row()
                                .px_1()
                                .pb_1()
                                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(title.clone()))
                                .child(widgets::faint(format!("{done}/{} jobs completed", members.len()))),
                        );
                    for job in members {
                        group = group.child(node(job, true));
                    }
                    column = column.child(group);
                }
            }
            lanes = lanes.child(column);
        }

        let file_name = path.rsplit('/').next().unwrap_or(&path).to_string();
        widgets::card()
            .child(
                widgets::card_header()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(file_name))
                    .child(widgets::dim(format!("on: {}", run.s("event")))),
            )
            .child(
                div()
                    .id("run-graph")
                    .overflow_x_scroll()
                    .track_scroll(&self.scroller("run-graph"))
                    .bg(rgb(p.deep_bg))
                    .child(if jobs.is_empty() { widgets::loading() } else { lanes.into_any_element() }),
            )
            .into_any_element()
    }

    /// Errors and warnings the jobs left, as GitHub lists them under the
    /// graph.
    fn run_annotations(&mut self, repo: &str, jobs: &[Value], cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette();
        let mut found: Vec<(String, Value)> = Vec::new();
        for job in jobs.iter().take(40) {
            if let Some(list) = self.fetch(&format!("/repos/{repo}/check-runs/{}/annotations", job.i("id")), cx).ready() {
                found.extend(list.list("").iter().map(|a| (job.s("name"), a.clone())));
            }
        }
        if found.is_empty() {
            return None;
        }
        let errors = found.iter().filter(|(_, a)| a.s("annotation_level") == "failure").count();
        let warnings = found.iter().filter(|(_, a)| a.s("annotation_level") == "warning").count();
        let mut card = widgets::card().child(
            widgets::card_header()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Annotations"))
                .child(widgets::dim(format!(
                    "{errors} error{} and {warnings} warning{}",
                    if errors == 1 { "" } else { "s" },
                    if warnings == 1 { "" } else { "s" }
                ))),
        );
        for (job, a) in &found {
            let (mark, color) = match a.s("annotation_level").as_str() {
                "failure" => ("x-circle", widgets::red()),
                "warning" => ("alert", widgets::yellow()),
                _ => ("dot", widgets::gray()),
            };
            let place = if a.s("path").is_empty() || a.s("path") == ".github" { String::new() } else { format!("{}#L{}", a.s("path"), a.i("start_line")) };
            card = card.child(
                widgets::row()
                    .items_start()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(p.divider))
                    .child(div().pt(px(2.0)).child(icon(mark, 14.0, color)))
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(if a.s("title").is_empty() { job.clone() } else { format!("{job}: {}", a.s("title")) }))
                            .child(div().child(json::clip(&a.s("message"), 400)))
                            .when(!place.is_empty(), |d| d.child(widgets::faint(place.clone()))),
                    ),
            );
        }
        Some(card.into_any_element())
    }

    /// A job: the run's sidebar, and the job's steps, each opening onto
    /// its part of the log.
    pub fn job(&mut self, repo: &str, id: u64, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let job_path = format!("/repos/{repo}/actions/jobs/{id}");
        let job = ready!(self.fetch(&job_path, cx));
        let run_id = job.i("run_id") as u64;
        let run_base = format!("/repos/{repo}/actions/runs/{run_id}");
        let run = self.fetch(&run_base, cx).ready().cloned();
        let jobs: Vec<Value> = match self.fetch(&format!("{run_base}/jobs?filter=latest&per_page=100"), cx) {
            Load::Ready(list) => list.list("jobs").to_vec(),
            _ => Vec::new(),
        };
        let header = match &run {
            Some(run) => self.run_header(repo, run),
            None => widgets::title(name.to_string()).into_any_element(),
        };
        let sidebar = match &run {
            Some(run) => self.run_sidebar(repo, run, &jobs, Some(id)).into_any_element(),
            None => div().into_any_element(),
        };
        let log = self.job_log(repo, id, &job, cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_4()
            .gap_4()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .gap_6()
                    .child(div().id("job-sidebar").overflow_y_scroll().track_scroll(&self.scroller("job-sidebar")).child(sidebar))
                    .child(log),
            )
            .into_any_element()
    }

    /// The dark log pane: the job's name and outcome, a search box, and
    /// every step with its lines.
    fn job_log(&mut self, repo: &str, id: u64, job: &Value, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let path = format!("/repos/{repo}/actions/jobs/{id}/logs");
        let finished = job.s("status") == "completed";
        // A running job's log is there for its finished steps; keep it,
        // and the steps, coming in until the job ends.
        if !finished {
            self.poll(&[format!("/repos/{repo}/actions/jobs/{id}"), format!("/repos/{repo}/actions/runs/{}", job.i("run_id"))], 5, cx);
        }
        let text = match self.fetch_text(&path, "application/vnd.github+json", cx) {
            Load::Ready(v) => Some(v.s("")),
            Load::Failed(_) => None,
            Load::Loading => Some(String::new()),
        };
        let steps = job.list("steps").to_vec();
        let logs = split_by_step(text.as_deref().unwrap_or(""), &steps);

        let search_id = format!("joblog.search:{id}");
        let query = self.field_text(&search_id).trim().to_lowercase();
        let search = self.input(&search_id, "Search logs", cx).w(px(240.0)).h(px(28.0));

        // One flat, virtualised list: each step's row, then its lines
        // when it's open. Failed steps and steps with matches open.
        let mut items: Vec<LogItem> = Vec::new();
        let mut matches = 0;
        for (n, step) in steps.iter().enumerate() {
            let lines = &logs[n];
            let hits = if query.is_empty() { 0 } else { lines.iter().filter(|l| l.text.to_lowercase().contains(&query)).count() };
            matches += hits;
            let key = format!("jobstep:{id}:{n}");
            let failed = step.s("conclusion") == "failure";
            // Failed steps start open; the set records what was toggled.
            let toggled = self.is_open(&key);
            let open = (failed != toggled) || hits > 0;
            items.push(LogItem::Step { n, open, key, hits });
            if open {
                for (i, line) in lines.iter().enumerate() {
                    let hit = !query.is_empty() && line.text.to_lowercase().contains(&query);
                    items.push(LogItem::Line { step: n, number: i + 1, hit });
                }
                if lines.is_empty() {
                    items.push(LogItem::Empty { done: step.s("status") == "completed" });
                }
            }
        }

        let (mark, color) = status_icon(&job.s("status"), &job.s("conclusion"));
        let took = time::span(&job.s("started_at"), &job.s("completed_at"));
        let outcome = if finished {
            format!("{} {} in {took}", verdict(&job.s("status"), &job.s("conclusion")).to_lowercase(), time::ago(&job.s("completed_at")))
        } else {
            format!("{} — started {}, updating live", verdict(&job.s("status"), &job.s("conclusion")).to_lowercase(), time::ago(&job.s("started_at")))
        };
        let reload_path = path.clone();
        let head = widgets::row()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(p.divider))
            .child(icon(mark, 18.0, color))
            .child(
                widgets::col()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child(job.s("name")))
                    .child(widgets::dim(outcome)),
            )
            .when(!query.is_empty(), |d| d.child(widgets::faint(format!("{matches} result{}", if matches == 1 { "" } else { "s" }))))
            .child(search)
            .child(
                IconButton::new("job-log-menu", "kebab").size(28.0).icon_size(16.0).on_click(on(Act::menu(vec![
                    MenuEntry::item("Copy log", Act::Copy(text.clone().unwrap_or_default())),
                    MenuEntry::item("Reload log", Act::run(move |hub, _, cx| {
                        hub.invalidate(&reload_path);
                        cx.notify();
                    })),
                    MenuEntry::item(
                        "Re-run this job",
                        Req::rest("POST", format!("/repos/{repo}/actions/jobs/{id}/rerun")).ok("Job re-run requested").inval(format!("/repos/{repo}/actions")).act(),
                    ),
                    MenuEntry::item("Open on GitHub", Act::Url(job.s("html_url"))),
                ]))),
            );

        let steps = Rc::new(steps);
        let logs = Rc::new(logs);
        let items = Rc::new(items);
        let count = items.len();
        let scroll = self.list_scroller("job-log");
        let body = gpui::uniform_list("job-log", count, move |range, _, _| {
            range
                .map(|i| match &items[i] {
                    LogItem::Step { n, open, key, hits } => {
                        let step = &steps[*n];
                        let (mark, color) = status_icon(&step.s("status"), &step.s("conclusion"));
                        let key = key.clone();
                        div()
                            .id(("job-step", i))
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(ROW))
                            .px_3()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(p.hover)))
                            .on_click(on(Act::run(move |hub, _, cx| {
                                if !hub.open.remove(&key) {
                                    hub.open.insert(key.clone());
                                }
                                cx.notify();
                            })))
                            .child(icon(if *open { "chevron-down" } else { "chevron-right" }, 12.0, p.text_dim))
                            .child(icon(mark, 14.0, color))
                            .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().whitespace_nowrap().font_family(".SystemUIFont").text_size(px(13.0)).child(step.s("name")))
                            .when(*hits > 0, |d| d.child(widgets::faint(format!("{hits} match{}", if *hits == 1 { "" } else { "es" }))))
                            .child(widgets::faint(time::span(&step.s("started_at"), &step.s("completed_at"))))
                            .into_any_element()
                    }
                    LogItem::Line { step, number, hit } => {
                        let line = &logs[*step][*number - 1];
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .h(px(ROW))
                            .pl(px(28.0))
                            .when(*hit, |d| d.bg(rgb(if crate::ui::is_light() { 0xFFF8C5 } else { 0x3B2E0A })))
                            .when(line.kind == Kind::Error, |d| d.bg(rgb(if crate::ui::is_light() { 0xFFEBE9 } else { 0x2D1517 })))
                            .child(div().w(px(48.0)).flex_none().pr_3().text_right().text_color(rgb(p.text_faint)).child(number.to_string()))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_color(rgb(match line.kind {
                                        Kind::Group => p.text,
                                        Kind::Error => widgets::red(),
                                        Kind::Warning => widgets::yellow(),
                                        Kind::Command => p.accent_hover,
                                        Kind::Debug => p.text_faint,
                                        Kind::Plain => p.text,
                                    }))
                                    .when(line.kind == Kind::Group || line.kind == Kind::Error, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(line.text.clone()),
                            )
                            .into_any_element()
                    }
                    LogItem::Empty { done } => div()
                        .h(px(ROW))
                        .pl(px(84.0))
                        .text_color(rgb(p.text_faint))
                        .child(if *done { "No output" } else { "Waiting for output…" })
                        .into_any_element(),
                })
                .collect()
        })
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .py_2()
        .font_family(widgets::MONO)
        .text_size(px(12.0));

        let body: AnyElement = match text {
            None if finished => widgets::col().p_4().child(widgets::dim("The log couldn't be loaded; it may have expired.")).into_any_element(),
            _ => body.into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .rounded_md()
            .border_1()
            .border_color(rgb(p.edge))
            .bg(rgb(p.deep_bg))
            .overflow_hidden()
            .child(head)
            .child(body)
            .into_any_element()
    }
}

const ROW: f32 = 22.0;

enum LogItem {
    Step { n: usize, open: bool, key: String, hits: usize },
    Line { step: usize, number: usize, hit: bool },
    /// An open step with nothing to show: done and silent, or not yet
    /// written.
    Empty { done: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Group,
    Error,
    Warning,
    Command,
    Debug,
    Plain,
}

struct LogLine {
    text: String,
    kind: Kind,
}

/// The job's log cut into its steps by time: each line carries a
/// timestamp, and belongs to the last step that had started by then.
fn split_by_step(text: &str, steps: &[Value]) -> Vec<Vec<LogLine>> {
    let starts: Vec<i64> = steps.iter().map(|s| time::parse(&s.s("started_at")).unwrap_or(i64::MAX)).collect();
    let mut out: Vec<Vec<LogLine>> = steps.iter().map(|_| Vec::new()).collect();
    if steps.is_empty() {
        return out;
    }
    let mut current = 0;
    for raw in text.lines() {
        let (stamp, rest) = match raw.split_once(' ') {
            Some((stamp, rest)) if stamp.len() >= 20 && stamp.as_bytes().get(4) == Some(&b'-') => (time::parse(stamp), rest),
            _ => (None, raw),
        };
        if let Some(t) = stamp {
            // Steps are in order; move on while the next has begun.
            while current + 1 < starts.len() && starts[current + 1] <= t {
                current += 1;
            }
        }
        let line = rest.trim_start_matches('\u{feff}');
        let (text, kind) = if let Some(r) = line.strip_prefix("##[group]") {
            (format!("▸ {r}"), Kind::Group)
        } else if line.starts_with("##[endgroup]") {
            continue;
        } else if let Some(r) = line.strip_prefix("##[error]") {
            (format!("Error: {r}"), Kind::Error)
        } else if let Some(r) = line.strip_prefix("##[warning]") {
            (format!("Warning: {r}"), Kind::Warning)
        } else if let Some(r) = line.strip_prefix("##[command]") {
            (r.to_string(), Kind::Command)
        } else if let Some(r) = line.strip_prefix("##[debug]") {
            (r.to_string(), Kind::Debug)
        } else {
            (strip_ansi(line), Kind::Plain)
        };
        out[current].push(LogLine { text: text.replace('\t', "    "), kind });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "name: CI\non: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: make\n  test:\n    name: Test suite\n    needs: build\n    strategy:\n      matrix:\n        os: [a, b]\n  deploy:\n    needs:\n      - build\n      - test\n";

    #[test]
    fn reads_jobs_and_needs() {
        let jobs = file_jobs(YAML);
        assert_eq!(jobs.iter().map(|j| j.key.as_str()).collect::<Vec<_>>(), ["build", "test", "deploy"]);
        assert_eq!(jobs[1].name, "Test suite");
        assert_eq!(jobs[1].needs, ["build"]);
        assert_eq!(jobs[2].needs, ["build", "test"]);
        let depth = stages(&jobs);
        assert_eq!((depth["build"], depth["test"], depth["deploy"]), (0, 1, 2));
        assert_eq!(file_job_for("Test suite (a)", &jobs).map(|j| j.key.as_str()), Some("test"));
        assert_eq!(file_job_for("build", &jobs).map(|j| j.key.as_str()), Some("build"));
    }

    #[test]
    fn splits_logs_by_step_time() {
        let steps = vec![
            json!({ "started_at": "2024-01-01T00:00:00Z" }),
            json!({ "started_at": "2024-01-01T00:00:05Z" }),
        ];
        let log = "2024-01-01T00:00:01.0000000Z one\n2024-01-01T00:00:06.0000000Z ##[group]Run two\n2024-01-01T00:00:06.1000000Z ##[endgroup]\n2024-01-01T00:00:07.0000000Z three";
        let split = split_by_step(log, &steps);
        assert_eq!(split[0].len(), 1);
        assert_eq!(split[1].iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["▸ Run two", "three"]);
    }
}
