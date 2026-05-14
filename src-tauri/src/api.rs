// src-tauri/src/api.rs
//
// HTTP client for the Kefilex desk-companion endpoints.
//
//   pair         POST /api/desk-companion/pair          exchange 6-digit code → token
//   heartbeat    POST /api/desk-companion/heartbeat     prove we're alive
//   call_event   POST /api/desk-companion/call-event    report a ringing/answered/ended event
//
// All errors bubble up as anyhow::Error; the calling Tauri commands
// convert them to strings for the WebView.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct PairResponse {
    pub ok: bool,
    pub token: String,
    pub device_id: String,
}

#[derive(Debug, Serialize)]
struct PairRequest<'a> {
    pairing_code: &'a str,
    device_label: &'a str,
    app_version: &'a str,
    os_version: String,
}

/// Exchange a 6-digit pairing code for a long-lived companion token.
/// Called exactly once per device pair operation.
pub async fn pair(
    api_base: &str,
    http: &reqwest::Client,
    code: &str,
    device_label: &str,
) -> Result<PairResponse> {
    let url = format!("{}/api/desk-companion/pair", api_base);
    let body = PairRequest {
        pairing_code: code,
        device_label,
        app_version: env!("CARGO_PKG_VERSION"),
        os_version: os_version_string(),
    };
    let res = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("network failure calling /pair")?;
    let status = res.status();
    let text = res.text().await.context("read /pair body")?;
    if !status.is_success() {
        return Err(anyhow!("/pair returned {}: {}", status, truncate(&text, 300)));
    }
    let parsed: PairResponse = serde_json::from_str(&text).context("parse /pair response")?;
    if !parsed.ok {
        return Err(anyhow!("/pair returned ok=false: {}", truncate(&text, 300)));
    }
    Ok(parsed)
}

#[derive(Debug, Serialize)]
struct HeartbeatRequest {
    app_version: &'static str,
    os_version: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    pub ok: bool,
    pub server_time: String,
}

/// Ping the server to refresh last_seen_at. Called every 60 seconds
/// by the heartbeat loop while paired.
pub async fn heartbeat(
    api_base: &str,
    http: &reqwest::Client,
    token: &str,
) -> Result<HeartbeatResponse> {
    let url = format!("{}/api/desk-companion/heartbeat", api_base);
    let body = HeartbeatRequest {
        app_version: env!("CARGO_PKG_VERSION"),
        os_version: os_version_string(),
    };
    let res = http
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .context("network failure calling /heartbeat")?;
    let status = res.status();
    let text = res.text().await.context("read /heartbeat body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "/heartbeat returned {}: {}",
            status,
            truncate(&text, 300)
        ));
    }
    serde_json::from_str(&text).context("parse /heartbeat response")
}

#[derive(Debug, Serialize, Clone)]
pub struct CallEvent<'a> {
    pub event_type: &'a str, // "ringing" | "answered" | "ended"
    pub caller_phone_e164: Option<String>,
    pub firm_phone_e164: Option<String>,
    pub source_app: Option<String>,
    pub started_at: String, // ISO-8601 UTC
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intended_fee_earner_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vxt_call_id: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CallEventResponse {
    pub ok: bool,
    pub call_id: String,
    pub server_time: String,
}

/// Push a call event to Kefilex. Fire-and-forget from the listener's
/// perspective — the listener thread doesn't block waiting for the
/// response.
pub async fn call_event(
    api_base: &str,
    http: &reqwest::Client,
    token: &str,
    event: &CallEvent<'_>,
) -> Result<CallEventResponse> {
    let url = format!("{}/api/desk-companion/call-event", api_base);
    let res = http
        .post(&url)
        .bearer_auth(token)
        .json(event)
        .send()
        .await
        .context("network failure calling /call-event")?;
    let status = res.status();
    let text = res.text().await.context("read /call-event body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "/call-event returned {}: {}",
            status,
            truncate(&text, 300)
        ));
    }
    serde_json::from_str(&text).context("parse /call-event response")
}

fn os_version_string() -> String {
    // tauri-plugin-os gives a richer version string but is async +
    // requires a Tauri context. For our purposes we report the
    // compile-time platform; the Windows build picks up the actual
    // OS version via heartbeat metadata later.
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
