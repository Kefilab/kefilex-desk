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
        eCapture, eConsole, eRender, AudioSessionStateActive, IAudioSessionControl,
        IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionManager2, IMMDevice,
        IMMDeviceCollection, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
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

        // One-shot diagnostic at startup: dump every audio endpoint
        // Windows knows about, in every state (Active / Disabled /
        // NotPresent / Unplugged), for both render and capture. This
        // helps us understand why session enumeration on a given
        // laptop only turns up the system PID-0 session — either
        // because devices we'd expect are Disabled/NotPresent, or
        // because the user's audio stack is doing something unusual
        // (per-app routing via virtual cables, MS Teams device
        // hijacking, headset switching, etc).
        log_audio_config_once(&device_enumerator);

        let mut state: HashMap<u32, PidState> = HashMap::new();

        loop {
            if let Err(err) = poll_once(&device_enumerator, &mut state, &ctx, &rt) {
                log::warn!("audio session poll failed: {:?}", err);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Walk every active audio session on EVERY active render endpoint,
    /// look up the owning process, match against the VoIP filter
    /// registry, and fire `ringing` for new sustained-active sessions.
    ///
    /// We enumerate ALL devices, not just the default, because each
    /// app attaches its audio session to one specific device. If the
    /// user has VXT routed through a headset and Spotify through
    /// speakers, those are sessions on two different devices and
    /// only the "default" device shows up under
    /// GetDefaultAudioEndpoint — we'd miss the other.
    fn poll_once(
        enumerator: &IMMDeviceEnumerator,
        state: &mut HashMap<u32, PidState>,
        ctx: &AppContext,
        rt: &tokio::runtime::Handle,
    ) -> windows::core::Result<()> {
        let device_collection: IMMDeviceCollection = unsafe {
            enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?
        };
        let device_count = unsafe { device_collection.GetCount()? };

        let mut active_voip_pids = Vec::new();
        let mut total_seen = 0u32;
        let mut active_seen = 0u32;

        for di in 0..device_count {
            let device: IMMDevice = unsafe { device_collection.Item(di)? };
            // Activate the per-device session manager. Errors here
            // (e.g. a device that's active but doesn't allow this
            // role) shouldn't kill the whole poll — log and move on.
            let session_mgr: IAudioSessionManager2 = match unsafe {
                device.Activate(CLSCTX_ALL, None)
            } {
                Ok(m) => m,
                Err(err) => {
                    log::trace!(
                        "device {} Activate(IAudioSessionManager2) failed: {:?}",
                        di,
                        err
                    );
                    continue;
                }
            };

            let sessions: IAudioSessionEnumerator =
                unsafe { session_mgr.GetSessionEnumerator()? };
            let count = unsafe { sessions.GetCount()? };

            for i in 0..count {
                total_seen += 1;
                let session_control = unsafe { sessions.GetSession(i)? };
                let session2: IAudioSessionControl2 = session_control.cast()?;

                let session_state = unsafe { session_control.GetState()? };
                let pid = unsafe { session2.GetProcessId().unwrap_or(0) };

                // Only Active sessions can drive an event. Idle sessions
                // (Spotify-but-paused, Discord-in-tray, etc.) routinely
                // sit in the enumerator and resolving their process name
                // on every poll is wasted work + log noise. Skip early.
                if session_state != AudioSessionStateActive {
                    continue;
                }
                active_seen += 1;
                if pid == 0 {
                    // System session (e.g. Windows notification chimes).
                    // Active but uninteresting.
                    continue;
                }

                let process_name = get_process_name(pid)
                    .unwrap_or_else(|| format!("<pid-{}>", pid));
                let voip_match = match_voip_process(&process_name);

                // TRACE level — only visible with RUST_LOG=trace. Useful
                // when investigating "what's playing on this machine" but
                // not part of normal operation.
                log::trace!(
                    "active audio session: device={} pid={} process={} voip_match={:?}",
                    di,
                    pid,
                    process_name,
                    voip_match
                );

                let Some(display_name) = voip_match else {
                    continue;
                };

                // One-shot diagnostic: dump every COM property we can pull
                // off the IAudioSessionControl / IAudioSessionControl2 for
                // the first VoIP-matched active session this process sees.
                // Goal: see if caller info hides in GetDisplayName,
                // GetSessionInstanceIdentifier, or other fields we don't
                // currently read.
                if !AUDIO_DUMP_DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    dump_audio_session_fully(
                        &session_control,
                        &session2,
                        pid,
                        &process_name,
                    );
                }

                active_voip_pids.push((pid, process_name, display_name));
            }
        }

        // Per-poll summary at INFO level. Now also reports device count.
        log_poll_summary(
            device_count,
            total_seen,
            active_seen,
            active_voip_pids.len() as u32,
        );

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

    /// Guard for the one-shot voip-dump diagnostic. Set true after the
    /// first VoIP-matched active session has been fully dumped, so we
    /// don't repeat the dump on every poll while the call continues.
    static AUDIO_DUMP_DONE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// Diagnostic — log every property we can pull off the
    /// IAudioSessionControl / IAudioSessionControl2 pair for one
    /// session. Used to investigate whether softphones write caller
    /// info into session metadata fields (DisplayName etc) that our
    /// regular path ignores. All errors are absorbed.
    fn dump_audio_session_fully(
        session_control: &IAudioSessionControl,
        session2: &IAudioSessionControl2,
        pid: u32,
        process_name: &str,
    ) {
        log::info!(
            "voip-dump: ── audio session pid={} process={} ──",
            pid,
            process_name
        );

        unsafe {
            match session_control.GetDisplayName() {
                Ok(pw) => {
                    let s = pw.to_string().unwrap_or_else(|_| "<utf16-err>".into());
                    log::info!("voip-dump:   GetDisplayName: '{}'", s);
                }
                Err(e) => log::info!("voip-dump:   GetDisplayName: <err {:?}>", e),
            }
            match session_control.GetIconPath() {
                Ok(pw) => {
                    let s = pw.to_string().unwrap_or_else(|_| "<utf16-err>".into());
                    log::info!("voip-dump:   GetIconPath: '{}'", s);
                }
                Err(e) => log::info!("voip-dump:   GetIconPath: <err {:?}>", e),
            }
            match session_control.GetGroupingParam() {
                Ok(guid) => log::info!("voip-dump:   GetGroupingParam: {:?}", guid),
                Err(e) => log::info!("voip-dump:   GetGroupingParam: <err {:?}>", e),
            }
            match session2.GetSessionInstanceIdentifier() {
                Ok(pw) => {
                    let s = pw.to_string().unwrap_or_else(|_| "<utf16-err>".into());
                    log::info!("voip-dump:   GetSessionInstanceIdentifier: '{}'", s);
                }
                Err(e) => log::info!(
                    "voip-dump:   GetSessionInstanceIdentifier: <err {:?}>",
                    e
                ),
            }
            match session2.GetSessionIdentifier() {
                Ok(pw) => {
                    let s = pw.to_string().unwrap_or_else(|_| "<utf16-err>".into());
                    log::info!("voip-dump:   GetSessionIdentifier: '{}'", s);
                }
                Err(e) => log::info!("voip-dump:   GetSessionIdentifier: <err {:?}>", e),
            }
            // IsSystemSoundsSession returns raw HRESULT in windows-rs 0.58:
            // S_OK (0) means yes, S_FALSE (1) means no. We must compare the
            // numeric code directly because both values map to "non-error"
            // in HRESULT::is_ok().
            let hr = session2.IsSystemSoundsSession();
            if hr.0 == 0 {
                log::info!("voip-dump:   IsSystemSoundsSession: yes (S_OK)");
            } else {
                log::info!(
                    "voip-dump:   IsSystemSoundsSession: no (HRESULT=0x{:08X})",
                    hr.0 as u32
                );
            }
        }

        log::info!("voip-dump: ── end audio session ──");
    }

    /// One-shot startup diagnostic. Walks every audio endpoint in
    /// every state, both render and capture, and logs id + dataflow
    /// + state. Also logs the default render device id (eConsole role)
    /// so we can cross-reference what Windows considers "the system's
    /// default speakers".
    ///
    /// All errors are absorbed — this is best-effort visibility, never
    /// blocks the polling loop.
    fn log_audio_config_once(enumerator: &IMMDeviceEnumerator) {
        log::info!("audio-config: ── start one-shot endpoint dump ──");

        for (flow, flow_name) in [
            (eRender, "render"),
            (eCapture, "capture"),
        ] {
            for (state_const, state_name) in [
                (DEVICE_STATE_ACTIVE, "ACTIVE"),
                (DEVICE_STATE_DISABLED, "DISABLED"),
                (DEVICE_STATE_NOTPRESENT, "NOTPRESENT"),
                (DEVICE_STATE_UNPLUGGED, "UNPLUGGED"),
            ] {
                let coll: IMMDeviceCollection = match unsafe {
                    enumerator.EnumAudioEndpoints(flow, state_const)
                } {
                    Ok(c) => c,
                    Err(err) => {
                        log::warn!(
                            "audio-config: EnumAudioEndpoints({}, {}) failed: {:?}",
                            flow_name,
                            state_name,
                            err
                        );
                        continue;
                    }
                };
                let count = unsafe { coll.GetCount() }.unwrap_or(0);
                log::info!(
                    "audio-config: {} {} devices: {}",
                    flow_name,
                    state_name,
                    count
                );
                for i in 0..count {
                    let device = match unsafe { coll.Item(i) } {
                        Ok(d) => d,
                        Err(err) => {
                            log::warn!(
                                "audio-config: {} {} #{} Item() failed: {:?}",
                                flow_name,
                                state_name,
                                i,
                                err
                            );
                            continue;
                        }
                    };
                    let id_str = unsafe { device.GetId() }
                        .ok()
                        .and_then(|p| unsafe { p.to_string() }.ok())
                        .unwrap_or_else(|| "<no-id>".to_string());
                    log::info!(
                        "audio-config: {} {} #{}: id={}",
                        flow_name,
                        state_name,
                        i,
                        id_str
                    );
                }
            }
        }

        // Default endpoint sanity check. If this returns something
        // and EnumAudioEndpoints(eRender, ACTIVE) doesn't contain it,
        // we've got a serious mismatch.
        match unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) } {
            Ok(d) => {
                let id_str = unsafe { d.GetId() }
                    .ok()
                    .and_then(|p| unsafe { p.to_string() }.ok())
                    .unwrap_or_else(|| "<no-id>".to_string());
                log::info!("audio-config: default render (console) id={}", id_str);
            }
            Err(err) => log::warn!(
                "audio-config: GetDefaultAudioEndpoint(render, console) failed: {:?}",
                err
            ),
        }

        log::info!("audio-config: ── end endpoint dump ──");
    }

    /// Diagnostic helper — emits a "poll summary" line at INFO only
    /// when something interesting happens: the count of active sessions
    /// changed, the count of voip-matched sessions changed, or every
    /// 30 seconds (60 polls @ 500ms) as a quiet heartbeat. Previously
    /// fired every poll while voip_active > 0, which drowned out the
    /// actual call-lifecycle events during a ring.
    fn log_poll_summary(devices: u32, total: u32, active: u32, voip_active: u32) {
        use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        // u32::MAX sentinel means "no prior reading" so the first poll
        // always logs a baseline.
        static PREV_ACTIVE: AtomicU32 = AtomicU32::new(u32::MAX);
        static PREV_VOIP: AtomicU32 = AtomicU32::new(u32::MAX);

        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prev_active = PREV_ACTIVE.swap(active, Ordering::Relaxed);
        let prev_voip = PREV_VOIP.swap(voip_active, Ordering::Relaxed);

        let state_changed = prev_active != active || prev_voip != voip_active;
        let periodic = n % 60 == 0;
        if state_changed || periodic {
            log::info!(
                "audio session poll: {} devices, {} sessions total, {} active, {} voip-matched",
                devices,
                total,
                active,
                voip_active
            );
        }
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
