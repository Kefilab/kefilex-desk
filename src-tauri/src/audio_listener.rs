// src-tauri/src/audio_listener.rs
//
// CoreAudio session monitor — detects "VXT (or any softphone) is
// playing audio right now" by polling the Windows audio session
// enumerator. When a VoIP process transitions from inactive →
// sustained-active (audio playing for 1+ seconds), we fire a
// `ringing` event to the Kefilex backend.
//
// Why this complements the notification listener:
//
//   - Notification listener catches POST-call signals from softphones
//     that do post-call notifications (VXT's "Missed call from X").
//   - This audio listener catches LIVE signals from ANY softphone
//     that plays a ringtone through Windows audio. Works even when
//     the softphone (eg VXT) doesn't fire any incoming-call event
//     via its own API.
//
//   Together they cover both ends of the call lifecycle.
//
// What we get + what we don't:
//
//   ✓ Sub-second live detection that "VXT audio is active"
//   ✓ Works for every softphone — they all play ringtones via the
//     same Windows audio APIs
//   ✗ NO caller phone number / name at audio-activation time —
//     the audio session API doesn't carry that metadata
//   ✗ Can't distinguish inbound ringing from outbound dialing
//     (both play audio). Reception treats both as a "phone is
//     active" signal; outbound is harmless noise.
//
// UI Automation can later be paired with this to fill in caller
// info by reading the softphone's ringing popup. That's a separate
// module landing in 31g.

use crate::AppContext;
use std::time::Duration;

/// Entry point. Runs synchronously on its own OS thread because
/// CoreAudio COM objects are not Send and can't ride Tokio's
/// work-stealing executor.
pub fn run_blocking(ctx: AppContext, rt: tokio::runtime::Handle) {
    log::info!(
        "audio session listener starting (platform: {})",
        std::env::consts::OS
    );
    #[cfg(target_os = "windows")]
    {
        windows_impl::run_loop(ctx, rt);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ctx;
        let _ = rt;
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

// ─── Windows implementation ──────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use chrono::SecondsFormat;
    use std::collections::HashMap;
    // Interface trait gives us .cast() on COM pointers (e.g. casting
    // IAudioSessionControl to its IAudioSessionControl2 superset).
    use windows::core::Interface;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IAudioSessionControl2, IAudioSessionEnumerator,
        IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
        AudioSessionStateActive,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::ProcessStatus::GetModuleFileNameExW;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// 500ms is the same cadence as the notification listener.
    /// Enumerating sessions is a single COM call (~5ms), so this
    /// adds maybe 1% CPU to one thread — imperceptible.
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    /// State machine per VoIP process — drives one-shot firing on
    /// "audio active for 1+ second" rather than every poll while
    /// the call is active.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum PidState {
        /// Not seen as Active this scan; may or may not exist as a process.
        Inactive,
        /// Saw Active for the first time on this scan. Hold for one
        /// more poll before firing to avoid spurious notification
        /// chimes triggering the call event.
        JustActivated,
        /// Sustained Active over ≥2 polls; "ringing" event already
        /// dispatched. No more events fire until the session goes
        /// Inactive and starts again.
        Sustained,
    }

    pub fn run_loop(ctx: AppContext, rt: tokio::runtime::Handle) {
        // CoInitializeEx is required before any COM call on this
        // thread. MULTITHREADED is right for a worker that may be
        // hit by callbacks from arbitrary threads later — but we're
        // mostly running blocking COM calls on this thread alone,
        // so APARTMENTTHREADED would also work. MULTITHREADED is
        // the safer default.
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() {
                log::error!(
                    "CoInitializeEx failed: 0x{:08X} — audio listener disabled",
                    hr.0
                );
                return;
            }
        }

        let device_enumerator: IMMDeviceEnumerator = match unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)
        } {
            Ok(e) => e,
            Err(err) => {
                log::error!(
                    "CoCreateInstance(MMDeviceEnumerator) failed: {:?} — audio listener disabled",
                    err
                );
                return;
            }
        };

        log::info!(
            "audio session listener: polling every {:?}",
            POLL_INTERVAL
        );

        let mut state: HashMap<u32, PidState> = HashMap::new();

        loop {
            if let Err(err) = poll_once(&device_enumerator, &mut state, &ctx, &rt) {
                log::warn!("audio session poll failed: {:?}", err);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Walk every active audio session on the default render device,
    /// look up the owning process, match against the VoIP filter
    /// registry, and fire `ringing` for new sustained-active
    /// sessions.
    fn poll_once(
        enumerator: &IMMDeviceEnumerator,
        state: &mut HashMap<u32, PidState>,
        ctx: &AppContext,
        rt: &tokio::runtime::Handle,
    ) -> windows::core::Result<()> {
        let device: IMMDevice =
            unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)? };

        // Activate the per-device session manager. In windows-rs 0.58
        // the typed wrapper takes 2 args (CLSCTX + optional activation
        // params) and returns Result<T> — T is inferred from the let
        // binding's type annotation. No manual out-pointer juggling.
        let session_mgr: IAudioSessionManager2 =
            unsafe { device.Activate(CLSCTX_ALL, None)? };

        let sessions: IAudioSessionEnumerator =
            unsafe { session_mgr.GetSessionEnumerator()? };
        let count = unsafe { sessions.GetCount()? };

        // Build the set of PIDs that ARE VoIP-matched and currently active.
        let mut active_voip_pids = Vec::new();

        for i in 0..count {
            let session_control = unsafe { sessions.GetSession(i)? };
            let session2: IAudioSessionControl2 = session_control.cast()?;

            // Skip if not active.
            let session_state = unsafe { session_control.GetState()? };
            if session_state != AudioSessionStateActive {
                continue;
            }

            // PID 0 is the System session (mixer, OS sounds).
            let pid = unsafe { session2.GetProcessId()? };
            if pid == 0 {
                continue;
            }

            // Read the process executable name — gives us
            // "vxt.exe" or "Microsoft Teams.exe" or whatever the
            // softphone calls itself. Substring-match against the
            // existing VoIP filter app_id_patterns; they're
            // permissive enough ("vxt", "teams", "ringcentral") to
            // catch both AUMIDs from the notification listener and
            // exe names here.
            let Some(process_name) = get_process_name(pid) else {
                continue;
            };
            let Some(display_name) = match_voip_process(&process_name) else {
                continue;
            };

            active_voip_pids.push((pid, process_name, display_name));
        }

        // Drive the state machine for each currently-active VoIP PID.
        for (pid, process_name, display_name) in &active_voip_pids {
            let next = match state.get(pid).copied() {
                Some(PidState::Sustained) => PidState::Sustained,
                Some(PidState::JustActivated) => {
                    // Second consecutive Active poll — confirmed
                    // sustained, fire the event.
                    log::info!(
                        "sustained VoIP audio: pid={} process={} app={}",
                        pid,
                        process_name,
                        display_name
                    );
                    spawn_ringing_event(
                        ctx.clone(),
                        rt,
                        display_name,
                        process_name.clone(),
                    );
                    PidState::Sustained
                }
                _ => {
                    // First time seeing this PID active — wait one
                    // more poll to confirm sustained activity.
                    log::debug!(
                        "VoIP audio activation observed (pending confirmation): pid={} process={}",
                        pid,
                        process_name
                    );
                    PidState::JustActivated
                }
            };
            state.insert(*pid, next);
        }

        // PIDs in state but not in this poll's active set go Inactive.
        let active_set: std::collections::HashSet<u32> =
            active_voip_pids.iter().map(|(pid, _, _)| *pid).collect();
        for pid in state.keys().copied().collect::<Vec<_>>() {
            if !active_set.contains(&pid) {
                state.insert(pid, PidState::Inactive);
            }
        }

        Ok(())
    }

    /// Resolve a Windows process id to its executable filename.
    /// Returns just the leaf name (e.g. "VXT.exe") rather than the
    /// full path so the caller can do simple substring matching.
    fn get_process_name(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            // GetModuleFileNameExW in windows-rs 0.58 takes the HANDLE
            // directly (it implements Param<HANDLE> by value), not
            // wrapped in Some(). For hmodule we pass None to mean
            // "the executable module of the target process".
            let len = GetModuleFileNameExW(handle, None, &mut buf);
            let _ = CloseHandle(handle);
            if len == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            // Extract just the leaf filename
            let filename = path
                .rsplit_once(['\\', '/'])
                .map(|(_, f)| f.to_string())
                .unwrap_or(path);
            Some(filename)
        }
    }

    /// Match a process executable name against the VoIP filter
    /// registry's app_id_patterns. The patterns are designed to be
    /// permissive substring matches (e.g. "vxt") so they catch both
    /// AUMIDs from the notification listener AND exe names here
    /// (where "VXT.exe", "vxt-desktop.exe", "nz.co.vxt.electron.exe"
    /// all contain "vxt").
    fn match_voip_process(process_name: &str) -> Option<&'static str> {
        let lower = process_name.to_lowercase();
        for filter in crate::voip_filters::BUILTIN_FILTERS {
            for pattern in filter.app_id_patterns {
                if lower.contains(&pattern.to_lowercase()) {
                    return Some(filter.display_name);
                }
            }
        }
        None
    }

    /// Spawn an async task on the Tokio runtime to POST the ringing
    /// event. We don't wait for completion — fire-and-forget so the
    /// COM polling loop stays tight.
    fn spawn_ringing_event(
        ctx: AppContext,
        rt: &tokio::runtime::Handle,
        source_app: &'static str,
        process_name: String,
    ) {
        rt.spawn(async move {
            let token = {
                let cfg = ctx.config.read().await;
                cfg.token.clone()
            };
            let Some(token) = token else {
                log::warn!(
                    "audio session active but companion not paired — ignoring"
                );
                return;
            };

            let now = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            let event = crate::api::CallEvent {
                event_type: "ringing",
                caller_phone_e164: None,
                firm_phone_e164: None,
                source_app: Some(source_app.to_string()),
                started_at: now,
                caller_display_name: None,
                intended_fee_earner_hint: None,
                vxt_call_id: None,
                metadata: serde_json::json!({
                    "trigger": "coreaudio_session_active",
                    "process_name": process_name,
                    "note": "Audio-only signal — caller details will be filled in by the notification listener or by reception manually.",
                }),
            };

            match crate::api::call_event(&ctx.api_base, &ctx.http, &token, &event).await
            {
                Ok(resp) => log::info!(
                    "posted audio-trigger ringing event: call_id={} source={}",
                    resp.call_id,
                    source_app
                ),
                Err(err) => log::warn!("audio-trigger event POST failed: {:?}", err),
            }
        });
    }
}
