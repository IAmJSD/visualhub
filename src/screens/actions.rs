//! GitHub Actions: workflow runs and their jobs, steps and logs; workflows
//! (run, enable, disable); caches; artifacts; and deployment approvals.

use super::pulls::status_icon;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, Req, Route, RepoTab};
use crate::json::{self, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, px, AnyElement, Context, ElementId, FontWeight, IntoElement as _, ParentElement as _, Styled as _};
use crate::ui::{icon, palette};
use serde_json::{json, Value};

fn dispatch_form(repo: &str, workflow: &Value, default_branch: &str) -> Act {
    let id = workflow.i("id");
    FormSpec::new(format!("Run {}", workflow.s("name")))
        .submit("Run workflow")
        .note("Runs the workflow's workflow_dispatch trigger. Inputs are a JSON object of the names the workflow declares.")
        .field(Field::text("ref", "Branch or tag").value(default_branch).required())
        .field(Field::json("inputs", "Inputs").value("{}"))
        .rest("POST", format!("/repos/{repo}/actions/workflows/{id}/dispatches"))
        .ok("Workflow run requested")
        .inval(format!("/repos/{repo}/actions"))
        .act()
}

fn run_row(repo: &str, run: &Value) -> Row {
    let (icon_name, color) = status_icon(&run.s("status"), &run.s("conclusion"));
    let id = run.i("id") as u64;
    let in_progress = run.s("status") != "completed";
    let duration = time::span(&run.s("run_started_at"), &run.s("updated_at"));
    let base = format!("/repos/{repo}/actions/runs/{id}");
    let mut row = Row::new(run.s("display_title"))
        .icon(icon_name, color)
        .meta(format!(
            "{} #{}: {} by {}  ·  {}  ·  {}{}",
            run.s("name"),
            run.i("run_number"),
            run.s("event"),
            run.s("actor.login"),
            run.s("head_branch"),
            time::ago(&run.s("created_at")),
            if duration.is_empty() || in_progress { String::new() } else { format!("  ·  {duration}") }
        ))
        .open(Act::Go(Route::Run { repo: repo.to_string(), id }));
    if in_progress {
        row = row.action(
            "Cancel run",
            Req::rest("POST", format!("{base}/cancel")).ok("Cancelling").inval(format!("/repos/{repo}/actions")).act(),
        );
    } else {
        row = row
            .action("Re-run all jobs", Req::rest("POST", format!("{base}/rerun")).ok("Re-run requested").inval(format!("/repos/{repo}/actions")).act())
            .action(
                "Re-run failed jobs",
                Req::rest("POST", format!("{base}/rerun-failed-jobs")).ok("Re-run requested").inval(format!("/repos/{repo}/actions")).act(),
            );
    }
    row.action("Open on GitHub", Act::Url(run.s("html_url"))).danger(
        "Delete run",
        Req::rest("DELETE", base)
            .ok("Run deleted")
            .inval(format!("/repos/{repo}/actions"))
            .act()
            .confirm("Delete this workflow run?", "Its logs and artifacts are deleted too.", "Delete"),
    )
}

impl Hub {
    pub fn repo_actions(&mut self, repo: &str, default_branch: &str, cx: &mut Context<Self>) -> AnyElement {
        let view_key = format!("actions.view:{repo}");
        let view = self.choice(&view_key, "runs");
        let chips = widgets::chips(vec![
            ("Runs".into(), view == "runs", Act::choose(&view_key, "runs")),
            ("Workflows".into(), view == "workflows", Act::choose(&view_key, "workflows")),
            ("Artifacts".into(), view == "artifacts", Act::choose(&view_key, "artifacts")),
            ("Caches".into(), view == "caches", Act::choose(&view_key, "caches")),
            ("Runners".into(), view == "runners", Act::choose(&view_key, "runners")),
        ]);
        let body = match view.as_str() {
            "workflows" => self.workflows(repo, default_branch, cx),
            "artifacts" => {
                let repo_s = repo.to_string();
                let spec = ListSpec::new(format!("/repos/{repo}/actions/artifacts"), move |a| {
                    Row::new(a.s("name"))
                        .icon("package", widgets::gray())
                        .meta(format!(
                            "{}  ·  {}  ·  {}",
                            json::bytes(a.i("size_in_bytes")),
                            time::ago(&a.s("created_at")),
                            if a.b("expired") { "expired".to_string() } else { format!("expires {}", time::date(&a.s("expires_at"))) }
                        ))
                        .open(Act::Go(Route::Run { repo: repo_s.clone(), id: a.i("workflow_run.id") as u64 }))
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("/repos/{repo_s}/actions/artifacts/{}", a.i("id")))
                                .ok("Artifact deleted")
                                .inval(format!("/repos/{repo_s}/actions/artifacts"))
                                .act(),
                        )
                        .inline()
                })
                .items("artifacts")
                .empty("No artifacts.");
                self.list(&spec, cx)
            }
            "caches" => {
                let repo_s = repo.to_string();
                let usage = self.fetch(&format!("/repos/{repo}/actions/cache/usage"), cx);
                let spec = ListSpec::new(format!("/repos/{repo}/actions/caches"), move |c| {
                    Row::new(c.s("key"))
                        .icon("archive", widgets::gray())
                        .meta(format!("{}  ·  {}  ·  last used {}", json::bytes(c.i("size_in_bytes")), c.s("ref"), time::ago(&c.s("last_accessed_at"))))
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("/repos/{repo_s}/actions/caches/{}", c.i("id")))
                                .ok("Cache deleted")
                                .inval(format!("/repos/{repo_s}/actions/cache"))
                                .act(),
                        )
                        .inline()
                })
                .items("actions_caches")
                .empty("No caches.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_2()
                    .when_some(usage.ready().cloned(), |d, u| {
                        d.child(widgets::dim(format!(
                            "{} caches using {}",
                            u.i("active_caches_count"),
                            json::bytes(u.i("active_caches_size_in_bytes"))
                        )))
                    })
                    .child(list)
                    .into_any_element()
            }
            "runners" => {
                let repo_s = repo.to_string();
                let spec = ListSpec::new(format!("/repos/{repo}/actions/runners"), move |r| {
                    let online = r.s("status") == "online";
                    Row::new(r.s("name"))
                        .icon("codespace", if online { widgets::green() } else { widgets::gray() })
                        .meta(format!(
                            "{}  ·  {}  ·  {}",
                            r.s("os"),
                            r.s("status"),
                            r.list("labels").iter().map(|l| l.s("name")).collect::<Vec<_>>().join(", ")
                        ))
                        .tag(if r.b("busy") { "busy" } else { "" }, widgets::yellow())
                        .danger(
                            "Remove",
                            Req::rest("DELETE", format!("/repos/{repo_s}/actions/runners/{}", r.i("id")))
                                .ok("Runner removed")
                                .inval(format!("/repos/{repo_s}/actions/runners"))
                                .act()
                                .confirm("Remove this runner?", "It will stop picking up jobs.", "Remove"),
                        )
                        .inline()
                })
                .items("runners")
                .empty("No self-hosted runners. GitHub-hosted runners are always available.");
                let list = self.list(&spec, cx);
                let repo_t = repo.to_string();
                let token = Req::rest("POST", format!("/repos/{repo}/actions/runners/registration-token"))
                    .then(move |hub, value, cx| {
                        let token = value.s("token");
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!(
                            "./config.sh --url {}/{repo_t} --token {token}",
                            crate::api::WEB
                        )));
                        hub.toast("Registration command copied to the clipboard", false, cx);
                    })
                    .act();
                widgets::col()
                    .gap_2()
                    .child(widgets::row().child(widgets::spacer()).child(widgets::btn("runner-token", "Copy registration command", token)))
                    .child(list)
                    .into_any_element()
            }
            _ => self.runs(repo, cx),
        };
        widgets::col().gap_3().child(chips).child(body).into_any_element()
    }

    fn runs(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let wf_key = format!("actions.workflow:{repo}");
        let workflow = self.choice(&wf_key, "");
        let status_key = format!("actions.status:{repo}");
        let status = self.choice(&status_key, "");
        let mut path = if workflow.is_empty() {
            format!("/repos/{repo}/actions/runs")
        } else {
            format!("/repos/{repo}/actions/workflows/{workflow}/runs")
        };
        if !status.is_empty() {
            path = crate::hub::with_query(&path, &format!("status={status}"));
        }
        let mut wf_menu = vec![MenuEntry::check("All workflows", workflow.is_empty(), Act::choose(&wf_key, ""))];
        let mut wf_name = "All workflows".to_string();
        if let Some(wfs) = self.fetch(&format!("/repos/{repo}/actions/workflows?per_page=100"), cx).ready().cloned() {
            for w in wfs.list("workflows") {
                let id = w.i("id").to_string();
                if id == workflow {
                    wf_name = w.s("name");
                }
                wf_menu.push(MenuEntry::check(w.s("name"), id == workflow, Act::choose(&wf_key, id)));
            }
        }
        let statuses = [
            ("", "Any status"),
            ("in_progress", "In progress"),
            ("queued", "Queued"),
            ("success", "Success"),
            ("failure", "Failure"),
            ("cancelled", "Cancelled"),
            ("action_required", "Action required"),
        ];
        let status_menu = Act::menu(
            statuses
                .iter()
                .map(|(v, l)| MenuEntry::check(*l, status == *v, Act::choose(&status_key, *v)))
                .collect(),
        );
        let repo_s = repo.to_string();
        let spec = ListSpec::new(path, move |run| run_row(&repo_s, run))
            .items("workflow_runs")
            .empty("No workflow runs.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::btn("wf-filter", format!("{wf_name} ▾"), Act::menu(wf_menu)))
                    .child(widgets::btn(
                        "status-filter",
                        format!("{} ▾", statuses.iter().find(|s| s.0 == status).map(|s| s.1).unwrap_or("Any status")),
                        status_menu,
                    )),
            )
            .child(list)
            .into_any_element()
    }

    fn workflows(&mut self, repo: &str, default_branch: &str, cx: &mut Context<Self>) -> AnyElement {
        let repo_s = repo.to_string();
        let branch = default_branch.to_string();
        let spec = ListSpec::new(format!("/repos/{repo}/actions/workflows"), move |w| {
            let id = w.i("id");
            let active = w.s("state") == "active";
            let path = w.s("path");
            let base = format!("/repos/{repo_s}/actions/workflows/{id}");
            let inval = format!("/repos/{repo_s}/actions");
            let mut row = Row::new(w.s("name"))
                .icon("play", if active { widgets::green() } else { widgets::gray() })
                .meta(format!("{path}  ·  {}", w.s("state")))
                .open(Act::run({
                    let repo = repo_s.clone();
                    move |hub, _, cx| {
                        hub.choices.insert(format!("actions.view:{repo}"), "runs".into());
                        hub.choices.insert(format!("actions.workflow:{repo}"), id.to_string());
                        cx.notify();
                    }
                }))
                .action("Run workflow…", dispatch_form(&repo_s, w, &branch))
                .action(
                    "View workflow file",
                    Act::Go(Route::Tree { repo: repo_s.clone(), git_ref: branch.clone(), path: path.clone(), file: true }),
                );
            row = if active {
                row.action("Disable workflow", Req::rest("PUT", format!("{base}/disable")).ok("Workflow disabled").inval(inval).act())
            } else {
                row.action("Enable workflow", Req::rest("PUT", format!("{base}/enable")).ok("Workflow enabled").inval(inval).act())
            };
            if let Some(badge) = Some(w.s("badge_url")).filter(|b| !b.is_empty()) {
                row = row.action("Copy status badge Markdown", Act::Copy(format!("![{}]({badge})", w.s("name"))));
            }
            row
        })
        .items("workflows")
        .empty("No workflows. Add a YAML file under .github/workflows to create one.");
        self.list(&spec, cx)
    }

    /// One workflow run: its summary, jobs and steps, and artifacts.
    pub fn run(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let base = format!("/repos/{repo}/actions/runs/{id}");
        let run = ready!(self.fetch(&base, cx));
        let (icon_name, color) = status_icon(&run.s("status"), &run.s("conclusion"));
        let in_progress = run.s("status") != "completed";
        let inval = format!("/repos/{repo}/actions");
        let mut buttons = widgets::row().flex_wrap();
        if in_progress {
            buttons = buttons
                .child(widgets::btn("cancel-run", "Cancel run", Req::rest("POST", format!("{base}/cancel")).ok("Cancelling").inval(inval.clone()).act()))
                .child(widgets::btn(
                    "force-cancel",
                    "Force cancel",
                    Req::rest("POST", format!("{base}/force-cancel")).ok("Force-cancelling").inval(inval.clone()).act(),
                ));
        } else {
            buttons = buttons
                .child(widgets::btn("rerun", "Re-run all jobs", Req::rest("POST", format!("{base}/rerun")).ok("Re-run requested").inval(inval.clone()).act()))
                .child(widgets::btn(
                    "rerun-failed",
                    "Re-run failed jobs",
                    Req::rest("POST", format!("{base}/rerun-failed-jobs")).ok("Re-run requested").inval(inval.clone()).act(),
                ));
        }
        if run.s("conclusion") == "action_required" {
            buttons = buttons.child(widgets::go_btn(
                "approve-run",
                "Approve and run",
                Req::rest("POST", format!("{base}/approve")).ok("Run approved").inval(inval.clone()).act(),
            ));
        }
        let repo_back = repo.to_string();
        buttons = buttons
            .child(widgets::btn(
                "workflow-file",
                "Workflow file",
                Act::Go(Route::Tree { repo: repo.to_string(), git_ref: run.s("head_sha"), path: run.s("path"), file: true }),
            ))
            .child(widgets::danger(
                "delete-run",
                "Delete run",
                Req::rest("DELETE", base.clone())
                    .ok("Run deleted")
                    .inval(inval.clone())
                    .then(move |hub, _, cx| hub.go(Route::Repo { repo: repo_back.clone(), tab: RepoTab::Actions }, cx))
                    .act()
                    .confirm("Delete this run?", "Its logs and artifacts are deleted too.", "Delete"),
            ));

        let summary = widgets::card().child(
            widgets::row()
                .p_4()
                .gap_6()
                .flex_wrap()
                .child(kv_col("Triggered via", format!("{} {}", run.s("event"), time::ago(&run.s("created_at")))))
                .child(kv_col("Actor", run.s("triggering_actor.login")))
                .child(kv_col("Branch", run.s("head_branch")))
                .child(kv_col("Commit", run.s("head_sha").chars().take(7).collect::<String>()))
                .child(kv_col("Status", if in_progress { run.s("status") } else { run.s("conclusion") }))
                .child(kv_col("Duration", time::span(&run.s("run_started_at"), &run.s("updated_at"))))
                .child(kv_col("Attempt", format!("#{}", run.i("run_attempt")))),
        );

        // Deployments waiting on a reviewer.
        let pending = self.fetch(&format!("{base}/pending_deployments"), cx);
        let mut approvals = widgets::col().gap_2();
        if let Some(list) = pending.ready() {
            for (i, d) in list.list("").iter().enumerate() {
                let env_id = d.i("environment.id");
                let env = d.s("environment.name");
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
                    widgets::card().child(
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

        // Jobs and their steps.
        let jobs = self.fetch(&format!("{base}/jobs?filter=latest&per_page=100"), cx);
        let mut job_cards = widgets::col().gap_3();
        match jobs {
            Load::Ready(list) => {
                for (i, job) in list.list("jobs").iter().enumerate() {
                    let (jicon, jcolor) = status_icon(&job.s("status"), &job.s("conclusion"));
                    let job_id = job.i("id") as u64;
                    let name = job.s("name");
                    let open_key = format!("job-open:{job_id}");
                    let open = self.is_open(&open_key) || job.s("conclusion") == "failure";
                    let toggle = open_key.clone();
                    let mut card = widgets::card().child(
                        widgets::card_header()
                            .child(
                                crate::ui::IconButton::new(ElementId::Name(format!("job-fold-{i}").into()), if open { "chevron-down" } else { "chevron-right" })
                                    .on_click(on(Act::run(move |hub, _, cx| {
                                        if !hub.open.remove(&toggle) {
                                            hub.open.insert(toggle.clone());
                                        }
                                        cx.notify();
                                    }))),
                            )
                            .child(icon(jicon, 16.0, jcolor))
                            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .child(widgets::dim(time::span(&job.s("started_at"), &job.s("completed_at"))))
                            .child(widgets::btn(
                                ElementId::Name(format!("logs-{i}").into()),
                                "View logs",
                                Act::Go(Route::Job { repo: repo.to_string(), id: job_id, name: name.clone() }),
                            ))
                            .when(job.s("status") == "completed", |d| {
                                d.child(widgets::btn(
                                    ElementId::Name(format!("rerun-job-{i}").into()),
                                    "Re-run",
                                    Req::rest("POST", format!("/repos/{repo}/actions/jobs/{job_id}/rerun"))
                                        .ok("Job re-run requested")
                                        .inval(inval.clone())
                                        .act(),
                                ))
                            }),
                    );
                    if open {
                        let mut steps = div().flex().flex_col().py_1();
                        for step in job.list("steps") {
                            let (sicon, scolor) = status_icon(&step.s("status"), &step.s("conclusion"));
                            steps = steps.child(
                                widgets::row()
                                    .px_4()
                                    .py_1()
                                    .child(icon(sicon, 14.0, scolor))
                                    .child(div().flex_1().child(step.s("name")))
                                    .child(widgets::faint(time::span(&step.s("started_at"), &step.s("completed_at")))),
                            );
                        }
                        card = card.child(steps);
                    }
                    job_cards = job_cards.child(card);
                }
            }
            other => job_cards = job_cards.child(widgets::placeholder(&other)),
        }

        let repo_s = repo.to_string();
        let artifacts = ListSpec::new(format!("{base}/artifacts"), move |a| {
            Row::new(a.s("name"))
                .icon("package", widgets::gray())
                .meta(json::bytes(a.i("size_in_bytes")))
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
        .empty("No artifacts.");
        let artifacts = self.list(&artifacts, cx);

        widgets::page()
            .child(
                widgets::row()
                    .child(icon(icon_name, 24.0, color))
                    .child(
                        widgets::col()
                            .gap_0()
                            .flex_1()
                            .child(widgets::title(run.s("display_title")))
                            .child(widgets::dim(format!("{} #{}", run.s("name"), run.i("run_number")))),
                    ),
            )
            .child(buttons)
            .child(summary)
            .child(approvals)
            .child(widgets::h2("Jobs"))
            .child(job_cards)
            .child(widgets::h2("Artifacts"))
            .child(artifacts)
            .child(div().h(px(1.0)).bg(rgb(p.divider)))
            .into_any_element()
    }

    /// A job's log, with GitHub's markers turned into styling.
    pub fn job(&mut self, repo: &str, id: u64, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let path = format!("/repos/{repo}/actions/jobs/{id}/logs");
        let text = match self.fetch_text(&path, "application/vnd.github+json", cx) {
            Load::Ready(v) => v.s(""),
            other => return widgets::page().child(widgets::title(name.to_string())).child(widgets::placeholder(&other)).into_any_element(),
        };
        let lines: std::rc::Rc<Vec<(String, u32, bool)>> = std::rc::Rc::new(
            text.lines()
                .map(|line| {
                    // "2024-01-01T00:00:00.0000000Z message"
                    let line = match line.split_once(' ') {
                        Some((stamp, rest)) if stamp.len() >= 20 && stamp.as_bytes().get(4) == Some(&b'-') => rest,
                        _ => line,
                    };
                    let line = line.trim_start_matches('\u{feff}');
                    if let Some(rest) = line.strip_prefix("##[group]") {
                        (format!("▸ {rest}"), p.text, true)
                    } else if line.starts_with("##[endgroup]") {
                        (String::new(), p.text_faint, false)
                    } else if let Some(rest) = line.strip_prefix("##[error]") {
                        (format!("Error: {rest}"), widgets::red(), true)
                    } else if let Some(rest) = line.strip_prefix("##[warning]") {
                        (format!("Warning: {rest}"), widgets::yellow(), false)
                    } else if let Some(rest) = line.strip_prefix("##[command]") {
                        (rest.to_string(), p.accent_hover, false)
                    } else if let Some(rest) = line.strip_prefix("##[debug]") {
                        (rest.to_string(), p.text_faint, false)
                    } else {
                        (strip_ansi(line), p.text, false)
                    }
                })
                .collect(),
        );
        let count = lines.len();
        let list_lines = lines.clone();
        let scroll = self.list_scroller("log-lines");
        let list = gpui::uniform_list("log-lines", count, move |range, _, _| {
            range
                .map(|i| {
                    let (text, color, bold) = &list_lines[i];
                    div()
                        .flex()
                        .flex_row()
                        .h(px(19.0))
                        .child(div().w(px(56.0)).flex_none().pr_3().text_right().text_color(rgb(p.text_faint)).child((i + 1).to_string()))
                        .child(
                            div()
                                .whitespace_nowrap()
                                .text_color(rgb(*color))
                                .when(*bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(text.clone()),
                        )
                })
                .collect()
        })
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .py_2()
        .font_family(widgets::MONO)
        .text_size(px(12.0));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_4()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::title(name.to_string()))
                    .child(widgets::spacer())
                    .child(widgets::dim(format!("{count} lines")))
                    .child(widgets::btn("copy-log", "Copy log", Act::Copy(text.clone())))
                    .child(widgets::btn(
                        "rerun-this",
                        "Re-run job",
                        Req::rest("POST", format!("/repos/{repo}/actions/jobs/{id}/rerun")).ok("Job re-run requested").act(),
                    ))
                    .child(widgets::btn("refresh-log", "Reload", Act::run(move |hub, _, cx| {
                        hub.invalidate(&path);
                        cx.notify();
                    }))),
            )
            .child(widgets::card().flex_1().min_h_0().bg(rgb(p.deep_bg)).child(list))
            .into_any_element()
    }
}

fn kv_col(label: &str, value: String) -> AnyElement {
    widgets::col()
        .gap_0()
        .child(widgets::faint(label.to_string()))
        .child(div().font_weight(FontWeight::MEDIUM).child(if value.is_empty() { "—".to_string() } else { value }))
        .into_any_element()
}

/// Terminal colour codes out of a log line.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}


#[cfg(test)]
mod tests {
    #[test]
    fn ansi_is_stripped() {
        assert_eq!(super::strip_ansi("\u{1b}[31mred\u{1b}[0m text"), "red text");
    }
}
