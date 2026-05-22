//! Codex quota / `/status` data source.
//!
//! Codex persists the latest rate-limit snapshot inside every `token_count`
//! event in the session rollout files. We pull the most recent one and surface
//! it as the same `QuotaSnapshot` shape the Claude path uses. No network call
//! is needed — the codex binary already wrote what OpenAI's backend returned.
//!
//! Source layout: `{config_path}/sessions/{YYYY}/{MM}/{DD}/rollout-*.jsonl`.
//! Each `event_msg` line with `payload.type == "token_count"` carries a
//! `payload.rate_limits` block:
//!
//! ```json
//! {
//!   "limit_id": "codex",
//!   "primary":   { "used_percent": 32.0, "window_minutes": 300,   "resets_at": 1779422413 },
//!   "secondary": { "used_percent":  5.0, "window_minutes": 10080, "resets_at": 1780009212 },
//!   "plan_type": "plus",
//!   "rate_limit_reached_type": null
//! }
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{QuotaSnapshot, QuotaSource, QuotaWindow};

// ── On-disk shape ─────────────────────────────────────────────────────────────

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
    primary: Option<RawWindow>,
    secondary: Option<RawWindow>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
}

#[derive(Deserialize)]
struct RawWindow {
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

// ── Public reader ─────────────────────────────────────────────────────────────

pub struct CodexQuota {
    codex_config_dir: PathBuf,
}

impl CodexQuota {
    pub fn new(codex_config_dir: PathBuf) -> Self {
        Self { codex_config_dir }
    }

    /// Read the most recent rate-limit snapshot, or return `Unavailable` if
    /// no `token_count` event is found anywhere under `sessions/`.
    pub fn snapshot(&self) -> QuotaSnapshot {
        match self.snapshot_inner() {
            Ok(Some(snap)) => snap,
            Ok(None) => QuotaSnapshot::unavailable(
                "No Codex session activity yet — start a session to populate quota.",
            ),
            Err(e) => QuotaSnapshot::unavailable(format!("Codex quota read failed: {e}")),
        }
    }

    fn snapshot_inner(&self) -> Result<Option<QuotaSnapshot>> {
        let files = list_rollout_files_newest_first(&self.codex_config_dir)?;
        for path in files {
            if let Some((observed_at, rl)) = latest_rate_limits_in_file(&path)? {
                return Ok(Some(build_snapshot(observed_at, rl)));
            }
        }
        Ok(None)
    }
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
    files.sort_by(|a, b| b.1.cmp(&a.1));
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

// ── Snapshot builder ──────────────────────────────────────────────────────────

fn build_snapshot(observed_at: String, rl: RawRateLimits) -> QuotaSnapshot {
    let mut windows = Vec::new();
    if let Some(w) = rl.primary {
        windows.push(window_from("Primary", &w));
    }
    if let Some(w) = rl.secondary {
        windows.push(window_from("Secondary", &w));
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
        source: QuotaSource::Api,
        note,
    }
}

fn window_from(default_label: &str, w: &RawWindow) -> QuotaWindow {
    let label = match w.window_minutes {
        Some(m) => format_window_label(default_label, m),
        None => default_label.to_string(),
    };
    QuotaWindow {
        label,
        used_percentage: w.used_percent,
        used_tokens: None,
        resets_at: w.resets_at.and_then(unix_to_datetime),
    }
}

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
        assert_eq!(snap.source, QuotaSource::Unavailable);
        assert!(snap.note.is_some());
    }

    #[test]
    fn snapshot_returns_most_recent_token_count_in_file() {
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

        let snap = CodexQuota::new(dir.path().to_path_buf()).snapshot();
        assert_eq!(snap.source, QuotaSource::Api);
        assert_eq!(snap.subscription_type.as_deref(), Some("plus"));
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].used_percentage, Some(32.0)); // latest, not first
        assert_eq!(snap.windows[1].used_percentage, Some(5.0));
        // window_minutes 300 → 5h, 10080 → weekly
        assert!(snap.windows[0].label.contains("5h"));
        assert!(snap.windows[1].label.contains("weekly"));
    }

    #[test]
    fn snapshot_prefers_newest_file_by_mtime() {
        let dir = tempdir().unwrap();
        let day1 = day_dir(dir.path(), "2026", "05", "20");
        let day2 = day_dir(dir.path(), "2026", "05", "21");

        // Write older first so its mtime is older
        let old = write_jsonl(
            &day1,
            "rollout-old.jsonl",
            &[&token_count_line("2026-05-20T10:00:00Z", 1.0, 0.5, "plus")],
        );
        // Sleep a moment to guarantee a different mtime, then write the newer
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

        let snap = CodexQuota::new(dir.path().to_path_buf()).snapshot();
        assert_eq!(snap.windows[0].used_percentage, Some(99.0));
    }

    #[test]
    fn snapshot_falls_back_to_earlier_file_when_latest_has_no_token_count() {
        let dir = tempdir().unwrap();
        let day = day_dir(dir.path(), "2026", "05", "21");

        // Older file has the data
        write_jsonl(
            &day,
            "rollout-old.jsonl",
            &[&token_count_line("2026-05-21T09:00:00Z", 42.0, 7.0, "plus")],
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Newer file has no token_count — just a session_meta
        write_jsonl(
            &day,
            "rollout-empty.jsonl",
            &[r#"{"timestamp":"2026-05-21T10:00:00Z","type":"session_meta","payload":{"id":"x"}}"#],
        );

        let snap = CodexQuota::new(dir.path().to_path_buf()).snapshot();
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
}
