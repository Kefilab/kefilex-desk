// src-tauri/src/tray.rs
//
// System tray icon + menu. The user's main interaction with the
// app is via the tray; the only window we ever show is the pairing
// flow (and only on first launch or after revocation).

use anyhow::{Context, Result};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

pub fn setup_tray(app: &AppHandle<Wry>) -> Result<()> {
    let menu = build_menu(app)?;

    TrayIconBuilder::with_id("kefilex-desk-tray")
        .tooltip("Kefilex Desk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id().as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click: open the main window (pairing / status).
            // Right-click is handled by the menu attached above.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)
        .context("failed to build tray icon")?;
    Ok(())
}

fn build_menu(app: &AppHandle<Wry>) -> Result<Menu<Wry>> {
    let open = MenuItem::with_id(app, "open", "Open Kefilex Desk", true, None::<&str>)?;
    let open_logs = MenuItem::with_id(app, "open_logs", "Open log file", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let about = MenuItem::with_id(app, "about", "About Kefilex Desk…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open, &open_logs, &sep, &about, &quit])?;
    Ok(menu)
}

fn handle_menu_event(app: &AppHandle<Wry>, id: &str) {
    match id {
        "open" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "open_logs" => {
            // Logs land in the platform's standard log directory.
            // For Phase 31b we just open the file in the system editor.
            if let Some(path) = log_file_path(app) {
                let _ = open_file_in_system_handler(&path);
            }
        }
        "about" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("window.location.hash = '#about'");
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn log_file_path(app: &AppHandle<Wry>) -> Option<std::path::PathBuf> {
    // env_logger writes to stderr. For a real release we'll wire up
    // file logging via fern or tracing-appender — out of scope for
    // the 31b initial scaffold but tracked as a follow-on.
    let _ = app;
    None
}

#[cfg(target_os = "windows")]
fn open_file_in_system_handler(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_file_in_system_handler(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "linux")]
fn open_file_in_system_handler(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}
