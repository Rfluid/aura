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
pub mod pacing;

pub use api::{QuotaApi, QuotaSource};
pub use codex::CodexQuota;
pub use forecast::{forecast, ForecastSnapshot, ForecastStatus, ForecastWindow};
pub use gemini::GeminiQuota;
pub use pacing::{
    session_budget, ActivityPattern, PacingStatus, SessionBudget, SessionTokens,
};

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
    /// Learned active-session pattern for budget pacing (F2). Populated by the
    /// app's refresh from a local JSONL scan when the source is `Api`; `None`
    /// otherwise. Skipped from the wire format when absent so existing
    /// consumers are unaffected. See [`pacing`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pacing_pattern: Option<ActivityPattern>,
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
