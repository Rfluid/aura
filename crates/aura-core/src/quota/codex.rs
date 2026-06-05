//! Codex quota / `/status` data source.
//!
//! Tries the live ChatGPT backend (`GET /backend-api/wham/usage`) first using
//! credentials from `~/.codex/auth.json`. On any failure — missing auth,
//! expired refresh token, network — falls back to the most recent
//! `rate_limits` block embedded in a `token_count` event in the local
//! session rollouts. Both paths produce the same `QuotaSnapshot` shape so
//! the UI and CLI render identically; `source` distinguishes them.
//!
//! Wham response shape (per the upstream OpenAPI):
//!
//! ```json
//! {
//!   "plan_type": "plus",
//!   "rate_limit": {
//!     "allowed": true,
//!     "limit_reached": false,
//!     "primary_window":   { "used_percent": 21, "limit_window_seconds": 18000,  "reset_at": 1779422413 },
//!     "secondary_window": { "used_percent":  5, "limit_window_seconds": 604800, "reset_at": 1780009212 }
//!   },
//!   "additional_rate_limits": [ … ],
//!   "rate_limit_reached_type": null
//! }
//! ```
//!
//! Local rollout shape (the historical source we now use as fallback):
//! `{config_path}/sessions/{YYYY}/{MM}/{DD}/rollout-*.jsonl`. Each `event_msg`
//! line with `payload.type == "token_count"` carries a `payload.rate_limits`
//! block with the same numbers Codex got back from the server.

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{codex_oauth, QuotaSnapshot, QuotaSource, QuotaWindow};

const WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

// ── On-disk shape (local fallback) ────────────────────────────────────────────

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TokenCountPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    rate_limits: Option<RawRateLimits>,
}

#[derive(Deserialize)]
struct RawRateLimits {
    primary: Option<LocalWindow>,
    secondary: Option<LocalWindow>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
}

#[derive(Deserialize)]
struct LocalWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

// ── Wham `/usage` shape (live API) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WhamUsage {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<WhamRateLimitDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<WhamAdditionalRateLimit>>,
    #[serde(default)]
    rate_limit_reached_type: Option<WhamRateLimitReached>,
}

#[derive(Debug, Deserialize)]
struct WhamRateLimitDetails {
    #[serde(default)]
    primary_window: Option<WhamWindow>,
    #[serde(default)]
    secondary_window: Option<WhamWindow>,
}

#[derive(Debug, Deserialize)]
struct WhamAdditionalRateLimit {
    /// e.g. "image_generation", used as the default label.
    #[serde(default)]
    metered_feature: Option<String>,
    /// Human-readable name from the backend; preferred over `metered_feature`.
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    rate_limit: Option<WhamRateLimitDetails>,
}

#[derive(Debug, Deserialize)]
struct WhamWindow {
    /// 0-100 integer per the OpenAPI spec, but accept fractional just in case
    /// the backend ever switches representations.
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WhamRateLimitReached {
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

// ── Public reader ─────────────────────────────────────────────────────────────

pub struct CodexQuota {
    codex_config_dir: PathBuf,
}

impl CodexQuota {
    pub fn new(codex_config_dir: PathBuf) -> Self {
        Self { codex_config_dir }
    }

    /// Try the API; on any failure, fall back to the last `token_count` event
    /// in the local session rollouts. Mirrors `quota::api::QuotaApi::snapshot`.
    pub fn snapshot(&self) -> QuotaSnapshot {
        match self.snapshot_via_api() {
            Ok(snap) => snap,
            Err(api_err) => match self.snapshot_local() {
                Ok(Some(mut snap)) => {
                    snap.source = QuotaSource::Fallback;
                    snap.note = Some(format!(
                        "API unavailable ({api_err}); showing last observed token_count rate limits"
                    ));
                    snap
                }
                Ok(None) => QuotaSnapshot::unavailable(format!(
                    "API failed: {api_err}; no local rate limits available yet"
                )),
                Err(local_err) => QuotaSnapshot::unavailable(format!(
                    "API failed: {api_err}; local fallback failed: {local_err}"
                )),
            },
        }
    }

    // ── API path ──────────────────────────────────────────────────────────────

    fn snapshot_via_api(&self) -> Result<QuotaSnapshot> {
        let tokens = codex_oauth::ensure_fresh(&self.codex_config_dir)?;

        let mut request = ureq::get(WHAM_USAGE_URL)
            .header("authorization", format!("Bearer {}", tokens.access_token))
            .header("user-agent", concat!("aura/", env!("CARGO_PKG_VERSION")))
            .header("accept", "application/json");
        if let Some(account_id) = &tokens.account_id {
            request = request.header("chatgpt-account-id", account_id.as_str());
        }

        let mut response = request
            .call()
            .map_err(|e| anyhow!("/wham/usage call failed: {e}"))?;

        if response.status() != 200 {
            let status = response.status();
            let body = response
                .body_mut()
                .read_to_string()
                .unwrap_or_else(|_| "<unreadable>".to_string());
            return Err(anyhow!("/wham/usage returned HTTP {status}: {body}"));
        }

        let raw_body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| anyhow!("reading /wham/usage body: {e}"))?;

        let usage: WhamUsage = serde_json::from_str(&raw_body).map_err(|e| {
            anyhow!(
                "parsing /wham/usage response: {e}; raw body: {}",
                truncate(&raw_body, 400)
            )
        })?;

        Ok(snapshot_from_wham(usage, tokens.chatgpt_plan_type))
    }

    // ── Local fallback ────────────────────────────────────────────────────────

    fn snapshot_local(&self) -> Result<Option<QuotaSnapshot>> {
        let files = list_rollout_files_newest_first(&self.codex_config_dir)?;
        for path in files {
            if let Some((observed_at, rl)) = latest_rate_limits_in_file(&path)? {
                return Ok(Some(snapshot_from_local(observed_at, rl)));
            }
        }
        Ok(None)
    }
}

// ── Wham → QuotaSnapshot ──────────────────────────────────────────────────────

fn snapshot_from_wham(usage: WhamUsage, fallback_plan: Option<String>) -> QuotaSnapshot {
    let mut windows = Vec::new();
    if let Some(details) = &usage.rate_limit {
        if let Some(w) = &details.primary_window {
            windows.push(window_from_wham("Primary", w));
        }
        if let Some(w) = &details.secondary_window {
            windows.push(window_from_wham("Secondary", w));
        }
    }
    if let Some(extras) = &usage.additional_rate_limits {
        for extra in extras {
            let label = extra
                .limit_name
                .clone()
                .or_else(|| extra.metered_feature.clone())
                .unwrap_or_else(|| "Additional".to_string());
            if let Some(details) = &extra.rate_limit {
                if let Some(w) = &details.primary_window {
                    windows.push(window_from_wham(&label, w));
                }
                if let Some(w) = &details.secondary_window {
                    windows.push(window_from_wham(&format!("{label} (secondary)"), w));
                }
            }
        }
    }

    let note = usage
        .rate_limit_reached_type
        .as_ref()
        .and_then(|r| r.kind.as_deref())
        .filter(|s| !s.is_empty())
        .map(|kind| format!("Rate limit reached ({kind})."));

    QuotaSnapshot {
        subscription_type: usage.plan_type.or(fallback_plan),
        windows,
        source: QuotaSource::Api,
        note,
        pacing_pattern: None,
    }
}

fn window_from_wham(default_label: &str, w: &WhamWindow) -> QuotaWindow {
    let window_minutes = w.limit_window_seconds.and_then(window_minutes_from_seconds);
    let label = match window_minutes {
        Some(m) => format_window_label(default_label, m as u64),
        None => default_label.to_string(),
    };
    QuotaWindow {
        label,
        used_percentage: w.used_percent,
        used_tokens: None,
        resets_at: w.reset_at.and_then(unix_to_datetime),
        length_minutes: window_minutes.and_then(|m| u32::try_from(m).ok()),
    }
}

/// Round up to whole minutes — matches the upstream Codex client's behaviour
/// in `backend-client/src/client.rs::window_minutes_from_seconds`.
fn window_minutes_from_seconds(seconds: i64) -> Option<i64> {
    if seconds <= 0 {
        return None;
    }
    Some((seconds + 59) / 60)
}

// ── File discovery (newest first) ─────────────────────────────────────────────

/// Returns `rollout-*.jsonl` paths sorted newest-first by modification time so
/// callers can short-circuit on the first file that yields a token_count event.
fn list_rollout_files_newest_first(config_path: &Path) -> Result<Vec<PathBuf>> {
    let sessions_dir = config_path.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for year in read_subdirs(&sessions_dir)? {
        for month in read_subdirs(&year)? {
            for day in read_subdirs(&month)? {
                for entry in fs::read_dir(&day)? {
                    let path = entry?.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let mtime = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(UNIX_EPOCH);
                    files.push((path, mtime));
                }
            }
        }
    }
    files.sort_by_key(|(_path, mtime)| std::cmp::Reverse(*mtime));
    Ok(files.into_iter().map(|(p, _)| p).collect())
}

fn read_subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    Ok(out)
}

// ── Per-file scan ─────────────────────────────────────────────────────────────

/// Walk a rollout file and return the timestamp + rate_limits from the LAST
/// `token_count` event it contains, or `None` if it has no token_count events.
fn latest_rate_limits_in_file(path: &Path) -> Result<Option<(String, RawRateLimits)>> {
    let content = fs::read_to_string(path)?;
    let mut latest: Option<(String, RawRateLimits)> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || !line.contains("token_count") {
            // Cheap text gate: token_count must appear as a substring before
            // we pay the cost of full JSON parsing.
            continue;
        }
        let entry: RawLine = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.entry_type != "event_msg" {
            continue;
        }
        let Some(payload) = entry.payload else {
            continue;
        };
        let Ok(p) = serde_json::from_value::<TokenCountPayload>(payload) else {
            continue;
        };
        if p.payload_type.as_deref() != Some("token_count") {
            continue;
        }
        let Some(rl) = p.rate_limits else {
            continue;
        };
        let ts = entry.timestamp.unwrap_or_default();
        latest = Some((ts, rl));
    }

    Ok(latest)
}

// ── Local snapshot builder ────────────────────────────────────────────────────

fn snapshot_from_local(observed_at: String, rl: RawRateLimits) -> QuotaSnapshot {
    let mut windows = Vec::new();
    if let Some(w) = rl.primary {
        windows.push(window_from_local("Primary", &w));
    }
    if let Some(w) = rl.secondary {
        windows.push(window_from_local("Secondary", &w));
    }

    let mut note = format_observed_note(&observed_at);
    if let Some(kind) = rl.rate_limit_reached_type {
        if !kind.is_empty() {
            note = Some(match note {
                Some(prev) => format!("Rate limit reached ({kind}). {prev}"),
                None => format!("Rate limit reached ({kind})."),
            });
        }
    }

    QuotaSnapshot {
        subscription_type: rl.plan_type,
        windows,
        source: QuotaSource::Fallback,
        note,
        pacing_pattern: None,
    }
}

fn window_from_local(default_label: &str, w: &LocalWindow) -> QuotaWindow {
    let label = match w.window_minutes {
        Some(m) => format_window_label(default_label, m),
        None => default_label.to_string(),
    };
    QuotaWindow {
        label,
        used_percentage: w.used_percent,
        used_tokens: None,
        resets_at: w.resets_at.and_then(unix_to_datetime),
        length_minutes: w.window_minutes.and_then(|m| u32::try_from(m).ok()),
    }
}

// ── Helpers shared by both paths ──────────────────────────────────────────────

fn format_window_label(default_label: &str, window_minutes: u64) -> String {
    let span = if window_minutes.is_multiple_of(60 * 24 * 7) {
        let weeks = window_minutes / (60 * 24 * 7);
        if weeks == 1 {
            "weekly".to_string()
        } else {
            format!("{weeks}-week")
        }
    } else if window_minutes.is_multiple_of(60 * 24) {
        let days = window_minutes / (60 * 24);
        format!("{days}-day")
    } else if window_minutes.is_multiple_of(60) {
        let hours = window_minutes / 60;
        format!("{hours}h")
    } else {
        format!("{window_minutes}m")
    };
    format!("{default_label} · {span}")
}

fn unix_to_datetime(secs: i64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(secs, 0)
}

fn format_observed_note(observed_at: &str) -> Option<String> {
    if observed_at.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(observed_at)
        .ok()
        .map(|dt| {
            format!(
                "Last observed: {}",
                dt.with_timezone(&Utc).format("%Y-%m-%d %H:%M UTC")
            )
        })
        .or_else(|| Some(format!("Last observed: {observed_at}")))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = String::with_capacity(max + 1);
        out.push_str(&s[..max]);
        out.push('…');
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        path
    }

    fn day_dir(base: &Path, year: &str, month: &str, day: &str) -> PathBuf {
        let dir = base.join("sessions").join(year).join(month).join(day);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn token_count_line(ts: &str, used_primary: f64, used_secondary: f64, plan: &str) -> String {
        serde_json::json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "last_token_usage": { "input_tokens": 10, "output_tokens": 5 } },
                "rate_limits": {
                    "limit_id": "codex",
                    "primary":   { "used_percent": used_primary,   "window_minutes": 300,   "resets_at": 1779422413i64 },
                    "secondary": { "used_percent": used_secondary, "window_minutes": 10080, "resets_at": 1780009212i64 },
                    "plan_type": plan,
                    "rate_limit_reached_type": null
                }
            }
        })
        .to_string()
    }

    #[test]
    fn snapshot_returns_unavailable_when_no_sessions() {
        let dir = tempdir().unwrap();
        let snap = CodexQuota::new(dir.path().to_path_buf()).snapshot();
        // No auth.json AND no sessions → API fails, local returns None → Unavailable.
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.note.is_some());
    }

    #[test]
    fn local_snapshot_returns_most_recent_token_count_in_file() {
        let dir = tempdir().unwrap();
        let day = day_dir(dir.path(), "2026", "05", "21");
        write_jsonl(
            &day,
            "rollout-1.jsonl",
            &[
                &token_count_line("2026-05-21T23:00:00Z", 10.0, 1.0, "plus"),
                &token_count_line("2026-05-21T23:30:00Z", 32.0, 5.0, "plus"),
            ],
        );

        // Drive the local path directly so we don't depend on the API failing
        // in a particular way for this assertion.
        let snap = CodexQuota::new(dir.path().to_path_buf())
            .snapshot_local()
            .unwrap()
            .expect("local snapshot present");
        assert_eq!(snap.source, QuotaSource::Fallback);
        assert_eq!(snap.subscription_type.as_deref(), Some("plus"));
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].used_percentage, Some(32.0));
        assert_eq!(snap.windows[1].used_percentage, Some(5.0));
        assert!(snap.windows[0].label.contains("5h"));
        assert!(snap.windows[1].label.contains("weekly"));
    }

    #[test]
    fn snapshot_uses_local_fallback_with_note_when_api_fails() {
        let dir = tempdir().unwrap();
        let day = day_dir(dir.path(), "2026", "05", "21");
        write_jsonl(
            &day,
            "rollout.jsonl",
            &[&token_count_line("2026-05-21T23:30:00Z", 32.0, 5.0, "plus")],
        );
        // No auth.json → `snapshot_via_api` errors immediately, no network hit.
        let snap = CodexQuota::new(dir.path().to_path_buf()).snapshot();
        assert_eq!(snap.source, QuotaSource::Fallback);
        let note = snap.note.unwrap();
        assert!(
            note.contains("API unavailable"),
            "expected API-unavailable note, got: {note}"
        );
        assert_eq!(snap.windows.len(), 2);
    }

    #[test]
    fn local_snapshot_prefers_newest_file_by_mtime() {
        let dir = tempdir().unwrap();
        let day1 = day_dir(dir.path(), "2026", "05", "20");
        let day2 = day_dir(dir.path(), "2026", "05", "21");

        let old = write_jsonl(
            &day1,
            "rollout-old.jsonl",
            &[&token_count_line("2026-05-20T10:00:00Z", 1.0, 0.5, "plus")],
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = old;
        write_jsonl(
            &day2,
            "rollout-new.jsonl",
            &[&token_count_line(
                "2026-05-21T10:00:00Z",
                99.0,
                50.0,
                "plus",
            )],
        );

        let snap = CodexQuota::new(dir.path().to_path_buf())
            .snapshot_local()
            .unwrap()
            .expect("local snapshot present");
        assert_eq!(snap.windows[0].used_percentage, Some(99.0));
    }

    #[test]
    fn local_snapshot_falls_back_to_earlier_file_when_latest_has_no_token_count() {
        let dir = tempdir().unwrap();
        let day = day_dir(dir.path(), "2026", "05", "21");

        write_jsonl(
            &day,
            "rollout-old.jsonl",
            &[&token_count_line("2026-05-21T09:00:00Z", 42.0, 7.0, "plus")],
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_jsonl(
            &day,
            "rollout-empty.jsonl",
            &[r#"{"timestamp":"2026-05-21T10:00:00Z","type":"session_meta","payload":{"id":"x"}}"#],
        );

        let snap = CodexQuota::new(dir.path().to_path_buf())
            .snapshot_local()
            .unwrap()
            .expect("local snapshot present");
        assert_eq!(snap.windows[0].used_percentage, Some(42.0));
    }

    #[test]
    fn window_label_formats_common_durations() {
        assert_eq!(format_window_label("Primary", 300), "Primary · 5h");
        assert_eq!(
            format_window_label("Secondary", 10080),
            "Secondary · weekly"
        );
        assert_eq!(format_window_label("X", 1440), "X · 1-day");
        assert_eq!(format_window_label("X", 30), "X · 30m");
    }

    #[test]
    fn wham_snapshot_parses_primary_secondary_windows() {
        let body = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window":   { "used_percent": 21, "limit_window_seconds": 18000,  "reset_at": 1779422413 },
                "secondary_window": { "used_percent":  5, "limit_window_seconds": 604800, "reset_at": 1780009212 }
            },
            "rate_limit_reached_type": null
        }"#;
        let usage: WhamUsage = serde_json::from_str(body).unwrap();
        let snap = snapshot_from_wham(usage, None);
        assert_eq!(snap.source, QuotaSource::Api);
        assert_eq!(snap.subscription_type.as_deref(), Some("plus"));
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].used_percentage, Some(21.0));
        assert_eq!(snap.windows[0].length_minutes, Some(300));
        assert!(snap.windows[0].label.contains("5h"));
        assert_eq!(snap.windows[1].length_minutes, Some(10080));
        assert!(snap.windows[1].label.contains("weekly"));
        assert!(snap.note.is_none());
    }

    #[test]
    fn wham_snapshot_surfaces_rate_limit_reached_type() {
        let body = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "allowed": false,
                "limit_reached": true,
                "primary_window":   { "used_percent": 100, "limit_window_seconds": 18000,  "reset_at": 1779422413 },
                "secondary_window": { "used_percent":  50, "limit_window_seconds": 604800, "reset_at": 1780009212 }
            },
            "rate_limit_reached_type": { "type": "rate_limit_reached" }
        }"#;
        let usage: WhamUsage = serde_json::from_str(body).unwrap();
        let snap = snapshot_from_wham(usage, None);
        let note = snap.note.unwrap();
        assert!(
            note.contains("Rate limit reached"),
            "expected reached-type note, got: {note}"
        );
    }

    #[test]
    fn wham_snapshot_falls_back_to_token_plan_when_payload_missing_it() {
        let body = r#"{
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": { "used_percent": 10, "limit_window_seconds": 18000, "reset_at": 1779422413 }
            }
        }"#;
        let usage: WhamUsage = serde_json::from_str(body).unwrap();
        let snap = snapshot_from_wham(usage, Some("pro".to_string()));
        assert_eq!(snap.subscription_type.as_deref(), Some("pro"));
    }
}
