// src-tauri/src/notification_listener.rs
//
// Watches OS notifications, pattern-matches them against the VoIP
// filter registry, and reports incoming calls to Kefilex.
//
// Implementation is cfg-gated:
//
//   target_os = "windows" → polls UserNotificationListener
//   everything else       → no-op stub so the project still compiles
//                           on macOS/Linux during development
//
// On Windows the listener has TWO read modes:
//
//   1. NotificationChanged event subscription — REQUIRES MSIX/UWP
//      packaging. Classic Win32 apps (which is what Tauri produces
//      by default) cannot register for this event; the call returns
//      HRESULT 0x80070490 "Element not found".
//
//   2. GetNotificationsAsync polling — works on unpackaged Win32.
//      Slightly higher latency (poll interval = 500ms ≈ worst-case
//      delay before we detect a new toast) but reliable. Same data,
//      same API, just a different read pattern.
//
// We use mode 2. If we later ship as MSIX, we can swap to the
// event subscription for sub-100ms detection.
//
// On non-Windows we don't have a cross-app notification API at all.
// macOS Notification Center observation requires Accessibility-tier
// permission and undocumented APIs; out of scope for v1.

use crate::{api, AppContext};
use chrono::SecondsFormat;
use std::time::Duration;

/// Entry point for the notification listener. Runs synchronously on
/// its own OS thread — windows-rs COM types are not Send so they
/// can't ride on Tokio's worker-stealing async runtime. We take a
/// tokio::runtime::Handle so per-notification work (the HTTP push)
/// can still be spawned onto the async pool.
pub fn run_blocking(ctx: AppContext, rt: tokio::runtime::Handle) {
    log::info!(
        "notification listener starting (platform: {})",
        std::env::consts::OS
    );
    #[cfg(target_os = "windows")]
    {
        windows_impl::run_loop(ctx, rt);
    }
    #[cfg(not(target_os = "windows"))]
    {
        // On non-Windows we don't have a cross-app notification API.
        // Park the thread so it doesn't churn the scheduler.
        let _ = ctx;
        let _ = rt;
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

/// Build a CallEvent for the Kefilex API from a matched notification.
/// The `event_type` (ringing / missed / voicemail / etc.) comes from
/// the matched filter rule — different softphones report different
/// lifecycle moments and we want each to map to the right calls
/// table status on the server side.
pub fn build_call_event(
    filter_match: &crate::voip_filters::FilterMatch,
    notification_text: &str,
) -> api::CallEvent<'static> {
    let now = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    api::CallEvent {
        event_type: filter_match.event_type,
        caller_phone_e164: filter_match.caller_phone_e164.clone(),
        firm_phone_e164: None,
        source_app: Some(filter_match.source_app.to_string()),
        started_at: now,
        caller_display_name: filter_match.caller_display_name.clone(),
        intended_fee_earner_hint: None,
        vxt_call_id: None,
        metadata: serde_json::json!({
            "notification_text": notification_text,
        }),
    }
}

// ─── Windows implementation ──────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use std::collections::HashSet;
    use windows::UI::Notifications::{
        KnownNotificationBindings,
        Management::{UserNotificationListener, UserNotificationListenerAccessStatus},
        NotificationKinds, UserNotification,
    };

    /// How often we ask the OS for the current toast list. 500ms
    /// gives 0-500ms worst-case latency on call detection — fast
    /// enough that reception staff see the live toast pop "as the
    /// phone rings" in practice, without burning CPU.
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    pub fn run_loop(ctx: AppContext, rt: tokio::runtime::Handle) {
        // Request access. The first time this runs on a machine,
        // Windows pops a permission prompt. Subsequent runs return
        // the cached answer.
        let listener = match UserNotificationListener::Current() {
            Ok(l) => l,
            Err(err) => {
                log::error!(
                    "UserNotificationListener::Current() failed: {:?} — notification listening disabled",
                    err
                );
                return;
            }
        };
        // RequestAccessAsync returns an IAsyncOperation. In
        // windows-rs 0.58 without the futures bridge, IAsyncOperation
        // isn't a Future — call .get() to block on completion. Fine
        // because we're at one-time startup, not a hot path.
        let access_op = match listener.RequestAccessAsync() {
            Ok(op) => op,
            Err(err) => {
                log::error!("RequestAccessAsync invocation failed: {:?}", err);
                return;
            }
        };
        match access_op.get() {
            Ok(access_status) => {
                if access_status != UserNotificationListenerAccessStatus::Allowed {
                    log::warn!(
                        "Notification access not granted (status={:?}). \
                         Ask the user to enable it in Settings → Privacy → Notifications.",
                        access_status
                    );
                    return;
                }
            }
            Err(err) => {
                log::error!("RequestAccessAsync.get() failed: {:?}", err);
                return;
            }
        }

        log::info!(
            "notification access granted; polling every {:?}",
            POLL_INTERVAL
        );

        // Polling loop. Maintains a "seen" set of notification IDs.
        // First pass primes the set without dispatching so existing
        // toasts already on screen don't all fire as fresh calls.
        // After that, any unseen id is treated as a new notification.
        //
        // UserNotification.Id() is a u32 issued by the OS, typically
        // monotonically increasing per session — but we track via a
        // HashSet rather than a single high-water mark to handle the
        // edge case where Windows recycles ids between sessions.
        let mut seen_ids: HashSet<u32> = HashSet::new();
        let mut primed = false;

        loop {
            if let Err(err) = poll_once(&listener, &mut seen_ids, primed, &ctx, &rt) {
                log::warn!("notification poll failed: {:?}", err);
            }
            primed = true;
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Pull the current list of toast notifications, diff against
    /// the seen-set, dispatch any newcomers. On first pass `primed`
    /// is false so we just populate the set without firing events.
    fn poll_once(
        listener: &UserNotificationListener,
        seen_ids: &mut HashSet<u32>,
        primed: bool,
        ctx: &AppContext,
        rt: &tokio::runtime::Handle,
    ) -> windows::core::Result<()> {
        let async_op = listener.GetNotificationsAsync(NotificationKinds::Toast)?;
        let notifications = async_op.get()?;
        let count = notifications.Size()?;

        for i in 0..count {
            let n = notifications.GetAt(i)?;
            let id = n.Id()?;
            if !seen_ids.insert(id) {
                // Already seen — HashSet::insert returned false.
                continue;
            }
            if !primed {
                // First pass — record but don't dispatch. Avoids a
                // flurry of "incoming call" events when the user
                // starts the companion while a bunch of toasts are
                // already on screen.
                continue;
            }
            // New notification — process it (errors don't stop the
            // loop; we just log and move on).
            if let Err(err) = handle_notification(&n, ctx, rt) {
                log::warn!("handle_notification failed: {:?}", err);
            }
        }
        Ok(())
    }

    /// Read the notification's source app + text, pattern-match
    /// against the filter registry, and POST a call event if
    /// something matches.
    fn handle_notification(
        n: &UserNotification,
        ctx: &AppContext,
        rt: &tokio::runtime::Handle,
    ) -> windows::core::Result<()> {
        let app_info = n.AppInfo()?;
        let app_id = app_info.AppUserModelId()?.to_string();

        let toast = n.Notification()?;
        let visual = toast.Visual()?;
        let binding = visual.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
        let texts = binding.GetTextElements()?;

        let mut title = String::new();
        let mut body = String::new();
        for (i, t) in texts.into_iter().enumerate() {
            let text = t.Text()?.to_string();
            match i {
                0 => title = text,
                _ => {
                    if !body.is_empty() {
                        body.push(' ');
                    }
                    body.push_str(&text);
                }
            }
        }

        let combined = format!("{} | {}", title, body);
        log::debug!("notification from {}: {}", app_id, combined);

        let Some(filter_match) =
            crate::voip_filters::match_notification(&app_id, &title, &body)
        else {
            return Ok(());
        };

        log::info!(
            "matched VoIP notification: app={} source={} phone={:?} name={:?}",
            app_id,
            filter_match.source_app,
            filter_match.caller_phone_e164,
            filter_match.caller_display_name
        );

        // Push HTTP work onto the Tokio runtime. The clones are
        // cheap (Arcs and small structs) and we move into the spawned
        // task so the COM thread isn't held by the network call.
        let ctx_clone = ctx.clone();
        let event = build_call_event(&filter_match, &combined);
        rt.spawn(async move {
            let token = {
                let cfg = ctx_clone.config.read().await;
                cfg.token.clone()
            };
            let Some(token) = token else {
                log::warn!("matched a call but not paired — ignoring");
                return;
            };
            match api::call_event(&ctx_clone.api_base, &ctx_clone.http, &token, &event).await {
                Ok(resp) => {
                    log::info!("posted call event: call_id={}", resp.call_id);
                }
                Err(err) => {
                    log::warn!("posting call event failed: {:?}", err);
                }
            }
        });
        Ok(())
    }
}
