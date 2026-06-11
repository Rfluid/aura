//! Per-session **budget pacing** for the Forecast tab — see `FEATURE_SPEC.md`
//! (F2). Forecasting answers "where does this window land at the current
//! rate?"; pacing answers the question a Max user actually has mid-week:
//!
//! > "How much can I spend **in this 5h session** without exhausting my
//! > **weekly** window before it resets?"
//!
//! ## Unit of pacing: the 5h rate-limit *window*
//!
//! A Max plan renews its 5h rate-limit window ~4–5×/day (24h / 5h), but the
//! user only *actively codes* in a few of them — and several JSONL session
//! files can land inside one 5h window. So we pace against **active 5h
//! windows**, not session files: history is bucketed onto the live reset grid
//! (`session.resets_at` gives the phase), a window is "active" when the tokens
//! charged inside it clear `active_session_min_tokens`, and we count distinct
//! active windows.
//!
//! ## Token caps from real window boundaries
//!
//! We never guess a plan's token cap. Instead we invert the API's reported
//! utilization over the *actual* window span: if `tokens_in(window) / used%`
//! gives the full-window cap, then `weekly_cap = tokens_in(weekly_window) /
//! (weekly_used% / 100)` and likewise for the 5h window. Everything downstream
//! is in tokens, then expressed as a share of one full 5h window at the end.
//!
//! ## Budget
//!
//! `weekly_remaining_tokens` is spread across the `windows_left` active 5h
//! windows projected before the weekly reset; the per-window budget, divided by
//! the 5h window's own cap, is the recommended share (0–100%) of one full 5h
//! window.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{QuotaSnapshot, QuotaSource, QuotaWindow};

/// Below this many *active 5h windows* of trailing history we refuse to emit a
/// number and show a "warming up" state instead — same spirit as
/// [`super::forecast::INSUFFICIENT_BELOW`], adapted to a window count.
pub const INSUFFICIENT_ACTIVE_WINDOWS: f64 = 3.0;

/// Never divide weekly-remaining by fewer than this many windows. Floors the
/// denominator so a near-empty week (a fraction of one window left) can't blow
/// the per-window budget up to an absurd number.
const MIN_WINDOWS_LEFT: f64 = 0.5;

/// Below this reported utilization (or with ~0 tokens charged) we can't invert
/// a reliable cap from `tokens / used%` — the division is too noisy — so we
/// return [`PacingStatus::Insufficient`] rather than fabricate a cap.
pub const MIN_PCT_FOR_CAP: f64 = 3.0;

/// Length of the rate-limit "session" window. Used to bucket history onto the
/// live reset grid and to derive the weekly/session window start instants.
const SESSION_WINDOW_MINUTES: i64 = 5 * 60;

/// Fraction trimmed from each tail when computing the trimmed mean of daily
/// active-window counts. 0.1 ⇒ drop the top 10% and bottom 10% of days so an
/// occasional 5-window marathon (or a single-window day) doesn't skew the
/// learned typical.
const TRIM_FRACTION: f64 = 0.1;

/// API label of the rolling 5h "session" window in a [`QuotaSnapshot`].
const SESSION_LABEL: &str = "Current session";
/// API label of the rolling 7d "weekly" window in a [`QuotaSnapshot`].
const WEEKLY_LABEL: &str = "Current week (all models)";

/// Status badge for the session-budget gauge. Mirrors the green / amber / red
/// mapping of [`super::forecast::ForecastStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PacingStatus {
    /// Current 5h usage is comfortably under the recommended ceiling.
    Ok,
    /// Current 5h usage is within ~10 percentage points of the ceiling.
    Watch,
    /// Current 5h usage already exceeds the ceiling — ease off to protect the
    /// weekly window.
    Over,
    /// Not enough history (or no live quota data) to pace — show "warming up"
    /// with **no** fabricated number.
    Insufficient,
}

/// The minimal per-session shape pacing needs from the JSONL scan: how many
/// `input + output` tokens the session used and when it started.
///
/// F2 computes this itself (see [`collect_session_tokens`]) so it does **not**
/// depend on the F3 `SessionStat` token enrichment landing first.
#[derive(Debug, Clone)]
pub struct SessionTokens {
    /// `input + output` tokens across all models in the session.
    pub total_tokens: u64,
    /// RFC 3339 timestamp of the session's first entry.
    pub start_timestamp: String,
}

/// Learned activity pattern — how many **active 5h windows** the user typically
/// burns per day, used to project how many windows remain before the weekly
/// reset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActivityPattern {
    /// Trimmed mean of active-window counts over **active days only** (days with
    /// ≥1 active window) in the trailing window. Naturally ≤ ~5 (24h / 5h).
    pub active_windows_per_day: f64,
    /// Active 5h windows already started today (since local midnight) —
    /// subtracted from the projected windows-left so we don't double-count the
    /// current burst.
    pub windows_used_today: f64,
    /// Number of active windows the pattern was learned from. Drives the
    /// [`PacingStatus::Insufficient`] gate.
    pub active_window_count: f64,
}

/// Token caps derived from the real window boundaries — see the module docs.
/// Computed by [`compute_caps`] where both the quota windows (for `used%`) and
/// the JSONL token sums (reader) are in scope, then handed to the pure
/// [`session_budget`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    /// Full-window token cap of the weekly window: `tokens_in(weekly) / used%`.
    pub weekly_cap_tokens: f64,
    /// Full-window token cap of the 5h window: `tokens_in(session) / used%`.
    pub session_cap_tokens: f64,
}

/// The session-budget gauge output — serializable, like [`super::ForecastWindow`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBudget {
    /// Recommended ceiling: the share (0–100) of one full 5h window you may
    /// burn so the weekly window lands at/under 100% at reset. `None` when
    /// `status` is [`PacingStatus::Insufficient`].
    pub recommended_pct: Option<f64>,
    /// `recommended_pct - session.used_percentage` — drives the gauge color.
    /// `None` when `recommended_pct` is `None`.
    pub headroom_pct: Option<f64>,
    pub status: PacingStatus,
    /// Live 5h utilization the gauge is plotted against (0–100). `None` when
    /// the session window has no API percentage.
    pub session_used_pct: Option<f64>,
    /// Learned typical active 5h windows per active day.
    pub active_windows_per_day: f64,
    /// Projected active 5h windows remaining this week (floored at
    /// [`MIN_WINDOWS_LEFT`]).
    pub windows_left: f64,
    /// Weekly-% remaining (`100 - weekly.used_percentage`).
    pub weekly_remaining_pct: Option<f64>,
    /// Free-form rationale / state note (e.g. "Needs live quota data").
    pub note: Option<String>,
}

impl SessionBudget {
    /// An `Insufficient` budget carrying only a note — no fabricated numbers.
    fn insufficient(note: impl Into<String>) -> Self {
        Self {
            recommended_pct: None,
            headroom_pct: None,
            status: PacingStatus::Insufficient,
            session_used_pct: None,
            active_windows_per_day: 0.0,
            windows_left: 0.0,
            weekly_remaining_pct: None,
            note: Some(note.into()),
        }
    }
}

/// Learn the user's active **5h-window** pattern from a trailing window of
/// sessions.
///
/// Each session is bucketed onto the 5h rate-limit grid anchored on
/// `session_resets_at` (the live reset phase): `window_index(ts) = floor((ts -
/// session_resets_at) / 5h)`. Tokens of all sessions in the same window are
/// summed; a window is **active** when that sum reaches `min_tokens`. Distinct
/// active windows are then counted per calendar day, and `active_windows_per_day`
/// is the **trimmed mean** over **active days only** (days with ≥1 active
/// window). `windows_used_today` is the count of active windows started today.
///
/// `now` and `history_days` bound the trailing window; `min_tokens` is the
/// active threshold (config `pacing.active_session_min_tokens`).
pub fn learn_pattern(
    sessions: &[SessionTokens],
    now: DateTime<Utc>,
    history_days: u32,
    min_tokens: u64,
    session_resets_at: DateTime<Utc>,
) -> ActivityPattern {
    let window_start = now - Duration::days(history_days as i64);
    let today = now.date_naive();

    // Sum tokens per 5h window index; remember each window's calendar day and
    // whether it started today. Multiple session files in one window collapse
    // into a single window bucket here.
    struct WindowAccum {
        tokens: u64,
        day: chrono::NaiveDate,
        is_today: bool,
    }
    let mut windows: std::collections::HashMap<i64, WindowAccum> = std::collections::HashMap::new();

    for s in sessions {
        let Some(ts) = parse_ts(&s.start_timestamp) else {
            continue;
        };
        if ts < window_start || ts > now {
            continue;
        }
        let idx = window_index(ts, session_resets_at);
        let entry = windows.entry(idx).or_insert_with(|| WindowAccum {
            tokens: 0,
            day: ts.date_naive(),
            is_today: ts.date_naive() == today,
        });
        entry.tokens += s.total_tokens;
    }

    // Keep only active windows, then tally active windows per calendar day.
    let mut per_day: std::collections::HashMap<chrono::NaiveDate, u64> =
        std::collections::HashMap::new();
    let mut active_count: u64 = 0;
    let mut used_today: u64 = 0;
    for w in windows.values() {
        if w.tokens < min_tokens {
            continue;
        }
        active_count += 1;
        *per_day.entry(w.day).or_insert(0) += 1;
        if w.is_today {
            used_today += 1;
        }
    }

    // Active days only — drop the zero-window days entirely.
    let mut daily_counts: Vec<f64> = per_day.values().map(|&c| c as f64).collect();
    let active_windows_per_day = trimmed_mean(&mut daily_counts, TRIM_FRACTION);

    ActivityPattern {
        active_windows_per_day,
        windows_used_today: used_today as f64,
        active_window_count: active_count as f64,
    }
}

/// Derive the per-window token caps from the live quota windows and the JSONL
/// token sums over each window's real boundary.
///
/// `weekly_cap_tokens = tokens_in(weekly_window_start, now) / (weekly_used% / 100)`
/// and likewise for the 5h window. Returns `Err` with a user-facing note when a
/// window lacks `used_percentage` / `resets_at` / `length_minutes`, when the
/// reported utilization is below [`MIN_PCT_FOR_CAP`], or when ~0 tokens were
/// charged in the window — in all those cases the division is unreliable and we
/// must not fabricate a cap.
pub fn compute_caps(
    config_path: &Path,
    weekly: &QuotaWindow,
    session: &QuotaWindow,
    now: DateTime<Utc>,
) -> Result<Caps, String> {
    let weekly_cap = derive_cap(config_path, weekly, now)?;
    let session_cap = derive_cap(config_path, session, now)?;
    Ok(Caps {
        weekly_cap_tokens: weekly_cap,
        session_cap_tokens: session_cap,
    })
}

/// Invert one window's `used%` over its real span to a full-window token cap.
fn derive_cap(config_path: &Path, window: &QuotaWindow, now: DateTime<Utc>) -> Result<f64, String> {
    let warming = || "Warming up — not enough usage yet to estimate caps".to_string();

    let used_pct = window.used_percentage.ok_or_else(|| "Needs live quota data".to_string())?;
    let resets_at = window.resets_at.ok_or_else(|| "Needs live quota data".to_string())?;
    let length_minutes = window
        .length_minutes
        .ok_or_else(|| "Needs live quota data".to_string())?;

    if used_pct < MIN_PCT_FOR_CAP {
        return Err(warming());
    }

    let window_start = resets_at - Duration::minutes(length_minutes as i64);
    let tokens = crate::reader::sum_tokens_in_range(config_path, window_start, now);
    if tokens == 0 {
        return Err(warming());
    }

    Ok(tokens as f64 / (used_pct / 100.0))
}

/// Compute the session budget gauge from the live quota snapshot, the learned
/// pattern, and the precomputed token [`Caps`].
///
/// Algorithm (all in tokens until the final share):
///
/// 1. `days_left = (weekly.resets_at - now)` as fractional days
/// 2. `windows_left = max(MIN_WINDOWS_LEFT,
///        active_windows_per_day * days_left - windows_used_today)`
/// 3. `weekly_remaining_tokens = weekly_cap_tokens * (1 - weekly_used% / 100)`
/// 4. `budget_tokens = weekly_remaining_tokens / windows_left`
/// 5. `recommended_pct = clamp(budget_tokens / session_cap_tokens * 100, 0, 100)`
/// 6. `headroom = recommended_pct - session.used_percentage`  (gauge color)
///
/// `caps` is `None` when [`compute_caps`] could not derive a reliable cap; that
/// surfaces as [`PacingStatus::Insufficient`]. The function is otherwise pure
/// and unit-testable. Returns `Insufficient` (no number) when the quota source
/// is not [`QuotaSource::Api`], when the 5h/7d windows are missing, or when the
/// learned pattern is too thin.
pub fn session_budget(
    snapshot: &QuotaSnapshot,
    caps: Option<Caps>,
    now: DateTime<Utc>,
) -> SessionBudget {
    // Pacing accuracy hinges on the API percentage. If we're on the local
    // fallback (or nothing), say so rather than pacing on approximate tokens.
    if snapshot.source != QuotaSource::Api {
        return SessionBudget::insufficient("Needs live quota data");
    }

    let pattern = match snapshot.pacing_pattern {
        Some(p) => p,
        None => return SessionBudget::insufficient("Warming up — need more history to pace"),
    };

    let Some(weekly) = find_window(snapshot, WEEKLY_LABEL) else {
        return SessionBudget::insufficient("Needs live quota data");
    };
    let Some(session) = find_window(snapshot, SESSION_LABEL) else {
        return SessionBudget::insufficient("Needs live quota data");
    };

    // No reliable caps → can't pace in tokens.
    let Some(caps) = caps else {
        return SessionBudget::insufficient(
            "Warming up — not enough usage yet to estimate caps",
        );
    };

    // Too little history → no fabricated number.
    if pattern.active_window_count < INSUFFICIENT_ACTIVE_WINDOWS
        || pattern.active_windows_per_day <= 0.0
    {
        return SessionBudget::insufficient("Warming up — need more history to pace");
    }

    let weekly_used = weekly.used_percentage.unwrap_or(0.0);
    let weekly_remaining = (100.0 - weekly_used).clamp(0.0, 100.0);
    let session_used = session.used_percentage.unwrap_or(0.0).clamp(0.0, 100.0);

    let Some(resets_at) = weekly.resets_at else {
        return SessionBudget::insufficient("Needs live quota data");
    };
    let days_left = ((resets_at - now).num_seconds() as f64 / 86_400.0).max(0.0);

    // Floor the denominator so we never divide by ~0 and emit an absurd budget.
    let raw_windows_left =
        pattern.active_windows_per_day * days_left - pattern.windows_used_today;
    let windows_left = raw_windows_left.max(MIN_WINDOWS_LEFT);

    let weekly_remaining_tokens = caps.weekly_cap_tokens * (weekly_remaining / 100.0);
    let budget_tokens = weekly_remaining_tokens / windows_left;
    let session_cap = caps.session_cap_tokens.max(f64::EPSILON);
    let recommended_pct = (budget_tokens / session_cap * 100.0).clamp(0.0, 100.0);
    let headroom = recommended_pct - session_used;

    let status = if session_used > recommended_pct {
        PacingStatus::Over
    } else if headroom <= 10.0 {
        PacingStatus::Watch
    } else {
        PacingStatus::Ok
    };

    let note = Some(format!(
        "~{:.0} active 5h-windows left this week · {:.0}% weekly left",
        windows_left, weekly_remaining
    ));

    SessionBudget {
        recommended_pct: Some(recommended_pct),
        headroom_pct: Some(headroom),
        status,
        session_used_pct: Some(session_used),
        active_windows_per_day: pattern.active_windows_per_day,
        windows_left,
        weekly_remaining_pct: Some(weekly_remaining),
        note,
    }
}

// ── Scan helper (F2-local, F3-independent) ──────────────────────────────────

/// Tally `input + output` tokens per session over the trailing `history_days`,
/// returning the minimal [`SessionTokens`] list `learn_pattern` needs.
///
/// This is the "tiny local token-per-session pass" the spec calls for: the base
/// `SessionStat` has timestamps but no per-session token total, so we read the
/// JSONL directly here rather than depend on the F3 enrichment. Subagent
/// (`isSidechain`) entries are skipped, matching the main scanner.
pub fn collect_session_tokens(
    config_path: &std::path::Path,
    now: DateTime<Utc>,
    history_days: u32,
) -> Vec<SessionTokens> {
    use crate::reader::scan::list_session_files;

    let from = (now - Duration::days(history_days as i64 + 1))
        .format("%Y-%m-%d")
        .to_string();
    let files = match list_session_files(config_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for (path, is_subagent) in files {
        if is_subagent {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut first_ts: Option<String> = None;
        let mut tokens: u64 = 0;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<ScanLine>(line) else {
                continue;
            };
            if entry.is_sidechain {
                continue;
            }
            if first_ts.is_none() {
                if let Some(ts) = &entry.timestamp {
                    first_ts = Some(ts.clone());
                }
            }
            if entry.entry_type.as_deref() == Some("assistant") {
                if let Some(usage) = entry.message.and_then(|m| m.usage) {
                    tokens += usage.input_tokens + usage.output_tokens;
                }
            }
        }
        if let Some(start) = first_ts {
            // mtime pruning happens at the OS level only; filter by start date.
            if start.as_str() >= from.as_str() {
                out.push(SessionTokens {
                    total_tokens: tokens,
                    start_timestamp: start,
                });
            }
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct ScanLine {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    message: Option<ScanMessage>,
}

#[derive(serde::Deserialize)]
struct ScanMessage {
    usage: Option<ScanUsage>,
}

#[derive(serde::Deserialize)]
struct ScanUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn find_window<'a>(snapshot: &'a QuotaSnapshot, label: &str) -> Option<&'a QuotaWindow> {
    snapshot.windows.iter().find(|w| w.label == label)
}

/// Map a timestamp to its 5h-window index on the grid anchored at `resets_at`.
/// Any consistent 5h grid works; we anchor on the live reset so buckets line up
/// with the rate-limit boundaries.
fn window_index(ts: DateTime<Utc>, resets_at: DateTime<Utc>) -> i64 {
    let secs_per_window = SESSION_WINDOW_MINUTES * 60;
    (ts - resets_at).num_seconds().div_euclid(secs_per_window)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    use chrono::NaiveDateTime;
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ").map(|d| d.and_utc())
        })
        .ok()
}

/// Mean of `values` after dropping the top and bottom `trim` fraction of
/// entries (rounded down). Returns 0.0 for an empty slice. With fewer than
/// 3 values, trimming would drop everything, so the plain mean is used.
fn trimmed_mean(values: &mut [f64], trim: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    let cut = ((n as f64) * trim).floor() as usize;
    // Keep at least one element; only trim when there's room on both ends.
    let slice = if n > 2 * cut && cut > 0 {
        &values[cut..n - cut]
    } else {
        &values[..]
    };
    let sum: f64 = slice.iter().sum();
    sum / slice.len() as f64
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn session(tokens: u64, ts: &str) -> SessionTokens {
        SessionTokens {
            total_tokens: tokens,
            start_timestamp: ts.to_string(),
        }
    }

    fn quota_with(weekly_pct: f64, session_pct: f64, weekly_resets_in_hours: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            windows: vec![
                QuotaWindow {
                    label: SESSION_LABEL.to_string(),
                    used_percentage: Some(session_pct),
                    used_tokens: None,
                    resets_at: Some(Utc::now() + Duration::hours(3)),
                    length_minutes: Some(5 * 60),
                },
                QuotaWindow {
                    label: WEEKLY_LABEL.to_string(),
                    used_percentage: Some(weekly_pct),
                    used_tokens: None,
                    resets_at: Some(Utc::now() + Duration::hours(weekly_resets_in_hours)),
                    length_minutes: Some(7 * 24 * 60),
                },
            ],
            source: QuotaSource::Api,
            ..Default::default()
        }
    }

    // ── learn_pattern ────────────────────────────────────────────────────────

    /// Multiple sessions inside one 5h window collapse to ONE active window.
    #[test]
    fn learn_collapses_sessions_into_one_window() {
        let now = Utc::now();
        // Anchor the 5h grid so day-1 noon is a clean window boundary.
        let resets_at = now;
        // Day -1: three session files, each 30k tokens, all within ~2h → one
        // window with 90k tokens ≥ 50k → 1 active window (not 3).
        let base = now - Duration::days(1);
        let s = |off_min: i64| (base + Duration::minutes(off_min)).to_rfc3339();
        let sessions = vec![
            session(30_000, &s(0)),
            session(30_000, &s(30)),
            session(30_000, &s(60)),
        ];

        let p = learn_pattern(&sessions, now, 14, 50_000, resets_at);
        assert_eq!(p.active_window_count, 1.0, "three sessions = one window");
        assert!(
            (p.active_windows_per_day - 1.0).abs() < 1e-9,
            "got {}",
            p.active_windows_per_day
        );
    }

    /// Two sessions in DIFFERENT 5h windows count as two active windows; a
    /// window that never clears the token threshold is dropped.
    #[test]
    fn learn_counts_distinct_active_windows() {
        let now = Utc::now();
        let resets_at = now;
        let base = now - Duration::days(1);
        // Window A (active, 60k) and window B 6h later (active, 60k) on the same
        // day; window C (idle, 1k) dropped.
        let sessions = vec![
            session(60_000, &(base + Duration::hours(0)).to_rfc3339()),
            session(60_000, &(base + Duration::hours(6)).to_rfc3339()),
            session(1_000, &(base + Duration::hours(12)).to_rfc3339()),
        ];
        let p = learn_pattern(&sessions, now, 14, 50_000, resets_at);
        assert_eq!(p.active_window_count, 2.0);
        // One active day with 2 windows → mean 2.0.
        assert!((p.active_windows_per_day - 2.0).abs() < 1e-9, "got {}", p.active_windows_per_day);
    }

    /// Trimmed mean drops an outlier marathon day (active *windows*, not files).
    #[test]
    fn learn_trimmed_mean_drops_outliers() {
        let now = Utc::now();
        let resets_at = now;
        let mut sessions = Vec::new();
        // 10 days, each with 2 active windows (6h apart so distinct windows).
        for d in 1..=10 {
            let base = now - Duration::days(d);
            sessions.push(session(60_000, &base.to_rfc3339()));
            sessions.push(session(60_000, &(base + Duration::hours(6)).to_rfc3339()));
        }
        // One marathon day with 4 windows (5 windows max/day) ~5h apart.
        let m = now - Duration::days(11);
        for k in 0..4 {
            sessions.push(session(60_000, &(m + Duration::hours(5 * k)).to_rfc3339()));
        }
        let p = learn_pattern(&sessions, now, 21, 50_000, resets_at);
        // 11 active days: ten 2.0's and one 4.0. trim=0.1 → cut=1 drops one tail
        // each → mean of nine 2.0's = 2.0.
        assert!(
            (p.active_windows_per_day - 2.0).abs() < 1e-9,
            "outlier not trimmed: got {}",
            p.active_windows_per_day
        );
    }

    /// Sessions outside the trailing window are excluded.
    #[test]
    fn learn_respects_history_window() {
        let now = Utc::now();
        let resets_at = now;
        let recent = (now - Duration::days(2)).to_rfc3339();
        let ancient = (now - Duration::days(40)).to_rfc3339();
        let sessions = vec![session(60_000, &recent), session(60_000, &ancient)];
        let p = learn_pattern(&sessions, now, 14, 50_000, resets_at);
        assert_eq!(p.active_window_count, 1.0);
    }

    // ── compute_caps ─────────────────────────────────────────────────────────

    /// Cap inversion: tokens / (used% / 100). Below MIN_PCT_FOR_CAP → Err.
    #[test]
    fn derive_cap_inverts_and_guards_low_pct() {
        // 30% used, no JSONL on disk → tokens=0 → warming-up Err.
        let now = Utc::now();
        let w = QuotaWindow {
            label: WEEKLY_LABEL.to_string(),
            used_percentage: Some(30.0),
            used_tokens: None,
            resets_at: Some(now + Duration::hours(48)),
            length_minutes: Some(7 * 24 * 60),
        };
        let missing = std::path::Path::new("/nonexistent-aura-test-dir");
        let err = derive_cap(missing, &w, now).unwrap_err();
        assert!(err.contains("Warming up"), "got {err}");

        // Below MIN_PCT_FOR_CAP → warming up regardless of tokens.
        let low = QuotaWindow {
            used_percentage: Some(2.0),
            ..w.clone()
        };
        let err = derive_cap(missing, &low, now).unwrap_err();
        assert!(err.contains("Warming up"), "got {err}");
    }

    // ── session_budget ───────────────────────────────────────────────────────

    /// Screenshot scenario: weekly ~15% used, several 5h windows/day, ~6.5 days
    /// left. The weekly budget is spread across ~30 remaining 5h windows, so the
    /// recommendation is MODEST — a fraction of one window, NOT ≈85%.
    #[test]
    fn budget_screenshot_scenario_is_modest() {
        let now = Utc::now();
        let mut snap = quota_with(15.0, 10.0, 156); // 156h ≈ 6.5 days left
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 4.5,
            windows_used_today: 0.0,
            active_window_count: 30.0,
        });
        // Caps: weekly 15% used has produced 1.5M tokens → cap 10M.
        // session 10% used has produced 50k → cap 500k.
        let caps = Caps {
            weekly_cap_tokens: 10_000_000.0,
            session_cap_tokens: 500_000.0,
        };
        let b = session_budget(&snap, Some(caps), now);
        let rec = b.recommended_pct.unwrap();
        // windows_left ≈ 4.5*6.5 = ~29. weekly_remaining_tokens = 10M*0.85 = 8.5M.
        // budget = 8.5M/29 ≈ 293k. rec = 293k/500k*100 ≈ 58%? Check it's modest
        // and not echoing weekly_remaining (85%).
        assert!(rec < 85.0, "must not echo weekly remaining: rec={rec}");
        assert!(rec > 0.0 && rec.is_finite(), "rec={rec}");
        assert!(b.windows_left > 25.0 && b.windows_left < 35.0, "windows_left={}", b.windows_left);
    }

    /// A known set of caps + pattern yields an exact recommended_pct.
    #[test]
    fn budget_known_inputs_exact() {
        let now = Utc::now();
        // weekly resets in 48h = 2 days; 2 windows/day → windows_left = 4.
        let mut snap = quota_with(50.0, 20.0, 48);
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 2.0,
            windows_used_today: 0.0,
            active_window_count: 20.0,
        });
        // weekly cap 1M, 50% used → 500k remaining. session cap 200k.
        let caps = Caps {
            weekly_cap_tokens: 1_000_000.0,
            session_cap_tokens: 200_000.0,
        };
        let b = session_budget(&snap, Some(caps), now);
        // budget = 500k / 4 = 125k. rec = 125k/200k*100 = 62.5%.
        let rec = b.recommended_pct.unwrap();
        assert!((rec - 62.5).abs() < 0.5, "expected ~62.5, got {rec}");
        assert!((b.windows_left - 4.0).abs() < 0.01, "got {}", b.windows_left);
    }

    /// Budget high enough that session usage exceeds it → Over.
    #[test]
    fn budget_session_over_ceiling() {
        let now = Utc::now();
        let mut snap = quota_with(95.0, 80.0, 48);
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 2.0,
            windows_used_today: 0.0,
            active_window_count: 20.0,
        });
        // weekly cap 1M, 95% used → 50k remaining over 4 windows → 12.5k budget.
        let caps = Caps {
            weekly_cap_tokens: 1_000_000.0,
            session_cap_tokens: 200_000.0,
        };
        let b = session_budget(&snap, Some(caps), now);
        // rec = 12.5k/200k*100 = 6.25%. session used 80% > 6.25% → Over.
        assert_eq!(b.status, PacingStatus::Over);
        assert!(b.recommended_pct.unwrap() < 80.0);
    }

    /// Missing caps (warming up) → Insufficient with the caps note.
    #[test]
    fn budget_no_caps_is_insufficient() {
        let now = Utc::now();
        let mut snap = quota_with(30.0, 20.0, 72);
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 2.0,
            windows_used_today: 0.0,
            active_window_count: 20.0,
        });
        let b = session_budget(&snap, None, now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert!(b.recommended_pct.is_none());
        assert!(b.note.as_deref().unwrap().contains("estimate caps"));
    }

    /// Thin history → Insufficient with no number.
    #[test]
    fn budget_thin_history_is_insufficient() {
        let now = Utc::now();
        let mut snap = quota_with(30.0, 20.0, 72);
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 1.0,
            windows_used_today: 0.0,
            active_window_count: 1.0, // below INSUFFICIENT_ACTIVE_WINDOWS
        });
        let caps = Caps {
            weekly_cap_tokens: 1_000_000.0,
            session_cap_tokens: 200_000.0,
        };
        let b = session_budget(&snap, Some(caps), now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert!(b.recommended_pct.is_none());
        assert!(b.note.is_some());
    }

    /// No pattern attached → Insufficient.
    #[test]
    fn budget_no_pattern_is_insufficient() {
        let now = Utc::now();
        let snap = quota_with(30.0, 20.0, 72); // pacing_pattern defaults to None
        let b = session_budget(&snap, None, now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert!(b.recommended_pct.is_none());
    }

    /// Non-API source → "Needs live quota data", never paced on local tokens.
    #[test]
    fn budget_requires_api_source() {
        let now = Utc::now();
        let mut snap = quota_with(30.0, 20.0, 72);
        snap.source = QuotaSource::Fallback;
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 2.0,
            windows_used_today: 0.0,
            active_window_count: 20.0,
        });
        let b = session_budget(&snap, None, now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert_eq!(b.note.as_deref(), Some("Needs live quota data"));
    }

    /// Divide-by-~0 guard: a nearly-spent week (resets imminently) floors
    /// windows_left so the budget stays finite and clamped, never absurd.
    #[test]
    fn budget_floors_windows_left() {
        let now = Utc::now();
        let mut snap = quota_with(50.0, 10.0, 0);
        // Weekly resets in 1 minute → days_left ≈ 0.0007, raw windows_left < 0.
        snap.windows[1].resets_at = Some(now + Duration::minutes(1));
        snap.pacing_pattern = Some(ActivityPattern {
            active_windows_per_day: 2.0,
            windows_used_today: 0.0,
            active_window_count: 20.0,
        });
        let caps = Caps {
            weekly_cap_tokens: 1_000_000.0,
            session_cap_tokens: 200_000.0,
        };
        let b = session_budget(&snap, Some(caps), now);
        assert_eq!(b.windows_left, MIN_WINDOWS_LEFT);
        let rec = b.recommended_pct.unwrap();
        assert!(rec.is_finite() && (0.0..=100.0).contains(&rec), "rec={rec}");
    }

    // ── window_index / trimmed_mean units ────────────────────────────────────

    #[test]
    fn window_index_buckets_5h() {
        let anchor = Utc::now();
        assert_eq!(window_index(anchor, anchor), 0);
        assert_eq!(window_index(anchor + Duration::hours(4), anchor), 0);
        assert_eq!(window_index(anchor + Duration::hours(5), anchor), 1);
        assert_eq!(window_index(anchor + Duration::hours(11), anchor), 2);
        assert_eq!(window_index(anchor - Duration::hours(1), anchor), -1);
    }

    #[test]
    fn trimmed_mean_empty_is_zero() {
        let mut v: Vec<f64> = vec![];
        assert_eq!(trimmed_mean(&mut v, 0.1), 0.0);
    }

    #[test]
    fn trimmed_mean_small_uses_plain_mean() {
        let mut v = vec![1.0, 3.0];
        assert_eq!(trimmed_mean(&mut v, 0.1), 2.0);
    }
}
