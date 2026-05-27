//! `/api/oauth/usage` client + local-fallback computation.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::reader::scan::{list_session_files, scan_files};

use super::{oauth, QuotaSnapshot, QuotaWindow};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Where the snapshot came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuotaSource {
    /// Got real numbers from `/api/oauth/usage`.
    Api,
    /// Computed from local JSONL data (no `% used` because limits unknown).
    Fallback,
    /// Neither API nor JSONL available; `note` explains why.
    #[default]
    Unavailable,
}

// ── /api/oauth/usage response shape ───────────────────────────────────────────
//
// Anthropic ships two flavours of the body in the wild:
//
//   { "rate_limits": { "five_hour": {...}, "seven_day": {...}, ... } }    // older
//   {                  "five_hour": {...}, "seven_day": {...}, ... }      // newer
//
// And each window object uses either:
//   - `utilization`     (0.0-1.0)   + `resetsAt`     (camelCase ISO)
//   - `used_percentage` (0.0-100.0) + `resets_at`    (snake_case ISO)
//
// We accept both — parse into a single `RawWindow` and normalize after.

#[derive(Debug, Deserialize)]
struct UsageEnvelope {
    /// Older shape: rate limits live under `rate_limits`.
    #[serde(default)]
    rate_limits: Option<RateLimits>,

    /// Newer shape: rate limits at the root level.
    #[serde(default)]
    five_hour: Option<RawWindow>,
    #[serde(default)]
    seven_day: Option<RawWindow>,
    #[serde(default)]
    seven_day_opus: Option<RawWindow>,
    #[serde(default)]
    seven_day_sonnet: Option<RawWindow>,
    #[serde(default)]
    overage: Option<RawWindow>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<RawWindow>,
    #[serde(default)]
    seven_day: Option<RawWindow>,
    #[serde(default)]
    seven_day_opus: Option<RawWindow>,
    #[serde(default)]
    seven_day_sonnet: Option<RawWindow>,
    #[serde(default)]
    overage: Option<RawWindow>,
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    /// 0.0-1.0 fraction (newer shape).
    #[serde(default)]
    utilization: Option<f64>,
    /// 0.0-100.0 percentage (older shape).
    #[serde(default)]
    used_percentage: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(rename = "resetsAt", default)]
    resets_at_camel: Option<String>,
}

impl RawWindow {
    /// Normalised percentage (0.0–100.0). The real `/api/oauth/usage` body
    /// returns `utilization` already in 0–100 (e.g. `seven_day: 60.0`),
    /// despite what the Claude Code binary suggests for header values.
    fn percentage(&self) -> Option<f64> {
        self.utilization.or(self.used_percentage)
    }

    fn resets_at(&self) -> Option<&str> {
        self.resets_at
            .as_deref()
            .or(self.resets_at_camel.as_deref())
    }
}

pub struct QuotaApi {
    claude_config_dir: PathBuf,
}

impl QuotaApi {
    pub fn new(claude_config_dir: PathBuf) -> Self {
        Self { claude_config_dir }
    }

    /// Try the API; on any failure, fall back to a local computation.
    pub fn snapshot(&self) -> QuotaSnapshot {
        match self.snapshot_via_api() {
            Ok(snap) => snap,
            Err(api_err) => match self.snapshot_local() {
                Ok(mut snap) => {
                    snap.note = Some(format!(
                        "API unavailable ({api_err}); showing local token counts"
                    ));
                    snap
                }
                Err(local_err) => QuotaSnapshot::unavailable(format!(
                    "API failed: {api_err}; local fallback failed: {local_err}"
                )),
            },
        }
    }

    // ── API path ──────────────────────────────────────────────────────────────

    fn snapshot_via_api(&self) -> Result<QuotaSnapshot> {
        let creds = oauth::ensure_fresh(&self.claude_config_dir)?;

        let mut response = ureq::get(USAGE_URL)
            .header("authorization", format!("Bearer {}", creds.access_token))
            .header("anthropic-beta", OAUTH_BETA)
            .header("content-type", "application/json")
            .call()
            .map_err(|e| anyhow!("/api/oauth/usage call failed: {e}"))?;

        if response.status() != 200 {
            let status = response.status();
            let body = response
                .body_mut()
                .read_to_string()
                .unwrap_or_else(|_| "<unreadable>".to_string());
            return Err(anyhow!("/api/oauth/usage returned HTTP {status}: {body}"));
        }

        // Read body as a string so we can surface it on parse failure.
        let raw_body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| anyhow!("reading /api/oauth/usage body: {e}"))?;

        let envelope: UsageEnvelope = serde_json::from_str(&raw_body).map_err(|e| {
            anyhow!(
                "parsing /api/oauth/usage response: {e}; raw body: {}",
                truncate(&raw_body, 400)
            )
        })?;

        // Newer shape lives at the root; older shape lives under `rate_limits`.
        // We accept either — collect into one structure.
        let (five_hour, seven_day, seven_day_opus, seven_day_sonnet, overage) =
            match envelope.rate_limits {
                Some(rl) => (
                    rl.five_hour.or(envelope.five_hour),
                    rl.seven_day.or(envelope.seven_day),
                    rl.seven_day_opus.or(envelope.seven_day_opus),
                    rl.seven_day_sonnet.or(envelope.seven_day_sonnet),
                    rl.overage.or(envelope.overage),
                ),
                None => (
                    envelope.five_hour,
                    envelope.seven_day,
                    envelope.seven_day_opus,
                    envelope.seven_day_sonnet,
                    envelope.overage,
                ),
            };

        let mut windows = Vec::new();
        if let Some(w) = five_hour {
            windows.push(api_window("Current session", &w));
        }
        if let Some(w) = seven_day {
            windows.push(api_window("Current week (all models)", &w));
        }
        if let Some(w) = seven_day_opus {
            windows.push(api_window("Current week (Opus only)", &w));
        }
        if let Some(w) = seven_day_sonnet {
            windows.push(api_window("Current week (Sonnet only)", &w));
        }
        if let Some(w) = overage {
            windows.push(api_window("Overage", &w));
        }

        // The API responded successfully but with no recognisable windows —
        // either the user has no active subscription, or Anthropic changed the
        // schema. Push the raw body up so the user can see what we got.
        if windows.is_empty() {
            return Err(anyhow!(
                "API returned HTTP 200 but no recognisable rate limit windows. \
                 Raw body: {}",
                truncate(&raw_body, 400)
            ));
        }

        Ok(QuotaSnapshot {
            subscription_type: creds.subscription_type.clone(),
            windows,
            source: QuotaSource::Api,
            note: None,
        })
    }

    // ── Local fallback ────────────────────────────────────────────────────────

    fn snapshot_local(&self) -> Result<QuotaSnapshot> {
        let files = list_session_files(&self.claude_config_dir)?;
        let now = Utc::now();
        let day7_ago = now - Duration::days(7);
        let hour5_ago = now - Duration::hours(5);

        // Scan with a date filter, then we manually filter by time-of-day
        // boundaries below (mtime-pruning at the scan level avoids reading
        // older files at all).
        let day8_filter = (now - Duration::days(8)).format("%Y-%m-%d").to_string();
        let accum = scan_files(&files, Some(&day8_filter), None)?;

        let mut seven_day = 0u64;
        let mut five_hour = 0u64;
        for s in &accum.sessions {
            if let Some(ts) = parse_ts(&s.start_timestamp) {
                if ts >= day7_ago {
                    // Use the per-session daily breakdown summed across models;
                    // ScanAccum doesn't track per-session token totals, so we
                    // approximate using the daily_model_tokens for the session
                    // date — close enough for a fallback view.
                    if let Some(date_str) = s.start_timestamp.get(..10) {
                        if let Some(per_model) = accum.daily_model_tokens.get(date_str) {
                            let day_total: u64 = per_model.values().sum();
                            // Spread daily total across sessions on that day
                            // proportional to session count (fallback only).
                            let n_sessions = accum
                                .daily_session_counts
                                .get(date_str)
                                .copied()
                                .unwrap_or(1)
                                .max(1);
                            let per_session = day_total / n_sessions;
                            seven_day = seven_day.saturating_add(per_session);
                            if ts >= hour5_ago {
                                five_hour = five_hour.saturating_add(per_session);
                            }
                        }
                    }
                }
            }
        }

        let mut windows = Vec::new();
        if accum.sessions.iter().any(|s| {
            parse_ts(&s.start_timestamp)
                .map(|t| t >= hour5_ago)
                .unwrap_or(false)
        }) {
            windows.push(QuotaWindow {
                label: "Current session (local est.)".to_string(),
                used_percentage: None,
                used_tokens: Some(five_hour),
                resets_at: Some(next_five_hour_reset(now)),
                length_minutes: None,
            });
        }
        windows.push(QuotaWindow {
            label: "Last 7 days (local est.)".to_string(),
            used_percentage: None,
            used_tokens: Some(seven_day),
            resets_at: Some(next_weekly_reset(now)),
            length_minutes: None,
        });

        Ok(QuotaSnapshot {
            subscription_type: None,
            windows,
            source: QuotaSource::Fallback,
            note: None,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn api_window(label: &str, w: &RawWindow) -> QuotaWindow {
    // Claude Code's `/api/oauth/usage` doesn't expose window lengths, so we
    // hardcode the closed set of windows the API actually returns.
    let length_minutes = match label {
        "Current session" => Some(5 * 60),
        "Current week (all models)" | "Current week (Opus only)" | "Current week (Sonnet only)" => {
            Some(7 * 24 * 60)
        }
        _ => None,
    };
    QuotaWindow {
        label: label.to_string(),
        used_percentage: w.percentage(),
        used_tokens: None,
        resets_at: w.resets_at().and_then(parse_ts),
        length_minutes,
    }
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

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                .ok()
                .map(|d| d.and_utc())
        })
}

/// Pure rolling 5h window — for local fallback we just project 5 hours forward
/// since we don't know when Claude considers the window started.
fn next_five_hour_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::hours(5)
}

/// Approximate: next Monday at 00:00 UTC. Real Claude weekly window is tied
/// to subscription start; this is a placeholder for the local fallback view.
fn next_weekly_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    let days_until_monday = (7 - now.weekday().num_days_from_monday()) % 7;
    let target_date = (now + Duration::days(days_until_monday as i64))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default();
    Utc.from_utc_datetime(&target_date)
}

// Force linker keep — silences "unused" if a downstream binary only uses
// `QuotaSource` indirectly.
#[allow(dead_code)]
fn _force_path_use(_: &Path) -> SystemTime {
    SystemTime::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_snake_case_response() {
        // Older flavour: rate_limits wrapper + used_percentage + resets_at.
        let body = r#"{
            "rate_limits": {
                "five_hour":      { "used_percentage": 21.0, "resets_at": "2026-05-21T18:50:00Z" },
                "seven_day":      { "used_percentage": 60.0, "resets_at": "2026-05-22T19:00:00Z" },
                "seven_day_opus": { "used_percentage": 45.0, "resets_at": "2026-05-22T19:00:00Z" }
            }
        }"#;
        let env: UsageEnvelope = serde_json::from_str(body).unwrap();
        let rl = env.rate_limits.unwrap();
        let five = rl.five_hour.unwrap();
        assert_eq!(five.percentage(), Some(21.0));
        assert_eq!(five.resets_at(), Some("2026-05-21T18:50:00Z"));
        assert!(rl.seven_day_opus.is_some());
    }

    #[test]
    fn parses_real_flat_response() {
        // Captured from a live `GET /api/oauth/usage` against a Pro account:
        // flat object at the root, utilization already in 0-100, nulls
        // for windows the user doesn't have, and `extra_usage` block.
        let body = r#"{
            "five_hour": { "utilization": 24.0, "resets_at": "2026-05-21T21:50:01.031453+00:00" },
            "seven_day": { "utilization": 60.0, "resets_at": "2026-05-22T19:00:01.031475+00:00" },
            "seven_day_oauth_apps": null,
            "seven_day_opus": null,
            "seven_day_sonnet": { "utilization": 8.0, "resets_at": "2026-05-22T19:00:00.031485+00:00" },
            "seven_day_cowork": null,
            "tangelo": null,
            "extra_usage": {
                "is_enabled": false, "monthly_limit": null, "used_credits": null,
                "utilization": null, "currency": null, "disabled_reason": null
            }
        }"#;
        let env: UsageEnvelope = serde_json::from_str(body).unwrap();
        assert!(env.rate_limits.is_none());
        let five = env.five_hour.as_ref().unwrap();
        assert_eq!(five.percentage(), Some(24.0));
        assert_eq!(five.resets_at(), Some("2026-05-21T21:50:01.031453+00:00"));
        let week = env.seven_day.as_ref().unwrap();
        assert_eq!(week.percentage(), Some(60.0));
        assert!(env.seven_day_opus.is_none());
        let sonnet = env.seven_day_sonnet.as_ref().unwrap();
        assert_eq!(sonnet.percentage(), Some(8.0));
    }

    #[test]
    fn parses_partial_response_with_only_seven_day() {
        let body = r#"{ "seven_day": { "utilization": 12.5 } }"#;
        let env: UsageEnvelope = serde_json::from_str(body).unwrap();
        assert!(env.five_hour.is_none());
        let week = env.seven_day.unwrap();
        assert_eq!(week.percentage(), Some(12.5));
        assert!(week.resets_at().is_none());
    }

    #[test]
    fn parses_empty_envelope() {
        let env: UsageEnvelope = serde_json::from_str("{}").unwrap();
        assert!(env.rate_limits.is_none());
        assert!(env.five_hour.is_none());
        assert!(env.seven_day.is_none());
    }
}
