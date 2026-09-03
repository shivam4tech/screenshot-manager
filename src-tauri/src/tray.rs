//! System tray: quick access to Show, Scan now, and Quit.
//!
//! Left-click shows and focuses the library window; the menu offers the same
//! plus a background scan trigger and quit. The tray never mutates files —
//! "Scan now" just starts the regular non-destructive scan thread.

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

/// Build the tray icon. Call once from `setup`.
pub fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Library", true, None::<&str>)?;
    let scan = MenuItem::with_id(app, "scan", "Scan now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &scan, &quit])?;

    // Prefer the bundled window icon; if packaging ever drops it, skip the
    // tray instead of crashing startup — the app works fine without one.
    let Some(icon) = app.default_window_icon().cloned() else {
        log::warn!("no window icon available; starting without a tray icon");
        return Ok(());
    };

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Screenshot Memory")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "scan" => {
                show_main(app);
                let state = app.state::<crate::AppState>();
                if let Err(e) = crate::commands::start_scan(app.clone(), state) {
                    log::warn!("tray scan request refused: {e}");
                }
            }
            "quit" => app.exit(0),
            other => log::debug!("unhandled tray menu item: {other}"),
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
