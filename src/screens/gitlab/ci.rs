//! GitLab CI/CD: a project's pipelines, jobs, schedules, environments and
//! runners; one pipeline, laid out by stage; and one job with its log,
//! which GitLab's API gives in full (and keeps giving while it runs).

use super::{ci_icon, ci_word, project_api};
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, RepoTab, Req, Route};
use crate::json::{self, enc, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use serde_json::{json, Value};

/// "Run pipeline": a branch or tag, and variables as `KEY=value` lines.
fn run_form(repo: &str, default_branch: &str) -> Act {
    let (api, target) = (project_api(repo), repo.to_string());
    FormSpec::new("Run pipeline")
        .submit("Run pipeline")
        .field(
            Field::text("ref", "Branch or tag")
                .value(default_branch)
                .required(),
        )
        .field(Field::multiline("variables", "Variables").hint("One KEY=value a line."))
        .build_with(move |v| {
            let variables: Vec<Value> = v
                .s("variables")
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, val)| json!({ "key": k.trim(), "value": val.trim() }))
                .collect();
            let target = target.clone();
            Ok(Req::rest("POST", format!("{api}/pipeline"))
                .body(json!({ "ref": v.s("ref").trim(), "variables": variables }))
                .ok("Pipeline started")
                .inval(format!("{api}/pipelines"))
                .then(move |hub, value, cx| {
                    hub.go(
                        Route::Run {
                            repo: target.clone(),
                            id: value.i("id") as u64,
                        },
                        cx,
                    )
                })
                .act())
        })
        .act()
}

/// What can be done to a pipeline in its state.
fn pipeline_acts(repo: &str, p: &Value) -> Vec<(String, Act, bool)> {
    let api = project_api(repo);
    let id = p.i("id");
    let base = format!("{api}/pipelines/{id}");
    let inval = format!("{api}/pipelines");
    let status = p.s("status");
    let mut acts = Vec::new();
    if matches!(
        status.as_str(),
        "running" | "pending" | "created" | "waiting_for_resource" | "preparing" | "scheduled"
    ) {
        acts.push((
            "Cancel".to_string(),
            Req::rest("POST", format!("{base}/cancel"))
                .ok("Cancelling")
                .inval(inval.clone())
                .act(),
            false,
        ));
    } else {
        acts.push((
            "Retry failed jobs".to_string(),
            Req::rest("POST", format!("{base}/retry"))
                .ok("Retrying")
                .inval(inval.clone())
                .act(),
            false,
        ));
    }
    acts.push((crate::forge::open_on(), Act::Url(p.s("web_url")), false));
    acts.push((
        "Delete".to_string(),
        Req::rest("DELETE", base)
            .ok("Pipeline deleted")
            .inval(inval)
            .act()
            .confirm(
                "Delete this pipeline?",
                "Its jobs, logs and artifacts are deleted too.",
                "Delete",
            ),
        true,
    ));
    acts
}

fn pipeline_row(repo: &str, p: &Value) -> Row {
    let (mark, color) = ci_icon(&p.s("status"));
    let title = if p.s("name").is_empty() {
        format!("Pipeline #{}", p.i("iid"))
    } else {
        p.s("name")
    };
    let mut row = Row::new(title)
        .icon(mark, color)
        .meta(format!(
            "{}  ·  {}  ·  {}  ·  {}",
            p.s("ref").trim_start_matches("refs/"),
            p.s("sha").chars().take(8).collect::<String>(),
            p.s("source").replace('_', " "),
            time::ago(&p.s("created_at"))
        ))
        .right(ci_word(&p.s("status")))
        .open(Act::Go(Route::Run {
            repo: repo.to_string(),
            id: p.i("id") as u64,
        }));
    for (label, act, danger) in pipeline_acts(repo, p) {
        row = if danger {
            row.danger(label, act)
        } else {
            row.action(label, act)
        };
    }
    row
}

/// What can be done to a job in its state.
fn job_acts(repo: &str, j: &Value) -> Vec<(String, Act)> {
    let api = project_api(repo);
    let base = format!("{api}/jobs/{}", j.i("id"));
    let inval = format!("{api}/");
    let mut acts = Vec::new();
    match j.s("status").as_str() {
        "manual" => acts.push((
            "Run".to_string(),
            Req::rest("POST", format!("{base}/play"))
                .ok("Job started")
                .inval(inval.clone())
                .act(),
        )),
        "running" | "pending" | "created" | "waiting_for_resource" | "preparing" => acts.push((
            "Cancel".to_string(),
            Req::rest("POST", format!("{base}/cancel"))
                .ok("Cancelling")
                .inval(inval.clone())
                .act(),
        )),
        _ => acts.push((
            "Retry".to_string(),
            Req::rest("POST", format!("{base}/retry"))
                .ok("Job retried")
                .inval(inval.clone())
                .act(),
        )),
    }
    if j.has("artifacts_file") {
        acts.push((
            "Download artifacts".to_string(),
            Act::Url(format!(
                "{}/{repo}/-/jobs/{}/artifacts/download",
                crate::forge::web(),
                j.i("id")
            )),
        ));
    }
    acts
}

impl Hub {
    /// A project's CI/CD tab.
    pub fn gl_ci(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_key = format!("ci.view:{repo}");
        let view = self.choice(&view_key, "pipelines");
        let chips = widgets::chips(
            [
                ("pipelines", "Pipelines"),
                ("jobs", "Jobs"),
                ("schedules", "Schedules"),
                ("environments", "Environments"),
                ("runners", "Runners"),
            ]
            .iter()
            .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
            .collect(),
        );
        let body = match view.as_str() {
            "jobs" => self.gl_jobs(repo, cx),
            "schedules" => self.gl_schedules(repo, default_branch, cx),
            "environments" => self.gl_environments(repo, cx),
            "runners" => self.gl_runners(repo, cx),
            _ => self.gl_pipelines(repo, default_branch, cx),
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    fn gl_pipelines(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let api = project_api(repo);
        let status_key = format!("ci.status:{repo}");
        let status = self.choice(&status_key, "");
        let ref_field = format!("ci.ref:{repo}");
        let applied_key = format!("ci.ref.applied:{repo}");
        let applied = self.choice(&applied_key, "");
        let (field, key) = (ref_field.clone(), applied_key.clone());
        self.submits.insert(
            ref_field.clone(),
            Act::run(move |hub, _, cx| {
                let text = hub.field_text(&field).trim().to_string();
                hub.choices.insert(key.clone(), text);
                cx.notify();
            }),
        );
        let mut path = format!("{api}/pipelines?order_by=id&sort=desc");
        if !status.is_empty() {
            path.push_str(&format!("&status={status}"));
        }
        if !applied.is_empty() {
            path.push_str(&format!("&ref={}", enc(&applied)));
        }
        let statuses = [
            ("", "Any status"),
            ("running", "Running"),
            ("pending", "Pending"),
            ("success", "Passed"),
            ("failed", "Failed"),
            ("canceled", "Canceled"),
            ("manual", "Manual"),
            ("skipped", "Skipped"),
        ];
        let status_menu = Act::menu(
            statuses
                .iter()
                .map(|(v, l)| MenuEntry::check(*l, status == *v, Act::choose(&status_key, *v)))
                .collect(),
        );
        let repo_s = repo.to_string();
        let spec =
            ListSpec::new(path, move |p| pipeline_row(&repo_s, p)).empty("No pipelines yet.");
        // Keep it live while something runs.
        if let Some(first) = self.fetch(&with_page(&spec.path), cx).ready() {
            if first.list("").iter().any(|p| {
                matches!(
                    p.s("status").as_str(),
                    "running" | "pending" | "created" | "preparing"
                )
            }) {
                self.poll(&[format!("{api}/pipelines")], 10, cx);
            }
        }
        let list = self.list(&spec, cx);
        let input = self
            .input(&ref_field, "Branch or tag — Enter", cx)
            .w(px(240.0));
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::btn(
                        "ci-status",
                        format!(
                            "{} ▾",
                            statuses
                                .iter()
                                .find(|s| s.0 == status)
                                .map(|s| s.1)
                                .unwrap_or("Any status")
                        ),
                        status_menu,
                    ))
                    .child(input)
                    .child(widgets::spacer())
                    .child(widgets::go_btn(
                        "run-pipeline",
                        "Run pipeline",
                        run_form(repo, default_branch),
                    )),
            )
            .child(list)
            .into_any_element()
    }

    fn gl_jobs(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let api = project_api(repo);
        let scope_key = format!("ci.jobs:{repo}");
        let scope = self.choice(&scope_key, "");
        let path = if scope.is_empty() {
            format!("{api}/jobs")
        } else {
            format!("{api}/jobs?scope[]={scope}")
        };
        let repo_s = repo.to_string();
        let spec = ListSpec::new(path, move |j| {
            let (mark, color) = ci_icon(&j.s("status"));
            let took = j.i("duration");
            let mut row = Row::new(j.s("name"))
                .icon(mark, color)
                .meta(format!(
                    "{}  ·  pipeline #{}  ·  {}  ·  {}{}",
                    j.s("stage"),
                    j.i("pipeline.id"),
                    j.s("ref"),
                    time::ago(&j.s("created_at")),
                    if took > 0 {
                        format!("  ·  {}", time::duration(took))
                    } else {
                        String::new()
                    }
                ))
                .right(ci_word(&j.s("status")))
                .open(Act::Go(Route::Job {
                    repo: repo_s.clone(),
                    id: j.i("id") as u64,
                }));
            for (label, act) in job_acts(&repo_s, j) {
                row = row.action(label, act);
            }
            row
        })
        .empty("No jobs.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(widgets::chips(
                [
                    ("", "All"),
                    ("running", "Running"),
                    ("pending", "Pending"),
                    ("failed", "Failed"),
                    ("success", "Passed"),
                    ("manual", "Manual"),
                ]
                .iter()
                .map(|(v, l)| (l.to_string(), scope == *v, Act::choose(&scope_key, *v)))
                .collect(),
            ))
            .child(list)
            .into_any_element()
    }

    fn gl_schedules(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let api = project_api(repo);
        let base = format!("{api}/pipeline_schedules");
        let create = FormSpec::new("New pipeline schedule")
            .submit("Create schedule")
            .field(Field::text("description", "Description").required())
            .field(
                Field::text("cron", "Interval")
                    .value("0 3 * * *")
                    .required()
                    .hint("Cron syntax: minute hour day month weekday."),
            )
            .field(Field::text("cron_timezone", "Time zone").value("UTC"))
            .field(
                Field::text("ref", "Branch or tag")
                    .value(default_branch)
                    .required(),
            )
            .field(Field::bool("active", "Active", true))
            .rest("POST", base.clone())
            .ok("Schedule created")
            .inval(base.clone())
            .act();
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |s| {
            let path = format!("{b}/{}", s.i("id"));
            let active = s.b("active");
            Row::new(s.s("description"))
                .icon(
                    "clock",
                    if active {
                        widgets::green()
                    } else {
                        widgets::gray()
                    },
                )
                .meta(format!(
                    "{} ({})  ·  {}  ·  next {}  ·  owned by {}",
                    s.s("cron"),
                    s.s("cron_timezone"),
                    s.s("ref"),
                    if active {
                        time::ago(&s.s("next_run_at")).replace(" ago", "")
                    } else {
                        "never: inactive".into()
                    },
                    s.s("owner.username")
                ))
                .action(
                    "Run now",
                    Req::rest("POST", format!("{path}/play"))
                        .ok("Pipeline scheduled to run")
                        .act(),
                )
                .action(
                    if active { "Deactivate" } else { "Activate" },
                    Req::rest("PUT", path.clone())
                        .body(json!({ "active": !active }))
                        .ok("Schedule updated")
                        .inval(b.clone())
                        .act(),
                )
                .action(
                    "Take ownership",
                    Req::rest("POST", format!("{path}/take_ownership"))
                        .ok("You own this schedule now")
                        .inval(b.clone())
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", path)
                        .ok("Schedule deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete this schedule?",
                            "Pipelines stop running on it.",
                            "Delete",
                        ),
                )
        })
        .empty("No pipeline schedules.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("new-schedule", "New schedule", create)),
            )
            .child(list)
            .into_any_element()
    }

    fn gl_environments(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("{}/environments", project_api(repo));
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |e| {
            let path = format!("{b}/{}", e.i("id"));
            let available = e.s("state") == "available";
            let mut row = Row::new(e.s("name"))
                .icon(
                    "deploy",
                    if available {
                        widgets::green()
                    } else {
                        widgets::gray()
                    },
                )
                .meta(format!(
                    "{}{}",
                    e.s("state"),
                    if e.s("external_url").is_empty() {
                        String::new()
                    } else {
                        format!("  ·  {}", e.s("external_url"))
                    }
                ));
            if !e.s("external_url").is_empty() {
                row = row.action("Open", Act::Url(e.s("external_url")));
            }
            if available {
                row = row.action(
                    "Stop",
                    Req::rest("POST", format!("{path}/stop"))
                        .ok("Environment stopping")
                        .inval(b.clone())
                        .act(),
                );
            }
            row.danger(
                "Delete",
                Req::rest("DELETE", path)
                    .ok("Environment deleted")
                    .inval(b.clone())
                    .act()
                    .confirm(
                        "Delete this environment?",
                        "Its deployment history goes too. Stop it first.",
                        "Delete",
                    ),
            )
        })
        .empty("No environments. Jobs create them when they deploy.");
        self.list(&spec, cx)
    }

    fn gl_runners(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let base = format!("{}/runners", project_api(repo));
        let b = base.clone();
        let spec = ListSpec::new(base.clone(), move |r| {
            let paused = r.b("paused") || !r.b("active");
            let online = r.s("status") == "online";
            Row::new(if r.s("description").is_empty() {
                format!("Runner {}", r.i("id"))
            } else {
                r.s("description")
            })
            .icon(
                "codespace",
                if online && !paused {
                    widgets::green()
                } else {
                    widgets::gray()
                },
            )
            .meta(format!(
                "{}  ·  {}{}",
                r.s("runner_type").replace('_', " "),
                r.s("status"),
                if paused { "  ·  paused" } else { "" }
            ))
            .action(
                if paused { "Resume" } else { "Pause" },
                Req::rest("PUT", format!("/api/v4/runners/{}", r.i("id")))
                    .body(json!({ "paused": !paused }))
                    .ok("Runner updated")
                    .inval(b.clone())
                    .act(),
            )
            .danger(
                "Remove from project",
                Req::rest("DELETE", format!("{b}/{}", r.i("id")))
                    .ok("Runner removed")
                    .inval(b.clone())
                    .act()
                    .confirm(
                        "Remove this runner?",
                        "It stops picking up this project's jobs.",
                        "Remove",
                    ),
            )
        })
        .empty(
            "No runners for this project. The instance's shared runners may still run its jobs.",
        );
        self.list(&spec, cx)
    }

    /// One pipeline: what started it, how it went, its jobs by stage,
    /// its tests and its artifacts.
    pub fn gl_pipeline(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let api = project_api(repo);
        let base = format!("{api}/pipelines/{id}");
        let pipeline = ready!(self.fetch(&base, cx));
        let running = matches!(
            pipeline.s("status").as_str(),
            "running" | "pending" | "created" | "preparing" | "waiting_for_resource"
        );
        if running {
            self.poll(&[base.clone(), format!("{api}/jobs")], 5, cx);
        }
        let jobs: Vec<Value> = match self.fetch(
            &format!("{base}/jobs?per_page=100&include_retried=false"),
            cx,
        ) {
            Load::Ready(list) => list.list("").to_vec(),
            _ => Vec::new(),
        };
        let bridges: Vec<Value> = self
            .fetch(&format!("{base}/bridges?per_page=100"), cx)
            .ready()
            .map(|v| v.list("").to_vec())
            .unwrap_or_default();
        let (mark, color) = ci_icon(&pipeline.s("status"));
        let acts = pipeline_acts(repo, &pipeline);
        let mut actions = widgets::row();
        if let Some((label, act, _)) = acts.first() {
            actions = actions.child(
                widgets::btn("pipeline-main", label.clone(), act.clone())
                    .h(px(32.0))
                    .px_3(),
            );
        }
        let more = Act::menu(
            acts.iter()
                .skip(1)
                .map(|(label, act, _)| MenuEntry::item(label.clone(), act.clone()))
                .collect(),
        );
        let title = if pipeline.s("name").is_empty() {
            format!("Pipeline #{}", pipeline.i("iid"))
        } else {
            pipeline.s("name")
        };
        let header = widgets::col()
            .gap_1()
            .child(
                crate::ui::Link::new("pipeline-back", "← Pipelines")
                    .text_size(px(12.0))
                    .on_click(on(Act::Go(Route::Repo {
                        repo: repo.to_string(),
                        tab: RepoTab::Actions,
                    }))),
            )
            .child(
                widgets::row()
                    .gap_3()
                    .child(icon(mark, 22.0, color))
                    .child(widgets::title(title).flex_1().min_w_0())
                    .child(actions)
                    .child(
                        IconButton::new("pipeline-more", "kebab")
                            .size(32.0)
                            .icon_size(16.0)
                            .on_click(on(more)),
                    ),
            );

        let sha = pipeline.s("sha");
        let avatar = self.avatar(&pipeline.s("user.avatar_url"), 20.0, cx);
        let block = |label: &str, body: AnyElement| {
            widgets::col()
                .gap_1()
                .child(widgets::faint(label.to_string()))
                .child(body)
        };
        let summary = widgets::card().child(
            widgets::row()
                .p_4()
                .gap_8()
                .flex_wrap()
                .items_start()
                .child(block(
                    &format!(
                        "Triggered by {} {}",
                        pipeline.s("source").replace('_', " "),
                        time::ago(&pipeline.s("created_at"))
                    ),
                    widgets::row()
                        .gap_2()
                        .child(avatar)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pipeline.s("user.username")),
                        )
                        .child(
                            crate::ui::Link::new(
                                "pipeline-sha",
                                sha.chars().take(8).collect::<String>(),
                            )
                            .on_click(on(Act::Go(Route::Commit {
                                repo: repo.to_string(),
                                sha: sha.clone(),
                            }))),
                        )
                        .child(widgets::tag(pipeline.s("ref"), widgets::gray()))
                        .into_any_element(),
                ))
                .child(block(
                    "Status",
                    widgets::row()
                        .gap_1p5()
                        .child(icon(mark, 14.0, color))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(ci_word(&pipeline.s("status"))),
                        )
                        .into_any_element(),
                ))
                .child(block(
                    "Duration",
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if pipeline.i("duration") > 0 {
                            time::duration(pipeline.i("duration"))
                        } else {
                            "–".into()
                        })
                        .into_any_element(),
                ))
                .when(pipeline.has("coverage"), |d| {
                    d.child(block(
                        "Coverage",
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{}%", pipeline.s("coverage")))
                            .into_any_element(),
                    ))
                }),
        );

        let graph = self.gl_stages(repo, &jobs, &bridges);
        let tests = self
            .fetch(&format!("{base}/test_report_summary"), cx)
            .ready()
            .cloned()
            .filter(|t| t.i("total.count") > 0)
            .map(|t| {
                let failed = t.i("total.failed") + t.i("total.error");
                widgets::card()
                    .child(
                        widgets::card_header()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Tests")),
                    )
                    .child(
                        widgets::row()
                            .p_4()
                            .gap_6()
                            .child(widgets::dim(format!("{} tests", t.i("total.count"))))
                            .child(
                                div()
                                    .text_color(rgb(widgets::green()))
                                    .child(format!("{} passed", t.i("total.success"))),
                            )
                            .child(
                                div()
                                    .text_color(rgb(if failed > 0 {
                                        widgets::red()
                                    } else {
                                        p.text_dim
                                    }))
                                    .child(format!("{failed} failed")),
                            )
                            .child(widgets::dim(format!("{} skipped", t.i("total.skipped")))),
                    )
            });
        let mut artifacts = widgets::card();
        let mut any = false;
        for (i, j) in jobs.iter().filter(|j| j.has("artifacts_file")).enumerate() {
            any = true;
            let job_page = format!("{}/{repo}/-/jobs/{}", crate::forge::web(), j.i("id"));
            artifacts = artifacts.child(
                widgets::list_row(
                    ElementId::Name(format!("artifact-{i}").into()),
                    Act::Url(format!("{job_page}/artifacts/browse")),
                )
                .items_center()
                .child(icon("package", 16.0, p.text_dim))
                .child(div().flex_1().child(format!(
                    "{}  ·  {}",
                    j.s("name"),
                    j.s("artifacts_file.filename")
                )))
                .child(widgets::dim(json::bytes(j.i("artifacts_file.size"))))
                .child(
                    widgets::btn(
                        ElementId::Name(format!("artifact-dl-{i}").into()),
                        "Download",
                        Act::Url(format!("{job_page}/artifacts/download")),
                    )
                    .h(px(24.0)),
                ),
            );
        }
        widgets::page()
            .max_w(px(1400.0))
            .child(header)
            .child(summary)
            .child(graph)
            .children(tests)
            .child(widgets::h3("Artifacts"))
            .child(if any {
                artifacts.into_any_element()
            } else {
                widgets::card()
                    .child(widgets::empty("No artifacts were kept from this pipeline."))
                    .into_any_element()
            })
            .into_any_element()
    }

    /// The jobs in columns, one per stage in the order they run, as
    /// GitLab draws a pipeline.
    fn gl_stages(&mut self, repo: &str, jobs: &[Value], bridges: &[Value]) -> AnyElement {
        let p = palette();
        let mut all: Vec<&Value> = jobs.iter().chain(bridges.iter()).collect();
        all.sort_by_key(|j| j.i("id"));
        let mut stages: Vec<(String, Vec<&Value>)> = Vec::new();
        for job in all {
            let stage = job.s("stage");
            match stages.iter_mut().find(|(s, _)| *s == stage) {
                Some((_, list)) => list.push(job),
                None => stages.push((stage, vec![job])),
            }
        }
        let mut lanes = div().flex().flex_row().items_start().gap_6().p_6();
        for (s, (stage, members)) in stages.iter().enumerate() {
            let mut column = widgets::col().gap_2().w(px(240.0)).flex_none().child(
                div()
                    .pb_1()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(p.text_dim))
                    .child(stage.clone()),
            );
            for (i, job) in members.iter().enumerate() {
                let (mark, color) = ci_icon(&job.s("status"));
                let bridge = job.has("downstream_pipeline");
                let act = if bridge {
                    match job.at("downstream_pipeline") {
                        d if !d.is_null() => Act::Url(d.s("web_url")),
                        _ => Act::None,
                    }
                } else {
                    Act::Go(Route::Job {
                        repo: repo.to_string(),
                        id: job.i("id") as u64,
                    })
                };
                let play = (job.s("status") == "manual" && !bridge).then(|| {
                    IconButton::new(ElementId::Name(format!("play-{s}-{i}").into()), "play")
                        .size(22.0)
                        .icon_size(12.0)
                        .tooltip("Run this job", None)
                        .consume_press()
                        .on_click(on(Req::rest(
                            "POST",
                            format!("{}/jobs/{}/play", project_api(repo), job.i("id")),
                        )
                        .ok("Job started")
                        .inval(format!("{}/", project_api(repo)))
                        .act()))
                });
                column = column.child(
                    div()
                        .id(ElementId::Name(format!("stage-job-{s}-{i}").into()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(36.0))
                        .px_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(p.edge))
                        .bg(rgb(p.panel_bg))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.hover)))
                        .on_click(on(act))
                        .child(icon(if bridge { "arrow-right" } else { mark }, 14.0, color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(job.s("name")),
                        )
                        .when(job.i("duration") > 0, |d| {
                            d.child(widgets::faint(time::duration(job.i("duration"))))
                        })
                        .children(play),
                );
            }
            lanes = lanes.child(column);
        }
        widgets::card()
            .child(
                widgets::card_header()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Stages"))
                    .child(widgets::dim(format!("{} jobs", jobs.len()))),
            )
            .child(
                div()
                    .id("pipeline-stages")
                    .overflow_x_scroll()
                    .track_scroll(&self.scroller("pipeline-stages"))
                    .bg(rgb(p.deep_bg))
                    .child(if stages.is_empty() {
                        widgets::loading()
                    } else {
                        lanes.into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// One job and its log.
    pub fn gl_job(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let api = project_api(repo);
        let base = format!("{api}/jobs/{id}");
        let job = ready!(self.fetch(&base, cx));
        let running = matches!(
            job.s("status").as_str(),
            "running" | "pending" | "created" | "preparing" | "waiting_for_resource"
        );
        if running {
            self.poll(&[base.clone()], 4, cx);
        }
        let (mark, color) = ci_icon(&job.s("status"));
        let mut buttons = widgets::row();
        for (i, (label, act)) in job_acts(repo, &job).into_iter().enumerate() {
            buttons = buttons.child(widgets::btn(
                ElementId::Name(format!("job-act-{i}").into()),
                label,
                act,
            ));
        }
        buttons = buttons
            .child(widgets::btn(
                "job-web",
                crate::forge::open_on(),
                Act::Url(job.s("web_url")),
            ))
            .when(!running, |d| {
                d.child(widgets::danger(
                    "job-erase",
                    "Erase",
                    Req::rest("POST", format!("{base}/erase"))
                        .ok("Job log and artifacts erased")
                        .inval(base.clone())
                        .act()
                        .confirm(
                            "Erase this job?",
                            "Its log and artifacts are deleted.",
                            "Erase",
                        ),
                ))
            });
        let pipeline = job.i("pipeline.id") as u64;
        let detail = format!(
            "{}  ·  {}  ·  {}{}{}",
            job.s("stage"),
            job.s("ref"),
            ci_word(&job.s("status")),
            if job.i("duration") > 0 {
                format!(" in {}", time::duration(job.i("duration")))
            } else {
                String::new()
            },
            if job.s("runner.description").is_empty() {
                String::new()
            } else {
                format!("  ·  on {}", job.s("runner.description"))
            }
        );
        let log = match self.fetch_text(&format!("{base}/trace"), "text/plain", cx) {
            Load::Ready(v) => {
                let text = clean_log(&v.s(""));
                if text.trim().is_empty() {
                    widgets::empty(if running {
                        "Waiting for the log…"
                    } else {
                        "This job left no log."
                    })
                } else {
                    crate::screens::repo::numbered_lines(
                        &text,
                        "job.log",
                        self.list_scroller("job-log"),
                    )
                }
            }
            other => widgets::placeholder(&other),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_3()
            .px_6()
            .py_4()
            .child(
                crate::ui::Link::new("job-back", format!("← Pipeline #{pipeline}"))
                    .text_size(px(12.0))
                    .on_click(on(Act::Go(Route::Run {
                        repo: repo.to_string(),
                        id: pipeline,
                    }))),
            )
            .child(
                widgets::row()
                    .gap_3()
                    .child(icon(mark, 22.0, color))
                    .child(
                        widgets::col()
                            .flex_1()
                            .min_w_0()
                            .gap_0()
                            .child(widgets::title(job.s("name")))
                            .child(widgets::dim(detail)),
                    )
                    .child(buttons),
            )
            .when(job.has("failure_reason"), |d| {
                d.child(
                    widgets::row()
                        .gap_2()
                        .child(icon("alert", 14.0, widgets::red()))
                        .child(
                            div()
                                .text_color(rgb(widgets::red()))
                                .child(job.s("failure_reason").replace('_', " ")),
                        ),
                )
            })
            .child(
                widgets::card()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(p.deep_bg))
                    .child(log),
            )
            .into_any_element()
    }
}

/// The first page of a list, as [`Hub::list`] asks for it.
fn with_page(path: &str) -> String {
    crate::hub::with_query(
        path,
        &format!("per_page={}&page=1", crate::resource::PER_PAGE),
    )
}

/// A job log as plain lines: colour codes and GitLab's section markers
/// gone, and a line rewritten with carriage returns showing its last
/// version, as a terminal would.
pub fn clean_log(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let mut text = String::with_capacity(line.len());
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                text.push(c);
                continue;
            }
            // An escape: `ESC [ parameters letter`.
            if chars.next() == Some('[') {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        }
        if text.starts_with("section_end:") {
            continue;
        }
        let shown = text.rsplit('\r').next().unwrap_or("");
        if shown.starts_with("section_start:") {
            continue;
        }
        out.push_str(shown);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::clean_log;

    #[test]
    fn logs_lose_their_escapes() {
        let raw = "\u{1b}[0KRunning with gitlab-runner\r\nsection_start:1700000000:prepare\r\u{1b}[0K\u{1b}[0;1mPreparing\u{1b}[0;m\nprogress 10%\rprogress 100%\nsection_end:1700000001:prepare\r\u{1b}[0K\n";
        assert_eq!(
            clean_log(raw),
            "Running with gitlab-runner\nPreparing\nprogress 100%\n"
        );
    }
}
