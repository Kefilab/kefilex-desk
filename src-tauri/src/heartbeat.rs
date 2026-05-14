// src-tauri/src/heartbeat.rs
//
// Background loop that POSTs to /api/desk-companion/heartbeat every
// 60 seconds while a pairing token exists. Updates server-side
// last_seen_at so the Reception UI knows we're alive.
//
// Reconnect strategy: simple — if a heartbeat fails (network blip,
// 5xx), we log and try again on the next interval. No exponential
// backoff needed; 60-second intervals are forgiving enough.
//
// If a heartbeat returns 401, the token has been revoked. We clear
// local config and the next launch shows the pairing window.

use crate::AppContext;
use std::time::Duration;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

pub async fn run_loop(ctx: AppContext) {
    log::info!("heartbeat loop starting");
    loop {
        // Take the token out of the lock quickly so we don't hold
        // it across the network call.
        let token = {
            let cfg = ctx.config.read().await;
            cfg.token.clone()
        };

        if let Some(token) = token {
            match crate::api::heartbeat(&ctx.api_base, &ctx.http, &token).await {
                Ok(resp) => {
                    log::debug!("heartbeat ok, server_time={}", resp.server_time);
                }
                Err(err) => {
                    let msg = err.to_string();
                    if msg.contains("returned 401") {
                        log::warn!("heartbeat got 401 — token revoked, clearing config");
                        let mut cfg = ctx.config.write().await;
                        cfg.token = None;
                    } else {
                        log::warn!("heartbeat failed: {}", msg);
                    }
                }
            }
        }
        // No token = unpaired; just sleep and try again later.
        // (Pairing flips token to Some, next loop wakes up paired.)

        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
}
