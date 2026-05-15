// src-tauri/src/voip_filters.rs
//
// Built-in registry of VoIP softphone notification patterns. When the
// notification listener sees a toast, it iterates this list. The
// FIRST filter whose source AND text both match wins; everything
// else is dropped.
//
// Filter shape: (source-app pattern, text regex, event_type).
//
//   source-app pattern → matched substring against the OS's
//     AppUserModelId (AUMID) string. e.g. "vxt" matches both
//     "vxt.desktop" and "nz.co.vxt.electron".
//
//   text regex → matched against (title + " — " + body) of the
//     notification. Named capture groups `phone` and `name` extract
//     the caller information when present.
//
//   event_type → what kind of call event to report. Drives the
//     companion's POST to /api/desk-companion/call-event with one
//     of: ringing / answered / missed / voicemail. Server maps to
//     the calls table status column.
//
// Adding a new softphone: append filter entries to BUILTIN_FILTERS.
// You can have MULTIPLE filters per softphone with different
// event_types — e.g. VXT has separate entries for "Incoming call"
// (ringing) and "Missed call" (missed), because VXT desktop posts
// notifications at different points in the call lifecycle.
//
// Empirical findings to date:
//   - VXT desktop (AUMID nz.co.vxt.electron) only posts post-call
//     notifications ("Missed call from <name>"). Same architectural
//     constraint as its webhook — VXT does not expose during-ring
//     events. So we can't do live capture for VXT specifically,
//     only post-call missed-call detection.
//   - Other softphones (Teams, RingCentral, Aircall, etc.) DO post
//     during-ring notifications ("Incoming call from X"), so live
//     capture works for them.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct VoipFilter {
    /// Display name of the softphone, sent in the source_app field
    /// of the outgoing call event. e.g. "VXT", "Microsoft Teams".
    pub display_name: &'static str,
    /// AppUserModelId substring(s) we match against. Case-insensitive.
    pub app_id_patterns: &'static [&'static str],
    /// Regex against (title + " — " + body). Optional named capture
    /// groups "phone" and "name" extract caller info when present.
    pub text_pattern: &'static str,
    /// What event_type to report to the backend when this filter
    /// matches. Must be one of: "ringing", "answered", "missed",
    /// "voicemail", "ended". See the calls table status mapping in
    /// app/api/desk-companion/call-event/route.ts.
    pub event_type: &'static str,
}

/// The built-in filter registry. v0.1 — extend as we discover new
/// softphone variants. ORDER MATTERS: the first matching filter wins,
/// so more-specific patterns (e.g. "Missed call from") should appear
/// before more-general ones to avoid mis-classification.
pub const BUILTIN_FILTERS: &[VoipFilter] = &[
    // ─── VXT (AUMID confirmed: nz.co.vxt.electron) ──────────────
    // VXT does NOT post during-ring notifications, only post-call.
    // We detect the missed-call notification specifically. Captures
    // either a phone number or a contact name from the "from X" tail.
    VoipFilter {
        display_name: "VXT",
        app_id_patterns: &["vxt"],
        // Real-world observation: VXT desktop's missed-call toast
        // has empty body, so the combined string we match against is
        // "Missed call from Bal | " (with the join separator trailing).
        // The name capture used to greedily slurp up to "$" which
        // included the " — " separator and trailing whitespace,
        // producing "Bal —" instead of "Bal". The non-greedy character
        // class below stops at separator characters, whitespace, or
        // pipe so we capture just the name.
        text_pattern: r"(?i)Missed call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\s|—–-][^|—–\n]*?))(?:\s*[—–|\-]|\s*$)",
        event_type: "missed",
    },
    // Voicemail notification from VXT — separate filter entry so the
    // status maps correctly on the server side. Same name/phone
    // capture rules.
    VoipFilter {
        display_name: "VXT",
        app_id_patterns: &["vxt"],
        text_pattern: r"(?i)(?:new voicemail|voicemail received).*?from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))$",
        event_type: "voicemail",
    },
    // Kept for future-proofing — if VXT ever does start posting
    // during-ring notifications, this matches.
    VoipFilter {
        display_name: "VXT",
        app_id_patterns: &["vxt"],
        text_pattern: r"(?i)(?:Incoming call from|is calling)\s*(?:[—–-]\s*)?(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))$",
        event_type: "ringing",
    },

    // ─── RingCentral ──────────────────────────────────────────────
    VoipFilter {
        display_name: "RingCentral",
        app_id_patterns: &["ringcentral", "com.ringcentral.app"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
        event_type: "ringing",
    },
    VoipFilter {
        display_name: "RingCentral",
        app_id_patterns: &["ringcentral", "com.ringcentral.app"],
        text_pattern: r"(?i)missed call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))$",
        event_type: "missed",
    },

    // ─── Aircall ──────────────────────────────────────────────────
    // Three filters per event type, ordered specific → general so that
    // when the toast carries BOTH name AND phone we capture both. The
    // fallback alternation catches name-only or phone-only toasts.
    //
    // Observed Aircall formats (from public docs / screenshots, not
    // verified end-to-end yet):
    //   "Incoming call from John Smith • +33 1 23 45 67 89"
    //   "Incoming call from +33 1 23 45 67 89"        (unknown caller)
    //   "Incoming call from John Smith"               (anonymous)
    VoipFilter {
        display_name: "Aircall",
        app_id_patterns: &["aircall", "com.aircall.desktop"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^•—–\n]+?)\s*[•—–\-]\s*(?P<phone>\+?[\d\s().-]{6,30})\s*$",
        event_type: "ringing",
    },
    VoipFilter {
        display_name: "Aircall",
        app_id_patterns: &["aircall", "com.aircall.desktop"],
        text_pattern: r"(?i)incoming call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))\s*$",
        event_type: "ringing",
    },
    VoipFilter {
        display_name: "Aircall",
        app_id_patterns: &["aircall", "com.aircall.desktop"],
        text_pattern: r"(?i)missed call from\s+(?P<name>[^•—–\n]+?)\s*[•—–\-]\s*(?P<phone>\+?[\d\s().-]{6,30})\s*$",
        event_type: "missed",
    },
    VoipFilter {
        display_name: "Aircall",
        app_id_patterns: &["aircall", "com.aircall.desktop"],
        text_pattern: r"(?i)missed call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))\s*$",
        event_type: "missed",
    },

    // ─── Microsoft Teams ──────────────────────────────────────────
    // Teams calls don't always include a phone number in the toast;
    // the regex makes phone optional and falls back to name.
    VoipFilter {
        display_name: "Microsoft Teams",
        app_id_patterns: &["microsoft.teams", "com.microsoft.teams", "teams"],
        text_pattern: r"(?i)(?:incoming call from|calling)\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
        event_type: "ringing",
    },

    // ─── Zoom Phone ───────────────────────────────────────────────
    VoipFilter {
        display_name: "Zoom Phone",
        app_id_patterns: &["zoom", "us.zoom.xos", "com.zoom.phone"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
        event_type: "ringing",
    },

    // ─── Dialpad ──────────────────────────────────────────────────
    VoipFilter {
        display_name: "Dialpad",
        app_id_patterns: &["dialpad", "com.dialpad.desktop"],
        text_pattern: r"(?i)(?P<name>.+?)\s+is calling.*?(?P<phone>\+?[\d\s().-]{6,30})?",
        event_type: "ringing",
    },

    // ─── 8x8 ──────────────────────────────────────────────────────
    // Same specific → general structure as Aircall — try to capture
    // both name and phone when present, fall back to either-or.
    VoipFilter {
        display_name: "8x8",
        app_id_patterns: &["8x8", "com.8x8.work"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^•—–\n]+?)\s*[•—–\-]\s*(?P<phone>\+?[\d\s().-]{6,30})\s*$",
        event_type: "ringing",
    },
    VoipFilter {
        display_name: "8x8",
        app_id_patterns: &["8x8", "com.8x8.work"],
        text_pattern: r"(?i)incoming call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))\s*$",
        event_type: "ringing",
    },
    VoipFilter {
        display_name: "8x8",
        app_id_patterns: &["8x8", "com.8x8.work"],
        text_pattern: r"(?i)missed call from\s+(?P<name>[^•—–\n]+?)\s*[•—–\-]\s*(?P<phone>\+?[\d\s().-]{6,30})\s*$",
        event_type: "missed",
    },
    VoipFilter {
        display_name: "8x8",
        app_id_patterns: &["8x8", "com.8x8.work"],
        text_pattern: r"(?i)missed call from\s+(?:(?P<phone>\+?[\d\s().-]{6,30})|(?P<name>[^\n]+?))\s*$",
        event_type: "missed",
    },

    // ─── Cisco Webex Calling ──────────────────────────────────────
    VoipFilter {
        display_name: "Cisco Webex Calling",
        app_id_patterns: &["webex", "com.cisco.webex", "cisco.webex"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
        event_type: "ringing",
    },
];

#[derive(Debug, Clone)]
pub struct FilterMatch {
    pub source_app: &'static str,
    pub caller_phone_e164: Option<String>,
    pub caller_display_name: Option<String>,
    /// What event_type to report — driven by the matched filter.
    pub event_type: &'static str,
}

/// Returns the matching filter result if any built-in pattern matches
/// the incoming notification. First-match-wins; filters are
/// evaluated in declaration order so more-specific entries (e.g.
/// "Missed call from") should precede more-general ones.
pub fn match_notification(app_id: &str, title: &str, body: &str) -> Option<FilterMatch> {
    let app_lower = app_id.to_lowercase();
    let combined = format!("{} — {}", title.trim(), body.trim());

    for filter in BUILTIN_FILTERS {
        let app_matches = filter
            .app_id_patterns
            .iter()
            .any(|p| app_lower.contains(&p.to_lowercase()));
        if !app_matches {
            continue;
        }
        let re = filter_regex(filter);
        if let Some(caps) = re.captures(&combined) {
            return Some(FilterMatch {
                source_app: filter.display_name,
                caller_phone_e164: caps
                    .name("phone")
                    .map(|m| normalise_phone(m.as_str()))
                    .filter(|s| !s.is_empty()),
                caller_display_name: caps
                    .name("name")
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty()),
                event_type: filter.event_type,
            });
        }
    }
    None
}

/// Lazy-compile each filter's regex on first use. Filters are
/// effectively static so this is a one-time cost per pattern.
fn filter_regex(filter: &VoipFilter) -> &'static Regex {
    // OnceLock-per-filter via boxed-leaked indices. With only ~10
    // filters this is cleaner than a HashMap.
    static CELLS: OnceLock<Vec<OnceLock<Regex>>> = OnceLock::new();
    let cells = CELLS.get_or_init(|| BUILTIN_FILTERS.iter().map(|_| OnceLock::new()).collect());
    let idx = BUILTIN_FILTERS
        .iter()
        .position(|f| std::ptr::eq(f as *const _, filter as *const _))
        .expect("filter must be in BUILTIN_FILTERS");
    cells[idx].get_or_init(|| {
        Regex::new(filter.text_pattern).unwrap_or_else(|err| {
            panic!(
                "invalid regex for filter {}: {} ({})",
                filter.display_name, filter.text_pattern, err
            )
        })
    })
}

/// Strip whitespace and decorative chars from a captured phone number.
/// We keep a leading + and digits only.
fn normalise_phone(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    if let Some('+') = chars.peek() {
        out.push('+');
        chars.next();
    }
    for c in chars {
        if c.is_ascii_digit() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vxt_missed_call_with_name() {
        let m = match_notification(
            "nz.co.vxt.electron",
            "VXT",
            "Missed call from Bal",
        );
        let m = m.expect("should match");
        assert_eq!(m.source_app, "VXT");
        assert_eq!(m.event_type, "missed");
        assert_eq!(m.caller_display_name.as_deref(), Some("Bal"));
        assert!(m.caller_phone_e164.is_none());
    }

    #[test]
    fn vxt_missed_call_with_empty_body() {
        // Real-world: VXT puts the message in the title and leaves
        // the body empty. The "{title} — {body}" join produces
        // "Missed call from Bal — " with trailing whitespace and a
        // dangling em-dash. Name capture must not slurp the em-dash.
        let m = match_notification(
            "nz.co.vxt.electron",
            "Missed call from Bal",
            "",
        );
        let m = m.expect("should match");
        assert_eq!(m.event_type, "missed");
        assert_eq!(
            m.caller_display_name.as_deref(),
            Some("Bal"),
            "name should not include trailing separator"
        );
    }

    #[test]
    fn vxt_missed_call_with_phone() {
        let m = match_notification(
            "nz.co.vxt.electron",
            "VXT",
            "Missed call from +44 7700 900101",
        );
        let m = m.expect("should match");
        assert_eq!(m.event_type, "missed");
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+447700900101"));
    }

    #[test]
    fn vxt_incoming_kept_for_future() {
        // VXT desktop doesn't fire this today, but if/when they add
        // during-ring notifications our filter is ready.
        let m = match_notification(
            "vxt.desktop",
            "Incoming call",
            "Incoming call from +44 7700 900101",
        );
        let m = m.expect("should match");
        assert_eq!(m.event_type, "ringing");
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+447700900101"));
    }

    #[test]
    fn teams_name_only() {
        let m = match_notification(
            "Microsoft.Teams_8wekyb3d8bbwe",
            "Calling…",
            "Incoming call from Jane Smith",
        );
        let m = m.expect("should match");
        assert_eq!(m.source_app, "Microsoft Teams");
        assert_eq!(m.event_type, "ringing");
        assert_eq!(m.caller_display_name.as_deref(), Some("Jane Smith"));
    }

    #[test]
    fn aircall_incoming_with_name_and_phone() {
        let m = match_notification(
            "com.aircall.desktop",
            "Aircall",
            "Incoming call from Jane Smith • +33 1 23 45 67 89",
        );
        let m = m.expect("should match");
        assert_eq!(m.source_app, "Aircall");
        assert_eq!(m.event_type, "ringing");
        assert_eq!(m.caller_display_name.as_deref(), Some("Jane Smith"));
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+33123456789"));
    }

    #[test]
    fn aircall_incoming_phone_only() {
        let m = match_notification(
            "com.aircall.desktop",
            "Aircall",
            "Incoming call from +44 7700 900101",
        );
        let m = m.expect("should match");
        assert_eq!(m.event_type, "ringing");
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+447700900101"));
    }

    #[test]
    fn eight_by_eight_incoming_with_name_and_phone() {
        let m = match_notification(
            "com.8x8.work",
            "8x8 Work",
            "Incoming call from John Doe • +44 20 7946 0958",
        );
        let m = m.expect("should match");
        assert_eq!(m.source_app, "8x8");
        assert_eq!(m.event_type, "ringing");
        assert_eq!(m.caller_display_name.as_deref(), Some("John Doe"));
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+442079460958"));
    }

    #[test]
    fn non_voip_app_ignored() {
        assert!(match_notification("com.slack.app", "DM", "Bob: hello").is_none());
    }

    #[test]
    fn kaspersky_mic_notification_ignored() {
        // From real observed log — we should NOT match Kaspersky's
        // "Mic access for Vxt Desktop is allowed" notification just
        // because "vxt" appears in the body text. The AUMID is
        // Kaspersky's, not VXT's.
        assert!(
            match_notification(
                "KasperskyLab.Kis.UI.Toasts",
                "Kaspersky",
                "Mic access for Vxt Desktop is allowed"
            )
            .is_none()
        );
    }
}
