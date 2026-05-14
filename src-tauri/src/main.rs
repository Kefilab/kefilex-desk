// src-tauri/src/main.rs
//
// Kefilex Desk — entry point.
//
// What this binary does:
//
//   1. Loads the persisted pairing config from the OS-secured store.
//   2. Sets up a Tauri tray icon (the K in the Windows system tray).
//   3. On Windows: subscribes to UserNotificationListener and pipes
//      matching incoming-call toasts to the Kefilex backend.
//   4. Sends a heartbeat to /api/desk-companion/heartbeat every 60s
//      while paired.
//   5. Pops the pairing window if no token is stored yet.
//
// What it doesn't do:
//
//   - Handle audio. This is not a softphone — VXT (or whatever the
//     firm uses) keeps that responsibility.
//   - Read non-call notifications. The VoIP filter registry in
//     src/voip_filters.rs is the strict pattern-allowlist.
//
// Cross-platform notes:
//
//   - The notification listener code in src/notification_listener.rs
//     is cfg-gated for Windows. On macOS / Linux it stubs out so the
//     project still compiles for dev work on a Mac.
//   - The HTTP client, config store, and tray UI work everywhere.

// Stop the Windows console window from popping up alongside the GUI
// in release builds. macOS / Linux ignore this attribute.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kefilex_desk_lib::run()
}
