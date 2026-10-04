//! VisualHub: a native GitHub client on GPUI, drawn with a widget kit
//! copied from Schist. Everything GitHub's web UI does except editing code.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod assets;
mod autoscroll;
mod diff;
mod form;
mod highlight;
mod hub;
mod json;
mod markdown;
mod picker;
mod resource;
mod scopes;
mod screens;
mod shell;
mod time;
mod ui;
mod widgets;

use gpui::{
    px, size, App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds,
    WindowOptions,
};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let app = Application::new().with_assets(assets::Assets);
    app.run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("VisualHub".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(900.0), px(560.0))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| hub::Hub::new(window, cx)),
        )
        .expect("failed to open the window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
