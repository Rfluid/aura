//! Claude subscription quota windows (the data behind `claude /usage`).
//!
//! The API path tries to read the OAuth credentials from
//! `~/.claude/.credentials.json`, refresh the access token if it's expired,
//! then call `https://api.anthropic.com/api/oauth/usage`. If any of that
//! fails, callers can fall back to local counts derived from JSONL data.

mod api;
mod oauth;

pub use api::{QuotaApi, QuotaSource};

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
