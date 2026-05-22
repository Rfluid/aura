pub mod claude_code;
pub mod codex;
pub(crate) mod codex_scan;
mod dates;
mod incremental;
pub(crate) mod scan;
mod stats_cache;
mod watcher;

pub use claude_code::ClaudeCodeReader;
pub use codex::CodexReader;
pub use incremental::read_jsonl_since;
pub use watcher::ProjectsWatcher;

use crate::config::{AgentConfig, AgentKind};

/// Construct the right `AgentReader` for an agent profile.
pub fn make_reader(agent: &AgentConfig) -> Box<dyn AgentReader> {
    let path = agent.resolved_config_path();
    match agent.kind {
        AgentKind::ClaudeCode => Box::new(ClaudeCodeReader::new(path)),
        AgentKind::Codex => Box::new(CodexReader::new(path)),
    }
}

use std::collections::HashMap;

use anyhow::Result;

// ── Period ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    AllTime,
    Last7Days,
    Last30Days,
}

// ── Per-model usage ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl ModelUsage {
    /// `input + output` only — matches how `/usage` counts "Total tokens".
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

// ── Daily breakdown ───────────────────────────────────────────────────────────

/// Tokens per model per calendar day. Only `input + output` (no cache),
/// matching the `dailyModelTokens` field in `stats-cache.json`.
#[derive(Debug, Clone, Default)]
pub struct DailyModelTokens {
    pub date: String, // "YYYY-MM-DD"
    pub by_model: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct DailyActivity {
    pub date: String,
    pub message_count: u64,
    pub session_count: u64,
}

// ── Streaks ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Streaks {
    pub current: u32,
    pub longest: u32,
}

// ── UsageSnapshot ─────────────────────────────────────────────────────────────

/// The final output of a snapshot read — all data needed by the Overview and
/// Models panels. Mirrors the object produced by `zT5` / `_T5` in `claude /usage`.
#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    // ── Token totals ──────────────────────────────────────────────────────────
    /// `input + output` across all models (the number `/usage` calls "Total tokens").
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,

    // ── Activity ──────────────────────────────────────────────────────────────
    pub total_sessions: u64,
    pub total_messages: u64,
    /// Calendar days that had at least one session.
    pub active_days: u32,
    /// Span from first to last session date in days (inclusive).
    pub total_days: u32,

    // ── Derived ───────────────────────────────────────────────────────────────
    pub favorite_model: Option<String>,
    pub longest_session_secs: Option<u64>,
    pub streaks: Streaks,
    /// Hour of day (0–23, local time) with the most session starts.
    pub peak_hour: Option<u8>,

    // ── Chart data ────────────────────────────────────────────────────────────
    /// Per-model usage, sorted by `total_tokens` descending.
    pub per_model: Vec<ModelUsage>,
    /// Daily token breakdown, sorted by date ascending.
    pub daily_tokens: Vec<DailyModelTokens>,
    /// Daily activity, sorted by date ascending.
    pub daily_activity: Vec<DailyActivity>,

    pub first_session_date: Option<String>,
    pub last_session_date: Option<String>,
}

// ── AgentReader ───────────────────────────────────────────────────────────────

pub trait AgentReader {
    fn snapshot(&self, period: Period) -> Result<UsageSnapshot>;
}
