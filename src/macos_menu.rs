//! Native macOS menus and the key bindings whose equivalents they display.

use gpui::{actions, App, KeyBinding, Menu, MenuItem, SystemMenuType};

actions!(
    visualhub,
    [
        QuitApp,
        HideApp,
        HideOthers,
        ShowAll,
        Refresh,
        Search,
        CopySelection,
        CutSelection,
        Paste,
        SelectAll
    ]
);

pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", QuitApp, None),
        KeyBinding::new("cmd-r", Refresh, Some("VisualHub")),
        KeyBinding::new("cmd-k", Search, Some("VisualHub")),
        KeyBinding::new("cmd-c", CopySelection, Some("VisualHub")),
        KeyBinding::new("cmd-x", CutSelection, Some("VisualHub")),
        KeyBinding::new("cmd-v", Paste, Some("VisualHub")),
        KeyBinding::new("cmd-a", SelectAll, Some("VisualHub")),
    ]);
    cx.on_action(|_: &QuitApp, cx| cx.quit());
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

    cx.set_menus(vec![
        Menu {
            name: "VisualHub".into(),
            items: vec![
                MenuItem::action("Hide VisualHub", HideApp),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit VisualHub", QuitApp),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Copy", CopySelection),
                MenuItem::action("Cut", CutSelection),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
            ],
        },
    ]);
}
