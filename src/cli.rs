//! Resolve command line tools from the user's login shell. Packaged GUI
//! apps inherit a minimal desktop PATH, which often omits Homebrew paths.

use std::path::PathBuf;
use std::sync::OnceLock;

pub fn gh() -> PathBuf {
    static GH: OnceLock<PathBuf> = OnceLock::new();
    GH.get_or_init(|| login_shell_lookup("gh")).clone()
}

/// Where the login shell finds `name`, or the bare name for the PATH to
/// settle.
fn login_shell_lookup(name: &str) -> PathBuf {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    std::process::Command::new(shell)
        .args(["-l", "-c", &format!("command -v {name}")])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            let path = String::from_utf8(out.stdout).ok()?.trim().to_owned();
            (!path.is_empty()).then(|| PathBuf::from(path))
        })
        .unwrap_or_else(|| PathBuf::from(name))
}

pub fn glab() -> PathBuf {
    static GLAB: OnceLock<PathBuf> = OnceLock::new();
    GLAB.get_or_init(|| login_shell_lookup("glab")).clone()
}
