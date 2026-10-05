//! A repository's Security, Insights and Settings tabs.

use super::common::{repo_row, user_row};
use crate::api;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Load, Req, Route, Work};
use crate::json::{self, enc, Json as _};
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::{icon, palette};
use crate::widgets::{self, rgb};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    Styled as _,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn severity_color(s: &str) -> u32 {
    match s {
        "critical" => widgets::red(),
        "high" => 0xDB6D28,
        "medium" | "moderate" | "warning" => widgets::yellow(),
        _ => widgets::gray(),
    }
}

/// Put a secret, sealed against the repository's (or environment's or
/// org's) public key first, as GitHub requires.
pub fn secret_form(
    title: &str,
    key_path: String,
    put_base: String,
    invalidate: String,
    name: &str,
) -> Act {
    FormSpec::new(title.to_string())
        .submit("Save secret")
        .field(
            Field::text("name", "Name")
                .value(name)
                .required()
                .hint("Letters, numbers and underscores."),
        )
        .field(Field::secret("value", "Value").required())
        .build_with(move |values| {
            let (key_path, put_base) = (key_path.clone(), put_base.clone());
            // Organization secrets also say which repositories get them.
            let org = put_base.starts_with("/orgs/");
            let (name, value) = (values.s("name").trim().to_uppercase(), values.s("value"));
            Ok(Req::custom(move |client| {
                let key = client.get(&key_path)?;
                let sealed = api::seal_secret(&key.s("key"), &value)?;
                let mut body = json!({ "encrypted_value": sealed, "key_id": key.s("key_id") });
                if org {
                    body["visibility"] = json!("private");
                }
                client.json("PUT", &format!("{put_base}/{name}"), Some(&body))
            })
            .ok("Secret saved")
            .inval(invalidate.clone())
            .act())
        })
        .act()
}

impl Hub {
    pub fn repo_security(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let view_key = format!("security.view:{repo}");
        let view = self.choice(&view_key, "dependabot");
        let chips = widgets::chips(vec![
            (
                "Dependabot".into(),
                view == "dependabot",
                Act::choose(&view_key, "dependabot"),
            ),
            (
                "Code scanning".into(),
                view == "code",
                Act::choose(&view_key, "code"),
            ),
            (
                "Secret scanning".into(),
                view == "secrets",
                Act::choose(&view_key, "secrets"),
            ),
            (
                "Advisories".into(),
                view == "advisories",
                Act::choose(&view_key, "advisories"),
            ),
            (
                "Policy & features".into(),
                view == "policy",
                Act::choose(&view_key, "policy"),
            ),
        ]);
        let state_key = format!("security.state:{repo}");
        let state = self.choice(&state_key, "open");
        let state_chips = widgets::chips(vec![
            (
                "Open".into(),
                state == "open",
                Act::choose(&state_key, "open"),
            ),
            (
                "Closed".into(),
                state != "open",
                Act::choose(&state_key, "closed"),
            ),
        ]);
        let repo_s = repo.to_string();
        let body = match view.as_str() {
            "code" => {
                let spec = ListSpec::new(
                    format!(
                        "/repos/{repo}/code-scanning/alerts?state={}",
                        if state == "open" { "open" } else { "dismissed" }
                    ),
                    move |a| {
                        let n = a.i("number");
                        let sev = a.s("rule.security_severity_level");
                        let sev = if sev.is_empty() {
                            a.s("rule.severity")
                        } else {
                            sev
                        };
                        let path = format!("/repos/{repo_s}/code-scanning/alerts/{n}");
                        let inval = format!("/repos/{repo_s}/code-scanning");
                        let mut row = Row::new(a.s("rule.description"))
                            .icon("shield", severity_color(&sev))
                            .tag(sev, severity_color(&a.s("rule.security_severity_level")))
                            .meta(format!(
                                "#{n}  ·  {}:{}  ·  {}  ·  {}",
                                a.s("most_recent_instance.location.path"),
                                a.i("most_recent_instance.location.start_line"),
                                a.s("tool.name"),
                                time::ago(&a.s("created_at"))
                            ))
                            .open(Act::Url(a.s("html_url")));
                        row = if a.s("state") == "open" {
                            row.action(
                                "Dismiss…",
                                FormSpec::new("Dismiss alert")
                                    .submit("Dismiss")
                                    .field(Field::choice(
                                        "dismissed_reason",
                                        "Reason",
                                        &[
                                            ("false positive", "False positive"),
                                            ("won't fix", "Won't fix"),
                                            ("used in tests", "Used in tests"),
                                        ],
                                    ))
                                    .field(Field::multiline("dismissed_comment", "Comment"))
                                    .rest("PATCH", path)
                                    .extra(json!({ "state": "dismissed" }))
                                    .ok("Alert dismissed")
                                    .inval(inval)
                                    .act(),
                            )
                        } else {
                            row.action(
                                "Reopen",
                                Req::rest("PATCH", path)
                                    .body(json!({ "state": "open" }))
                                    .ok("Alert reopened")
                                    .inval(inval)
                                    .act(),
                            )
                        };
                        row
                    },
                )
                .empty("No code scanning alerts (or code scanning is not set up).");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(state_chips)
                    .child(list)
                    .into_any_element()
            }
            "secrets" => {
                let spec = ListSpec::new(
                    format!(
                        "/repos/{repo}/secret-scanning/alerts?state={}",
                        if state == "open" { "open" } else { "resolved" }
                    ),
                    move |a| {
                        let n = a.i("number");
                        let path = format!("/repos/{repo_s}/secret-scanning/alerts/{n}");
                        let inval = format!("/repos/{repo_s}/secret-scanning");
                        let mut row = Row::new(a.s("secret_type_display_name"))
                            .icon("key", widgets::red())
                            .meta(format!(
                                "#{n}  ·  {}  ·  {}",
                                a.s("state"),
                                time::ago(&a.s("created_at"))
                            ))
                            .open(Act::Url(a.s("html_url")));
                        row = if a.s("state") == "open" {
                            row.action(
                                "Close as…",
                                FormSpec::new("Close alert")
                                    .submit("Close alert")
                                    .field(Field::choice(
                                        "resolution",
                                        "Resolution",
                                        &[
                                            ("revoked", "Revoked"),
                                            ("false_positive", "False positive"),
                                            ("wont_fix", "Won't fix"),
                                            ("used_in_tests", "Used in tests"),
                                        ],
                                    ))
                                    .field(Field::multiline("resolution_comment", "Comment"))
                                    .rest("PATCH", path)
                                    .extra(json!({ "state": "resolved" }))
                                    .ok("Alert closed")
                                    .inval(inval)
                                    .act(),
                            )
                        } else {
                            row.action(
                                "Reopen",
                                Req::rest("PATCH", path)
                                    .body(json!({ "state": "open" }))
                                    .ok("Alert reopened")
                                    .inval(inval)
                                    .act(),
                            )
                        };
                        row
                    },
                )
                .empty("No secret scanning alerts (or secret scanning is not enabled).");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(state_chips)
                    .child(list)
                    .into_any_element()
            }
            "advisories" => {
                let create = FormSpec::new("New draft security advisory")
                    .submit("Create draft advisory")
                    .width(640.0)
                    .field(Field::text("summary", "Title").required())
                    .field(Field::multiline("description", "Description").required())
                    .field(Field::choice("severity", "Severity", &[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("critical", "Critical")]))
                    .field(Field::text("cve_id", "CVE ID"))
                    .field(Field::choice("ecosystem", "Ecosystem", &[("npm", "npm"), ("pip", "pip"), ("rust", "Rust"), ("go", "Go"), ("maven", "Maven"), ("nuget", "NuGet"), ("rubygems", "RubyGems"), ("composer", "Composer"), ("actions", "Actions"), ("other", "Other")]))
                    .field(Field::text("package", "Affected package").required())
                    .field(Field::text("vulnerable", "Affected versions").hint("e.g. < 1.2.3"))
                    .field(Field::text("patched", "Patched versions").hint("e.g. 1.2.3"))
                    .rest("POST", format!("/repos/{repo}/security-advisories"))
                    .map(|_, v| {
                        let mut body = json!({
                            "summary": v.s("summary"),
                            "description": v.s("description"),
                            "severity": v.s("severity"),
                            "vulnerabilities": [{
                                "package": { "ecosystem": v.s("ecosystem"), "name": v.s("package") },
                                "vulnerable_version_range": v.s("vulnerable"),
                                "patched_versions": v.s("patched"),
                            }],
                        });
                        if !v.s("cve_id").is_empty() {
                            body["cve_id"] = json!(v.s("cve_id"));
                        }
                        body
                    })
                    .ok("Draft advisory created")
                    .inval(format!("/repos/{repo}/security-advisories"))
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/security-advisories"), move |a| {
                    let id = a.s("ghsa_id");
                    let path = format!("/repos/{repo_s}/security-advisories/{id}");
                    let inval = format!("/repos/{repo_s}/security-advisories");
                    let mut row = Row::new(a.s("summary"))
                        .icon("shield", severity_color(&a.s("severity")))
                        .tag(a.s("severity"), severity_color(&a.s("severity")))
                        .tag(a.s("state"), widgets::gray())
                        .meta(format!("{id}  ·  {}", time::ago(&a.s("created_at"))))
                        .open(Act::Url(a.s("html_url")));
                    if a.s("state") == "draft" || a.s("state") == "triage" {
                        row = row
                            .action(
                                "Publish",
                                Req::rest("PATCH", path.clone())
                                    .body(json!({ "state": "published" }))
                                    .ok("Advisory published")
                                    .inval(inval.clone())
                                    .act(),
                            )
                            .action(
                                "Request CVE",
                                Req::rest("POST", format!("{path}/cve"))
                                    .ok("CVE requested")
                                    .inval(inval.clone())
                                    .act(),
                            )
                            .danger(
                                "Close",
                                Req::rest("PATCH", path)
                                    .body(json!({ "state": "closed" }))
                                    .ok("Advisory closed")
                                    .inval(inval)
                                    .act(),
                            );
                    }
                    row
                })
                .empty("No security advisories.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn(
                                "new-advisory",
                                "New draft advisory",
                                create,
                            )),
                    )
                    .child(list)
                    .into_any_element()
            }
            "policy" => self.security_policy(repo, cx),
            _ => {
                let spec = ListSpec::new(
                    format!(
                        "/repos/{repo}/dependabot/alerts?state={}",
                        if state == "open" {
                            "open"
                        } else {
                            "dismissed,fixed,auto_dismissed"
                        }
                    ),
                    move |a| {
                        let n = a.i("number");
                        let sev = a.s("security_advisory.severity");
                        let path = format!("/repos/{repo_s}/dependabot/alerts/{n}");
                        let inval = format!("/repos/{repo_s}/dependabot");
                        let mut row = Row::new(a.s("security_advisory.summary"))
                            .icon("alert", severity_color(&sev))
                            .tag(sev.clone(), severity_color(&sev))
                            .meta(format!(
                                "#{n}  ·  {} ({})  ·  {}  ·  {}",
                                a.s("dependency.package.name"),
                                a.s("dependency.package.ecosystem"),
                                a.s("dependency.manifest_path"),
                                time::ago(&a.s("created_at"))
                            ))
                            .open(Act::Url(a.s("html_url")));
                        if a.has("security_vulnerability.first_patched_version.identifier") {
                            row = row.body(format!(
                                "Patched in {}",
                                a.s("security_vulnerability.first_patched_version.identifier")
                            ));
                        }
                        row = if a.s("state") == "open" {
                            row.action(
                                "Dismiss…",
                                FormSpec::new("Dismiss alert")
                                    .submit("Dismiss alert")
                                    .field(Field::choice(
                                        "dismissed_reason",
                                        "Reason",
                                        &[
                                            ("fix_started", "A fix has already been started"),
                                            ("no_bandwidth", "No bandwidth to fix this"),
                                            ("tolerable_risk", "Risk is tolerable"),
                                            ("inaccurate", "Alert is inaccurate"),
                                            ("not_used", "Vulnerable code is not used"),
                                        ],
                                    ))
                                    .field(Field::multiline("dismissed_comment", "Comment"))
                                    .rest("PATCH", path)
                                    .extra(json!({ "state": "dismissed" }))
                                    .ok("Alert dismissed")
                                    .inval(inval)
                                    .act(),
                            )
                        } else {
                            row.action(
                                "Reopen",
                                Req::rest("PATCH", path)
                                    .body(json!({ "state": "open" }))
                                    .ok("Alert reopened")
                                    .inval(inval)
                                    .act(),
                            )
                        };
                        row
                    },
                )
                .empty("No Dependabot alerts (or Dependabot alerts are disabled).");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(state_chips)
                    .child(list)
                    .into_any_element()
            }
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    /// The security policy and the switches for the security features.
    fn security_policy(&mut self, repo: &str, cx: &mut Context<Self>) -> AnyElement {
        let probe = |hub: &mut Hub, path: String, cx: &mut Context<Hub>| -> Option<bool> {
            let key = format!("{path}#probe");
            hub.fetch_with(
                key,
                Work::Custom(Arc::new(move |client| {
                    let reply = client.send("GET", &path, None, None)?;
                    Ok(Value::Bool(match reply.status {
                        200..=299 => serde_json::from_str::<Value>(&reply.body)
                            .map(|v| v.b("enabled") || v.is_null() || !v.has("enabled"))
                            .unwrap_or(true),
                        _ => false,
                    }))
                })),
                cx,
            )
            .ready()
            .map(|v| v.b(""))
        };
        let alerts = probe(self, format!("/repos/{repo}/vulnerability-alerts"), cx);
        let fixes = probe(self, format!("/repos/{repo}/automated-security-fixes"), cx);
        let reporting = probe(
            self,
            format!("/repos/{repo}/private-vulnerability-reporting"),
            cx,
        );
        let toggle = |id: &'static str, label: &str, state: Option<bool>, path: String| {
            let on = state.unwrap_or(false);
            widgets::row()
                .p_3()
                .border_b_1()
                .border_color(rgb(palette().divider))
                .child(div().flex_1().child(label.to_string()))
                .child(widgets::dim(match state {
                    None => "…",
                    Some(true) => "Enabled",
                    Some(false) => "Disabled",
                }))
                .child(widgets::btn(
                    id,
                    if on { "Disable" } else { "Enable" },
                    Req::rest(if on { "DELETE" } else { "PUT" }, path.clone())
                        .ok("Saved")
                        .inval(path)
                        .act(),
                ))
        };
        let policy = self.fetch_text(
            &format!("/repos/{repo}/contents/SECURITY.md"),
            "application/vnd.github.raw",
            cx,
        );
        let policy_el = match policy {
            Load::Ready(text) => {
                let md = self.markdown("security-md", &text.s(""), cx);
                widgets::card().p_6().child(md).into_any_element()
            }
            Load::Loading => widgets::loading(),
            Load::Failed(_) => widgets::empty("No security policy. Add a SECURITY.md to tell people how to report vulnerabilities."),
        };
        widgets::col()
            .gap_3()
            .child(
                widgets::card()
                    .child(toggle(
                        "t-alerts",
                        "Dependabot alerts",
                        alerts,
                        format!("/repos/{repo}/vulnerability-alerts"),
                    ))
                    .child(toggle(
                        "t-fixes",
                        "Dependabot security updates",
                        fixes,
                        format!("/repos/{repo}/automated-security-fixes"),
                    ))
                    .child(toggle(
                        "t-report",
                        "Private vulnerability reporting",
                        reporting,
                        format!("/repos/{repo}/private-vulnerability-reporting"),
                    )),
            )
            .child(widgets::h2("Security policy"))
            .child(policy_el)
            .into_any_element()
    }

    pub fn repo_insights(
        &mut self,
        repo: &str,
        info: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_key = format!("insights.view:{repo}");
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
        let view = self.choice(
            &view_key,
            if gitlab || bitbucket {
                "contributors"
            } else {
                "activity"
            },
        );
        let gitlab_names = [
            ("contributors", "Contributors"),
            ("forks", "Forks"),
            ("stargazers", "Stars"),
        ];
        let bitbucket_names = [
            ("contributors", "Recent contributors"),
            ("forks", "Forks"),
            ("watchers", "Watchers"),
        ];
        let github_names = [
            ("activity", "Commit activity"),
            ("contributors", "Contributors"),
            ("traffic", "Traffic"),
            ("community", "Community"),
            ("dependencies", "Dependencies"),
            ("forks", "Forks"),
            ("stargazers", "Stargazers"),
            ("watchers", "Watchers"),
        ];
        let names: &[(&str, &str)] = if bitbucket {
            &bitbucket_names
        } else if gitlab {
            &gitlab_names
        } else {
            &github_names
        };
        let chips = widgets::chips(
            names
                .iter()
                .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
                .collect(),
        );
        let body = match view.as_str() {
            "contributors" => {
                let spec = ListSpec::new(format!("/repos/{repo}/contributors"), |c| {
                    user_row(c)
                        .meta(format!("{} commits", c.i("contributions")))
                        .body(String::new())
                })
                .people();
                self.list(&spec, cx)
            }
            "traffic" => {
                let views = self.fetch(&format!("/repos/{repo}/traffic/views"), cx);
                let clones = self.fetch(&format!("/repos/{repo}/traffic/clones"), cx);
                let mut col = widgets::col().gap_4();
                let chart = |v: &Value, key: &str, title: &str| {
                    let bars = v
                        .list(key)
                        .iter()
                        .map(|d| (time::date(&d.s("timestamp")), d.i("count")))
                        .collect();
                    widgets::bar_chart(
                        key,
                        &format!(
                            "{title} — {} total, {} unique",
                            v.i("count"),
                            v.i("uniques")
                        ),
                        bars,
                        120.0,
                    )
                };
                col = match views {
                    Load::Ready(v) => {
                        col.child(widgets::card().p_4().child(chart(&v, "views", "Views")))
                    }
                    other => col.child(widgets::placeholder(&other)),
                };
                if let Some(v) = clones.ready() {
                    col = col.child(widgets::card().p_4().child(chart(v, "clones", "Clones")));
                }
                let paths = ListSpec::new(format!("/repos/{repo}/traffic/popular/paths"), |p| {
                    Row::new(p.s("title")).meta(p.s("path")).right(format!(
                        "{} views · {} unique",
                        p.i("count"),
                        p.i("uniques")
                    ))
                })
                .unpaged()
                .empty("No popular content yet.");
                let referrers =
                    ListSpec::new(format!("/repos/{repo}/traffic/popular/referrers"), |r| {
                        Row::new(r.s("referrer")).right(format!(
                            "{} views · {} unique",
                            r.i("count"),
                            r.i("uniques")
                        ))
                    })
                    .unpaged()
                    .empty("No referring sites yet.");
                let paths = self.list(&paths, cx);
                let referrers = self.list(&referrers, cx);
                col.child(widgets::h2("Popular content"))
                    .child(paths)
                    .child(widgets::h2("Referring sites"))
                    .child(referrers)
                    .into_any_element()
            }
            "community" => {
                let profile = self.fetch(&format!("/repos/{repo}/community/profile"), cx);
                match profile {
                    Load::Ready(v) => {
                        let items = [
                            ("Description", v.has("description")),
                            ("README", v.has("files.readme")),
                            ("Code of conduct", v.has("files.code_of_conduct")),
                            ("Contributing guide", v.has("files.contributing")),
                            ("License", v.has("files.license")),
                            ("Issue templates", v.has("files.issue_template")),
                            (
                                "Pull request template",
                                v.has("files.pull_request_template"),
                            ),
                        ];
                        let mut card = widgets::card().child(widgets::card_header().child(
                            widgets::h3(format!(
                                "Community standards — {}% complete",
                                v.i("health_percentage")
                            )),
                        ));
                        for (label, done) in items {
                            card = card.child(
                                widgets::row()
                                    .px_4()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgb(palette().divider))
                                    .child(icon(
                                        if done { "check-circle" } else { "circle" },
                                        16.0,
                                        if done {
                                            widgets::green()
                                        } else {
                                            widgets::gray()
                                        },
                                    ))
                                    .child(label),
                            );
                        }
                        card.into_any_element()
                    }
                    other => widgets::placeholder(&other),
                }
            }
            "dependencies" => {
                let sbom = self.fetch(&format!("/repos/{repo}/dependency-graph/sbom"), cx);
                match sbom {
                    Load::Ready(v) => {
                        let packages = v.list("sbom.packages");
                        let mut card = widgets::card().child(
                            widgets::card_header()
                                .child(widgets::h3(format!("{} packages", packages.len()))),
                        );
                        for (i, pkg) in packages.iter().take(400).enumerate() {
                            card = card.child(
                                widgets::row()
                                    .id(("dep", i))
                                    .px_4()
                                    .py_1()
                                    .border_b_1()
                                    .border_color(rgb(palette().divider))
                                    .child(div().flex_1().child(pkg.s("name")))
                                    .child(widgets::dim(pkg.s("versionInfo")))
                                    .child(widgets::faint(pkg.s("licenseConcluded"))),
                            );
                        }
                        let export =
                            Act::Copy(serde_json::to_string_pretty(&*v).unwrap_or_default());
                        widgets::col()
                            .gap_2()
                            .child(widgets::row().child(widgets::spacer()).child(widgets::btn(
                                "export-sbom",
                                "Copy SBOM (SPDX JSON)",
                                export,
                            )))
                            .child(card)
                            .into_any_element()
                    }
                    other => widgets::placeholder(&other),
                }
            }
            "forks" => {
                let spec = ListSpec::new(format!("/repos/{repo}/forks?sort=stargazers"), repo_row)
                    .empty("No forks.");
                self.list(&spec, cx)
            }
            "stargazers" => {
                let spec = ListSpec::new(format!("/repos/{repo}/stargazers"), user_row)
                    .empty("No stargazers yet.")
                    .people();
                self.list(&spec, cx)
            }
            "watchers" => {
                let spec = ListSpec::new(format!("/repos/{repo}/subscribers"), user_row)
                    .empty("No watchers.")
                    .people();
                self.list(&spec, cx)
            }
            _ => {
                // The stats endpoints answer 202 while GitHub computes them.
                let path = format!("/repos/{repo}/stats/commit_activity");
                let load = self.fetch_with(
                    path.clone(),
                    Work::Custom(Arc::new(move |client| {
                        let reply = client.send("GET", &path, None, None)?;
                        if reply.status == 202 {
                            return Ok(json!({ "computing": true }));
                        }
                        Ok(serde_json::from_str(&reply.body).unwrap_or(Value::Null))
                    })),
                    cx,
                );
                match load {
                    Load::Ready(v) if v.b("computing") => widgets::col()
                        .gap_2()
                        .child(widgets::empty(
                            "GitHub is still computing these statistics.",
                        ))
                        .child(widgets::btn(
                            "retry-stats",
                            "Try again",
                            Act::run({
                                let repo = repo.to_string();
                                move |hub, _, cx| {
                                    hub.invalidate(&format!("/repos/{repo}/stats"));
                                    cx.notify();
                                }
                            }),
                        ))
                        .into_any_element(),
                    Load::Ready(v) => {
                        let weeks = v.list("");
                        let bars = weeks
                            .iter()
                            .map(|w| {
                                let secs = w.i("week");
                                (format!("Week of {}", date_from_secs(secs)), w.i("total"))
                            })
                            .collect();
                        widgets::col()
                            .gap_3()
                            .child(widgets::card().p_4().child(widgets::bar_chart(
                                "commits",
                                "Commits per week, past year",
                                bars,
                                140.0,
                            )))
                            .child(
                                widgets::row()
                                    .gap_6()
                                    .child(widgets::dim(format!(
                                        "{} open issues and pull requests",
                                        info.i("open_issues_count")
                                    )))
                                    .child(widgets::dim(format!("{} KB on disk", info.i("size"))))
                                    .child(widgets::dim(format!(
                                        "Created {}",
                                        time::date(&info.s("created_at"))
                                    ))),
                            )
                            .into_any_element()
                    }
                    other => widgets::placeholder(&other),
                }
            }
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    pub fn repo_settings(
        &mut self,
        repo: &str,
        info: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_key = format!("settings.view:{repo}");
        let view = self.choice(&view_key, "general");
        let names = [
            ("general", "General"),
            ("access", "Collaborators"),
            ("teams", "Teams"),
            ("invites", "Invitations"),
            ("hooks", "Webhooks"),
            ("keys", "Deploy keys"),
            ("secrets", "Secrets"),
            ("variables", "Variables"),
            ("environments", "Environments"),
            ("pages", "Pages"),
            ("rules", "Rulesets"),
            ("autolinks", "Autolinks"),
            ("actions", "Actions permissions"),
        ];
        let chips = widgets::chips(
            names
                .iter()
                .map(|(v, l)| (l.to_string(), view == *v, Act::choose(&view_key, *v)))
                .collect(),
        );
        let r = repo.to_string();
        let body = match view.as_str() {
            "access" => {
                let invite = FormSpec::new("Add a collaborator")
                    .submit("Send invitation")
                    .field(Field::text("username", "GitHub login").required())
                    .field(Field::choice(
                        "permission",
                        "Role",
                        &[
                            ("pull", "Read"),
                            ("triage", "Triage"),
                            ("push", "Write"),
                            ("maintain", "Maintain"),
                            ("admin", "Admin"),
                        ],
                    ))
                    .build_with({
                        let r = r.clone();
                        move |v| {
                            Ok(Req::rest(
                                "PUT",
                                format!("/repos/{r}/collaborators/{}", enc(v.s("username").trim())),
                            )
                            .body(json!({ "permission": v.s("permission") }))
                            .ok("Invitation sent")
                            .inval(format!("/repos/{r}/invitations"))
                            .inval(format!("/repos/{r}/collaborators"))
                            .act())
                        }
                    })
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/collaborators"), move |u| {
                    let login = u.s("login");
                    let path = format!("/repos/{r}/collaborators/{login}");
                    let inval = format!("/repos/{r}/collaborators");
                    user_row(u)
                        .meta(u.s("role_name"))
                        .action(
                            "Change role…",
                            FormSpec::new(format!("Role for {login}"))
                                .field(Field::choice(
                                    "permission",
                                    "Role",
                                    &[
                                        ("pull", "Read"),
                                        ("triage", "Triage"),
                                        ("push", "Write"),
                                        ("maintain", "Maintain"),
                                        ("admin", "Admin"),
                                    ],
                                ))
                                .rest("PUT", path.clone())
                                .ok("Role updated")
                                .inval(inval.clone())
                                .act(),
                        )
                        .danger(
                            "Remove",
                            Req::rest("DELETE", path)
                                .ok("Collaborator removed")
                                .inval(inval)
                                .act()
                                .confirm(
                                    "Remove collaborator?",
                                    format!("{login} will lose access to this repository."),
                                    "Remove",
                                ),
                        )
                })
                .empty("No collaborators.")
                .people();
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-collab", "Add people", invite)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "teams" => {
                let owner = repo.split('/').next().unwrap_or("").to_string();
                let add = FormSpec::new("Add a team")
                    .field(Field::text("slug", "Team slug").required())
                    .field(Field::choice(
                        "permission",
                        "Role",
                        &[
                            ("pull", "Read"),
                            ("triage", "Triage"),
                            ("push", "Write"),
                            ("maintain", "Maintain"),
                            ("admin", "Admin"),
                        ],
                    ))
                    .build_with({
                        let (r, owner) = (r.clone(), owner.clone());
                        move |v| {
                            Ok(Req::rest(
                                "PUT",
                                format!("/orgs/{owner}/teams/{}/repos/{r}", v.s("slug")),
                            )
                            .body(json!({ "permission": v.s("permission") }))
                            .ok("Team added")
                            .inval(format!("/repos/{r}/teams"))
                            .act())
                        }
                    })
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/teams"), move |t| {
                    let slug = t.s("slug");
                    Row::new(t.s("name"))
                        .icon("people", widgets::gray())
                        .meta(format!("{}  ·  {}", t.s("permission"), t.s("description")))
                        .open(Act::Go(Route::Team {
                            org: owner.clone(),
                            slug: slug.clone(),
                        }))
                        .danger(
                            "Remove",
                            Req::rest("DELETE", format!("/orgs/{owner}/teams/{slug}/repos/{r}"))
                                .ok("Team removed")
                                .inval(format!("/repos/{r}/teams"))
                                .act(),
                        )
                        .inline()
                })
                .empty("No teams have access (teams exist only for organization repositories).");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-team", "Add team", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "invites" => {
                let spec = ListSpec::new(format!("/repos/{repo}/invitations"), move |i| {
                    let id = i.i("id");
                    Row::new(i.s("invitee.login"))
                        .avatar(i.s("invitee.avatar_url"))
                        .meta(format!(
                            "{}  ·  invited {} by {}",
                            i.s("permissions"),
                            time::ago(&i.s("created_at")),
                            i.s("inviter.login")
                        ))
                        .action(
                            "Change role…",
                            FormSpec::new("Invitation role")
                                .field(Field::choice(
                                    "permissions",
                                    "Role",
                                    &[
                                        ("read", "Read"),
                                        ("triage", "Triage"),
                                        ("write", "Write"),
                                        ("maintain", "Maintain"),
                                        ("admin", "Admin"),
                                    ],
                                ))
                                .rest("PATCH", format!("/repos/{r}/invitations/{id}"))
                                .ok("Invitation updated")
                                .inval(format!("/repos/{r}/invitations"))
                                .act(),
                        )
                        .danger(
                            "Cancel invitation",
                            Req::rest("DELETE", format!("/repos/{r}/invitations/{id}"))
                                .ok("Invitation cancelled")
                                .inval(format!("/repos/{r}/invitations"))
                                .act(),
                        )
                })
                .empty("No pending invitations.");
                self.list(&spec, cx)
            }
            "hooks" => self.webhooks(&format!("/repos/{repo}/hooks"), cx),
            "keys" => {
                let add = FormSpec::new("Add deploy key")
                    .submit("Add key")
                    .field(Field::text("title", "Title").required())
                    .field(Field::multiline("key", "Public key").required())
                    .field(Field::bool(
                        "read_only",
                        "Read-only (no write access)",
                        true,
                    ))
                    .rest("POST", format!("/repos/{repo}/keys"))
                    .ok("Deploy key added")
                    .inval(format!("/repos/{repo}/keys"))
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/keys"), move |k| {
                    Row::new(k.s("title"))
                        .icon("key", widgets::gray())
                        .meta(format!(
                            "{}  ·  added {}  ·  {}",
                            if k.b("read_only") {
                                "read-only"
                            } else {
                                "read/write"
                            },
                            time::ago(&k.s("created_at")),
                            json::clip(&k.s("key"), 40)
                        ))
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("/repos/{r}/keys/{}", k.i("id")))
                                .ok("Deploy key deleted")
                                .inval(format!("/repos/{r}/keys"))
                                .act()
                                .confirm(
                                    "Delete deploy key?",
                                    "Anything using it will lose access.",
                                    "Delete",
                                ),
                        )
                })
                .empty("No deploy keys.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-key", "Add deploy key", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "secrets" => {
                let scope_key = format!("secrets.scope:{repo}");
                let scope = self.choice(&scope_key, "actions");
                let base = format!("/repos/{repo}/{scope}/secrets");
                let add = secret_form(
                    "New secret",
                    format!("{base}/public-key"),
                    base.clone(),
                    base.clone(),
                    "",
                );
                let (b, k) = (base.clone(), format!("{base}/public-key"));
                let spec = ListSpec::new(base.clone(), move |s| {
                    let name = s.s("name");
                    Row::new(name.clone())
                        .icon("lock", widgets::gray())
                        .meta(format!("Updated {}", time::ago(&s.s("updated_at"))))
                        .action(
                            "Update…",
                            secret_form("Update secret", k.clone(), b.clone(), b.clone(), &name),
                        )
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("{b}/{name}"))
                                .ok("Secret deleted")
                                .inval(b.clone())
                                .act()
                                .confirm(
                                    "Delete secret?",
                                    format!("Workflows using {name} will no longer receive it."),
                                    "Delete",
                                ),
                        )
                })
                .items("secrets")
                .empty("No secrets.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::chips(vec![
                                ("Actions".into(), scope == "actions", Act::choose(&scope_key, "actions")),
                                ("Dependabot".into(), scope == "dependabot", Act::choose(&scope_key, "dependabot")),
                                ("Codespaces".into(), scope == "codespaces", Act::choose(&scope_key, "codespaces")),
                            ]))
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-secret", "New secret", add)),
                    )
                    .child(widgets::faint("Values are encrypted on this machine with the repository's public key before they are sent."))
                    .child(list)
                    .into_any_element()
            }
            "variables" => self.variables(&format!("/repos/{repo}/actions/variables"), cx),
            "environments" => {
                let add = FormSpec::new("New environment")
                    .submit("Configure environment")
                    .field(Field::text("name", "Name").required())
                    .field(Field::number("wait_timer", "Wait timer (minutes)"))
                    .field(
                        Field::list("reviewers", "Required reviewers")
                            .hint("User IDs, comma separated (optional)."),
                    )
                    .build_with({
                        let r = r.clone();
                        move |v| {
                            let mut body = json!({});
                            if let Ok(n) = v.s("wait_timer").trim().parse::<i64>() {
                                body["wait_timer"] = json!(n);
                            }
                            let reviewers: Vec<Value> = v
                                .items("reviewers")
                                .iter()
                                .filter_map(|id| id.parse::<i64>().ok())
                                .map(|id| json!({ "type": "User", "id": id }))
                                .collect();
                            if !reviewers.is_empty() {
                                body["reviewers"] = Value::Array(reviewers);
                            }
                            Ok(Req::rest(
                                "PUT",
                                format!("/repos/{r}/environments/{}", enc(v.s("name").trim())),
                            )
                            .body(body)
                            .ok("Environment saved")
                            .inval(format!("/repos/{r}/environments"))
                            .act())
                        }
                    })
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/environments"), move |e| {
                    let name = e.s("name");
                    let rules: Vec<String> = e
                        .list("protection_rules")
                        .iter()
                        .map(|p| p.s("type"))
                        .collect();
                    let base = format!("/repos/{r}/environments/{}", enc(&name));
                    Row::new(name.clone())
                        .icon("deploy", widgets::gray())
                        .meta(if rules.is_empty() {
                            "No protection rules".to_string()
                        } else {
                            rules.join(", ")
                        })
                        .action(
                            "Secrets…",
                            secret_form(
                                &format!("New secret for {name}"),
                                format!("{base}/secrets/public-key"),
                                format!("{base}/secrets"),
                                base.clone(),
                                "",
                            ),
                        )
                        .action(
                            "Add variable…",
                            FormSpec::new(format!("New variable for {name}"))
                                .field(Field::text("name", "Name").required())
                                .field(Field::multiline("value", "Value").required())
                                .rest("POST", format!("{base}/variables"))
                                .ok("Variable created")
                                .inval(base.clone())
                                .act(),
                        )
                        .danger(
                            "Delete",
                            Req::rest("DELETE", base)
                                .ok("Environment deleted")
                                .inval(format!("/repos/{r}/environments"))
                                .act()
                                .confirm(
                                    "Delete environment?",
                                    format!(
                                        "{name}, its secrets and protection rules will be removed."
                                    ),
                                    "Delete",
                                ),
                        )
                })
                .items("environments")
                .empty("No environments.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-env", "New environment", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "pages" => self.pages_settings(repo, info, cx),
            "rules" => {
                let spec = ListSpec::new(format!("/repos/{repo}/rulesets"), move |s| {
                    Row::new(s.s("name"))
                        .icon("shield", widgets::gray())
                        .meta(format!("{}  ·  {}  ·  {}", s.s("target"), s.s("enforcement"), s.s("source_type")))
                        .open(Act::Url(format!("{}/{r}/rules/{}", api::WEB, s.i("id"))))
                        .action(
                            if s.s("enforcement") == "active" { "Disable" } else { "Enable" },
                            Req::rest("PUT", format!("/repos/{r}/rulesets/{}", s.i("id")))
                                .body(json!({ "enforcement": if s.s("enforcement") == "active" { "disabled" } else { "active" } }))
                                .ok("Ruleset updated")
                                .inval(format!("/repos/{r}/rulesets"))
                                .act(),
                        )
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("/repos/{r}/rulesets/{}", s.i("id"))).ok("Ruleset deleted").inval(format!("/repos/{r}/rulesets")).act().confirm(
                                "Delete ruleset?",
                                "Its rules stop applying immediately.",
                                "Delete",
                            ),
                        )
                })
                .empty("No rulesets. Branch protection rules are managed from the Branches tab.");
                self.list(&spec, cx)
            }
            "autolinks" => {
                let add = FormSpec::new("Add autolink reference")
                    .field(
                        Field::text("key_prefix", "Reference prefix")
                            .required()
                            .hint("e.g. TICKET-"),
                    )
                    .field(
                        Field::text("url_template", "Target URL")
                            .required()
                            .hint("Must contain <num>, e.g. https://example.com/ticket?q=<num>"),
                    )
                    .field(Field::bool(
                        "is_alphanumeric",
                        "Alphanumeric references",
                        true,
                    ))
                    .rest("POST", format!("/repos/{repo}/autolinks"))
                    .ok("Autolink added")
                    .inval(format!("/repos/{repo}/autolinks"))
                    .act();
                let spec = ListSpec::new(format!("/repos/{repo}/autolinks"), move |a| {
                    Row::new(a.s("key_prefix"))
                        .icon("link", widgets::gray())
                        .meta(a.s("url_template"))
                        .danger(
                            "Delete",
                            Req::rest("DELETE", format!("/repos/{r}/autolinks/{}", a.i("id")))
                                .ok("Autolink deleted")
                                .inval(format!("/repos/{r}/autolinks"))
                                .act(),
                        )
                })
                .unpaged()
                .empty("No autolink references.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-autolink", "Add autolink", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "actions" => {
                let perms = self.fetch(&format!("/repos/{repo}/actions/permissions"), cx);
                let workflow =
                    self.fetch(&format!("/repos/{repo}/actions/permissions/workflow"), cx);
                let mut card = widgets::card().p_4().gap_2();
                if let Some(v) = perms.ready() {
                    card = card
                        .child(
                            widgets::row()
                                .child(div().flex_1().child("Actions enabled"))
                                .child(widgets::dim(v.s("enabled"))),
                        )
                        .child(
                            widgets::row()
                                .child(div().flex_1().child("Allowed actions"))
                                .child(widgets::dim(v.s("allowed_actions"))),
                        );
                }
                if let Some(v) = workflow.ready() {
                    card = card
                        .child(
                            widgets::row()
                                .child(div().flex_1().child("Default GITHUB_TOKEN permissions"))
                                .child(widgets::dim(v.s("default_workflow_permissions"))),
                        )
                        .child(
                            widgets::row()
                                .child(div().flex_1().child("Actions can approve pull requests"))
                                .child(widgets::dim(v.s("can_approve_pull_request_reviews"))),
                        );
                }
                let edit = FormSpec::new("Actions permissions")
                    .field(Field::bool(
                        "enabled",
                        "Allow GitHub Actions in this repository",
                        perms.ready().map(|v| v.b("enabled")).unwrap_or(true),
                    ))
                    .field(
                        Field::choice(
                            "allowed_actions",
                            "Allowed actions",
                            &[
                                ("all", "All"),
                                ("local_only", "Local only"),
                                ("selected", "Selected"),
                            ],
                        )
                        .value(
                            perms
                                .ready()
                                .map(|v| v.s("allowed_actions"))
                                .unwrap_or_else(|| "all".into()),
                        ),
                    )
                    .rest("PUT", format!("/repos/{repo}/actions/permissions"))
                    .ok("Saved")
                    .inval(format!("/repos/{repo}/actions/permissions"))
                    .act();
                let token = FormSpec::new("Workflow permissions")
                    .field(
                        Field::choice(
                            "default_workflow_permissions",
                            "GITHUB_TOKEN default",
                            &[
                                ("read", "Read repository contents"),
                                ("write", "Read and write"),
                            ],
                        )
                        .value(
                            workflow
                                .ready()
                                .map(|v| v.s("default_workflow_permissions"))
                                .unwrap_or_else(|| "read".into()),
                        ),
                    )
                    .field(Field::bool(
                        "can_approve_pull_request_reviews",
                        "Allow Actions to create and approve pull requests",
                        workflow
                            .ready()
                            .map(|v| v.b("can_approve_pull_request_reviews"))
                            .unwrap_or(false),
                    ))
                    .rest("PUT", format!("/repos/{repo}/actions/permissions/workflow"))
                    .ok("Saved")
                    .inval(format!("/repos/{repo}/actions/permissions"))
                    .act();
                widgets::col()
                    .gap_3()
                    .child(card)
                    .child(
                        widgets::row()
                            .child(widgets::btn(
                                "edit-actions",
                                "Edit Actions permissions",
                                edit,
                            ))
                            .child(widgets::btn(
                                "edit-token",
                                "Edit workflow permissions",
                                token,
                            )),
                    )
                    .into_any_element()
            }
            _ => self.general_settings(repo, info, cx),
        };
        widgets::col()
            .gap_3()
            .child(chips)
            .child(body)
            .into_any_element()
    }

    /// Webhooks for a repository or an organization (`base` = .../hooks).
    pub fn webhooks(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let hook_fields = |v: &Value| {
            vec![
                Field::text("config.url", "Payload URL")
                    .value(v.s("config.url"))
                    .required(),
                Field::choice(
                    "config.content_type",
                    "Content type",
                    &[
                        ("json", "application/json"),
                        ("form", "application/x-www-form-urlencoded"),
                    ],
                )
                .value(if v.s("config.content_type").is_empty() {
                    "json".to_string()
                } else {
                    v.s("config.content_type")
                }),
                Field::secret("config.secret", "Secret"),
                Field::list("events", "Events")
                    .value(if v.list("events").is_empty() {
                        "push".to_string()
                    } else {
                        v.list("events")
                            .iter()
                            .filter_map(|e| e.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .hint("push, pull_request, issues, release, workflow_run, * …"),
                Field::bool("active", "Active", v.b("active") || v.is_null()),
            ]
        };
        let add = FormSpec::new("Add webhook")
            .submit("Add webhook")
            .fields(hook_fields(&Value::Null))
            .rest("POST", base.to_string())
            .extra(json!({ "name": "web" }))
            .ok("Webhook added")
            .inval(base.to_string())
            .act();
        let b = base.to_string();
        let spec = ListSpec::new(base.to_string(), move |h| {
            let id = h.i("id");
            let path = format!("{b}/{id}");
            Row::new(h.s("config.url"))
                .icon(
                    "webhook",
                    if h.b("active") {
                        widgets::green()
                    } else {
                        widgets::gray()
                    },
                )
                .meta(format!(
                    "{}  ·  last response {} {}",
                    h.list("events")
                        .iter()
                        .filter_map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    h.s("last_response.code"),
                    h.s("last_response.status")
                ))
                .action(
                    "Edit…",
                    FormSpec::new("Edit webhook")
                        .fields(hook_fields(h))
                        .rest("PATCH", path.clone())
                        .ok("Webhook saved")
                        .inval(b.clone())
                        .act(),
                )
                .action(
                    "Ping",
                    Req::rest("POST", format!("{path}/pings"))
                        .ok("Ping sent")
                        .inval(b.clone())
                        .act(),
                )
                .action(
                    "Test push",
                    Req::rest("POST", format!("{path}/tests"))
                        .ok("Test push sent")
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", path)
                        .ok("Webhook deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete webhook?",
                            "Deliveries to this URL will stop.",
                            "Delete",
                        ),
                )
        })
        .empty("No webhooks.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("add-hook", "Add webhook", add)),
            )
            .child(list)
            .into_any_element()
    }

    /// Actions variables for a repository or organization.
    pub fn variables(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let add = FormSpec::new("New variable")
            .field(Field::text("name", "Name").required())
            .field(Field::multiline("value", "Value").required())
            .rest("POST", base.to_string())
            .map({
                let org = base.starts_with("/orgs/");
                move |mut body, _| {
                    if org {
                        body["visibility"] = json!("private");
                    }
                    body
                }
            })
            .ok("Variable created")
            .inval(base.to_string())
            .act();
        let b = base.to_string();
        let spec = ListSpec::new(base.to_string(), move |v| {
            let name = v.s("name");
            let path = format!("{b}/{name}");
            Row::new(name.clone())
                .icon("code", widgets::gray())
                .meta(json::clip(&v.s("value"), 80))
                .action(
                    "Edit…",
                    FormSpec::new(format!("Edit {name}"))
                        .field(
                            Field::multiline("value", "Value")
                                .value(v.s("value"))
                                .required(),
                        )
                        .rest("PATCH", path.clone())
                        .ok("Variable saved")
                        .inval(b.clone())
                        .act(),
                )
                .danger(
                    "Delete",
                    Req::rest("DELETE", path)
                        .ok("Variable deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete variable?",
                            format!("{name} will be removed."),
                            "Delete",
                        ),
                )
        })
        .items("variables")
        .empty("No variables.");
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("add-var", "New variable", add)),
            )
            .child(list)
            .into_any_element()
    }

    fn pages_settings(&mut self, repo: &str, info: &Value, cx: &mut Context<Self>) -> AnyElement {
        let path = format!("/repos/{repo}/pages");
        let load = self.fetch(&path, cx);
        let enable = FormSpec::new("Publish with GitHub Pages")
            .submit("Enable Pages")
            .field(Field::choice(
                "build_type",
                "Source",
                &[
                    ("legacy", "Deploy from a branch"),
                    ("workflow", "GitHub Actions"),
                ],
            ))
            .field(Field::text("source.branch", "Branch").value(info.s("default_branch")))
            .field(Field::choice(
                "source.path",
                "Folder",
                &[("/", "/ (root)"), ("/docs", "/docs")],
            ))
            .rest("POST", path.clone())
            .ok("Pages enabled")
            .inval(path.clone())
            .act();
        match load {
            Load::Ready(v) => {
                let edit = FormSpec::new("Pages settings")
                    .field(
                        Field::text("cname", "Custom domain")
                            .value(v.s("cname"))
                            .keep_empty(),
                    )
                    .field(Field::bool(
                        "https_enforced",
                        "Enforce HTTPS",
                        v.b("https_enforced"),
                    ))
                    .field(
                        Field::choice(
                            "build_type",
                            "Source",
                            &[
                                ("legacy", "Deploy from a branch"),
                                ("workflow", "GitHub Actions"),
                            ],
                        )
                        .value(v.s("build_type")),
                    )
                    .field(Field::text("source.branch", "Branch").value(v.s("source.branch")))
                    .field(
                        Field::choice(
                            "source.path",
                            "Folder",
                            &[("/", "/ (root)"), ("/docs", "/docs")],
                        )
                        .value(v.s("source.path")),
                    )
                    .rest("PUT", path.clone())
                    .ok("Pages settings saved")
                    .inval(path.clone())
                    .act();
                widgets::card()
                    .p_4()
                    .gap_2()
                    .child(
                        widgets::row()
                            .child(icon("globe", 16.0, widgets::green()))
                            .child(widgets::h3("Your site is live"))
                            .child(
                                crate::ui::Link::new("pages-url", v.s("html_url"))
                                    .url(v.s("html_url")),
                            ),
                    )
                    .child(widgets::dim(format!(
                        "Status: {}  ·  Source: {} {}",
                        v.s("status"),
                        v.s("source.branch"),
                        v.s("source.path")
                    )))
                    .child(
                        widgets::row()
                            .child(widgets::btn("edit-pages", "Edit settings", edit))
                            .child(widgets::btn(
                                "build-pages",
                                "Request a build",
                                Req::rest("POST", format!("{path}/builds"))
                                    .ok("Build requested")
                                    .inval(path.clone())
                                    .act(),
                            ))
                            .child(widgets::danger(
                                "disable-pages",
                                "Unpublish",
                                Req::rest("DELETE", path.clone())
                                    .ok("Pages disabled")
                                    .inval(path.clone())
                                    .act()
                                    .confirm(
                                        "Unpublish the site?",
                                        "The site will go offline.",
                                        "Unpublish",
                                    ),
                            )),
                    )
                    .into_any_element()
            }
            Load::Loading => widgets::loading(),
            Load::Failed(_) => widgets::card()
                .p_4()
                .gap_2()
                .child(widgets::dim(
                    "GitHub Pages is not enabled for this repository.",
                ))
                .child(div().child(widgets::go_btn("enable-pages", "Enable Pages", enable)))
                .into_any_element(),
        }
    }

    fn general_settings(
        &mut self,
        repo: &str,
        info: &Value,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette();
        let path = format!("/repos/{repo}");
        let edit = FormSpec::new("Repository settings")
            .width(640.0)
            .field(
                Field::text("name", "Repository name")
                    .value(info.s("name"))
                    .required(),
            )
            .field(
                Field::text("description", "Description")
                    .value(info.s("description"))
                    .keep_empty(),
            )
            .field(
                Field::text("homepage", "Website")
                    .value(info.s("homepage"))
                    .keep_empty(),
            )
            .field(Field::text("default_branch", "Default branch").value(info.s("default_branch")))
            .field(Field::bool(
                "is_template",
                "Template repository",
                info.b("is_template"),
            ))
            .field(Field::bool("has_issues", "Issues", info.b("has_issues")))
            .field(Field::bool(
                "has_projects",
                "Projects",
                info.b("has_projects"),
            ))
            .field(Field::bool("has_wiki", "Wiki", info.b("has_wiki")))
            .field(Field::bool(
                "has_discussions",
                "Discussions",
                info.b("has_discussions"),
            ))
            .field(Field::bool(
                "allow_merge_commit",
                "Allow merge commits",
                info.b("allow_merge_commit"),
            ))
            .field(Field::bool(
                "allow_squash_merge",
                "Allow squash merging",
                info.b("allow_squash_merge"),
            ))
            .field(Field::bool(
                "allow_rebase_merge",
                "Allow rebase merging",
                info.b("allow_rebase_merge"),
            ))
            .field(Field::bool(
                "allow_auto_merge",
                "Allow auto-merge",
                info.b("allow_auto_merge"),
            ))
            .field(Field::bool(
                "allow_update_branch",
                "Always suggest updating pull request branches",
                info.b("allow_update_branch"),
            ))
            .field(Field::bool(
                "delete_branch_on_merge",
                "Automatically delete head branches",
                info.b("delete_branch_on_merge"),
            ))
            .field(Field::bool(
                "web_commit_signoff_required",
                "Require contributors to sign off on web commits",
                info.b("web_commit_signoff_required"),
            ))
            .rest("PATCH", path.clone())
            .ok("Settings saved")
            .inval(path.clone())
            .then(|hub, value, cx| {
                let full = value.s("full_name");
                if let Some(r) = hub.route.repo() {
                    if r != full && !full.is_empty() {
                        hub.go(
                            Route::Repo {
                                repo: full,
                                tab: crate::hub::RepoTab::Settings,
                            },
                            cx,
                        );
                    }
                }
            })
            .act();
        let topics = FormSpec::new("Topics")
            .field(
                Field::list("names", "Topics")
                    .value(
                        info.list("topics")
                            .iter()
                            .filter_map(|t| t.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                    .keep_empty(),
            )
            .rest("PUT", format!("{path}/topics"))
            .ok("Topics saved")
            .inval(path.clone())
            .act();
        let private = info.b("private");
        let visibility = Req::rest("PATCH", path.clone())
            .body(json!({ "private": !private }))
            .ok("Visibility changed")
            .inval(path.clone())
            .act()
            .confirm(
                if private {
                    "Make this repository public?"
                } else {
                    "Make this repository private?"
                },
                if private {
                    "Everyone will be able to see the code, issues and history."
                } else {
                    "Stars and watchers from people without access are removed."
                },
                "Change visibility",
            );
        let archived = info.b("archived");
        let archive = Req::rest("PATCH", path.clone())
            .body(json!({ "archived": !archived }))
            .ok(if archived {
                "Repository unarchived"
            } else {
                "Repository archived"
            })
            .inval(path.clone())
            .act()
            .confirm(
                if archived {
                    "Unarchive this repository?"
                } else {
                    "Archive this repository?"
                },
                "Archived repositories are read-only.",
                if archived { "Unarchive" } else { "Archive" },
            );
        let transfer = FormSpec::new("Transfer ownership")
            .submit("Transfer repository")
            .danger()
            .note("Transfer this repository to another user or an organization you can create repositories in.")
            .field(Field::text("new_owner", "New owner").required())
            .field(Field::text("new_name", "New name (optional)"))
            .rest("POST", format!("{path}/transfer"))
            .ok("Transfer started")
            .act();
        let name = info.s("name");
        let delete = FormSpec::new(format!("Delete {repo}"))
            .submit("Delete this repository")
            .danger()
            .note("This permanently deletes the repository, wiki, issues, comments, packages, secrets, workflow runs and team permissions. It cannot be undone.")
            .field(Field::text("confirm", format!("Type “{repo}” to confirm").as_str()).required())
            .build_with({
                let repo = repo.to_string();
                move |v| {
                    if v.s("confirm").trim() != repo {
                        return Err("The name does not match.".into());
                    }
                    Ok(Req::rest("DELETE", format!("/repos/{repo}"))
                        .ok("Repository deleted")
                        .inval("/user/repos")
                        .then(|hub, _, cx| hub.go(Route::Repos, cx))
                        .act())
                }
            })
            .act();
        let _ = name;
        let danger_row = |title: &str, text: &str, button: AnyElement| {
            widgets::row()
                .p_4()
                .border_b_1()
                .border_color(rgb(p.divider))
                .child(
                    widgets::col()
                        .gap_0()
                        .flex_1()
                        .child(widgets::h3(title.to_string()))
                        .child(widgets::dim(text.to_string())),
                )
                .child(button)
        };
        widgets::col()
            .gap_4()
            .child(
                widgets::card()
                    .p_4()
                    .gap_2()
                    .child(widgets::h2("General"))
                    .child(widgets::dim(
                        "Name, description, features, merge options and branch defaults.",
                    ))
                    .child(
                        widgets::row()
                            .child(widgets::primary("edit-settings", "Edit settings", edit))
                            .child(widgets::btn("edit-topics", "Edit topics", topics)),
                    ),
            )
            .child(widgets::h2("Danger zone"))
            .child(
                widgets::card()
                    .border_color(rgb(widgets::red()))
                    .child(danger_row(
                        "Change visibility",
                        if private {
                            "This repository is private."
                        } else {
                            "This repository is public."
                        },
                        widgets::danger("visibility", "Change visibility", visibility)
                            .into_any_element(),
                    ))
                    .child(danger_row(
                        "Transfer ownership",
                        "Move this repository to another user or organization.",
                        widgets::danger("transfer", "Transfer", transfer).into_any_element(),
                    ))
                    .child(danger_row(
                        if archived {
                            "Unarchive this repository"
                        } else {
                            "Archive this repository"
                        },
                        "Mark this repository archived and read-only.",
                        widgets::danger(
                            "archive",
                            if archived { "Unarchive" } else { "Archive" },
                            archive,
                        )
                        .into_any_element(),
                    ))
                    .child(danger_row(
                        "Delete this repository",
                        "Once deleted, it is gone for good.",
                        widgets::danger("delete-repo", "Delete this repository", delete)
                            .into_any_element(),
                    )),
            )
            .into_any_element()
    }
}

/// "Mar 5, 2024" from seconds since the epoch.
pub fn date_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    time::date(&format!("{y:04}-{m:02}-{d:02}T00:00:00Z"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_from_seconds() {
        assert_eq!(super::date_from_secs(0), "Jan 1, 1970");
        assert_eq!(super::date_from_secs(951_868_800), "Mar 1, 2000");
    }
}
