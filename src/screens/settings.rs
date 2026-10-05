//! Your account: profile, emails, keys, social accounts, blocked users,
//! interaction limits, installed apps, the session, and updates.

use super::common::user_row;
use crate::form::{Field, FormSpec};
use crate::hub::{Act, Hub, Load, Req};
use crate::json::Json as _;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::ui::palette;
use crate::widgets::{self, rgb};
use gpui::prelude::FluentBuilder as _;
use gpui::{div, AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};
use serde_json::json;

const SECTIONS: [(&str, &str); 11] = [
    ("profile", "Public profile"),
    ("emails", "Emails"),
    ("ssh", "SSH keys"),
    ("signing", "SSH signing keys"),
    ("gpg", "GPG keys"),
    ("social", "Social accounts"),
    ("blocked", "Blocked users"),
    ("limits", "Interaction limits"),
    ("apps", "Applications"),
    ("session", "Session"),
    ("updates", "Updates"),
];

/// Bitbucket's account sections: the rest is the Atlassian account's.
const BITBUCKET_SECTIONS: [(&str, &str); 6] = [
    ("profile", "Profile"),
    ("emails", "Emails"),
    ("ssh", "SSH keys"),
    ("gpg", "GPG keys"),
    ("session", "Session"),
    ("updates", "Updates"),
];

/// Where an Atlassian account's profile is edited.
const ATLASSIAN_PROFILE: &str = "https://id.atlassian.com/manage-profile/profile-and-visibility";

/// GitLab's account sections: no social accounts, blocks, interaction
/// limits or apps to list, and its own access tokens.
const GITLAB_SECTIONS: [(&str, &str); 8] = [
    ("profile", "Profile"),
    ("emails", "Emails"),
    ("ssh", "SSH keys"),
    ("signing", "SSH signing keys"),
    ("gpg", "GPG keys"),
    ("tokens", "Access tokens"),
    ("session", "Session"),
    ("updates", "Updates"),
];

impl Hub {
    pub fn settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let section = self.choice("settings.section", "profile");
        let gitlab = crate::forge::is_gitlab();
        let bitbucket = crate::forge::is_bitbucket();
        let web = crate::forge::web();
        let body = match section.as_str() {
            "tokens" if gitlab => self.gl_tokens(cx),
            // Bitbucket shows the Atlassian account's emails, changed there.
            "emails" if bitbucket => {
                let spec = ListSpec::new("/user/emails", |e| {
                    let mut row = Row::new(e.s("email")).icon("mail", widgets::gray()).meta(
                        if e.b("verified") {
                            "verified"
                        } else {
                            "unverified"
                        },
                    );
                    if e.b("primary") {
                        row = row.tag("Primary", widgets::green());
                    }
                    row.inline()
                })
                .empty("No email addresses.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::dim(
                                "Your emails belong to your Atlassian account.",
                            ))
                            .child(widgets::spacer())
                            .child(widgets::btn(
                                "atlassian-emails",
                                "Change them at Atlassian",
                                Act::Url(ATLASSIAN_PROFILE.into()),
                            )),
                    )
                    .child(list)
                    .into_any_element()
            }
            "updates" => {
                let at_launch = crate::update::check_at_launch();
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::card()
                            .p_4()
                            .gap_2()
                            .child(widgets::h3(format!("VisualHub {}", crate::update::current_version())))
                            .child(widgets::dim(if crate::update::self_installable() {
                                "When there's a newer release, VisualHub can download it and restart into it."
                            } else {
                                "This copy is updated the way it was installed; VisualHub links to each new release."
                            }))
                            .child(
                                widgets::row()
                                    .child(widgets::primary("check-updates", "Check for updates", Act::run(|hub, _, cx| hub.check_for_update(cx))))
                                    .child(widgets::btn("all-releases", "All releases", Act::Url(crate::update::RELEASES_PAGE.into()))),
                            ),
                    )
                    .child(widgets::card().child(widgets::setting_row(
                        "Check for updates at launch",
                        "At most once a day. It asks GitHub for the latest release and sends nothing else.",
                        widgets::switch(
                            "check-at-launch",
                            at_launch,
                            Act::run(move |_, _, cx| {
                                crate::update::set_check_at_launch(!at_launch);
                                cx.notify();
                            }),
                        ),
                        true,
                    )))
                    .into_any_element()
            }
            "emails" => {
                let add = FormSpec::new("Add email address")
                    .field(Field::text("email", "Email").required())
                    .rest("POST", "/user/emails")
                    .map(|_, v| json!({ "emails": [v.s("email")] }))
                    .ok("Email added — check your inbox to verify it")
                    .inval("/user/emails")
                    .act();
                let spec = ListSpec::new("/user/emails", |e| {
                    let email = e.s("email");
                    let mut row =
                        Row::new(email.clone())
                            .icon("mail", widgets::gray())
                            .meta(format!(
                                "{}  ·  {}",
                                if e.b("verified") {
                                    "verified"
                                } else {
                                    "unverified"
                                },
                                e.s("visibility")
                            ));
                    if e.b("primary") {
                        row = row.tag("Primary", widgets::green());
                    } else {
                        row = row.danger(
                            "Remove",
                            Req::rest("DELETE", "/user/emails")
                                .body(json!({ "emails": [email] }))
                                .ok("Email removed")
                                .inval("/user/emails")
                                .act(),
                        );
                    }
                    row.inline()
                })
                .empty("No email addresses.");
                let list = self.list(&spec, cx);
                let visibility = FormSpec::new("Primary email visibility")
                    .field(Field::choice(
                        "visibility",
                        "Visibility",
                        &[("public", "Public"), ("private", "Private")],
                    ))
                    .rest("PATCH", "/user/email/visibility")
                    .ok("Visibility changed")
                    .inval("/user/emails")
                    .act();
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::btn(
                                "email-vis",
                                "Primary email visibility",
                                visibility,
                            ))
                            .child(widgets::go_btn("add-email", "Add email", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "ssh" => self.key_list("/user/keys", "SSH key", "Authentication key", cx),
            "signing" => self.key_list(
                "/user/ssh_signing_keys",
                "SSH signing key",
                "Signing key",
                cx,
            ),
            "gpg" => {
                let add = FormSpec::new("Add GPG key")
                    .field(Field::text("name", "Title"))
                    .field(
                        Field::multiline("armored_public_key", "Key")
                            .required()
                            .hint("Begins with -----BEGIN PGP PUBLIC KEY BLOCK-----"),
                    )
                    .rest("POST", "/user/gpg_keys")
                    .ok("GPG key added")
                    .inval("/user/gpg_keys")
                    .act();
                let spec = ListSpec::new("/user/gpg_keys", |k| {
                    let emails: Vec<String> =
                        k.list("emails").iter().map(|e| e.s("email")).collect();
                    Row::new(if k.s("name").is_empty() {
                        k.s("key_id")
                    } else {
                        k.s("name")
                    })
                    .icon("key", widgets::gray())
                    .meta(format!(
                        "Key ID {}  ·  {}  ·  added {}",
                        k.s("key_id"),
                        emails.join(", "),
                        time::ago(&k.s("created_at"))
                    ))
                    .danger(
                        "Delete",
                        Req::rest("DELETE", format!("/user/gpg_keys/{}", k.i("id")))
                            .ok("GPG key deleted")
                            .inval("/user/gpg_keys")
                            .act()
                            .confirm(
                                "Delete GPG key?",
                                "Commits signed with it will show as unverified.",
                                "Delete",
                            ),
                    )
                })
                .empty("No GPG keys.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-gpg", "New GPG key", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "social" => {
                let add = FormSpec::new("Add social accounts")
                    .field(Field::list("account_urls", "Profile URLs").required())
                    .rest("POST", "/user/social_accounts")
                    .ok("Added")
                    .inval("/user/social_accounts")
                    .act();
                let spec = ListSpec::new("/user/social_accounts", |a| {
                    let url = a.s("url");
                    Row::new(url.clone())
                        .icon("link", widgets::gray())
                        .meta(a.s("provider"))
                        .open(Act::Url(url.clone()))
                        .danger(
                            "Remove",
                            Req::rest("DELETE", "/user/social_accounts")
                                .body(json!({ "account_urls": [url] }))
                                .ok("Removed")
                                .inval("/user/social_accounts")
                                .act(),
                        )
                })
                .empty("No social accounts.");
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::go_btn("add-social", "Add accounts", add)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "blocked" => {
                let block = FormSpec::new("Block a user")
                    .field(Field::text("login", "Login").required())
                    .build_with(|v| {
                        Ok(
                            Req::rest("PUT", format!("/user/blocks/{}", v.s("login").trim()))
                                .ok("User blocked")
                                .inval("/user/blocks")
                                .act(),
                        )
                    })
                    .act();
                let spec = ListSpec::new("/user/blocks", |u| {
                    user_row(u).action(
                        "Unblock",
                        Req::rest("DELETE", format!("/user/blocks/{}", u.s("login")))
                            .ok("Unblocked")
                            .inval("/user/blocks")
                            .act(),
                    )
                })
                .empty("You haven't blocked anyone.")
                .people();
                let list = self.list(&spec, cx);
                widgets::col()
                    .gap_3()
                    .child(
                        widgets::row()
                            .child(widgets::spacer())
                            .child(widgets::danger("block-user", "Block a user", block)),
                    )
                    .child(list)
                    .into_any_element()
            }
            "limits" => {
                let current = self.fetch("/user/interaction-limits", cx);
                let set = FormSpec::new("Temporary interaction limits")
                    .note("Limit who can comment, open issues and open pull requests on all your public repositories.")
                    .field(Field::choice("limit", "Allow only", &[("existing_users", "Existing users"), ("contributors_only", "Prior contributors"), ("collaborators_only", "Collaborators")]))
                    .field(Field::choice("expiry", "For", &[("one_day", "24 hours"), ("three_days", "3 days"), ("one_week", "1 week"), ("one_month", "1 month"), ("six_months", "6 months")]))
                    .rest("PUT", "/user/interaction-limits")
                    .ok("Limits applied")
                    .inval("/user/interaction-limits")
                    .act();
                let status = match current.ready() {
                    Some(v) if v.has("limit") => format!(
                        "{} until {}",
                        v.s("limit").replace('_', " "),
                        time::date(&v.s("expires_at"))
                    ),
                    Some(_) => "No limits are in effect.".into(),
                    None => "…".into(),
                };
                widgets::card()
                    .p_4()
                    .gap_2()
                    .child(widgets::h3("Interaction limits"))
                    .child(widgets::dim(status))
                    .child(
                        widgets::row()
                            .child(widgets::primary("set-limits", "Set limits", set))
                            .child(widgets::btn(
                                "clear-limits",
                                "Remove limits",
                                Req::rest("DELETE", "/user/interaction-limits")
                                    .ok("Limits removed")
                                    .inval("/user/interaction-limits")
                                    .act(),
                            )),
                    )
                    .into_any_element()
            }
            "apps" => {
                // `/user/installations` only answers GitHub App user tokens,
                // never the classic or CLI tokens VisualHub signs in with.
                widgets::card()
                    .p_4()
                    .gap_3()
                    .child(widgets::h3("Applications"))
                    .child(widgets::dim("GitHub only lists the apps you've installed or authorized to the apps themselves, so these open on github.com."))
                    .child(
                        widgets::row()
                            .gap_2()
                            .child(widgets::btn("installed-apps", "Installed GitHub Apps", Act::Url(format!("{}/settings/installations", crate::api::WEB))))
                            .child(widgets::btn("authorized-apps", "Authorized GitHub Apps", Act::Url(format!("{}/settings/apps/authorizations", crate::api::WEB))))
                            .child(widgets::btn("oauth-apps", "Authorized OAuth apps", Act::Url(format!("{}/settings/applications", crate::api::WEB)))),
                    )
                    .into_any_element()
            }
            "session" => {
                let rate = if !crate::forge::is_github() {
                    crate::hub::Load::Loading
                } else {
                    self.fetch("/rate_limit", cx)
                };
                let mut card = widgets::card()
                    .p_4()
                    .gap_2()
                    .child(widgets::h3("Signed in"));
                card = card
                    .child(widgets::dim(format!(
                        "As {} on {}",
                        self.login(),
                        crate::forge::host()
                    )))
                    .child(widgets::dim(format!(
                        "Token scopes: {}",
                        if self.scopes.is_empty() {
                            "(fine-grained token or none reported)".to_string()
                        } else {
                            self.scopes.clone()
                        }
                    )))
                    .children(crate::scopes::settings_note(self));
                if let Load::Ready(r) = rate {
                    for (name, key) in [
                        ("REST API", "resources.core"),
                        ("Search", "resources.search"),
                        ("GraphQL", "resources.graphql"),
                    ] {
                        let used = r.i(&format!("{key}.used"));
                        let limit = r.i(&format!("{key}.limit")).max(1);
                        card = card.child(
                            widgets::row()
                                .child(div().w(gpui::px(90.0)).child(name))
                                .child(
                                    crate::ui::ProgressBar::new(used as f32 / limit as f32)
                                        .flex_1()
                                        .h(gpui::px(6.0)),
                                )
                                .child(widgets::dim(format!("{used}/{limit} used"))),
                        );
                    }
                }
                card.child(
                    widgets::row()
                        .child(widgets::btn(
                            "tokens",
                            format!("Manage tokens on {}", crate::forge::name()),
                            Act::Url(if bitbucket {
                                "https://id.atlassian.com/manage-profile/security/api-tokens".into()
                            } else if gitlab {
                                format!("{web}/-/user_settings/personal_access_tokens")
                            } else {
                                format!("{web}/settings/tokens")
                            }),
                        ))
                        .child(widgets::danger(
                            "sign-out",
                            "Sign out",
                            Act::run(|hub, _, cx| hub.sign_out(cx)),
                        )),
                )
                .into_any_element()
            }
            _ => {
                let me = self.me.clone();
                let edit = FormSpec::new("Edit profile")
                    .width(600.0)
                    .field(Field::text("name", "Name").value(me.s("name")).keep_empty())
                    .field(
                        Field::multiline("bio", "Bio")
                            .value(me.s("bio"))
                            .keep_empty(),
                    )
                    .field(
                        Field::text("email", "Public email")
                            .value(me.s("email"))
                            .keep_empty(),
                    )
                    .field(Field::text("blog", "URL").value(me.s("blog")).keep_empty())
                    .field(
                        Field::text("twitter_username", "X (Twitter) username")
                            .value(me.s("twitter_username"))
                            .keep_empty(),
                    )
                    .field(
                        Field::text("company", "Company")
                            .value(me.s("company"))
                            .keep_empty(),
                    )
                    .field(
                        Field::text("location", "Location")
                            .value(me.s("location"))
                            .keep_empty(),
                    )
                    .field(Field::bool(
                        "hireable",
                        "Available for hire",
                        me.b("hireable"),
                    ))
                    .rest("PATCH", "/user")
                    .ok("Profile updated")
                    .then(|hub, value, cx| {
                        hub.me = std::rc::Rc::new(value.clone());
                        cx.notify();
                    })
                    .act();
                let avatar = self.avatar(&me.s("avatar_url"), 120.0, cx);
                let mut info = widgets::col().gap_1().flex_1();
                for (label, key) in [
                    ("Name", "name"),
                    ("Bio", "bio"),
                    ("Public email", "email"),
                    ("URL", "blog"),
                    ("Company", "company"),
                    ("Location", "location"),
                ] {
                    info = info.child(
                        widgets::row()
                            .child(
                                div()
                                    .w(gpui::px(110.0))
                                    .text_color(rgb(palette().text_dim))
                                    .child(label),
                            )
                            .child(div().flex_1().child(me.s(key))),
                    );
                }
                widgets::card()
                    .p_4()
                    .child(
                        widgets::row().items_start().gap_4().child(info).child(
                            widgets::col()
                                .items_center()
                                .child(avatar)
                                .child(widgets::btn(
                                    "change-avatar",
                                    "Change picture",
                                    Act::Url(if bitbucket {
                                        ATLASSIAN_PROFILE.into()
                                    } else if gitlab {
                                        format!("{web}/-/user_settings/profile")
                                    } else {
                                        format!("{web}/settings/profile")
                                    }),
                                )),
                        ),
                    )
                    // GitLab's API doesn't let you edit your own profile.
                    .child(div().pt_3().child(if bitbucket {
                        widgets::primary(
                            "edit-profile",
                            "Edit profile at Atlassian",
                            Act::Url(ATLASSIAN_PROFILE.into()),
                        )
                    } else if gitlab {
                        widgets::primary(
                            "edit-profile",
                            "Edit profile on GitLab",
                            Act::Url(format!("{web}/-/user_settings/profile")),
                        )
                    } else {
                        widgets::primary("edit-profile", "Edit profile", edit)
                    }))
                    .into_any_element()
            }
        };
        let mut nav = widgets::col().gap_1().w(gpui::px(200.0)).flex_none();
        let sections: &[(&str, &str)] = if bitbucket {
            &BITBUCKET_SECTIONS
        } else if gitlab {
            &GITLAB_SECTIONS
        } else {
            &SECTIONS
        };
        for (i, (key, label)) in sections.iter().enumerate() {
            let selected = section == *key;
            nav = nav.child(
                widgets::list_row(("settings-nav", i), Act::choose("settings.section", *key))
                    .px_3()
                    .py_1()
                    .border_b_0()
                    .rounded_md()
                    .when(selected, |d| d.bg(rgb(palette().selection_bg)))
                    .child(label.to_string()),
            );
        }
        widgets::page()
            .child(widgets::title("Settings"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_6()
                    .items_start()
                    .child(nav)
                    .child(div().flex_1().min_w_0().child(body)),
            )
            .into_any_element()
    }

    fn key_list(
        &mut self,
        base: &str,
        noun: &str,
        kind: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let add = FormSpec::new(format!("Add new {noun}"))
            .field(Field::text("title", "Title").required())
            .field(
                Field::multiline("key", "Key")
                    .required()
                    .hint("Begins with ssh-ed25519, ssh-rsa, ecdsa-sha2-… or sk-…"),
            )
            .rest("POST", base.to_string())
            .ok(format!("{noun} added"))
            .inval(base.to_string())
            .act();
        let b = base.to_string();
        let kind = kind.to_string();
        let spec = ListSpec::new(base.to_string(), move |k| {
            Row::new(k.s("title"))
                .icon("key", widgets::gray())
                .meta(format!(
                    "{kind}  ·  added {}  ·  {}",
                    time::ago(&k.s("created_at")),
                    crate::json::clip(&k.s("key"), 48)
                ))
                .danger(
                    "Delete",
                    Req::rest("DELETE", format!("{b}/{}", k.i("id")))
                        .ok("Key deleted")
                        .inval(b.clone())
                        .act()
                        .confirm(
                            "Delete this key?",
                            "Anything using it will lose access.",
                            "Delete",
                        ),
                )
        })
        .empty(format!("No {noun}s."));
        let list = self.list(&spec, cx);
        widgets::col()
            .gap_3()
            .child(
                widgets::row()
                    .child(widgets::spacer())
                    .child(widgets::go_btn("add-key", format!("New {noun}"), add)),
            )
            .child(list)
            .into_any_element()
    }
}
