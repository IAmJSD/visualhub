//! The browser extension (`extension/`): finding the browsers on this
//! machine and putting the extension in front of each.
//!
//! No list of browsers is kept. Each platform already has one, and what
//! a browser is built on shows in its files:
//!
//! * **macOS** — the apps in `/Applications` and `~/Applications`. A
//!   Firefox has `Contents/MacOS/XUL`; a Chromium has a `… Framework`
//!   bundle and says it opens `https` links, and isn't an Electron app
//!   (which carries `Resources/app.asar`).
//! * **Windows** — the browsers registered under `StartMenuInternet`. A
//!   Firefox has `xul.dll` beside its `.exe`; a Chromium, a `*_elf.dll`
//!   beside it or in its version's folder.
//! * **Linux** — the desktop entries in the `WebBrowser` category, by the
//!   name of what they run.
//!
//! Neither kind takes an extension from outside its store without a step
//! of the user's own, so installing does as much as the browser lets:
//!
//! * **Chromium** — the extension is written, unpacked, to the config
//!   directory, where every Chromium browser can load the same copy. Its
//!   path goes on the clipboard and the browser opens its extensions
//!   page, where Load unpacked (with Developer mode on) takes it.
//! * **Firefox** — the add-on is opened in the browser, which asks
//!   whether to add it. Release builds of Firefox only add signed ones,
//!   so this is the release's signed `VisualHub-firefox.xpi` when there
//!   is one, and otherwise an unsigned copy built here.

use crate::json::Json as _;
use anyhow::{bail, Context as _};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Chromium => "Chromium",
            Family::Firefox => "Firefox",
        }
    }
}

/// A browser on this machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Browser {
    pub name: String,
    pub family: Family,
    launch: Launch,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
enum Launch {
    /// A macOS app bundle, opened with `open -a`.
    App(PathBuf),
    /// An executable, given what to open as its argument.
    Exe(PathBuf),
    /// A desktop entry's command, without its field codes.
    Command(Vec<String>),
}

/// The extension's files, the same for both families.
const FILES: &[(&str, &[u8])] = &[
    (
        "background.js",
        include_bytes!("../extension/background.js"),
    ),
    ("supported.js", include_bytes!("../extension/supported.js")),
    (
        "icons/on-16.png",
        include_bytes!("../extension/icons/on-16.png"),
    ),
    (
        "icons/on-32.png",
        include_bytes!("../extension/icons/on-32.png"),
    ),
    (
        "icons/on-48.png",
        include_bytes!("../extension/icons/on-48.png"),
    ),
    (
        "icons/on-128.png",
        include_bytes!("../extension/icons/on-128.png"),
    ),
    (
        "icons/off-16.png",
        include_bytes!("../extension/icons/off-16.png"),
    ),
    (
        "icons/off-32.png",
        include_bytes!("../extension/icons/off-32.png"),
    ),
    (
        "icons/off-48.png",
        include_bytes!("../extension/icons/off-48.png"),
    ),
    (
        "icons/off-128.png",
        include_bytes!("../extension/icons/off-128.png"),
    ),
];

const CHROMIUM_MANIFEST: &str = include_str!("../extension/manifest.chromium.json");
const FIREFOX_MANIFEST: &str = include_str!("../extension/manifest.firefox.json");

/// The signed add-on a release carries, named the same in
/// `.github/workflows/release.yml`.
pub const SIGNED_XPI: &str = "VisualHub-firefox.xpi";

/// What installing left for the user to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Installed {
    /// Load this folder from the extensions page.
    Unpacked { dir: PathBuf },
    /// Accept the browser's prompt. `signed` is false for a copy built
    /// here, which only browsers that allow unsigned add-ons take.
    AddOn { signed: bool },
}

// ---------------------------------------------------------------------------
// Finding browsers.

/// Tor Browser is built on Firefox, but asks not to be given add-ons:
/// each one makes its users easier to tell apart.
fn is_tor(said: &str) -> bool {
    said.to_lowercase().contains("tor browser") || said.to_lowercase().contains("torbrowser")
}

/// Every Chromium or Firefox browser on this machine, by name.
pub fn detect() -> Vec<Browser> {
    let mut found = platform_browsers();
    found.sort_by_key(|b| b.name.to_lowercase());
    found.dedup_by(|a, b| a.name == b.name);
    found
}

#[cfg(target_os = "macos")]
fn platform_browsers() -> Vec<Browser> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "app"))
        .filter_map(|app| {
            let family = mac_family(&app)?;
            Some(Browser {
                name: app.file_stem()?.to_string_lossy().into_owned(),
                family,
                launch: Launch::App(app),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn mac_family(app: &Path) -> Option<Family> {
    let contents = app.join("Contents");
    if is_tor(&app.to_string_lossy()) {
        return None;
    }
    if contents.join("MacOS/XUL").exists() {
        return Some(Family::Firefox);
    }
    // An Electron app, whatever its framework is called, carries its own
    // code as an archive.
    if contents.join("Resources/app.asar").exists() {
        return None;
    }
    let chromium = std::fs::read_dir(contents.join("Frameworks"))
        .ok()?
        .flatten()
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(" Framework.framework")
                && !name.starts_with("Electron ")
                && !name.starts_with("Chromium Embedded ")
        });
    // Binary plists keep their strings as they are, so this reads either
    // kind; a browser lists `https` among the links it opens.
    let plist = std::fs::read(contents.join("Info.plist")).ok()?;
    let opens_web = plist.windows(5).any(|w| w == b"https");
    (chromium && opens_web).then_some(Family::Chromium)
}

#[cfg(windows)]
fn platform_browsers() -> Vec<Browser> {
    let mut found = Vec::new();
    for root in [
        r"HKCU\SOFTWARE\Clients\StartMenuInternet",
        r"HKLM\SOFTWARE\Clients\StartMenuInternet",
        r"HKLM\SOFTWARE\WOW6432Node\Clients\StartMenuInternet",
    ] {
        let Some(listing) = reg(&["query", root]) else {
            continue;
        };
        for key in listing
            .lines()
            .map(str::trim)
            .filter(|l| l.len() > root.len() && l.to_lowercase().starts_with(&root.to_lowercase()))
        {
            let name = reg(&["query", key, "/ve"]).and_then(|o| reg_value(&o));
            let command = reg(&["query", &format!(r"{key}\shell\open\command"), "/ve"])
                .and_then(|o| reg_value(&o));
            let (Some(name), Some(command)) = (name, command) else {
                continue;
            };
            if is_tor(&name) {
                continue;
            }
            let exe = PathBuf::from(
                command
                    .trim()
                    .trim_matches('"')
                    .split("\" ")
                    .next()
                    .unwrap_or(""),
            );
            let Some(family) = windows_family(&exe) else {
                continue;
            };
            found.push(Browser {
                name,
                family,
                launch: Launch::Exe(exe),
            });
        }
    }
    found
}

#[cfg(windows)]
fn reg(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt as _;
    let out = Command::new("reg")
        .args(args)
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The value in `reg query`'s answer: what follows its type.
#[cfg(windows)]
fn reg_value(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        Some(value.trim().to_string()).filter(|v| !v.is_empty())
    })
}

#[cfg(windows)]
fn windows_family(exe: &Path) -> Option<Family> {
    let dir = exe.parent()?;
    if dir.join("xul.dll").exists() {
        return Some(Family::Firefox);
    }
    let chromium_dll = |dir: &Path| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let name = e.file_name().to_string_lossy().to_lowercase();
                name.ends_with("_elf.dll") || name == "chrome.dll" || name == "opera_browser.dll"
            })
    };
    // Chromium keeps its libraries in a folder named for its version.
    let found = chromium_dll(dir)
        || std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .any(|e| chromium_dll(&e.path()));
    found.then_some(Family::Chromium)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_browsers() -> Vec<Browser> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let mut dirs: Vec<PathBuf> = data_home.iter().map(|d| d.join("applications")).collect();
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .map(|d| Path::new(d).join("applications")),
    );
    dirs.push("/var/lib/flatpak/exports/share/applications".into());
    dirs.push("/var/lib/snapd/desktop/applications".into());
    if let Some(d) = &data_home {
        dirs.push(d.join("flatpak/exports/share/applications"));
    }
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "desktop"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path()).ok()?;
            desktop_browser(&e.file_name().to_string_lossy(), &text)
        })
        .collect()
}

/// The browser a desktop entry starts, when it is one.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn desktop_browser(file: &str, text: &str) -> Option<Browser> {
    let mut name = None;
    let mut exec = None;
    let mut categories = "";
    let mut hidden = false;
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_string()),
            "Exec" => exec = Some(value.trim().to_string()),
            "Categories" => categories = value,
            "NoDisplay" | "Hidden" => hidden |= value.trim() == "true",
            _ => {}
        }
    }
    if hidden || !categories.split(';').any(|c| c == "WebBrowser") {
        return None;
    }
    let exec = exec?;
    let said = format!("{file} {exec}").to_lowercase();
    if is_tor(&said) || is_tor(name.as_deref().unwrap_or("")) {
        return None;
    }
    let family = if [
        "firefox",
        "zen",
        "librewolf",
        "floorp",
        "waterfox",
        "icecat",
    ]
    .iter()
    .any(|k| said.contains(k))
    {
        Family::Firefox
    } else if [
        "chrom", "edge", "brave", "vivaldi", "opera", "helium", "thorium", "comet", "yandex",
    ]
    .iter()
    .any(|k| said.contains(k))
    {
        Family::Chromium
    } else {
        return None;
    };
    let command: Vec<String> = split_command(&exec)
        .into_iter()
        .filter(|arg| !(arg.len() == 2 && arg.starts_with('%')))
        .collect();
    let name = name?;
    (!command.is_empty()).then(|| Browser {
        name,
        family,
        launch: Launch::Command(command),
    })
}

/// A desktop entry's `Exec` split as its spec quotes it.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn split_command(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut arg = String::new();
    let mut quoted = false;
    let mut chars = exec.chars();
    let mut started = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => arg.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut arg));
                    started = false;
                }
            }
            c => {
                arg.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(arg);
    }
    args
}

// ---------------------------------------------------------------------------
// Opening things in a browser.

impl Browser {
    /// Open `what` (a URL or a file) in this browser.
    fn open(&self, what: &str) -> anyhow::Result<()> {
        let mut command = match &self.launch {
            Launch::App(app) => {
                let mut c = Command::new("open");
                c.arg("-a").arg(app);
                c
            }
            Launch::Exe(exe) => Command::new(exe),
            Launch::Command(argv) => {
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            }
        };
        command
            .arg(what)
            .spawn()
            .with_context(|| format!("starting {}", self.name))?;
        Ok(())
    }

    /// The page listing its extensions. Chromium's own name for it works
    /// in every Chromium, which mostly redirect it to theirs.
    pub fn extensions_page(&self) -> &'static str {
        let name = self.name.to_lowercase();
        match self.family {
            Family::Firefox => "about:addons",
            Family::Chromium if name.contains("edge") => "edge://extensions",
            Family::Chromium if name.contains("opera") => "opera://extensions",
            Family::Chromium => "chrome://extensions",
        }
    }
}

// ---------------------------------------------------------------------------
// Writing the extension.

fn extension_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::api::config_dir()
        .context("no config directory")?
        .join("browser-extension"))
}

/// Where the unpacked Chromium extension lives. It stays put, since the
/// browser loads it from there every time it starts.
pub fn chromium_dir() -> anyhow::Result<PathBuf> {
    Ok(extension_dir()?.join("chromium"))
}

/// The manifest for `family`, at this version.
fn manifest(family: Family) -> String {
    let template = match family {
        Family::Chromium => CHROMIUM_MANIFEST,
        Family::Firefox => FIREFOX_MANIFEST,
    };
    template.replace("@VERSION@", crate::update::current_version())
}

/// The self-managed GitLabs signed in to (`webs`, as `https://host`),
/// which the extension knows before it has seen a page of theirs.
fn hosts_js(webs: &[String]) -> String {
    let sites: Vec<serde_json::Value> = webs
        .iter()
        .map(|root| serde_json::json!({ "root": root, "forge": "gitlab" }))
        .collect();
    format!(
        "// The self-hosted GitLabs VisualHub was signed in to when it wrote\n\
         // this copy.\n\
         globalThis.VISUALHUB_SITES = {};\n",
        serde_json::to_string(&sites).unwrap_or_else(|_| "[]".into())
    )
}

/// The extension's files for `family`, with `webs` among its hosts.
fn files(family: Family, webs: &[String]) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = FILES
        .iter()
        .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
        .collect();
    files.push(("hosts.js".into(), hosts_js(webs).into_bytes()));
    files.push(("manifest.json".into(), manifest(family).into_bytes()));
    files
}

/// Write the unpacked Chromium extension, replacing any older copy.
/// Returns whether anything changed.
pub fn write_chromium(webs: &[String]) -> anyhow::Result<bool> {
    let dir = chromium_dir()?;
    let mut changed = false;
    for (name, bytes) in files(Family::Chromium, webs) {
        let path = dir.join(&name);
        if std::fs::read(&path).ok().as_deref() == Some(&bytes[..]) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        changed = true;
    }
    Ok(changed)
}

/// Keep an unpacked copy that was installed before in step with this
/// version and these hosts. The browser picks it up when it next starts.
pub fn refresh_installed(webs: &[String]) {
    if chromium_dir().is_ok_and(|d| d.join("manifest.json").exists()) {
        if let Err(e) = write_chromium(webs) {
            log::warn!("refreshing the browser extension: {e:#}");
        }
    }
}

/// Where the unsigned add-on is built.
pub fn unsigned_xpi_path() -> anyhow::Result<PathBuf> {
    Ok(extension_dir()?.join("visualhub-unsigned.xpi"))
}

/// The add-on, unsigned, built here.
fn unsigned_xpi(webs: &[String]) -> anyhow::Result<PathBuf> {
    let path = unsigned_xpi_path()?;
    std::fs::create_dir_all(extension_dir()?)?;
    std::fs::write(&path, zip(&files(Family::Firefox, webs)))?;
    Ok(path)
}

/// The release's signed add-on for this version, downloaded once.
fn signed_xpi() -> anyhow::Result<PathBuf> {
    let version = crate::update::current_version();
    let path = extension_dir()?.join(format!("visualhub-{version}.xpi"));
    if path.exists() {
        return Ok(path);
    }
    let release = format!(
        "https://api.github.com/repos/{}/releases/tags/v{version}",
        crate::update::RELEASES_REPO
    );
    let text = ureq::get(&release)
        .header("User-Agent", "visualhub-extension")
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_to_string()?;
    let release: serde_json::Value = serde_json::from_str(&text)?;
    let Some(asset) = release
        .list("assets")
        .into_iter()
        .find(|a| a.s("name") == SIGNED_XPI)
    else {
        bail!("release v{version} has no {SIGNED_XPI}");
    };
    let mut bytes = Vec::new();
    ureq::get(&asset.s("browser_download_url"))
        .header("User-Agent", "visualhub-extension")
        .call()?
        .into_body()
        .into_reader()
        .take(16 << 20)
        .read_to_end(&mut bytes)?;
    // A zip's first bytes; anything else is an error page.
    anyhow::ensure!(
        bytes.starts_with(b"PK\x03\x04"),
        "{SIGNED_XPI} isn't an add-on"
    );
    std::fs::create_dir_all(extension_dir()?)?;
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Put the extension in front of `browser`. Blocking — call it off the
/// UI thread. `webs` are the self-managed GitLab instances signed in to.
pub fn install(browser: &Browser, webs: &[String]) -> anyhow::Result<Installed> {
    if let Err(e) = crate::link::register() {
        log::warn!("registering visualhub:// links: {e:#}");
    }
    match browser.family {
        Family::Chromium => {
            write_chromium(webs)?;
            browser.open(browser.extensions_page())?;
            Ok(Installed::Unpacked {
                dir: chromium_dir()?,
            })
        }
        Family::Firefox => {
            let (xpi, signed) = match signed_xpi() {
                Ok(path) => (path, true),
                Err(e) => {
                    log::info!("no signed add-on, building one: {e:#}");
                    (unsigned_xpi(webs)?, false)
                }
            };
            browser.open(&xpi.to_string_lossy())?;
            Ok(Installed::AddOn { signed })
        }
    }
}

/// `entries` as a zip, stored rather than compressed: the add-on is a
/// few small files, mostly PNGs that wouldn't shrink anyway.
fn zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let size = data.len() as u32;
        // Local header: version 2.0, no flags, stored, no timestamp.
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&0x21u16.to_le_bytes()); // 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&[0; 2]);
        central.extend_from_slice(&[0; 2]);
        central.extend_from_slice(&[0; 2]);
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 2]); // extra
        central.extend_from_slice(&[0; 2]); // comment
        central.extend_from_slice(&[0; 2]); // disk
        central.extend_from_slice(&[0; 2]); // internal attributes
        central.extend_from_slice(&[0; 4]); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_carry_the_version_and_hosts() {
        for family in [Family::Chromium, Family::Firefox] {
            let m: serde_json::Value = serde_json::from_str(&manifest(family)).unwrap();
            assert_eq!(m["version"], crate::update::current_version());
            // Every site, to tell a self-hosted forge by its page.
            assert_eq!(m["host_permissions"], serde_json::json!(["*://*/*"]));
        }
        let js = hosts_js(&["http://git.example.com:8080".into()]);
        assert!(
            js.contains(r#""root":"http://git.example.com:8080""#),
            "{js}"
        );
        assert!(js.contains(r#""forge":"gitlab""#), "{js}");
    }

    #[test]
    fn every_file_the_manifests_name_is_shipped() {
        let shipped: Vec<String> = files(Family::Firefox, &[])
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for template in [CHROMIUM_MANIFEST, FIREFOX_MANIFEST] {
            let m: serde_json::Value = serde_json::from_str(template).unwrap();
            let mut named: Vec<String> = Vec::new();
            for key in ["icons", "action.default_icon"] {
                let mut v = &m;
                for part in key.split('.') {
                    v = &v[part];
                }
                named.extend(
                    v.as_object()
                        .unwrap()
                        .values()
                        .map(|p| p.as_str().unwrap().into()),
                );
            }
            named.extend(
                m["background"]["scripts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|s| s.as_str().unwrap().into()),
            );
            named.extend(m["background"]["service_worker"].as_str().map(String::from));
            for name in named {
                assert!(shipped.contains(&name), "{name} is missing");
            }
        }
    }

    #[test]
    fn desktop_entries_name_browsers() {
        let zen = desktop_browser(
            "app.zen_browser.zen.desktop",
            "[Desktop Entry]\nName=Zen\nExec=/usr/bin/flatpak run --branch=stable app.zen_browser.zen %u\n\
             Categories=Network;WebBrowser;\n[Desktop Action new-window]\nName=New Window\n",
        )
        .unwrap();
        assert_eq!(zen.name, "Zen");
        assert_eq!(zen.family, Family::Firefox);
        assert_eq!(
            zen.launch,
            Launch::Command(vec![
                "/usr/bin/flatpak".into(),
                "run".into(),
                "--branch=stable".into(),
                "app.zen_browser.zen".into()
            ])
        );
        let helium = desktop_browser(
            "helium.desktop",
            "[Desktop Entry]\nName=Helium\nExec=\"/opt/helium/helium\" %U\nCategories=WebBrowser;\n",
        )
        .unwrap();
        assert_eq!(helium.family, Family::Chromium);
        assert_eq!(
            helium.launch,
            Launch::Command(vec!["/opt/helium/helium".into()])
        );
        assert!(desktop_browser(
            "editor.desktop",
            "[Desktop Entry]\nName=Editor\nExec=editor %f\nCategories=Development;\n"
        )
        .is_none());
    }

    #[test]
    fn the_add_on_is_a_zip() {
        let bytes = zip(&[("a.txt".into(), b"hello".to_vec())]);
        assert!(bytes.starts_with(b"PK\x03\x04"));
        assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], b"PK\x05\x06");
    }
}
