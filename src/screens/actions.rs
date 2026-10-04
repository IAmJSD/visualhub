//! GitHub Actions: workflow runs and their jobs, steps and logs; workflows
//! (run, enable, disable); caches; artifacts; and deployment approvals.

use super::pulls::status_icon;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, MenuEntry, Req, Route};
use crate::json::{self, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::Value;

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

}
