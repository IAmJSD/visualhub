//! Checking for a newer release, and the dialog that offers it.
//!
//! Where VisualHub can replace itself — a macOS bundle, a Windows install
//! — it offers to, and restarts into the new one. Where it cannot, the
//! copy belongs to whatever installed it, so the dialog says where the
//! release is and gets out of the way.

use crate::hub::{Hub, Modal};
use crate::ui::{palette, Button, ProgressBar};
use crate::update::{self, Progress, Update, UpdateStatus};
use crate::widgets::rgb;
use gpui::{div, px, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The update dialog's width, which its progress bar has to match.
const DIALOG_WIDTH: f32 = 440.0;

impl Hub {
    /// The launch-time check, when the setting allows one and the last
    /// one was long enough ago. Delayed: the first seconds after launch
    /// belong to signing in, not to another round trip.
    pub fn check_for_update_at_launch(&mut self, cx: &mut Context<Self>) {
        if !update::check_at_launch() || !update::check_due() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            this.update(cx, |hub, cx| hub.run_update_check(true, cx))
                .ok();
        })
        .detach();
    }

    /// Ask upstream whether a newer release exists, and say what it said.
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) {
        self.toast("Checking for updates…", false, cx);
        self.run_update_check(false, cx);
    }

    /// `quiet` is the launch-time check: silent unless there is something
    /// to say, since a machine that is offline at login should not be
    /// shown a failure it never asked for.
    fn run_update_check(&mut self, quiet: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async {
                    let status = update::check();
                    // A check that never reached GitHub is not one, so a
                    // machine that was offline at launch tries again at
                    // the next one rather than in a day.
                    if !matches!(status, UpdateStatus::Failed(_)) {
                        update::mark_checked();
                    }
                    status
                })
                .await;
            this.update(cx, |hub, cx| {
                match status {
                    UpdateStatus::Available(update) => {
                        log::info!("update {} available at {}", update.version, update.page);
                        // The launch-time check lands five seconds in, by
                        // which time the user may be in a dialog of their
                        // own. Theirs wins; a toast still says an update
                        // is there, and Settings ▸ Updates brings this back.
                        if !quiet || hub.modal.is_none() {
                            hub.modal = Some(Modal::Update { update });
                        } else {
                            hub.toast(
                                format!("VisualHub {} is available", update.version),
                                false,
                                cx,
                            );
                        }
                    }
                    UpdateStatus::UpToDate if !quiet => {
                        hub.toast(
                            format!("VisualHub {} is up to date", update::current_version()),
                            false,
                            cx,
                        );
                    }
                    UpdateStatus::Failed(err) if !quiet => {
                        hub.toast(format!("Couldn't check for updates: {err}"), true, cx);
                    }
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Download the release and install it over this copy, then quit so
    /// the relauncher can start the new build.
    ///
    /// The dialog stays up throughout: it is what shows the progress.
    pub fn start_update(&mut self, update: Update, cx: &mut Context<Self>) {
        let Some(installer) = update.install.clone() else {
            return;
        };
        if self.update_progress.is_some() {
            return;
        }
        self.update_progress = Some(Progress::Downloading {
            received: 0,
            total: installer.size,
        });
        cx.notify();

        // The download runs on a background thread and counts bytes into
        // this; the dialog reads it on a timer, rather than that thread
        // reaching into the entity once per 64 KiB.
        let received = Arc::new(AtomicU64::new(0));
        cx.spawn({
            let received = received.clone();
            async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let downloading = this.update(cx, |hub, cx| {
                    let Some(Progress::Downloading { received: got, .. }) =
                        hub.update_progress.as_mut()
                    else {
                        return false;
                    };
                    *got = received.load(Ordering::Relaxed);
                    cx.notify();
                    true
                });
                if !matches!(downloading, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let fetched = {
                let received = received.clone();
                cx.background_executor()
                    .spawn(async move { update::download(&installer, &received) })
                    .await
            };
            let file = match fetched {
                Ok(file) => file,
                Err(err) => {
                    this.update(cx, |hub, cx| hub.update_failed(&format!("{err:#}"), cx))
                        .ok();
                    return;
                }
            };
            // Nothing is installed if the user cancelled while it was
            // coming down: `update_progress` is cleared by Cancel, and
            // that is what says the download was still wanted.
            let wanted = this.update(cx, |hub, cx| {
                if hub.update_progress.is_none() {
                    return false;
                }
                hub.update_progress = Some(Progress::Installing);
                cx.notify();
                true
            });
            if !matches!(wanted, Ok(true)) {
                update::clean_downloads();
                return;
            }
            let installed = cx
                .background_executor()
                .spawn(async move { update::install_and_restart(&file) })
                .await;
            this.update(cx, |hub, cx| {
                hub.update_progress = None;
                match installed {
                    // The relauncher is waiting for this process to go.
                    Ok(()) => cx.quit(),
                    Err(err) => hub.update_failed(&format!("{err:#}"), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Abandon a download in progress.
    ///
    /// The transfer itself is left to run out into the temporary
    /// directory, since a blocking read cannot be interrupted, but its
    /// result is dropped and nothing is installed.
    pub fn cancel_update(&mut self, cx: &mut Context<Self>) {
        self.update_progress = None;
        self.close_modal(cx);
        self.toast("Update cancelled", false, cx);
    }

    /// Give up on an update, leaving the user where they were.
    fn update_failed(&mut self, err: &str, cx: &mut Context<Self>) {
        log::error!("update failed: {err}");
        update::clean_downloads();
        self.update_progress = None;
        if matches!(self.modal, Some(Modal::Update { .. })) {
            self.close_modal(cx);
        }
        self.toast(format!("The update failed: {err}"), true, cx);
    }

    /// The update-available prompt and its download progress.
    pub fn update_dialog(&mut self, update: &Update, cx: &mut Context<Self>) -> AnyElement {
        let p = palette();
        let progress = self.update_progress;
        let installer = update.install.clone();

        let mut modal = crate::ui::Modal::new("Update available")
            .width(DIALOG_WIDTH)
            .text_size(px(13.0))
            .p_4()
            .gap_3()
            .child(div().text_color(rgb(p.text)).child(format!(
                "VisualHub {} is available. You have {}.",
                update.version,
                update::current_version()
            )));
        let bar = |fraction: f32| {
            // The card's width less its padding, which is `p_4` on both sides.
            ProgressBar::new(fraction)
                .w(px(DIALOG_WIDTH - 32.0))
                .h(px(6.0))
        };
        modal = match (&installer, progress) {
            (_, Some(Progress::Downloading { received, total })) => modal
                .child(div().text_color(rgb(p.text_dim)).child(format!(
                    "Downloading… {} of {}",
                    megabytes(received),
                    megabytes(total)
                )))
                .child(bar(received as f32 / total.max(1) as f32)),
            (_, Some(Progress::Installing)) => {
                modal.child(div().text_color(rgb(p.text_dim)).child("Installing…")).child(bar(1.0))
            }
            (Some(installer), None) => modal.child(div().text_color(rgb(p.text_dim)).child(format!(
                "It's a {} download. VisualHub will restart to finish installing it.",
                megabytes(installer.size)
            ))),
            (None, None) => modal.child(
                div()
                    .text_color(rgb(p.text_dim))
                    .child("Update this copy the way you installed it, or download the new version from its release page."),
            ),
        };

        let page = update.page.clone();
        match progress {
            // Nothing to press while the bundle is being swapped: it takes
            // a moment and there is no half of it to back out to.
            Some(Progress::Installing) => {}
            Some(Progress::Downloading { .. }) => {
                modal = modal.action(
                    Button::new("update-cancel", "Cancel")
                        .h(px(28.0))
                        .on_click(cx.listener(|hub, _, _, cx| hub.cancel_update(cx))),
                );
            }
            None => {
                modal = modal.action(
                    Button::new("update-later", "Later")
                        .h(px(28.0))
                        .on_click(cx.listener(|hub, _, _, cx| hub.close_modal(cx))),
                );
                modal = match installer {
                    Some(_) => {
                        let update = update.clone();
                        modal
                            .action(
                                Button::new("update-notes", "Release notes")
                                    .h(px(28.0))
                                    .on_click(move |_, _, cx| cx.open_url(&page)),
                            )
                            .action(
                                Button::new("update-install", "Update and restart")
                                    .h(px(28.0))
                                    .primary()
                                    .on_click(cx.listener(move |hub, _, _, cx| {
                                        hub.start_update(update.clone(), cx)
                                    })),
                            )
                    }
                    None => modal.action(
                        Button::new("update-page", "Open release page")
                            .h(px(28.0))
                            .primary()
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    ),
                };
            }
        }
        modal.into_any_element()
    }
}

/// A download's size, in the megabytes a release page would quote.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
