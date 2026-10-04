//! Making sure the token can reach everything the app shows.
//!
//! A classic token reports its scopes on every reply, so signing in is
//! enough to know what's missing. A GitHub CLI token can be widened in
//! place with `gh auth refresh`, which runs here with its output shown
//! in a terminal pane; any other token has to be replaced.

use crate::api::{self, Client};
use crate::hub::{Act, Hub};
use crate::ui::{palette, Button};
use crate::widgets;
use gpui::{
    div, prelude::FluentBuilder as _, px, rgb, AnyElement, Context, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _,
};
use serde_json::Value;
use std::io::{BufRead as _, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Every scope the app uses, with what it's for.
pub const SCOPES: &[(&str, &str)] = &[
    ("repo", "Repositories, issues, pull requests and notifications"),
    ("workflow", "Editing Actions workflows"),
    ("admin:org", "Organizations and teams"),
    ("gist", "Gists"),
    ("user", "Profile, emails, blocked users and interaction limits"),
    ("delete_repo", "Deleting repositories"),
    ("admin:public_key", "SSH keys"),
    ("admin:ssh_signing_key", "SSH signing keys"),
    ("admin:gpg_key", "GPG keys"),
    ("admin:repo_hook", "Repository webhooks"),
    ("project", "Projects"),
    ("codespace", "Codespaces"),
    ("read:packages", "Packages"),
    ("delete:packages", "Deleting packages"),
    ("write:discussion", "Team discussions"),
];

/// Scopes that a broader one already grants.
fn covered_by(scope: &str) -> &'static [&'static str] {
    match scope {
        "read:packages" => &["write:packages"],
        _ => &[],
    }
}

/// The scopes in [`SCOPES`] that `have` (an `X-OAuth-Scopes` list) lacks.
/// An empty list means a fine-grained token, which reports none and can't
/// be checked this way.
pub fn missing(have: &str) -> Vec<&'static str> {
    let have: Vec<&str> = have.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if have.is_empty() {
        return Vec::new();
    }
    SCOPES
        .iter()
        .map(|(scope, _)| *scope)
        .filter(|scope| !have.contains(scope) && !covered_by(scope).iter().any(|c| have.contains(c)))
        .collect()
}

/// A new-token page on GitHub with every scope ticked.
pub fn new_token_url() -> String {
    let scopes: Vec<&str> = SCOPES.iter().map(|(s, _)| *s).collect();
    format!("{}/settings/tokens/new?description=VisualHub&scopes={}", api::WEB, scopes.join(","))
}

pub enum Step {
    /// Showing what's missing.
    Ask,
    /// `gh auth refresh` is running.
    Running,
    /// It finished; `ok` is whether it exited cleanly.
    Finished { ok: bool },
}

pub struct ScopeFix {
    pub missing: Vec<&'static str>,
    pub step: Step,
    /// What `gh` has printed so far, stdout and stderr interleaved.
    output: Arc<Mutex<Vec<String>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl ScopeFix {
    pub fn new(missing: Vec<&'static str>) -> Self {
        ScopeFix {
            missing,
            step: Step::Ask,
            output: Arc::default(),
            child: Arc::default(),
        }
    }

    fn kill(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    /// The device code `gh` asked for, once it has printed it.
    fn code(&self) -> Option<String> {
        self.output.lock().unwrap().iter().find_map(|line| {
            let (_, rest) = line.split_once("one-time code:")?;
            Some(rest.trim().to_string())
        })
    }
}

impl Drop for ScopeFix {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Hub {
    /// After signing in: offer to fix the token if it's short of scopes.
    pub fn check_scopes(&mut self) {
        let missing = missing(&self.scopes);
        self.scope_fix = (!missing.is_empty()).then(|| ScopeFix::new(missing));
    }

    pub fn close_scope_fix(&mut self, cx: &mut Context<Self>) {
        self.scope_fix = None;
        cx.notify();
    }

    /// Run `gh auth refresh` for the missing scopes, streaming its output
    /// into the pane until it exits.
    fn run_gh_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(fix) = &mut self.scope_fix else { return };
        let mut command = Command::new(crate::cli::gh());
        command
            .args(["auth", "refresh", "--hostname", "github.com", "--scopes", &fix.missing.join(",")])
            // No terminal, so gh prints the code and URL rather than
            // waiting on Enter, then polls until the browser approves.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        {
            let mut output = fix.output.lock().unwrap();
            output.clear();
            output.push(format!("$ gh auth refresh -h github.com -s {}", fix.missing.join(",")));
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                fix.output.lock().unwrap().push(format!("Couldn't start gh: {e}"));
                fix.step = Step::Finished { ok: false };
                cx.notify();
                return;
            }
        };
        fn pipe(from: impl Read + Send + 'static, into: Arc<Mutex<Vec<String>>>) {
            std::thread::spawn(move || {
                for line in BufReader::new(from).lines().map_while(Result::ok) {
                    into.lock().unwrap().push(line);
                }
            });
        }
        if let Some(out) = child.stdout.take() {
            pipe(out, fix.output.clone());
        }
        if let Some(err) = child.stderr.take() {
            pipe(err, fix.output.clone());
        }
        *fix.child.lock().unwrap() = Some(child);
        fix.step = Step::Running;
        let child = fix.child.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let ok = loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let status = match child.lock().unwrap().as_mut() {
                    Some(c) => c.try_wait(),
                    None => return,
                };
                match status {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) => {
                        // Still waiting on the browser; show what's new.
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                    Err(_) => break false,
                }
            };
            // Let the readers drain the last lines.
            cx.background_executor().timer(Duration::from_millis(100)).await;
            this.update(cx, |hub, cx| {
                // Closed while it ran: nothing to report.
                let Some(fix) = &mut hub.scope_fix else { return };
                if !Arc::ptr_eq(&fix.child, &child) {
                    return;
                }
                fix.step = Step::Finished { ok };
                if ok {
                    hub.reload_cli_token(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pick up the GitHub CLI's widened token and carry on with it.
    fn reload_cli_token(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let token = api::gh_cli_token().ok_or_else(|| anyhow::anyhow!("gh has no token"))?;
                    let client = Client::new(&token);
                    let reply = client.send("GET", "/user", None, None)?;
                    if reply.status != 200 {
                        anyhow::bail!("GitHub refused the new token ({}).", reply.status);
                    }
                    let me: Value = serde_json::from_str(&reply.body)?;
                    Ok((client, me, reply.scopes.unwrap_or_default()))
                })
                .await;
            this.update(cx, |hub, cx| {
                match result {
                    Ok((client, me, scopes)) => {
                        hub.refresh_token(client, me, scopes);
                        let still = missing(&hub.scopes);
                        if still.is_empty() {
                            hub.scope_fix = None;
                            hub.toast("GitHub access updated", false, cx);
                        } else if let Some(fix) = &mut hub.scope_fix {
                            fix.missing = still;
                            fix.step = Step::Ask;
                        }
                    }
                    Err(e) => hub.toast(format!("{e:#}"), true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_scope_fix(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette();
        self.scope_fix.as_ref()?;
        let terminal_scroll = self.scroller("gh-terminal");
        let fix = self.scope_fix.as_ref()?;
        let from_cli = self.token_source == "GitHub CLI";

        let mut list = widgets::col().gap_1();
        for scope in &fix.missing {
            let what = SCOPES.iter().find(|(s, _)| s == scope).map(|(_, w)| *w).unwrap_or("");
            list = list.child(
                widgets::row()
                    .gap_2()
                    .child(div().font_family(widgets::MONO).text_size(px(12.0)).w(px(170.0)).flex_none().child(scope.to_string()))
                    .child(widgets::dim(what)),
            );
        }

        let intro = if from_cli {
            "Your GitHub CLI sign-in is missing some permissions, so these parts of VisualHub won't load. \
             VisualHub can ask gh to add them: you'll get a code to enter on GitHub."
        } else {
            "The token you signed in with is missing some permissions, so these parts of VisualHub won't load. \
             Create a token with everything ticked, then sign out and paste it in."
        };

        let mut modal = crate::ui::Modal::new("More access needed")
            .width(600.0)
            .text_size(px(13.0))
            .p_4()
            .gap_3();

        match fix.step {
            Step::Ask => {
                modal = modal.child(div().child(intro)).child(list);
            }
            Step::Running | Step::Finished { .. } => {
                if let Some(code) = fix.code() {
                    let copy_open = Act::run({
                        let code = code.clone();
                        move |hub, window, cx| {
                            hub.perform(Act::Copy(code.clone()), window, cx);
                            cx.open_url(&format!("{}/login/device", api::WEB));
                        }
                    });
                    modal = modal.when(matches!(fix.step, Step::Running), |m| {
                        m.child(
                            widgets::row()
                                .gap_3()
                                .child(widgets::dim("Enter this code on GitHub:"))
                                .child(
                                    div()
                                        .font_family(widgets::MONO)
                                        .text_size(px(20.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(code),
                                )
                                .child(div().flex_1())
                                .child(widgets::primary("copy-open", "Copy code and open GitHub", copy_open)),
                        )
                    });
                }
                let lines = fix.output.lock().unwrap().clone();
                let mut term = div()
                    .id("gh-terminal")
                    .flex()
                    .flex_col()
                    .h(px(180.0))
                    .overflow_y_scroll()
                    .track_scroll(&terminal_scroll)
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x0D1117))
                    .text_color(rgb(0xC9D1D9))
                    .font_family(widgets::MONO)
                    .text_size(px(12.0));
                for line in lines {
                    term = term.child(div().child(if line.is_empty() { " ".to_string() } else { line }));
                }
                if matches!(fix.step, Step::Running) {
                    term = term.child(div().text_color(rgb(p.text_dim)).child("Waiting for GitHub…"));
                }
                modal = modal.child(term);
                if let Step::Finished { ok: false } = fix.step {
                    modal = modal.child(
                        div()
                            .text_color(rgb(widgets::red()))
                            .child("gh didn't finish. You can try again, or run the command above in a terminal yourself."),
                    );
                }
            }
        }

        let running = matches!(fix.step, Step::Running);
        let close_label = if running { "Cancel" } else { "Not now" };
        modal = modal
            .action(div().flex_1().when(running, |d| d.child(crate::ui::Spinner::new("scope-busy").size(16.0))))
            .action(
                Button::new("scope-close", close_label)
                    .h(px(28.0))
                    .on_click(cx.listener(|hub, _, _, cx| hub.close_scope_fix(cx))),
            );
        if from_cli {
            if !running {
                let label = if matches!(fix.step, Step::Ask) { "Grant access with gh" } else { "Try again" };
                modal = modal.action(
                    Button::new("scope-run", label)
                        .primary()
                        .h(px(28.0))
                        .on_click(cx.listener(|hub, _, _, cx| hub.run_gh_refresh(cx))),
                );
            }
        } else {
            modal = modal
                .action(widgets::btn("scope-new-token", "Create a token", Act::Url(new_token_url())))
                .action(widgets::primary(
                    "scope-sign-out",
                    "Sign out to paste it",
                    Act::run(|hub, _, cx| {
                        hub.scope_fix = None;
                        hub.sign_out(cx);
                    }),
                ));
        }
        Some(modal.into_any_element())
    }
}

/// The settings line saying what's missing, with a way back into the fix.
pub fn settings_note(hub: &Hub) -> Option<AnyElement> {
    let missing = missing(&hub.scopes);
    if missing.is_empty() {
        return None;
    }
    Some(
        widgets::row()
            .gap_2()
            .child(widgets::icon_text("alert", format!("Missing scopes: {}", missing.join(", ")), widgets::yellow()))
            .child(widgets::btn(
                "scope-fix",
                "Fix…",
                Act::run(|hub, _, cx| {
                    hub.check_scopes();
                    cx.notify();
                }),
            ))
            .into_any_element(),
    )
}
