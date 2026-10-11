//! `visualhub://open?url=<a forge page>`: how the browser extension (or
//! anything else) hands VisualHub a page to show. The extension adds
//! `forge=` (`github`, `gitlab` or `bitbucket`) and `root=` (the site's
//! address, which for a self-hosted GitLab may end in a path), so a page on
//! a GitLab VisualHub isn't signed in to can lead to signing in there.
//!
//! macOS delivers the link to the running app as an Apple event, which
//! GPUI passes to [`Application::on_open_urls`](gpui::Application). Linux
//! and Windows start a second copy with the link as its argument, so that
//! copy finds the first through a loopback socket whose port it left in
//! the config directory, hands the link over and exits; only when no copy
//! answers does it carry on as the app.
//!
//! The links arrive on [`receiver`], which the hub reads.

use crate::forge::Forge;
use std::sync::OnceLock;

pub const SCHEME: &str = "visualhub";

/// A page handed over, and what the sender said of its site.
#[derive(Clone, Debug, PartialEq)]
pub struct Handed {
    pub url: String,
    pub forge: Option<Forge>,
    /// The site's address, `https://host` or `https://host/path`.
    pub root: Option<String>,
}

type Channel = (
    async_channel::Sender<Handed>,
    async_channel::Receiver<Handed>,
);

fn channel() -> &'static Channel {
    static CHANNEL: OnceLock<Channel> = OnceLock::new();
    CHANNEL.get_or_init(async_channel::unbounded)
}

/// Pages handed to the app while it runs, in the order they came.
pub fn receiver() -> async_channel::Receiver<Handed> {
    channel().1.clone()
}

/// Hand `link` (a `visualhub://` link or a page's URL) to the hub.
pub fn deliver(link: &str) {
    if let Some(page) = page(link) {
        channel().0.try_send(page).ok();
    }
}

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// The page a `visualhub://open?url=` link names, or a bare `http(s)://`
/// page's URL as it is.
pub fn page(link: &str) -> Option<Handed> {
    let link = link.trim();
    if is_web(link) {
        return Some(Handed {
            url: link.to_string(),
            forge: None,
            root: None,
        });
    }
    let rest = link.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let query = rest.strip_prefix("open")?.trim_start_matches('/');
    let query = query.strip_prefix('?')?;
    let param = |name: &str| {
        query.split('&').find_map(|pair| {
            let value = pair.strip_prefix(name)?.strip_prefix('=')?;
            Some(crate::gitlab::decode(&value.replace('+', " ")))
        })
    };
    let url = param("url").filter(|u| is_web(u))?;
    Some(Handed {
        url,
        forge: param("forge").and_then(|f| Forge::from_key(&f)),
        root: param("root")
            .map(|r| r.trim_end_matches('/').to_string())
            .filter(|r| is_web(r)),
    })
}

/// The link to start on: `VISUALHUB_OPEN`, or one on the command line
/// (which is how Linux and Windows pass one to a copy they start).
pub fn initial() -> Option<String> {
    std::env::var("VISUALHUB_OPEN")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::args().skip(1).find(|arg| page(arg).is_some()))
}

/// Where the running copy leaves its port and the token a sender must
/// show, for the next copy to find.
#[cfg(not(target_os = "macos"))]
fn instance_file() -> Option<std::path::PathBuf> {
    Some(crate::api::config_dir()?.join("instance"))
}

/// Give `link` to a copy that is already running. True when one took it,
/// and this copy should quit.
#[cfg(not(target_os = "macos"))]
pub fn forward(link: &str) -> bool {
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let Some(text) = instance_file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return false;
    };
    let mut parts = text.split_whitespace();
    let (Some(port), Some(token)) = (
        parts.next().and_then(|p| p.parse::<u16>().ok()),
        parts.next(),
    ) else {
        return false;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    if writeln!(stream, "{token} {link}").is_err() {
        return false;
    }
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok();
    answer.trim() == "ok"
}

#[cfg(target_os = "macos")]
pub fn forward(_link: &str) -> bool {
    false
}

/// Take links from copies started after this one.
#[cfg(not(target_os = "macos"))]
pub fn listen() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
        return;
    };
    let (Ok(addr), Some(file)) = (listener.local_addr(), instance_file()) else {
        return;
    };
    // Only a copy that can read this user's config directory knows it,
    // which keeps other users on the machine from driving the app.
    let token = token();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if std::fs::write(&file, format!("{} {token}\n", addr.port())).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            if let Some((given, link)) = line.trim().split_once(' ') {
                if given == token {
                    deliver(link);
                    (&stream).write_all(b"ok\n").ok();
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
pub fn listen() {}

/// Sixteen random bytes as hex, from the OS's hasher seeds.
#[cfg(not(target_os = "macos"))]
fn token() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    (0..2)
        .map(|_| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default(),
            );
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Make sure `visualhub://` links open this copy, where nothing else has
/// claimed them: the Windows installer and the Linux packages register
/// it, but a portable `.exe`, an AppImage or a bare binary don't. macOS
/// learns it from the bundle's Info.plist.
pub fn register() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use anyhow::Context as _;
        use std::os::windows::process::CommandExt as _;
        const NO_WINDOW: u32 = 0x0800_0000;
        let exe = std::env::current_exe()?;
        let command = format!("\"{}\" \"%1\"", exe.display());
        let current = std::process::Command::new("reg")
            .args(["query", r"HKCR\visualhub\shell\open\command", "/ve"])
            .creation_flags(NO_WINDOW)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if current.contains(&exe.display().to_string()) {
            return Ok(());
        }
        let key = r"HKCU\Software\Classes\visualhub";
        for args in [
            vec!["add", key, "/ve", "/d", "URL:VisualHub", "/f"],
            vec!["add", key, "/v", "URL Protocol", "/d", "", "/f"],
            vec![
                "add",
                &format!(r"{key}\shell\open\command"),
                "/ve",
                "/d",
                &command,
                "/f",
            ],
        ] {
            let status = std::process::Command::new("reg")
                .args(&args)
                .creation_flags(NO_WINDOW)
                .status()
                .context("running reg")?;
            anyhow::ensure!(status.success(), "reg {} failed", args.join(" "));
        }
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use anyhow::Context as _;
        let handler = std::process::Command::new("xdg-mime")
            .args(["query", "default", "x-scheme-handler/visualhub"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if !handler.is_empty() && handler != "visualhub-link.desktop" {
            return Ok(());
        }
        // An AppImage runs from a mount that moves every launch; the
        // AppImage file itself is what to start.
        let exe = std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .map_or_else(std::env::current_exe, Ok)?;
        let home = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })
            .context("no home directory")?;
        let dir = home.join("applications");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("visualhub-link.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=VisualHub\nExec=\"{}\" %u\n\
                 NoDisplay=true\nMimeType=x-scheme-handler/visualhub;\n",
                exe.display()
            ),
        )?;
        std::process::Command::new("xdg-mime")
            .args([
                "default",
                "visualhub-link.desktop",
                "x-scheme-handler/visualhub",
            ])
            .status()
            .context("running xdg-mime")?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_name_their_page() {
        let url = |link: &str| page(link).map(|h| h.url);
        assert_eq!(
            url("visualhub://open?url=https%3A%2F%2Fgithub.com%2Frust-lang%2Frust%2Fpull%2F7")
                .as_deref(),
            Some("https://github.com/rust-lang/rust/pull/7")
        );
        assert_eq!(
            url("visualhub://open/?url=https%3A%2F%2Fgitlab.com%2Fa%2Fb").as_deref(),
            Some("https://gitlab.com/a/b")
        );
        assert_eq!(
            page("https://bitbucket.org/acme/widget"),
            Some(Handed {
                url: "https://bitbucket.org/acme/widget".into(),
                forge: None,
                root: None,
            })
        );
        assert_eq!(page("visualhub://open?url=javascript%3Aalert(1)"), None);
        assert_eq!(page("visualhub://other?url=https%3A%2F%2Fgithub.com"), None);
        assert_eq!(page("--flag"), None);
    }

    #[test]
    fn links_say_which_site_they_are_on() {
        assert_eq!(
            page(
                "visualhub://open?url=https%3A%2F%2Fexample.com%2Fgitlab%2Fa%2Fb\
                 &forge=gitlab&root=https%3A%2F%2Fexample.com%2Fgitlab%2F"
            ),
            Some(Handed {
                url: "https://example.com/gitlab/a/b".into(),
                forge: Some(Forge::GitLab),
                root: Some("https://example.com/gitlab".into()),
            })
        );
        let odd =
            page("visualhub://open?url=https%3A%2F%2Fx.test%2F&forge=svn&root=file%3A%2F%2F%2F")
                .unwrap();
        assert_eq!((odd.forge, odd.root), (None, None));
    }
}
