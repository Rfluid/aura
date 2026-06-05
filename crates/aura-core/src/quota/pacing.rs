//! Per-session **budget pacing** for the Forecast tab — see `FEATURE_SPEC.md`
//! (F2). Forecasting answers "where does this window land at the current
//! rate?"; pacing answers the question a Max user actually has mid-week:
//!
//! > "How much can I spend **in this 5h session** without exhausting my
//! > **weekly** window before it resets?"
//!
//! Naively dividing weekly-remaining by remaining 5h renewals is wrong: a Max
//! plan renews the 5h window ~4–5×/day, but the user is only *actively coding*
//! in a few of them. We pace against the user's **learned active-session
//! pattern** instead, and we do all the math in **percentage of the weekly
//! window** (never a guessed token cap) — `QuotaWindow.used_percentage` is the
//! API-reported real utilization, so this stays correct across plan tiers.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{QuotaSnapshot, QuotaSource, QuotaWindow};

/// Below this many *active sessions* of trailing history we refuse to emit a
/// number and show a "warming up" state instead — same spirit as
/// [`super::forecast::INSUFFICIENT_BELOW`], adapted to a session count.
pub const INSUFFICIENT_ACTIVE_SESSIONS: f64 = 3.0;

/// Never divide weekly-remaining by fewer than this many sessions. Floors the
/// denominator so a near-empty week (a fraction of one session left) can't
/// blow `weekly_pct_per_session` up to an absurd budget.
const MIN_SESSIONS_LEFT: f64 = 0.5;

/// Fraction trimmed from each tail when computing the trimmed mean of daily
/// active-session counts. 0.1 ⇒ drop the top 10% and bottom 10% of days so the
/// occasional 8-session marathon (or a single-session day) doesn't skew the
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
/// depend on the F3 `SessionStat` token enrichment landing first. When the
/// shared `SessionStat` shape gains a token field upstream, callers can map
/// into this type at the boundary.
#[derive(Debug, Clone)]
pub struct SessionTokens {
    /// `input + output` tokens across all models in the session.
    pub total_tokens: u64,
    /// RFC 3339 timestamp of the session's first entry.
    pub start_timestamp: String,
}

/// Learned activity pattern — how the user *actually* uses Claude Code, used to
/// anchor the budget to typical behaviour rather than the raw renewal cadence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ActivityPattern {
    /// Trimmed mean of active-session counts over **active days only** in the
    /// trailing window.
    pub active_per_day: f64,
    /// Weekly-% historically consumed per active session: total active
    /// sessions in the window divided into the elapsed-weekly span. Used to
    /// express the budget as a share of one full 5h window.
    pub avg_weekly_pct_per_active_session: f64,
    /// Active sessions already started today — subtracted from the projected
    /// sessions-left so we don't double-count the current burst.
    pub used_active_sessions_today: f64,
    /// Number of active sessions the pattern was learned from. Drives the
    /// [`PacingStatus::Insufficient`] gate.
    pub active_session_count: f64,
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
    /// Learned typical active sessions per active day.
    pub active_per_day: f64,
    /// Projected active sessions remaining this week (floored at
    /// [`MIN_SESSIONS_LEFT`]).
    pub sessions_left: f64,
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
            active_per_day: 0.0,
            sessions_left: 0.0,
            weekly_remaining_pct: None,
            note: Some(note.into()),
        }
    }
}

/// Learn the user's active-session pattern from a trailing window of sessions.
///
/// An "active session" is one whose `total_tokens >= active_session_min_tokens`
/// — so idle / one-message renewals don't count. `active_per_day` is the
/// **trimmed mean** of per-day active-session counts over **active days only**
/// (days with zero active sessions are ignored so vacations don't deflate the
/// estimate), with the top/bottom [`TRIM_FRACTION`] of days dropped so a single
/// marathon day can't skew it.
///
/// `now` and `history_days` bound the window; `min_tokens` is the active
/// threshold (config `pacing.active_session_min_tokens`).
pub fn learn_pattern(
    sessions: &[SessionTokens],
    now: DateTime<Utc>,
    history_days: u32,
    min_tokens: u64,
) -> ActivityPattern {
    let window_start = now - Duration::days(history_days as i64);
    let today = now.date_naive();

    // Bucket active sessions by calendar day; tally today's separately.
    let mut per_day: std::collections::HashMap<chrono::NaiveDate, u64> =
        std::collections::HashMap::new();
    let mut active_count: u64 = 0;
    let mut used_today: u64 = 0;

    for s in sessions {
        if s.total_tokens < min_tokens {
            continue;
        }
        let Some(ts) = parse_ts(&s.start_timestamp) else {
            continue;
        };
        if ts < window_start || ts > now {
            continue;
        }
        active_count += 1;
        let day = ts.date_naive();
        *per_day.entry(day).or_insert(0) += 1;
        if day == today {
            used_today += 1;
        }
    }

    // Active days only — drop the zero-usage days entirely.
    let mut daily_counts: Vec<f64> = per_day.values().map(|&c| c as f64).collect();
    let active_per_day = trimmed_mean(&mut daily_counts, TRIM_FRACTION);

    // avg weekly-% per active session: how much weekly utilization a typical
    // active session moves. We can't read historical weekly-% directly, so we
    // anchor it to the renewal geometry: one weekly window holds (7d / 5h) =
    // 33.6 raw renewals, but only `active_per_day * 7` of them are active. A
    // "full" active session (one that hits 100% of its 5h window) therefore
    // moves the weekly window by `100 / active_sessions_per_week`. This is the
    // share-of-weekly one saturated active session represents.
    let active_sessions_per_week = (active_per_day * 7.0).max(1.0);
    let avg_weekly_pct_per_active_session = 100.0 / active_sessions_per_week;

    ActivityPattern {
        active_per_day,
        avg_weekly_pct_per_active_session,
        used_active_sessions_today: used_today as f64,
        active_session_count: active_count as f64,
    }
}

/// Compute the session budget gauge from the live quota snapshot and the
/// learned pattern.
///
/// All pacing is in **percentage of the weekly window**:
///
/// 1. `weekly_remaining = 100 - weekly.used_percentage`
/// 2. `days_left = (weekly.resets_at - now)` as fractional days
/// 3. `sessions_left = max(MIN_SESSIONS_LEFT,
///        active_per_day * days_left - used_active_sessions_today)`
/// 4. `weekly_pct_per_session = weekly_remaining / sessions_left`
/// 5. `recommended_pct = min(100,
///        100 * weekly_pct_per_session / avg_weekly_pct_per_active_session)`
/// 6. `headroom = recommended_pct - session.used_percentage`  (gauge color)
///
/// Returns [`PacingStatus::Insufficient`] (no number) when the quota source is
/// not [`QuotaSource::Api`], when the 5h/7d windows are missing, or when the
/// learned pattern is too thin.
pub fn session_budget(snapshot: &QuotaSnapshot, now: DateTime<Utc>) -> SessionBudget {
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

    // Too little history → no fabricated number.
    if pattern.active_session_count < INSUFFICIENT_ACTIVE_SESSIONS || pattern.active_per_day <= 0.0 {
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
    let raw_sessions_left = pattern.active_per_day * days_left - pattern.used_active_sessions_today;
    let sessions_left = raw_sessions_left.max(MIN_SESSIONS_LEFT);

    let weekly_pct_per_session = weekly_remaining / sessions_left;
    let per_full = pattern.avg_weekly_pct_per_active_session.max(f64::EPSILON);
    let recommended_pct = (100.0 * weekly_pct_per_session / per_full).clamp(0.0, 100.0);
    let headroom = recommended_pct - session_used;

    let status = if session_used > recommended_pct {
        PacingStatus::Over
    } else if headroom <= 10.0 {
        PacingStatus::Watch
    } else {
        PacingStatus::Ok
    };

    let note = Some(format!(
        "~{:.0} active session(s) left this week · {:.0}% weekly remaining",
        sessions_left, weekly_remaining
    ));

    SessionBudget {
        recommended_pct: Some(recommended_pct),
        headroom_pct: Some(headroom),
        status,
        session_used_pct: Some(session_used),
        active_per_day: pattern.active_per_day,
        sessions_left,
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

    /// Sub-threshold sessions are excluded; zero-usage days are ignored.
    #[test]
    fn learn_excludes_subthreshold_and_zero_days() {
        let now = Utc::now();
        let day = |d: i64| (now - Duration::days(d)).format("%Y-%m-%dT12:00:00Z").to_string();

        let sessions = vec![
            // Day -1: two real sessions + one trivial (excluded).
            session(60_000, &day(1)),
            session(80_000, &day(1)),
            session(1_000, &day(1)), // below threshold → excluded
            // Day -2: one real session.
            session(70_000, &day(2)),
            // Day -3: only trivial sessions → not an active day at all.
            session(500, &day(3)),
        ];

        let p = learn_pattern(&sessions, now, 14, 50_000);
        // Active days: day-1 (2 sessions), day-2 (1 session). Trivial day-3 gone.
        // With 2 active days [2.0, 1.0], trimmed mean (n<3 → plain mean) = 1.5.
        assert!((p.active_per_day - 1.5).abs() < 1e-9, "got {}", p.active_per_day);
        assert_eq!(p.active_session_count, 3.0);
    }

    /// Trimmed mean drops an outlier marathon day.
    #[test]
    fn learn_trimmed_mean_drops_outliers() {
        let now = Utc::now();
        // 10 days, each with 2 active sessions, plus one 20-session marathon.
        let mut sessions = Vec::new();
        for d in 1..=10 {
            let ts = (now - Duration::days(d)).format("%Y-%m-%dT12:00:00Z").to_string();
            sessions.push(session(60_000, &ts));
            sessions.push(session(60_000, &ts));
        }
        let marathon = (now - Duration::days(11)).format("%Y-%m-%dT12:00:00Z").to_string();
        for _ in 0..20 {
            sessions.push(session(60_000, &marathon));
        }

        let p = learn_pattern(&sessions, now, 14, 50_000);
        // 11 active days: ten 2.0's and one 20.0. trim=0.1 → cut=floor(1.1)=1
        // drops one from each tail (the 20 and one 2) → mean of nine 2.0's = 2.0.
        assert!(
            (p.active_per_day - 2.0).abs() < 1e-9,
            "outlier not trimmed: got {}",
            p.active_per_day
        );
    }

    /// Sessions outside the trailing window are excluded.
    #[test]
    fn learn_respects_history_window() {
        let now = Utc::now();
        let recent = (now - Duration::days(2)).format("%Y-%m-%dT12:00:00Z").to_string();
        let ancient = (now - Duration::days(40)).format("%Y-%m-%dT12:00:00Z").to_string();
        let sessions = vec![session(60_000, &recent), session(60_000, &ancient)];
        let p = learn_pattern(&sessions, now, 14, 50_000);
        assert_eq!(p.active_session_count, 1.0);
    }

    // ── session_budget ───────────────────────────────────────────────────────

    /// Known inputs → known recommended_pct.
    ///
    /// Pattern: 2 active/day, avg_weekly_pct_per_active_session = 100/(2*7)=7.142857.
    /// Quota: weekly 30% used → 70% remaining; weekly resets in 72h = 3 days.
    /// sessions_left = 2*3 - 0 = 6. weekly_pct_per_session = 70/6 = 11.6667.
    /// recommended = 100 * 11.6667 / 7.142857 = 163.3 → clamped to 100.
    #[test]
    fn budget_known_inputs_clamp_to_100() {
        let now = Utc::now();
        let mut snap = quota_with(30.0, 20.0, 72);
        snap.pacing_pattern = Some(ActivityPattern {
            active_per_day: 2.0,
            avg_weekly_pct_per_active_session: 100.0 / 14.0,
            used_active_sessions_today: 0.0,
            active_session_count: 28.0,
        });
        let b = session_budget(&snap, now);
        assert_eq!(b.status, PacingStatus::Ok);
        assert_eq!(b.recommended_pct, Some(100.0));
        assert!((b.sessions_left - 6.0).abs() < 0.01, "got {}", b.sessions_left);
    }

    /// A tighter week yields a sub-100 budget; arithmetic is exact.
    ///
    /// weekly 80% used → 20% remaining; resets in 24h = 1 day; 2 active/day.
    /// sessions_left = 2*1 = 2. weekly_pct_per_session = 20/2 = 10.
    /// per_full = 100/14 = 7.142857. recommended = 100*10/7.142857 = 140 → 100.
    /// Drop active_per_day to make it land mid-range:
    /// active_per_day = 0.5 → per_full = 100/3.5 = 28.571; sessions_left=max(0.5,0.5)=0.5
    /// weekly_pct_per_session = 20/0.5 = 40; recommended=100*40/28.571=140→100. Still clamps.
    /// Use weekly 95% remaining tiny: weekly 95% used → 5% remaining, 2/day, 2 days.
    /// sessions_left=4; per_session=1.25; per_full=7.142857; rec=17.5.
    #[test]
    fn budget_midrange_value_is_exact() {
        let now = Utc::now();
        let mut snap = quota_with(95.0, 5.0, 48);
        snap.pacing_pattern = Some(ActivityPattern {
            active_per_day: 2.0,
            avg_weekly_pct_per_active_session: 100.0 / 14.0,
            used_active_sessions_today: 0.0,
            active_session_count: 28.0,
        });
        let b = session_budget(&snap, now);
        let rec = b.recommended_pct.unwrap();
        assert!((rec - 17.5).abs() < 0.5, "expected ~17.5, got {rec}");
        // session used 5% < 17.5 ceiling, headroom 12.5 > 10 → Ok.
        assert_eq!(b.status, PacingStatus::Ok);
    }

    /// Weekly already ~100% used → over budget.
    #[test]
    fn budget_weekly_near_100_is_over() {
        let now = Utc::now();
        let mut snap = quota_with(99.0, 40.0, 48);
        snap.pacing_pattern = Some(ActivityPattern {
            active_per_day: 2.0,
            avg_weekly_pct_per_active_session: 100.0 / 14.0,
            used_active_sessions_today: 0.0,
            active_session_count: 28.0,
        });
        let b = session_budget(&snap, now);
        // 1% remaining over 4 sessions → tiny ceiling, 40% used blows past it.
        assert_eq!(b.status, PacingStatus::Over);
        assert!(b.recommended_pct.unwrap() < 40.0);
    }

    /// Thin history → Insufficient with no number.
    #[test]
    fn budget_thin_history_is_insufficient() {
        let now = Utc::now();
        let mut snap = quota_with(30.0, 20.0, 72);
        snap.pacing_pattern = Some(ActivityPattern {
            active_per_day: 1.0,
            avg_weekly_pct_per_active_session: 100.0 / 7.0,
            used_active_sessions_today: 0.0,
            active_session_count: 1.0, // below INSUFFICIENT_ACTIVE_SESSIONS
        });
        let b = session_budget(&snap, now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert!(b.recommended_pct.is_none());
        assert!(b.note.is_some());
    }

    /// No pattern attached → Insufficient.
    #[test]
    fn budget_no_pattern_is_insufficient() {
        let now = Utc::now();
        let snap = quota_with(30.0, 20.0, 72); // pacing_pattern defaults to None
        let b = session_budget(&snap, now);
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
            active_per_day: 2.0,
            avg_weekly_pct_per_active_session: 100.0 / 14.0,
            used_active_sessions_today: 0.0,
            active_session_count: 28.0,
        });
        let b = session_budget(&snap, now);
        assert_eq!(b.status, PacingStatus::Insufficient);
        assert_eq!(b.note.as_deref(), Some("Needs live quota data"));
    }

    /// Divide-by-~0 guard: a nearly-spent week (resets imminently) floors
    /// sessions_left so the budget stays finite and clamped, never absurd.
    #[test]
    fn budget_floors_sessions_left() {
        let now = Utc::now();
        // Weekly resets in 1 minute → days_left ≈ 0.0007, raw sessions_left < 0.
        let mut snap = quota_with(50.0, 10.0, 0);
        // Override weekly reset to 1 minute out.
        snap.windows[1].resets_at = Some(now + Duration::minutes(1));
        snap.pacing_pattern = Some(ActivityPattern {
            active_per_day: 2.0,
            avg_weekly_pct_per_active_session: 100.0 / 14.0,
            used_active_sessions_today: 0.0,
            active_session_count: 28.0,
        });
        let b = session_budget(&snap, now);
        assert_eq!(b.sessions_left, MIN_SESSIONS_LEFT);
        let rec = b.recommended_pct.unwrap();
        assert!(rec.is_finite() && (0.0..=100.0).contains(&rec), "rec={rec}");
    }

    // ── trimmed_mean unit ────────────────────────────────────────────────────

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
