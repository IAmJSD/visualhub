//! Embed every SVG under `assets/icons` so the binary carries its own
//! icon set: the asset source is a table from `icons/<name>.svg` to the
//! file's bytes, generated here so adding an icon is just adding a file.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    // On Windows the executable carries the app icon, so Explorer, the
    // taskbar and shortcuts show it rather than a blank one.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=packaging/windows/visualhub.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/visualhub.ico");
        res.compile().expect("embedding the Windows icon");
    }

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icons = root.join("assets").join("icons");
    println!("cargo:rerun-if-changed={}", icons.display());

    let mut names: Vec<String> = fs::read_dir(&icons)
        .expect("assets/icons is missing")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".svg"))
        .collect();
    names.sort();

    let mut table = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for name in names {
        let path = icons.join(&name);
        writeln!(
            table,
            "    (\"icons/{name}\", include_bytes!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    table.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("icons.rs");
    fs::write(out, table).unwrap();
}
