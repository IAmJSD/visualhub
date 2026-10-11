//! Settings ▸ Browser extension: the browsers on this machine, and
//! putting the extension in each. See `src/browsers.rs` for what
//! installing can and can't do in each kind.

use crate::browsers::{self, Browser, Installed};
use crate::hub::{Act, Hub};
use crate::widgets;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};

impl Hub {
    pub fn browser_extension(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.browsers.is_none() && !self.looking_for_browsers {
            self.look_for_browsers(cx);
        }
        let about = widgets::card()
            .p_4()
            .gap_2()
            .child(widgets::h3("Browser extension"))
            .child(widgets::dim(
                "Adds a VisualHub button to your browser's toolbar. It lights up on GitHub, GitLab \
                 and Bitbucket pages VisualHub can show, and a click opens the page here.",
            ))
            .child(widgets::faint(
                "It also lights up on self-hosted GitLab, which it recognises from the page, so it \
                 asks to read every site. The check happens in the browser, once per site, and \
                 nothing is sent anywhere. Bitbucket Data Center is recognised too, but VisualHub \
                 only signs in to Bitbucket Cloud.",
            ));
        let mut list = widgets::card();
        match &self.browsers {
            None => list = list.child(widgets::loading()),
            Some(found) if found.is_empty() => {
                list = list.child(widgets::empty(
                    "No Chromium or Firefox browser found on this computer.",
                ))
            }
            Some(found) => {
                for (i, browser) in found.iter().enumerate() {
                    let busy = self.extension_busy.as_deref() == Some(&browser.name);
                    let done =
                        matches!(&self.extension_installed, Some((n, _)) if n == &browser.name);
                    let b = browser.clone();
                    let install = Act::run(move |hub, _, cx| hub.install_extension(b.clone(), cx));
                    let label = if busy {
                        "Installing…"
                    } else if done {
                        "Install again"
                    } else {
                        "Install"
                    };
                    let button = if done {
                        widgets::btn(("install-extension", i), label, install)
                    } else {
                        widgets::primary(("install-extension", i), label, install)
                    }
                    .disabled(self.extension_busy.is_some());
                    list = list.child(widgets::setting_row(
                        &browser.name,
                        format!("Built on {}", browser.family.name()),
                        button,
                        i == 0,
                    ));
                }
            }
        }
        let mut page = widgets::col().gap_3().child(about).child(list);
        if let Some((name, installed)) = self.extension_installed.clone() {
            if let Some(browser) = self
                .browsers
                .iter()
                .flatten()
                .find(|b| b.name == name)
                .cloned()
            {
                page = page.child(self.extension_steps(&browser, &installed));
            }
        }
        page.child(widgets::row().child(widgets::spacer()).child(widgets::btn(
            "look-for-browsers",
            "Look for browsers again",
            Act::run(|hub, _, cx| hub.look_for_browsers(cx)),
        )))
        .into_any_element()
    }

    fn look_for_browsers(&mut self, cx: &mut Context<Self>) {
        self.looking_for_browsers = true;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async { browsers::detect() })
                .await;
            this.update(cx, |hub, cx| {
                hub.looking_for_browsers = false;
                hub.browsers = Some(found);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn install_extension(&mut self, browser: Browser, cx: &mut Context<Self>) {
        self.extension_busy = Some(browser.name.clone());
        let webs = self.extension_hosts();
        cx.spawn(async move |this, cx| {
            let b = browser.clone();
            let result = cx
                .background_executor()
                .spawn(async move { browsers::install(&b, &webs) })
                .await;
            this.update(cx, |hub, cx| {
                hub.extension_busy = None;
                match result {
                    Ok(installed) => {
                        if let Installed::Unpacked { dir } = &installed {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                dir.display().to_string(),
                            ));
                        }
                        hub.extension_installed = Some((browser.name.clone(), installed));
                    }
                    Err(e) => hub.toast(
                        format!("Couldn't install the extension in {}: {e:#}", browser.name),
                        true,
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// What's left to do in `browser` once installing has done its part.
    fn extension_steps(&self, browser: &Browser, installed: &Installed) -> AnyElement {
        let name = &browser.name;
        let mut card = widgets::card().p_4().gap_2();
        match installed {
            Installed::Unpacked { dir } => {
                let path = dir.display().to_string();
                let reveal = dir.clone();
                let picker = if cfg!(target_os = "macos") {
                    " (⇧⌘G in the folder picker takes a path)"
                } else {
                    ""
                };
                card = card
                    .child(widgets::h3(format!("Finish in {name}")))
                    .child(widgets::dim(format!(
                        "{name} only adds extensions from outside its store by hand, so it has \
                         opened {} for you to finish there.",
                        browser.extensions_page()
                    )))
                    .child(widgets::step(
                        1,
                        "Turn on Developer mode",
                        false,
                        widgets::dim("The switch is in the extensions page's top corner."),
                    ))
                    .child(widgets::step(
                        2,
                        "Click Load unpacked and choose this folder",
                        false,
                        widgets::col()
                            .gap_2()
                            .child(widgets::dim(format!(
                                "Its path is on the clipboard{picker}. Dropping the folder on the \
                                 page works too."
                            )))
                            .child(widgets::mono(path.clone()))
                            .child(
                                widgets::row()
                                    .child(widgets::btn(
                                        "copy-extension-path",
                                        "Copy path",
                                        Act::Copy(path),
                                    ))
                                    .child(widgets::btn(
                                        "show-extension-folder",
                                        "Show folder",
                                        Act::run(move |_, _, cx| cx.reveal_path(&reveal)),
                                    )),
                            ),
                    ))
                    .child(widgets::step(
                        3,
                        "Pin it and try it",
                        true,
                        widgets::dim(
                            "Pin VisualHub from the extensions menu, then click it on a GitHub, \
                             GitLab or Bitbucket page. The first time, the browser asks whether \
                             to open VisualHub; let it always do so to skip that.",
                        ),
                    ))
                    .child(widgets::faint(
                        "VisualHub keeps this folder up to date; the browser picks up a new \
                         version when it restarts. Leave the folder where it is.",
                    ));
            }
            Installed::AddOn { signed: true } => {
                card = card
                    .child(widgets::h3(format!("Finish in {name}")))
                    .child(widgets::dim(format!(
                        "{name} asks whether to add VisualHub. Click Add, then pin its button to \
                         the toolbar and click it on a GitHub, GitLab or Bitbucket page."
                    )));
            }
            Installed::AddOn { signed: false } => {
                card = card
                    .child(widgets::h3(format!("Finish in {name}")))
                    .child(widgets::dim(format!(
                        "This copy of VisualHub has no signed add-on, so {name} adds it only if \
                         it allows unsigned add-ons: Firefox Developer Edition and Nightly do once \
                         xpinstall.signatures.required is false in about:config, as do some \
                         Firefox-based browsers. Otherwise, {name} says it couldn't verify it."
                    )))
                    .child(widgets::dim(
                        "To try it anyway, open about:debugging, choose This Firefox, then Load \
                         Temporary Add-on and pick visualhub-unsigned.xpi from the folder below. \
                         It stays until the browser quits.",
                    ))
                    .child(widgets::row().child(widgets::btn(
                        "show-addon-folder",
                        "Show folder",
                        Act::run(|_, _, cx| {
                            if let Ok(xpi) = browsers::unsigned_xpi_path() {
                                cx.reveal_path(&xpi);
                            }
                        }),
                    )));
            }
        }
        card.into_any_element()
    }
}
