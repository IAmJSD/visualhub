//! Bitbucket Pipelines: a repository's pipelines, one pipeline with its
//! steps, and one step with its log.

use super::repo_api;
use crate::form::{Field, FormSpec};
use crate::hub::{on, Act, Hub, Load, MenuEntry, RepoTab, Req, Route};
use crate::json::{enc, Json as _};
use crate::ready;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette, IconButton};
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Context, FontWeight, IntoElement as _, ParentElement as _, Styled as _,
};
use serde_json::{json, Value};

/// A pipeline's or step's state as a build status's (`SUCCESSFUL`,
/// `FAILED`, `STOPPED`, `INPROGRESS`, `PENDING`) and in Bitbucket's words.
pub fn state(v: &Value) -> (&'static str, String) {
    let words = |s: &str| {
        let s = s.replace('_', " ").to_lowercase();
        let mut chars = s.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default()
    };
    match v.s("state.name").as_str() {
        "COMPLETED" => {
            let result = v.s("state.result.name");
            let key = match result.as_str() {
                "SUCCESSFUL" => "SUCCESSFUL",
                "STOPPED" => "STOPPED",
                "NOT_RUN" | "SKIPPED" => "STOPPED",
                _ => "FAILED",
            };
            (key, words(&result))
        }
        "IN_PROGRESS" | "RUNNING" => {
            let stage = v.s("state.stage.name");
            (
                "INPROGRESS",
                if stage.is_empty() || stage == "RUNNING" {
                    "Running".into()
                } else {
                    words(&stage)
                },
            )
        }
        "PAUSED" | "HALTED" => ("PENDING", words(&v.s("state.name"))),
        "" => ("PENDING", "Pending".into()),
        other => ("PENDING", words(other)),
    }
}

fn running(v: &Value) -> bool {
    matches!(state(v).0, "INPROGRESS" | "PENDING")
}

/// A state's icon and colour.
pub fn ci_icon(v: &Value) -> (&'static str, u32) {
    let (status, conclusion) = crate::bitbucket::shape::ci_status(state(v).0);
    crate::screens::pulls::status_icon(status, conclusion)
}

/// What started a pipeline, and on what.
fn target(p: &Value) -> String {
    let t = p.at("target");
    let on = if t.has("pullrequest.id") {
        format!(
            "{} → {} (#{})",
            t.s("source"),
            t.s("destination"),
            t.i("pullrequest.id")
        )
    } else if t.has("ref_name") {
        t.s("ref_name")
    } else {
        t.s("commit.hash").chars().take(8).collect()
    };
    let selector = t.s("selector.pattern");
    if selector.is_empty() {
        on
    } else {
        format!("{on}  ·  {selector}")
    }
}

/// "Run pipeline": a branch, a custom pipeline, and variables.
fn run_form(repo: &str, default_branch: &str) -> Act {
    let (api, target) = (repo_api(repo), repo.to_string());
    FormSpec::new("Run pipeline")
        .submit("Run pipeline")
        .field(
            Field::text("ref", "Branch")
                .value(default_branch)
                .required(),
        )
        .field(Field::text("pattern", "Custom pipeline").hint(
            "A name under `custom:` in bitbucket-pipelines.yml; empty runs the branch's own.",
        ))
        .field(Field::multiline("variables", "Variables").hint("One KEY=value a line."))
        .build_with(move |v| {
            let variables: Vec<Value> = v
                .s("variables")
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, val)| json!({ "key": k.trim(), "value": val.trim() }))
                .collect();
            let mut target_body = json!({
                "type": "pipeline_ref_target",
                "ref_type": "branch",
                "ref_name": v.s("ref").trim(),
            });
            let pattern = v.s("pattern").trim().to_string();
            if !pattern.is_empty() {
                target_body["selector"] = json!({ "type": "custom", "pattern": pattern });
            }
            let target = target.clone();
            Ok(Req::rest("POST", format!("{api}/pipelines/"))
                .body(json!({ "target": target_body, "variables": variables }))
                .ok("Pipeline started")
                .inval(format!("{api}/pipelines"))
                .then(move |hub, value, cx| {
                    hub.go(
                        Route::Run {
                            repo: target.clone(),
                            id: value.i("build_number") as u64,
                        },
                        cx,
                    )
                })
                .act())
        })
        .act()
}

/// What can be done to a pipeline in its state.
fn pipeline_acts(repo: &str, p: &Value) -> Vec<(String, Act)> {
    let api = repo_api(repo);
    let inval = format!("{api}/pipelines");
    let mut acts = Vec::new();
    if running(p) {
        acts.push((
            "Stop".to_string(),
            Req::rest(
                "POST",
                format!("{api}/pipelines/{}/stopPipeline", enc(&p.s("uuid"))),
            )
            .ok("Stopping")
            .inval(inval.clone())
            .act(),
        ));
    } else {
        // Bitbucket re-runs a pipeline by starting its target again.
        let mut again = json!({});
        for key in ["type", "ref_type", "ref_name", "selector", "commit"] {
            let v = p.at(&format!("target.{key}"));
            if !v.is_null() {
                again[key] = v.clone();
            }
        }
        if let Some(commit) = again.get_mut("commit") {
            *commit = json!({ "type": "commit", "hash": commit.s("hash") });
        }
        let target = repo.to_string();
        acts.push((
            "Run again".to_string(),
            Req::rest("POST", format!("{api}/pipelines/"))
                .body(json!({ "target": again }))
                .ok("Pipeline started")
                .inval(inval)
                .then(move |hub, value, cx| {
                    hub.go(
                        Route::Run {
                            repo: target.clone(),
                            id: value.i("build_number") as u64,
                        },
                        cx,
                    )
                })
                .act(),
        ));
    }
    acts.push((
        crate::forge::open_on(),
        Act::Url(
            Route::Run {
                repo: repo.to_string(),
                id: p.i("build_number") as u64,
            }
            .web_url(),
        ),
    ));
    acts
}

fn pipeline_row(repo: &str, p: &Value) -> Row {
    let (mark, color) = ci_icon(p);
    let duration = p.i("duration_in_seconds");
    let mut row = Row::new(format!("#{}  {}", p.i("build_number"), target(p)))
        .icon(mark, color)
        .meta(format!(
            "{}  ·  {}  ·  {}{}",
            p.s("target.commit.hash")
                .chars()
                .take(8)
                .collect::<String>(),
            p.s("trigger.name").to_lowercase(),
            time::ago(&p.s("created_on")),
            if duration > 0 {
                format!("  ·  {}", time::duration(duration))
            } else {
                String::new()
            }
        ))
        .right(state(p).1)
        .open(Act::Go(Route::Run {
            repo: repo.to_string(),
            id: p.i("build_number") as u64,
        }));
    for (label, act) in pipeline_acts(repo, p) {
        row = row.action(label, act);
    }
    row
}

impl Hub {
    /// A repository's Pipelines tab.
    pub fn bb_pipelines(
        &mut self,
        repo: &str,
        default_branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let api = repo_api(repo);
        let r = repo.to_string();
        let spec = ListSpec::new(format!("{api}/pipelines/?sort=-created_on"), move |p| {
            pipeline_row(&r, p)
        })
        .empty("No pipelines yet. Add a bitbucket-pipelines.yml and turn Pipelines on in the repository's settings.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn(
                        "bb-run-pipeline",
                        "Run pipeline",
                        run_form(repo, default_branch),
                    )),
            )
            .child(list)
            .into_any_element()
    }

    /// One pipeline: what started it, how it went, and its steps.
    pub fn bb_pipeline(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let api = repo_api(repo);
        let base = format!("{api}/pipelines/{id}");
        let pipeline = ready!(self.fetch(&base, cx));
        let steps_path = format!("{base}/steps/");
        if running(&pipeline) {
            self.poll(&[base.clone(), steps_path.clone()], 5, cx);
        }
        let steps: Vec<Value> = match self.fetch(&steps_path, cx) {
            Load::Ready(list) => list.list("").to_vec(),
            _ => Vec::new(),
        };
        let (mark, color) = ci_icon(&pipeline);
        let acts = pipeline_acts(repo, &pipeline);
        let mut actions = widgets::row();
        if let Some((label, act)) = acts.first() {
            actions = actions.child(
                widgets::btn("pipeline-main", label.clone(), act.clone())
                    .h(px(32.0))
                    .px_3(),
            );
        }
        let more = Act::menu(
            acts.iter()
                .skip(1)
                .map(|(label, act)| MenuEntry::item(label.clone(), act.clone()))
                .collect(),
        );
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
                    .child(widgets::title(format!("Pipeline #{id}")).flex_1().min_w_0())
                    .child(actions)
                    .child(
                        IconButton::new("pipeline-more", "kebab")
                            .size(32.0)
                            .icon_size(16.0)
                            .on_click(on(more)),
                    ),
            );
        let sha = pipeline.s("target.commit.hash");
        let avatar = self.avatar(&pipeline.s("creator.links.avatar.href"), 20.0, cx);
        let block = |label: &str, body: AnyElement| {
            widgets::col()
                .gap_1()
                .child(widgets::faint(label.to_string()))
                .child(body)
        };
        let duration = pipeline.i("duration_in_seconds");
        let summary = widgets::card().child(
            widgets::row()
                .p_4()
                .gap_8()
                .flex_wrap()
                .items_start()
                .child(block(
                    &format!(
                        "Triggered by {} {}",
                        pipeline.s("trigger.name").to_lowercase(),
                        time::ago(&pipeline.s("created_on"))
                    ),
                    widgets::row()
                        .gap_2()
                        .child(avatar)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pipeline.s("creator.display_name")),
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
                        .child(widgets::tag(target(&pipeline), widgets::gray()))
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
                                .child(state(&pipeline).1),
                        )
                        .into_any_element(),
                ))
                .child(block(
                    "Duration",
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if duration > 0 {
                            time::duration(duration)
                        } else {
                            "–".into()
                        })
                        .into_any_element(),
                )),
        );
        let mut list = widgets::card();
        if steps.is_empty() {
            list = list.child(div().p_3().child(widgets::dim("No steps yet.")));
        }
        for (i, step) in steps.iter().enumerate() {
            let (mark, color) = ci_icon(step);
            let duration = step.i("duration_in_seconds");
            let row = Row::new(if step.s("name").is_empty() {
                format!("Step {}", i + 1)
            } else {
                step.s("name")
            })
            .icon(mark, color)
            .meta(format!(
                "{}{}",
                step.s("image.name"),
                if duration > 0 {
                    format!("  ·  {}", time::duration(duration))
                } else {
                    String::new()
                }
            ))
            .right(state(step).1)
            .open(Act::Go(Route::Job {
                repo: repo.to_string(),
                id: crate::forge::step_job_id(id, i + 1),
            }));
            list = list.child(self.render_row(&format!("bb-step-{i}"), row, cx));
        }
        widgets::page()
            .child(header)
            .child(summary)
            .child(widgets::h3("Steps"))
            .child(list.border_color(rgb(p.divider)))
            .into_any_element()
    }

    /// One step and its log.
    pub fn bb_step(&mut self, repo: &str, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let (build, index) = crate::forge::job_step(id);
        let base = format!("{}/pipelines/{build}", repo_api(repo));
        let steps_path = format!("{base}/steps/");
        let steps = ready!(self.fetch(&steps_path, cx));
        let Some(step) = steps.list("").get(index.saturating_sub(1)).cloned() else {
            return widgets::empty("This pipeline has no such step.");
        };
        let running = running(&step);
        let log_path = format!("{base}/steps/{}/log", enc(&step.s("uuid")));
        if running {
            self.poll(&[steps_path.clone(), log_path.clone()], 4, cx);
        }
        let (mark, color) = ci_icon(&step);
        let duration = step.i("duration_in_seconds");
        let detail = format!(
            "{}  ·  {}{}",
            step.s("image.name"),
            state(&step).1,
            if duration > 0 {
                format!(" in {}", time::duration(duration))
            } else {
                String::new()
            }
        );
        let log = match self.fetch_text(&log_path, "text/plain", cx) {
            Load::Ready(v) => {
                let text = crate::screens::gitlab::ci::clean_log(&v.s(""));
                if text.trim().is_empty() {
                    widgets::empty(if running {
                        "Waiting for the log…"
                    } else {
                        "This step left no log."
                    })
                } else {
                    crate::screens::repo::numbered_lines(
                        &text,
                        "job.log",
                        self.list_scroller("job-log"),
                    )
                }
            }
            // Steps that never ran have no log at all.
            Load::Failed(_) if !running => widgets::empty("This step left no log."),
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
                crate::ui::Link::new("job-back", format!("← Pipeline #{build}"))
                    .text_size(px(12.0))
                    .on_click(on(Act::Go(Route::Run {
                        repo: repo.to_string(),
                        id: build,
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
                            .child(widgets::title(step.s("name")))
                            .child(widgets::dim(detail)),
                    )
                    .child(widgets::btn(
                        "job-web",
                        crate::forge::open_on(),
                        Act::Url(
                            Route::Job {
                                repo: repo.to_string(),
                                id,
                            }
                            .web_url(),
                        ),
                    )),
            )
            .when(step.has("state.result.error.message"), |d| {
                d.child(
                    widgets::row()
                        .gap_2()
                        .child(icon("alert", 14.0, widgets::red()))
                        .child(
                            div()
                                .text_color(rgb(widgets::red()))
                                .child(step.s("state.result.error.message")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_read_as_build_statuses() {
        let done = json!({ "state": { "name": "COMPLETED", "result": { "name": "FAILED" } } });
        assert_eq!(state(&done), ("FAILED", "Failed".to_string()));
        let going = json!({ "state": { "name": "IN_PROGRESS", "stage": { "name": "RUNNING" } } });
        assert_eq!(state(&going), ("INPROGRESS", "Running".to_string()));
        assert!(running(&going) && !running(&done));
    }
}
