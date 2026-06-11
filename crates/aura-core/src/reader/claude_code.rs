use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use chrono::NaiveDate;

use super::{
    dates::{add_one_day, date_from_timestamp, n_days_ago, today},
    insights::build_insights,
    scan::{compute_streaks, list_session_files, scan_files, ModelAccum, ScanAccum},
    stats_cache::StatsCache,
    AgentReader, DailyActivity, DailyModelTokens, ModelUsage, Period, Streaks, UsageSnapshot,
};

/// How many ranked rows the reader retains per Insights list. The UI slices
/// this down to the user's `[insights] top_n`; keeping a generous cap here means
/// raising `top_n` never requires a re-scan.
const INSIGHTS_CAP: usize = 25;

// ── ClaudeCodeReader ──────────────────────────────────────────────────────────

pub struct ClaudeCodeReader {
    /// Path to the Claude Code config directory (the folder containing
    /// `projects/` and `stats-cache.json`).
    pub config_path: PathBuf,
}

impl ClaudeCodeReader {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl AgentReader for ClaudeCodeReader {
    fn snapshot(&self, period: Period) -> Result<UsageSnapshot> {
        let files = list_session_files(&self.config_path)?;

        match period {
            Period::Last7Days => {
                let accum = scan_files(&files, Some(&n_days_ago(6)), Some(&today()))?;
                Ok(build_snapshot(accum, None))
            }
            Period::Last30Days => {
                let accum = scan_files(&files, Some(&n_days_ago(29)), Some(&today()))?;
                Ok(build_snapshot(accum, None))
            }
            Period::AllTime => {
                let cache = StatsCache::load(&self.config_path)?.unwrap_or_default();
                let delta_from = if cache.last_computed_date.is_empty() {
                    None
                } else {
                    add_one_day(&cache.last_computed_date)
                };
                let accum = scan_files(&files, delta_from.as_deref(), None)?;
                Ok(build_snapshot(accum, Some(&cache)))
            }
        }
    }
}

// ── Snapshot builder ──────────────────────────────────────────────────────────

/// Build a `UsageSnapshot` from a scan accumulator, optionally merging in a
/// `StatsCache` baseline (used for the AllTime period).
pub(crate) fn build_snapshot(accum: ScanAccum, cache: Option<&StatsCache>) -> UsageSnapshot {
    // ── Insights (F3) ─────────────────────────────────────────────────────────
    // Computed up-front, while `accum` still owns its per-project / per-session
    // data (the field moves below consume the rest of the accumulator).
    //
    // Note: insights cover only the scanned window. For the AllTime period the
    // StatsCache baseline carries no per-project / per-session breakdown, so
    // insights there reflect the post-`lastComputedDate` delta scan. This is a
    // known limitation documented in the tab footnote.
    let insights = build_insights(&accum, INSIGHTS_CAP);

    // ── Start with scan data ──────────────────────────────────────────────────
    let mut model_usage: HashMap<String, ModelAccum> = accum.model_usage;
    let mut daily_msg: HashMap<String, u64> = accum.daily_message_counts;
    let mut daily_sess: HashMap<String, u64> = accum.daily_session_counts;
    let mut daily_tokens: HashMap<String, HashMap<String, u64>> = accum.daily_model_tokens;
    let mut hour_counts: HashMap<u8, u64> = accum.hour_counts;
    let mut total_sessions = accum.sessions.len() as u64;
    let mut total_messages = accum.total_messages;
    let mut longest_secs: Option<u64> = accum.sessions.iter().map(|s| s.duration_secs).max();

    let scan_first = accum
        .sessions
        .iter()
        .filter_map(|s| date_from_timestamp(&s.start_timestamp))
        .min();
    let scan_last = accum
        .sessions
        .iter()
        .filter_map(|s| date_from_timestamp(&s.start_timestamp))
        .max();

    let mut first_date: Option<String> = scan_first;
    let mut last_date: Option<String> = scan_last;

    // ── Merge cache baseline (AllTime only) ───────────────────────────────────
    if let Some(c) = cache {
        for (model, cu) in &c.model_usage {
            let e = model_usage.entry(model.clone()).or_default();
            e.input_tokens += cu.input_tokens;
            e.output_tokens += cu.output_tokens;
            e.cache_read_tokens += cu.cache_read_input_tokens;
            e.cache_write_tokens += cu.cache_creation_input_tokens;
        }

        for day in &c.daily_activity {
            *daily_msg.entry(day.date.clone()).or_insert(0) += day.message_count;
            *daily_sess.entry(day.date.clone()).or_insert(0) += day.session_count;
        }

        for day in &c.daily_model_tokens {
            let e = daily_tokens.entry(day.date.clone()).or_default();
            for (model, tokens) in &day.tokens_by_model {
                *e.entry(model.clone()).or_insert(0) += tokens;
            }
        }

        for (hs, count) in &c.hour_counts {
            if let Ok(h) = hs.parse::<u8>() {
                *hour_counts.entry(h).or_insert(0) += count;
            }
        }

        total_sessions += c.total_sessions;
        total_messages += c.total_messages;

        let cache_longest = c.longest_session.as_ref().map(|ls| ls.duration / 1000);
        longest_secs = match (longest_secs, cache_longest) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };

        let cache_first = c
            .first_session_date
            .as_deref()
            .and_then(date_from_timestamp);
        first_date = match (first_date, cache_first) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        // Last date from cache: max day in daily_activity
        let cache_last = c
            .daily_activity
            .iter()
            .map(|d| d.date.as_str())
            .max()
            .map(|s| s.to_string());
        last_date = match (last_date, cache_last) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }

    // ── Per-model output ──────────────────────────────────────────────────────
    let mut per_model: Vec<ModelUsage> = model_usage
        .iter()
        .map(|(model, ma)| ModelUsage {
            model: model.clone(),
            input_tokens: ma.input_tokens,
            output_tokens: ma.output_tokens,
            cache_read_tokens: ma.cache_read_tokens,
            cache_write_tokens: ma.cache_write_tokens,
        })
        .collect();
    per_model.sort_by_key(|m| std::cmp::Reverse(m.total_tokens()));

    let favorite_model = per_model.first().map(|m| m.model.clone());
    let total_tokens: u64 = per_model.iter().map(|m| m.total_tokens()).sum();
    let total_input_tokens: u64 = per_model.iter().map(|m| m.input_tokens).sum();
    let total_output_tokens: u64 = per_model.iter().map(|m| m.output_tokens).sum();
    let total_cache_read_tokens: u64 = per_model.iter().map(|m| m.cache_read_tokens).sum();
    let total_cache_write_tokens: u64 = per_model.iter().map(|m| m.cache_write_tokens).sum();

    // ── Activity metrics ──────────────────────────────────────────────────────
    let active_days = daily_sess.len() as u32;

    let total_days = match (&first_date, &last_date) {
        (Some(f), Some(l)) => {
            let fd = NaiveDate::parse_from_str(f, "%Y-%m-%d").ok();
            let ld = NaiveDate::parse_from_str(l, "%Y-%m-%d").ok();
            match (fd, ld) {
                (Some(fd), Some(ld)) => ((ld - fd).num_days() + 1).max(0) as u32,
                _ => 0,
            }
        }
        _ => 0,
    };

    let mut active_dates: Vec<String> = daily_sess.keys().cloned().collect();
    active_dates.sort_unstable();
    let (current_streak, longest_streak) = compute_streaks(&active_dates);

    let peak_hour = hour_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(h, _)| *h);

    // ── Daily breakdown vectors ───────────────────────────────────────────────
    let mut daily_tokens_vec: Vec<DailyModelTokens> = daily_tokens
        .into_iter()
        .map(|(date, by_model)| DailyModelTokens { date, by_model })
        .collect();
    daily_tokens_vec.sort_by(|a, b| a.date.cmp(&b.date));

    // Union of message-count and session-count dates
    let all_dates: std::collections::HashSet<String> =
        daily_msg.keys().chain(daily_sess.keys()).cloned().collect();
    let mut daily_activity_vec: Vec<DailyActivity> = all_dates
        .into_iter()
        .map(|date| DailyActivity {
            message_count: *daily_msg.get(&date).unwrap_or(&0),
            session_count: *daily_sess.get(&date).unwrap_or(&0),
            date,
        })
        .collect();
    daily_activity_vec.sort_by(|a, b| a.date.cmp(&b.date));

    // ── Assemble snapshot ─────────────────────────────────────────────────────
    UsageSnapshot {
        total_tokens,
        total_input_tokens,
        total_output_tokens,
        total_cache_read_tokens,
        total_cache_write_tokens,
        total_sessions,
        total_messages,
        active_days,
        total_days,
        favorite_model,
        longest_session_secs: longest_secs,
        streaks: Streaks {
            current: current_streak,
            longest: longest_streak,
        },
        peak_hour,
        per_model,
        daily_tokens: daily_tokens_vec,
        daily_activity: daily_activity_vec,
        first_session_date: first_date,
        last_session_date: last_date,
        insights,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};
    use tempfile::tempdir;

    fn write_jsonl(dir: &std::path::Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        path
    }

    fn assistant_entry(ts: &str, model: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": ts,
            "isSidechain": false,
            "cwd": "/Users/pedro/Downloads/my-proj",
            "message": {
                "model": model,
                "usage": {
                    "input_tokens": input,
                    "output_tokens": output,
                    "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 0,
                }
            }
        })
        .to_string()
    }

    fn user_entry(ts: &str) -> String {
        serde_json::json!({
            "type": "user",
            "timestamp": ts,
            "cwd": "/Users/pedro/Downloads/my-proj",
        })
        .to_string()
    }

    fn setup_claude_dir(base: &std::path::Path) -> std::path::PathBuf {
        let projects = base.join("projects").join("my-proj");
        fs::create_dir_all(&projects).unwrap();
        projects
    }

    #[test]
    fn last7days_snapshot_counts_tokens() {
        let dir = tempdir().unwrap();
        let projects = setup_claude_dir(dir.path());

        // Use today's date for the session so it falls within Last7Days
        let today = today();
        let ts = format!("{}T10:00:00Z", today);
        let ts2 = format!("{}T10:01:00Z", today);

        write_jsonl(
            &projects,
            "sess.jsonl",
            &[
                &user_entry(&ts),
                &assistant_entry(&ts2, "claude-opus-4-7", 100, 200),
            ],
        );

        let reader = ClaudeCodeReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::Last7Days).unwrap();
        assert_eq!(snap.total_tokens, 300);
        assert_eq!(snap.total_sessions, 1);
        assert_eq!(snap.favorite_model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn alltime_merges_cache_and_delta() {
        let dir = tempdir().unwrap();
        let projects = setup_claude_dir(dir.path());

        // Cache covers 2026-01-15
        let cache = serde_json::json!({
            "version": 2,
            "lastComputedDate": "2026-01-15",
            "dailyActivity": [
                { "date": "2026-01-15", "messageCount": 10, "sessionCount": 1 }
            ],
            "dailyModelTokens": [
                { "date": "2026-01-15", "tokensByModel": { "claude-opus-4-6": 500 } }
            ],
            "modelUsage": {
                "claude-opus-4-6": {
                    "inputTokens": 300, "outputTokens": 200,
                    "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0
                }
            },
            "totalSessions": 1,
            "totalMessages": 10,
            "firstSessionDate": "2026-01-15T10:00:00Z",
            "hourCounts": { "10": 1 }
        });

        let cache_path = dir.path().join("stats-cache.json");
        let mut f = fs::File::create(&cache_path).unwrap();
        write!(f, "{}", cache).unwrap();

        // Delta: a new session on 2026-01-16
        write_jsonl(
            &projects,
            "new.jsonl",
            &[
                &user_entry("2026-01-16T11:00:00Z"),
                &assistant_entry("2026-01-16T11:01:00Z", "claude-opus-4-6", 50, 50),
            ],
        );

        let reader = ClaudeCodeReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();

        // Cache: 300+200=500, delta: 50+50=100 → total 600
        assert_eq!(snap.total_tokens, 600);
        // Cache: 1 session, delta: 1 session → 2
        assert_eq!(snap.total_sessions, 2);
        // Should have 2 days of daily_activity (Jan 15 + Jan 16)
        assert_eq!(snap.daily_activity.len(), 2);
    }

    #[test]
    fn alltime_no_cache_scans_all() {
        let dir = tempdir().unwrap();
        let projects = setup_claude_dir(dir.path());

        write_jsonl(
            &projects,
            "s.jsonl",
            &[
                &user_entry("2026-03-01T10:00:00Z"),
                &assistant_entry("2026-03-01T10:01:00Z", "model-x", 200, 100),
            ],
        );

        let reader = ClaudeCodeReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();
        assert_eq!(snap.total_tokens, 300);
    }
}
