# Kefilex Desk

Live incoming-call capture for [Kefilex](https://kefilex.com) reception staff. A tiny Windows companion app that watches OS-level notifications from any softphone (VXT, RingCentral, Aircall, Microsoft Teams, Zoom Phone, Dialpad, 8x8, Cisco Webex Calling) and pipes the incoming-call event to the Kefilex Reception inbox in real time.

Open source, MIT licensed. Signed by [SignPath.io](https://signpath.io) under their free-for-open-source program.

## What it does

```
Phone rings on receptionist's desk      Kefilex Desk sees the toast      Kefilex Reception inbox
   ┌──────────────────────┐               ┌──────────────────┐              ┌─────────────────┐
   │ VXT shows OS toast:  │  ──────────►  │ Reads via        │  WebSocket   │ "+44 7700 …     │
   │ "Jane Smith calling" │   ~50ms       │ UserNotification │  ~200ms      │   is calling    │
   └──────────────────────┘               │ Listener API     │              │   Jane Smith"   │
                                          │                  │              │                 │
                                          │ Extracts number  │              │ Reception clicks│
                                          │ Pings Kefilex    │              │ → call card     │
                                          └──────────────────┘              │   pre-populated │
                                                                            └─────────────────┘
```

## What it does NOT do

- **It is not a softphone.** It does not handle audio, ring, mute, hold, transfer, etc. Your existing phone (VXT, Teams, whatever) keeps doing that job.
- **It does not read your other notifications.** Only Windows notifications whose source app + text match the built-in VoIP filter registry are read; everything else is ignored in-process and never transmitted.
- **It does not store call audio.** Call recordings live in your VoIP system if at all.

## Requirements

- Windows 10 (version 1809 or newer) or Windows 11
- ~6 MB disk space
- Outbound HTTPS / WebSocket access to `api.kefilex.com`
- A paired Kefilex account with the **reception**, **intake**, **manager**, or **super_user** role

## Install

Download the latest signed installer from [app.kefilex.com/get-kefilex-desk](https://app.kefilex.com/get-kefilex-desk).

Silent install for IT-managed firms:

```powershell
msiexec /i KefilexDesk-1.0.0.msi /quiet /norestart
```

## Pair the device

1. Open Kefilex Reception in your browser at [app.kefilex.com/reception](https://app.kefilex.com/reception)
2. Click *Pair this device* — a 6-digit code appears on screen
3. Open Kefilex Desk from the system tray → enter the code → done

The pairing token is stored in **Windows Credential Manager** (DPAPI-encrypted, scoped to your Windows user account). It survives reboots and Windows updates and never needs to be entered again unless a super_user explicitly revokes the device or you uninstall the app.

## Privacy

This companion observes OS-level notifications, which is sensitive territory. Every line of code is here in this repository — audit it if you have any doubts. Specifically:

- The filter registry lives in [`src/voip_filters.rs`](src/voip_filters.rs). Only notifications whose source app + title match these patterns are read. Everything else is ignored at the OS handler level.
- Outbound traffic is one WebSocket connection to `api.kefilex.com/api/desk-companion/ws` plus a check-for-updates poll. Both visible in network traces.
- Heartbeat payload: `{ device_id, app_version, os_version, timestamp }`. No PII.
- Call event payload: `{ caller_phone_e164, source_app, intended_recipient_hint, started_at, captured_at }`. No notification body beyond the extracted fields.

See the [Kefilex privacy policy](https://kefilex.com/privacy) for the data-processing terms.

## Distribution

- **Official build**: [app.kefilex.com/get-kefilex-desk](https://app.kefilex.com/get-kefilex-desk) — signed by SignPath.io with our open-source EV certificate. Auto-updates silently.
- **Build from source**: see [BUILDING.md](BUILDING.md). Useful if your firm's IT prefers an internally-built binary.

## License

MIT — see [LICENSE](LICENSE).

## Contact

Security issues: privacy@kefilab.com — please report privately first.

Bugs / suggestions: open an issue on this repo.
