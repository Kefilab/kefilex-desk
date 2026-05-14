// src-tauri/src/lib.rs
//
// Library entry — wired up to main.rs's main() and to the Tauri
// runtime. Splits the responsibilities into modules so each piece
// has one job:
//
//   api               HTTP client for pair / heartbeat / call-event
//   config            persists the pairing token via tauri-plugin-store
//                     (Windows: backed by Credential Manager via DPAPI)
//   voip_filters      built-in pattern registry for known softphones
//   notification_listener
//                     Windows-only OS notification subscriber. On
//                     other platforms this is a stub that does nothing.
//   tray              system tray icon + menu
//   heartbeat         60-second loop posting to /api/desk-companion/heartbeat

mod api;
mod config;
mod heartbeat;
mod notification_listener;
mod tray;
mod voip_filters;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

/// Runtime context shared between the Tauri tray, notification
/// listener, and heartbeat loop. The pairing token can change at
/// runtime (user pairs / repairs) so it lives behind RwLock.
#[derive(Clone)]
pub struct AppContext {
    pub api_base: String,
    pub config: Arc<RwLock<config::PairingConfig>>,
    pub http: reqwest::Client,
}

impl AppContext {
    pub fn new(api_base: String, config: config::PairingConfig) -> Self {
        Self {
            api_base,
            config: Arc::new(RwLock::new(config)),
            http: reqwest::Client::builder()
                .user_agent(format!(
                    "KefilexDesk/{} ({})",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS
                ))
                .build()
                .expect("reqwest client init"),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    // Production points at app.kefilex.com. Override via env when
    // running against a local dev backend.
    let api_base =
        std::env::var("KEFILEX_DESK_API").unwrap_or_else(|_| "https://app.kefilex.com".to_string());
    log::info!("Kefilex Desk starting — API base: {}", api_base);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_pairing_status,
            commands::submit_pairing_code,
            commands::clear_pairing,
            commands::ping_backend,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Load the persisted pairing config from the OS-secured
            // store. First-launch returns an unpaired config.
            let initial_config = config::load_or_default(&app_handle).unwrap_or_else(|err| {
                log::error!("Failed to load config: {}. Starting unpaired.", err);
                config::PairingConfig::default()
            });
            let ctx = AppContext::new(api_base.clone(), initial_config);
            app.manage(ctx.clone());

            // Set up the system tray. On click → show window.
            tray::setup_tray(&app_handle)?;

            // Kick off the background work threads.
            let ctx_for_hb = ctx.clone();
            tauri::async_runtime::spawn(async move {
                heartbeat::run_loop(ctx_for_hb).await;
            });

            // Notification listener: starts up the Windows
            // UserNotificationListener subscription. windows-rs COM
            // event handlers contain NonNull<c_void> which is !Send,
            // so the listener can't run on Tauri's async runtime
            // (work-stealing executor moves tasks between threads).
            // It gets its own dedicated OS thread instead. We pass
            // a Tokio handle in so per-notification work (the HTTP
            // push to /api/desk-companion/call-event) can still be
            // spawned on the async pool.
            //
            // We grab Handle::current() from INSIDE an async task
            // because the Tauri/Tokio runtime is not active on the
            // main thread at setup-closure-call time — only on the
            // worker threads it spawns. The async task is cheap: it
            // does one std::thread::spawn then exits, while the OS
            // thread it spawned inherits the Tokio handle.
            let ctx_for_nl = ctx.clone();
            tauri::async_runtime::spawn(async move {
                let rt = tokio::runtime::Handle::current();
                if let Err(err) = std::thread::Builder::new()
                    .name("kefilex-desk-notif".into())
                    .spawn(move || {
                        notification_listener::run_blocking(ctx_for_nl, rt);
                    })
                {
                    log::error!("failed to spawn notification listener thread: {}", err);
                }
            });

            // First-launch UX: if there's no pairing token, pop the
            // window immediately so the user enters the 6-digit code.
            let cfg = ctx.config.clone();
            tauri::async_runtime::spawn(async move {
                if cfg.read().await.token.is_none() {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running Tauri application");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            // Keep the app alive in the tray when all windows close.
            // User must explicitly choose Quit from the tray menu.
            api.prevent_exit();
        }
    });
}

/// Tauri commands invoked from the front-end WebView (pairing UI).
mod commands {
    use super::AppContext;
    use serde::Serialize;
    use tauri::State;

    #[derive(Serialize)]
    pub struct PairingStatus {
        pub paired: bool,
        pub device_label: Option<String>,
        pub device_id: Option<String>,
    }

    #[tauri::command]
    pub async fn get_pairing_status(ctx: State<'_, AppContext>) -> Result<PairingStatus, String> {
        let cfg = ctx.config.read().await;
        Ok(PairingStatus {
            paired: cfg.token.is_some(),
            device_label: cfg.device_label.clone(),
            device_id: cfg.device_id.clone(),
        })
    }

    #[tauri::command]
    pub async fn submit_pairing_code(
        ctx: State<'_, AppContext>,
        app_handle: tauri::AppHandle,
        code: String,
        device_label: String,
    ) -> Result<(), String> {
        use crate::{api, config};

        let result = api::pair(&ctx.api_base, &ctx.http, &code, &device_label)
            .await
            .map_err(|e| e.to_string())?;

        let new_config = config::PairingConfig {
            token: Some(result.token),
            device_id: Some(result.device_id.clone()),
            device_label: Some(device_label),
        };
        config::save(&app_handle, &new_config).map_err(|e| e.to_string())?;
        *ctx.config.write().await = new_config;

        log::info!("Successfully paired. device_id={}", result.device_id);
        Ok(())
    }

    #[tauri::command]
    pub async fn clear_pairing(
        ctx: State<'_, AppContext>,
        app_handle: tauri::AppHandle,
    ) -> Result<(), String> {
        use crate::config;
        let new_config = config::PairingConfig::default();
        config::save(&app_handle, &new_config).map_err(|e| e.to_string())?;
        *ctx.config.write().await = new_config;
        Ok(())
    }

    #[tauri::command]
    pub async fn ping_backend(ctx: State<'_, AppContext>) -> Result<bool, String> {
        let cfg = ctx.config.read().await;
        let Some(token) = cfg.token.clone() else {
            return Ok(false);
        };
        drop(cfg);
        crate::api::heartbeat(&ctx.api_base, &ctx.http, &token)
            .await
            .map(|_| true)
            .map_err(|e| e.to_string())
    }
}
