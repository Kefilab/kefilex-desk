// src-tauri/src/voip_filters.rs
//
// Built-in registry of VoIP softphone notification patterns. When the
// notification listener sees a toast, it iterates this list. The
// FIRST filter whose source AND text both match wins; everything
// else is dropped.
//
// Source matching: app_id is a string the OS gives us identifying
// the app that posted the notification. On Windows that's the
// "AppUserModelId" (AUMID) — e.g. "vxt.desktop", "Microsoft.Teams_…".
// Some softphones use different AUMIDs across desktop / web; we
// list known variants.
//
// Text matching: regex against the notification's title plus body
// joined with " — ". Named capture `phone` extracts the phone number
// in E.164. Named capture `name` extracts a display name if the
// softphone provides one.
//
// Adding a new softphone: append an entry below. The filter file is
// versioned with the binary so users can `Help → Update filters` if
// we ship a new release with new patterns.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct VoipFilter {
    /// Display name of the softphone in the source_app field of the
    /// outgoing call event. e.g. "VXT", "Microsoft Teams".
    pub display_name: &'static str,
    /// AppUserModelId substring(s) we match against. Case-insensitive.
    pub app_id_patterns: &'static [&'static str],
    /// Regex against (title + " — " + body). Must contain a named
    /// capture group "phone" yielding the caller's number. Optional
    /// "name" capture for the display name.
    pub text_pattern: &'static str,
}

/// The built-in filter registry. v0.1 — extend as we discover new
/// softphone variants. Keep VXT first because that's the priority
/// firm's stack.
pub const BUILTIN_FILTERS: &[VoipFilter] = &[
    VoipFilter {
        display_name: "VXT",
        app_id_patterns: &["vxt", "vxt.desktop", "vxt.app"],
        // Examples observed:
        //   "Incoming call from +447700900101"
        //   "Jane Smith is calling — +447700900101"
        text_pattern: r"(?i)(?:incoming call from|is calling)\s*(?:[—–-]\s*)?(?P<phone>\+?[\d\s().-]{6,30})",
    },
    VoipFilter {
        display_name: "RingCentral",
        app_id_patterns: &["ringcentral", "com.ringcentral.app"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
    },
    VoipFilter {
        display_name: "Aircall",
        app_id_patterns: &["aircall", "com.aircall.desktop"],
        text_pattern: r"(?i)incoming call.*?(?P<phone>\+?[\d\s().-]{6,30})",
    },
    VoipFilter {
        display_name: "Microsoft Teams",
        app_id_patterns: &["microsoft.teams", "com.microsoft.teams", "teams"],
        // Teams calls show as "Incoming call from <name>" with the
        // number not always present; we still want to fire the event
        // even without a phone, so the regex makes phone optional.
        text_pattern: r"(?i)(?:incoming call from|calling)\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
    },
    VoipFilter {
        display_name: "Zoom Phone",
        app_id_patterns: &["zoom", "us.zoom.xos", "com.zoom.phone"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
    },
    VoipFilter {
        display_name: "Dialpad",
        app_id_patterns: &["dialpad", "com.dialpad.desktop"],
        text_pattern: r"(?i)(?P<name>.+?)\s+is calling.*?(?P<phone>\+?[\d\s().-]{6,30})?",
    },
    VoipFilter {
        display_name: "8x8",
        app_id_patterns: &["8x8", "com.8x8.work"],
        text_pattern: r"(?i)incoming call.*?(?P<phone>\+?[\d\s().-]{6,30})",
    },
    VoipFilter {
        display_name: "Cisco Webex Calling",
        app_id_patterns: &["webex", "com.cisco.webex", "cisco.webex"],
        text_pattern: r"(?i)incoming call from\s+(?P<name>[^—–\n]+?)(?:\s*[—–-]\s*(?P<phone>\+?[\d\s().-]{6,30}))?$",
    },
];

#[derive(Debug, Clone)]
pub struct FilterMatch {
    pub source_app: &'static str,
    pub caller_phone_e164: Option<String>,
    pub caller_display_name: Option<String>,
}

/// Returns the matching filter result if any built-in pattern matches
/// the incoming notification.
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
    fn vxt_simple_incoming() {
        let m = match_notification("vxt.desktop", "Incoming call", "Incoming call from +44 7700 900101");
        let m = m.expect("should match");
        assert_eq!(m.source_app, "VXT");
        assert_eq!(m.caller_phone_e164.as_deref(), Some("+447700900101"));
    }

    #[test]
    fn teams_name_only() {
        let m = match_notification("Microsoft.Teams_8wekyb3d8bbwe", "Calling…", "Incoming call from Jane Smith");
        let m = m.expect("should match");
        assert_eq!(m.source_app, "Microsoft Teams");
        assert_eq!(m.caller_display_name.as_deref(), Some("Jane Smith"));
    }

    #[test]
    fn non_voip_app_ignored() {
        assert!(match_notification("com.slack.app", "DM", "Bob: hello").is_none());
    }
}
