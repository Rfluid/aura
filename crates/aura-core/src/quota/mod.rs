//! Claude subscription quota windows (the data behind `claude /usage`).
//!
//! The API path tries to read the OAuth credentials from
//! `~/.claude/.credentials.json`, refresh the access token if it's expired,
//! then call `https://api.anthropic.com/api/oauth/usage`. If any of that
//! fails, callers can fall back to local counts derived from JSONL data.

mod api;
mod codex;
mod codex_oauth;
pub mod forecast;
mod gemini;
mod oauth;

pub use api::{QuotaApi, QuotaSource};
pub use codex::CodexQuota;
pub use forecast::{forecast, ForecastSnapshot, ForecastStatus, ForecastWindow};
pub use gemini::GeminiQuota;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single rate-limit window (5h "session" or 7d "weekly").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaWindow {
    pub label: String,
    /// 0.0–100.0. `None` when only absolute counts are available (fallback).
    pub used_percentage: Option<f64>,
    /// Absolute input+output tokens used in the window (may be approximate).
    pub used_tokens: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
    /// Total length of this window. Forecasts use it to derive `started_at`
    /// from `resets_at`. `None` for backends that don't expose it (e.g. the
    /// local-fallback estimator); such windows are skipped by the Forecast
    /// tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_minutes: Option<u32>,
}

/// A snapshot of the user's current quota state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSnapshot {
    /// "pro", "max", "team", "enterprise", "default_claude_ai", …
    pub subscription_type: Option<String>,
    pub windows: Vec<QuotaWindow>,
    /// Indicates whether this came from the API or was computed locally.
    pub source: QuotaSource,
    /// Set when source is `Fallback` and we want to explain why.
    pub note: Option<String>,
    /// The backend answered HTTP 429 on this poll: the numbers here are a
    /// fallback (or nothing at all), not a fresh reading. The tray keeps its
    /// last good reading rather than degrade on a throttled poll — see
    /// `tray_status::summarize_sticky`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rate_limited: bool,
}

/// An API call that failed, plus whether it failed because we were throttled.
///
/// The backends swallow API errors into a local fallback, so the HTTP status
/// would otherwise be lost by the time a snapshot reaches a caller. Carrying
/// the 429 up lets the tray tell "throttled, reuse the last reading" apart
/// from "genuinely has no data".
pub(crate) struct ApiFailure {
    pub error: anyhow::Error,
    pub rate_limited: bool,
}

impl ApiFailure {
    pub fn rate_limited(error: anyhow::Error) -> Self {
        Self {
            error,
            rate_limited: true,
        }
    }
}

/// Turn a failed `ureq` call into an [`ApiFailure`].
///
/// ureq 3 raises a non-2xx status as `Error::StatusCode` from `call()` rather
/// than handing back a response, so the 429 has to be read off the error —
/// a `response.status()` check downstream never sees it.
pub(crate) fn call_failure(what: &str, err: ureq::Error) -> ApiFailure {
    let rate_limited = matches!(err, ureq::Error::StatusCode(429));
    ApiFailure {
        error: anyhow::anyhow!("{what} call failed: {err}"),
        rate_limited,
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            rate_limited: false,
        }
    }
}

impl std::fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl QuotaSnapshot {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            source: QuotaSource::Unavailable,
            note: Some(reason.into()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ureq 3 raises non-2xx as an error from `call()`, which is why the
    /// classification lives here and not on a returned response.
    #[test]
    fn a_429_status_error_is_classified_as_rate_limited() {
        let failure = call_failure("/api/oauth/usage", ureq::Error::StatusCode(429));
        assert!(failure.rate_limited);
        assert!(failure.to_string().contains("429"));
    }

    #[test]
    fn other_failures_are_not_rate_limits() {
        assert!(!call_failure("/api/oauth/usage", ureq::Error::StatusCode(500)).rate_limited);
        assert!(!call_failure("/api/oauth/usage", ureq::Error::HostNotFound).rate_limited);
    }
}
