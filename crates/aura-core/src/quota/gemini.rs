//! Gemini quota / `gemini /stats` data source.
//!
//! Gemini's CLI doesn't expose rate-limit windows in its session files the
//! way Codex does. The most useful thing we can surface in the Quota tab is
//! what `gemini /stats` itself shows: the active session's token totals,
//! plus a rolling 7-day rollup. Both are computed locally from session
//! JSONL files under `{config_path}/tmp/<project>/chats/`.
//!
//! Result is always a `Fallback`-source snapshot — these are local estimates,
//! not subscription-limit data from a backend.
//!
//! Token semantics mirror `gemini_scan`:
//!   - `input - cached` counts as new input
//!   - `output + thoughts + tool` counts as output
//!   - Same `id` appearing twice is deduped (the second emit lands after
//!     tool calls resolve)

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use super::{QuotaSnapshot, QuotaSource, QuotaWindow};

// ── On-disk shape ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawEntry {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    entry_type: Option<String>,
    tokens: Option<RawTokens>,
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawTokens {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cached: u64,
    #[serde(default)]
    thoughts: u64,
    #[serde(default)]
    tool: u64,
}

// ── Public reader ─────────────────────────────────────────────────────────────

pub struct GeminiQuota {
    gemini_config_dir: PathBuf,
}

impl GeminiQuota {
    pub fn new(gemini_config_dir: PathBuf) -> Self {
        Self { gemini_config_dir }
    }

    /// Compute "Current session" + "Last 7 days" token rollups from session
    /// JSONL files. Returns `Unavailable` when no sessions exist.
    pub fn snapshot(&self) -> QuotaSnapshot {
        match self.snapshot_inner() {
            Ok(Some(snap)) => snap,
            Ok(None) => QuotaSnapshot::unavailable(
                "No Gemini session activity yet — start a session to see usage.",
            ),
            Err(e) => QuotaSnapshot::unavailable(format!("Gemini quota read failed: {e}")),
        }
    }

    fn snapshot_inner(&self) -> Result<Option<QuotaSnapshot>> {
        let files = list_session_files_newest_first(&self.gemini_config_dir)?;
        if files.is_empty() {
            return Ok(None);
        }

        let now = Utc::now();
        let day7_ago = now - Duration::days(7);

        let mut current_session_tokens: Option<u64> = None;
        let mut current_session_last_ts: Option<DateTime<Utc>> = None;
        let mut seven_day_tokens: u64 = 0;

        for (idx, path) in files.iter().enumerate() {
            let Some(session) = scan_session(path) else {
                continue;
            };

            // First valid file (newest by mtime) becomes "Current session".
            if idx == 0 || current_session_tokens.is_none() {
                current_session_tokens = Some(session.total_tokens);
                current_session_last_ts = session
                    .last_activity
                    .as_deref()
                    .and_then(parse_ts)
                    .or(current_session_last_ts);
            }

            // 7-day rollup: include if the session was last active within 7 days.
            if let Some(ts) = session.last_activity.as_deref().and_then(parse_ts) {
                if ts >= day7_ago {
                    seven_day_tokens = seven_day_tokens.saturating_add(session.total_tokens);
                }
            }
        }

        let Some(session_tokens) = current_session_tokens else {
            return Ok(None);
        };

        let windows = vec![
            QuotaWindow {
                label: "Current session".to_string(),
                used_percentage: None,
                used_tokens: Some(session_tokens),
                resets_at: None,
            },
            QuotaWindow {
                label: "Last 7 days".to_string(),
                used_percentage: None,
                used_tokens: Some(seven_day_tokens),
                resets_at: None,
            },
        ];

        let note = current_session_last_ts.map(|ts| {
            format!(
                "Local estimate — Gemini doesn't publish per-window quotas. \
                 Last active: {}",
                ts.format("%Y-%m-%d %H:%M UTC")
            )
        });

        Ok(Some(QuotaSnapshot {
            subscription_type: None,
            windows,
            source: QuotaSource::Fallback,
            note,
        }))
    }
}

// ── Per-session scan ──────────────────────────────────────────────────────────

struct SessionTotals {
    total_tokens: u64,
    last_activity: Option<String>,
}

fn scan_session(path: &Path) -> Option<SessionTotals> {
    let content = fs::read_to_string(path).ok()?;
    let mut total: u64 = 0;
    let mut last_activity: Option<String> = None;
    let mut seen_ids: HashSet<String> = HashSet::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("{\"$") {
            continue;
        }
        let entry: RawEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Header line carries `startTime`/`lastUpdated`.
        if entry.entry_type.is_none() && entry.start_time.is_some() {
            last_activity = entry
                .last_updated
                .clone()
                .or_else(|| entry.start_time.clone())
                .or(last_activity);
            continue;
        }

        if let Some(ts) = entry.timestamp.clone() {
            last_activity = Some(ts);
        }

        if entry.entry_type.as_deref() != Some("gemini") {
            continue;
        }
        let Some(id) = entry.id.clone() else {
            continue;
        };
        if !seen_ids.insert(id) {
            continue;
        }
        let Some(t) = &entry.tokens else { continue };

        let new_input = t.input.saturating_sub(t.cached);
        let output = t.output + t.thoughts + t.tool;
        total = total.saturating_add(new_input + output);
    }

    if total == 0 && last_activity.is_none() {
        return None;
    }
    Some(SessionTotals {
        total_tokens: total,
        last_activity,
    })
}

// ── File discovery (newest first) ─────────────────────────────────────────────

fn list_session_files_newest_first(config_path: &Path) -> Result<Vec<PathBuf>> {
    let tmp_dir = config_path.join("tmp");
    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for project_entry in fs::read_dir(&tmp_dir)? {
        let project_dir = project_entry?.path();
        if !project_dir.is_dir() {
            continue;
        }
        let chats_dir = project_dir.join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        for file in fs::read_dir(&chats_dir)? {
            let path = file?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime = fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            files.push((path, mtime));
        }
    }
    files.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(files.into_iter().map(|(p, _)| p).collect())
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    use chrono::NaiveDateTime;
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ").map(|d| d.and_utc()))
        .ok()
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

    fn chats_dir(base: &Path, project: &str) -> PathBuf {
        let dir = base.join("tmp").join(project).join("chats");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn header(start: &str, last: &str) -> String {
        format!(
            r#"{{"sessionId":"x","projectHash":"y","startTime":"{start}","lastUpdated":"{last}","kind":"main"}}"#
        )
    }

    fn gemini_turn(
        id: &str,
        ts: &str,
        input: u64,
        output: u64,
        cached: u64,
        thoughts: u64,
    ) -> String {
        serde_json::json!({
            "id": id,
            "timestamp": ts,
            "type": "gemini",
            "model": "gemini-3-flash",
            "tokens": {
                "input": input,
                "output": output,
                "cached": cached,
                "thoughts": thoughts,
                "tool": 0,
            },
        })
        .to_string()
    }

    #[test]
    fn unavailable_when_no_sessions() {
        let dir = tempdir().unwrap();
        let snap = GeminiQuota::new(dir.path().to_path_buf()).snapshot();
        assert_eq!(snap.source, QuotaSource::Unavailable);
    }

    #[test]
    fn current_session_uses_newest_file_by_mtime() {
        let dir = tempdir().unwrap();
        let chats = chats_dir(dir.path(), "proj");

        // Older session
        let now = Utc::now();
        let recent_ts = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        write_jsonl(
            &chats,
            "session-old.jsonl",
            &[
                &header("2026-01-01T10:00:00Z", "2026-01-01T10:01:00Z"),
                &gemini_turn("g1", "2026-01-01T10:01:00Z", 100, 50, 0, 0),
            ],
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_jsonl(
            &chats,
            "session-new.jsonl",
            &[
                &header(&recent_ts, &recent_ts),
                &gemini_turn("g2", &recent_ts, 200, 100, 0, 0),
            ],
        );

        let snap = GeminiQuota::new(dir.path().to_path_buf()).snapshot();
        assert_eq!(snap.source, QuotaSource::Fallback);
        assert_eq!(snap.windows.len(), 2);
        // Current session is the newer file → 200 + 100 = 300
        assert_eq!(snap.windows[0].label, "Current session");
        assert_eq!(snap.windows[0].used_tokens, Some(300));
    }

    #[test]
    fn seven_day_rollup_excludes_old_sessions() {
        let dir = tempdir().unwrap();
        let chats = chats_dir(dir.path(), "proj");

        let now = Utc::now();
        let recent = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let stale = (now - Duration::days(30))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        // Stale session: 30 days ago — outside the window.
        write_jsonl(
            &chats,
            "session-stale.jsonl",
            &[
                &header(&stale, &stale),
                &gemini_turn("g1", &stale, 9999, 9999, 0, 0),
            ],
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Recent session: today — counts.
        write_jsonl(
            &chats,
            "session-recent.jsonl",
            &[
                &header(&recent, &recent),
                &gemini_turn("g2", &recent, 50, 25, 0, 0),
            ],
        );

        let snap = GeminiQuota::new(dir.path().to_path_buf()).snapshot();
        let seven_day = &snap.windows[1];
        assert_eq!(seven_day.label, "Last 7 days");
        // Only the recent session counts: 50 + 25 = 75
        assert_eq!(seven_day.used_tokens, Some(75));
    }

    #[test]
    fn dedupes_repeated_ids_in_a_session() {
        let dir = tempdir().unwrap();
        let chats = chats_dir(dir.path(), "proj");

        let now = Utc::now();
        let ts = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        write_jsonl(
            &chats,
            "session-1.jsonl",
            &[
                &header(&ts, &ts),
                &gemini_turn("g1", &ts, 100, 50, 0, 0),
                &gemini_turn("g1", &ts, 100, 50, 0, 0), // duplicate
                &gemini_turn("g2", &ts, 200, 100, 0, 0),
            ],
        );

        let snap = GeminiQuota::new(dir.path().to_path_buf()).snapshot();
        // Counted once: (100+50) + (200+100) = 450
        assert_eq!(snap.windows[0].used_tokens, Some(450));
    }
}
