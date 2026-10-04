//! Resolve command line tools from the user's login shell. Packaged GUI
//! apps inherit a minimal desktop PATH, which often omits Homebrew paths.

use std::path::PathBuf;
use std::sync::OnceLock;

pub fn gh() -> PathBuf {
    static GH: OnceLock<PathBuf> = OnceLock::new();
    GH.get_or_init(|| {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        std::process::Command::new(shell)
            .args(["-l", "-c", "command -v gh"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                let path = String::from_utf8(out.stdout).ok()?.trim().to_owned();
                (!path.is_empty()).then(|| PathBuf::from(path))
            })
            .unwrap_or_else(|| PathBuf::from("gh"))
    })
    .clone()
}
