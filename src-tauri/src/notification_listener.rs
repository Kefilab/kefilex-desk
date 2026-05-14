// src-tauri/src/notification_listener.rs
//
// Watches OS notifications, pattern-matches them against the VoIP
// filter registry, and reports incoming calls to Kefilex.
//
// Implementation is cfg-gated:
//
//   target_os = "windows" → real UserNotificationListener subscription
//   everything else       → no-op stub so the project still compiles
//                           on macOS/Linux during development
//
// The Windows implementation needs Accessibility-tier permission
// (Settings → Privacy → Notifications → "Let apps access your
// notifications"). UserNotificationListener::Current().RequestAccessAsync()
// triggers the OS prompt the first time, and the user grants or
// denies once.

use crate::{api, AppContext};
use chrono::SecondsFormat;
use std::time::Duration;

/// Entry point for the notification listener. Runs synchronously on
/// its own OS thread — windows-rs COM event handlers are not Send so
/// they can't ride on Tokio's worker-stealing async runtime. We take
/// a tokio::runtime::Handle so per-notification work (the HTTP push)
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
/// Pulled out so future call sites (manual test trigger, e.g.) can
/// reuse the shape without duplicating fields.
pub fn build_call_event(
    filter_match: &crate::voip_filters::FilterMatch,
    notification_text: &str,
) -> api::CallEvent<'static> {
    let now = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    api::CallEvent {
        event_type: "ringing",
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
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        KnownNotificationBindings,
        Management::{UserNotificationListener, UserNotificationListenerAccessStatus},
        UserNotificationChangedEventArgs,
    };

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
        // RequestAccessAsync returns an IAsyncOperation. In windows-rs
        // 0.58 without the futures bridge, IAsyncOperation isn't a
        // Future — call .get() to block on completion. Fine here
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

        log::info!("notification access granted; subscribing to events");

        // Listener gives us a delegate-based change event. Each
        // notification spawns onto the Tokio runtime so the regex
        // match + HTTP push doesn't block the COM thread. We pass
        // the runtime handle in because this code runs on a plain
        // std::thread where Handle::current() wouldn't find a
        // Tokio context.
        let ctx_for_handler = ctx.clone();
        let rt_for_handler = rt.clone();
        let handler = TypedEventHandler::<
            UserNotificationListener,
            UserNotificationChangedEventArgs,
        >::new(move |sender, args| {
            let ctx = ctx_for_handler.clone();
            let listener_clone = sender.clone();
            let args_clone = args.clone();
            rt_for_handler.spawn(async move {
                if let (Some(listener), Some(args)) = (listener_clone, args_clone) {
                    if let Err(err) = handle_change(&listener, &args, &ctx).await {
                        log::warn!("error handling notification: {:?}", err);
                    }
                }
            });
            Ok(())
        });

        let _registration_token = match listener.NotificationChanged(&handler) {
            Ok(token) => {
                log::info!("notification subscription active");
                token
            }
            Err(err) => {
                log::error!("failed to subscribe to NotificationChanged: {:?}", err);
                return;
            }
        };

        // Park this OS thread. The actual work happens inside the
        // event handler closure registered above. blocking sleep is
        // correct here because this isn't an async task.
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    async fn handle_change(
        listener: &UserNotificationListener,
        args: &UserNotificationChangedEventArgs,
        ctx: &AppContext,
    ) -> anyhow::Result<()> {
        // Only care about additions. Removals are when the user
        // dismisses an existing toast — not our trigger.
        use windows::UI::Notifications::UserNotificationChangedKind;
        if args.ChangeKind()? != UserNotificationChangedKind::Added {
            return Ok(());
        }
        let notification_id = args.UserNotificationId()?;

        // Pull the full notification (the change event only gives us
        // the id). GetNotification returns the toast bundled with
        // its source app info and text content.
        let n = listener.GetNotification(notification_id)?;
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

        // Pattern-match against the built-in filter registry. If
        // nothing matches, the notification wasn't a call — silently
        // ignore.
        let Some(filter_match) = crate::voip_filters::match_notification(&app_id, &title, &body)
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

        // Token check — if we're unpaired, log + skip rather than
        // panic. Reception's status pill will show the unpaired
        // state to nudge the user.
        let token = {
            let cfg = ctx.config.read().await;
            cfg.token.clone()
        };
        let Some(token) = token else {
            log::warn!("matched a call but not paired — ignoring");
            return Ok(());
        };

        let event = build_call_event(&filter_match, &combined);
        match api::call_event(&ctx.api_base, &ctx.http, &token, &event).await {
            Ok(resp) => {
                log::info!("posted call event: call_id={}", resp.call_id);
            }
            Err(err) => {
                log::warn!("posting call event failed: {:?}", err);
            }
        }
        Ok(())
    }
}
