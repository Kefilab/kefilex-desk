# Supported Softphones

Kefilex Desk captures incoming-call events from any softphone running on the
reception computer. It uses two complementary signals:

1. **CoreAudio session listener** — fires the moment a known softphone process
   starts playing audio (a ringtone). Universal: works for every Windows
   softphone because they all play ringtones through the same Windows audio
   stack. Limitation: gives us "the phone is ringing right now" but no caller
   identity. Outbound calls also trigger this signal (see *Known
   constraints* below).
2. **Toast notification listener** — reads Windows toasts the softphone posts.
   Captures whatever the softphone chooses to put in the toast: name, phone,
   missed/voicemail status. Coverage is per-softphone and depends on what
   that softphone actually emits.

## Support matrix

Legend:

- ✅ — works
- ⚠️ — best-effort, depends on softphone version / config
- ❌ — softphone does not expose this signal at all
- 🟡 — pattern written but not empirically verified yet

| Softphone | Live ring (audio) | Caller name | Caller phone | Missed-call | Verified |
|---|---|---|---|---|---|
| **VXT** | ✅ | ✅ post-call only (~20s after ring ends) | ❌ never | ✅ ("Missed call from X") | ✅ end-to-end |
| **RingCentral** | ✅ | ✅ live | ⚠️ if formatted "name — phone" | ✅ | 🟡 pattern-based |
| **Aircall** | ✅ | ✅ live | ✅ live (when in toast) | ✅ | 🟡 pattern-based |
| **Microsoft Teams** | ✅ | ✅ live | ⚠️ optional | 🟡 not yet | 🟡 pattern-based |
| **Zoom Phone** | ✅ | ✅ live | ⚠️ optional | 🟡 not yet | 🟡 pattern-based |
| **Dialpad** | ✅ | ✅ live | ⚠️ optional | 🟡 not yet | 🟡 pattern-based |
| **8x8 Work** | ✅ | ✅ live | ✅ live (when in toast) | ✅ | 🟡 pattern-based |
| **Cisco Webex Calling** | ✅ | ✅ live | ⚠️ optional | 🟡 not yet | 🟡 pattern-based |

Everything except VXT is **pattern-based** — we wrote the regex from public
documentation and screenshots, not from real end-to-end testing. Expect minor
adjustments to the patterns once we have a customer using each one in
production. Filters live in `src-tauri/src/voip_filters.rs`; add a unit test
for the real-world toast text once observed.

## Per-softphone notes

### VXT (`nz.co.vxt.electron`)

- **Empirical limitation:** VXT desktop emits only a post-call `Missed call
  from {name}` toast using the simplest legacy `ToastText01` template. There
  is no during-ring notification, no caller phone number in any toast, and
  no phone number in any session-metadata field we can read
  (`GetDisplayName`, `GetSessionInstanceIdentifier`, etc. all empty or
  zero-GUID). Verified by raw dump on 2026-05-15.
- **What we capture:** live ringing signal from CoreAudio + caller display
  name from the post-call toast. Reception sees a banner the moment the
  phone rings; the caller name fills in ~20s later when the call ends in
  missed state.
- **What's missing:** caller phone number. Reception staff need to read this
  from the VXT screen during the ring, or wait for the VXT post-call
  webhook (Phase 31e, not yet wired up).

### RingCentral

- Pattern assumes format `"Incoming call from {name}"` optionally followed by
  ` — {phone}`. Confirm with a real RingCentral instance before relying on
  the phone capture.

### Aircall

- Pattern assumes `"Incoming call from {name} • {phone}"` (bullet separator)
  with fallbacks for name-only and phone-only variants. Aircall's exact
  separator may differ in your build; check `voip_filters.rs` and a real
  toast.

### Microsoft Teams

- Teams toasts usually carry name only. Phone number capture is best-effort
  for PSTN inbound calls only — Teams-to-Teams calls have no phone.

### Zoom Phone

- Toast format documented as `"Incoming call from {name}"` — confirm and
  add unit test once verified.

### Dialpad

- Format observed in docs: `"{name} is calling"` with optional phone.

### 8x8 Work

- Pattern mirrors Aircall (bullet-separator format). 8x8 may use different
  formatting in different regions — verify before relying on phone capture.

### Cisco Webex Calling

- Webex toasts usually carry name only. Phone capture optional.

## How to add a new softphone

1. Identify the AUMID from a live toast (`PowerShell: Get-StartApps`, or run
   the companion with `RUST_LOG=debug` and watch for
   `notification from <aumid>: ...`).
2. Identify the process executable name from `Get-Process`.
3. Append entries to `BUILTIN_FILTERS` in `src-tauri/src/voip_filters.rs`:
   - `display_name` — what we send to the backend as `source_app`
   - `app_id_patterns` — substrings to match the AUMID (case-insensitive).
     Make sure at least one pattern also appears in the process executable
     name (e.g. `"vxt"` matches both `nz.co.vxt.electron` and `Vxt
     Desktop.exe`) so the audio listener also picks up the softphone.
   - `text_pattern` — regex against `"{title} — {body}"` with named groups
     `phone` and `name`
   - `event_type` — one of `ringing`, `missed`, `voicemail`, `answered`
4. Add unit tests under `#[cfg(test)] mod tests` covering the real toast
   text you observed.
5. Update this matrix.

## Known constraints

### Audio-device prerequisite

Live ringing detection only works if your softphone's audio routes through
an audio device the companion can see. On most laptops this is the laptop
speakers — works out of the box. On laptops with separate "Default Device"
and "Default Communication Device" settings pointing at different endpoints
(common after Bluetooth headset use), the softphone may route audio through
a device that's marked NotPresent or Disabled and the audio listener will
not detect anything.

Fix: open `mmsys.cpl` (Win+R), set the working speaker as both Default
Device and Default Communication Device on the Playback tab.

### Outbound calls fire too

The audio listener fires on **any** softphone audio activity, not just
inbound ringtones. If a reception staff member makes an outbound call, the
companion will post a `ringing` event for it. Server-side / UI-side logic
should treat these as harmless noise (Reception will see a banner pop and
immediately realise it's their own outbound call).

We could distinguish inbound vs outbound by pairing the audio signal with
a notification (no during-ring notification → likely outbound), but this is
deferred until reception users tell us the false positives are actually
disruptive.

### Caller phone capture is post-call for VXT

See VXT note above. For VXT-only firms, plan to either:

- Wire up the VXT post-call webhook (Phase 31e) for accurate phone+recording
- Build UI Automation against VXT's incoming-call popup (Phase 31g) if live
  phone capture is critical
- Accept that reception manually enters the phone for VXT calls

## Emergency disable switches

If a softphone pattern causes problems in production, you can disable either
listener via environment variable without uninstalling the companion:

```powershell
# Disable the toast notification listener (audio listener stays on)
setx KEFILEX_DESK_DISABLE_NOTIFICATION_LISTENER 1

# Disable the audio session listener (notification listener stays on)
setx KEFILEX_DESK_DISABLE_AUDIO_LISTENER 1
```

Restart the companion for the env var to take effect. Remove the var (or
set it to nothing) to re-enable.
