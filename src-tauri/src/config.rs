// src-tauri/src/config.rs
//
// Persists the pairing token + device metadata using
// tauri-plugin-store, which on Windows is backed by Credential
// Manager (DPAPI-encrypted, scoped to the current Windows user).
//
// The store file lives in the platform's standard config dir:
//   Windows: %APPDATA%\com.kefilab.desk\store\config.json
//   macOS:   ~/Library/Application Support/com.kefilab.desk/store/config.json
//   Linux:   ~/.config/com.kefilab.desk/store/config.json
//
// The token itself is encrypted at rest by the underlying tauri
// store implementation. On Windows that means the token never
// appears in plaintext on disk.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_store::StoreExt;

const STORE_FILE: &str = "config.json";
const TOKEN_KEY: &str = "pairing.token";
const DEVICE_ID_KEY: &str = "pairing.device_id";
const DEVICE_LABEL_KEY: &str = "pairing.device_label";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairingConfig {
    pub token: Option<String>,
    pub device_id: Option<String>,
    pub device_label: Option<String>,
}

/// Load the existing pairing config from the OS-secured store.
/// First-launch returns `Default` (unpaired).
pub fn load_or_default(app: &AppHandle<Wry>) -> Result<PairingConfig> {
    let store = app
        .store(STORE_FILE)
        .context("failed to open config store")?;

    let token = store.get(TOKEN_KEY).and_then(|v| v.as_str().map(String::from));
    let device_id = store
        .get(DEVICE_ID_KEY)
        .and_then(|v| v.as_str().map(String::from));
    let device_label = store
        .get(DEVICE_LABEL_KEY)
        .and_then(|v| v.as_str().map(String::from));

    Ok(PairingConfig {
        token,
        device_id,
        device_label,
    })
}

/// Save the pairing config. Overwrites any previous values; clearing
/// pairing sets each value to None (removed from the store).
pub fn save(app: &AppHandle<Wry>, cfg: &PairingConfig) -> Result<()> {
    let store = app
        .store(STORE_FILE)
        .context("failed to open config store")?;

    match &cfg.token {
        Some(v) => store.set(TOKEN_KEY, serde_json::json!(v)),
        None => {
            store.delete(TOKEN_KEY);
        }
    }
    match &cfg.device_id {
        Some(v) => store.set(DEVICE_ID_KEY, serde_json::json!(v)),
        None => {
            store.delete(DEVICE_ID_KEY);
        }
    }
    match &cfg.device_label {
        Some(v) => store.set(DEVICE_LABEL_KEY, serde_json::json!(v)),
        None => {
            store.delete(DEVICE_LABEL_KEY);
        }
    }

    store.save().context("failed to save config store")?;
    Ok(())
}
