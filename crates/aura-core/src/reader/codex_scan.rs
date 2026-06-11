/// Codex-specific JSONL scanning logic.
///
/// Codex rollout files live at `{config_path}/sessions/{YYYY}/{MM}/{DD}/rollout-*.jsonl`
/// and use a different event shape from Claude Code. Three line types matter:
///   - `session_meta`     — session id + start timestamp (first line)
///   - `turn_context`     — `payload.model` switches the model for subsequent turns
///   - `event_msg` with   — `payload.last_token_usage` is the per-turn delta
///     `payload.type ==     attributed to the most-recent `turn_context.model`
///     "token_count"`
///
/// Token semantics differ from Claude: Codex's `input_tokens` *includes* the
/// cached portion. We split it so it matches Claude's UsageSnapshot contract:
///   - `input_tokens`     = codex.input_tokens − codex.cached_input_tokens
///   - `output_tokens`    = codex.output_tokens (includes reasoning)
///   - `cache_read_tokens`  = codex.cached_input_tokens
///   - `cache_write_tokens` = 0 (Codex doesn't expose a separate "creation" bucket)
use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Deserialize;

use super::{
    dates::{date_from_timestamp, hour_from_timestamp},
    scan::ScanAccum,
};

// ── JSONL entry types ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SessionMetaPayload {
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct TurnContextPayload {
    model: Option<String>,
}

#[derive(Deserialize)]
struct TokenCountPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    info: Option<TokenCountInfo>,
}

#[derive(Deserialize)]
struct TokenCountInfo {
    last_token_usage: Option<TokenUsage>,
}

#[derive(Deserialize, Default)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

// ── File discovery ────────────────────────────────────────────────────────────

/// Returns all `rollout-*.jsonl` paths under `{config_path}/sessions/`.
/// Walks year → month → day directories. Non-jsonl files are skipped.
pub fn list_session_files(config_path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let sessions_dir = config_path.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for year_entry in read_subdirs(&sessions_dir)? {
        for month_entry in read_subdirs(&year_entry)? {
            for day_entry in read_subdirs(&month_entry)? {
                for file in fs::read_dir(&day_entry)? {
                    let path = file?.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        files.push(path);
                    }
                }
            }
        }
    }
    Ok(files)
}

fn read_subdirs(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    Ok(out)
}

// ── Scanner ───────────────────────────────────────────────────────────────────

/// Scan a list of Codex rollout files, optionally restricting to entries whose
/// session start date falls within `[from_date, to_date]` (inclusive,
/// "YYYY-MM-DD"). Mirrors `scan::scan_files` for the Codex format.
pub fn scan_files(
    files: &[PathBuf],
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> anyhow::Result<ScanAccum> {
    let mut accum = ScanAccum::default();

    for path in files {
        // ── Fast mtime check ──────────────────────────────────────────────────
        if let Some(from) = from_date {
            if let Some(mtime) = file_mtime_date(path) {
                if mtime.as_str() < from {
                    continue;
                }
            }
        }

        // ── Read and parse entries ────────────────────────────────────────────
        let entries = match read_jsonl_entries(path) {
            Ok(e) => e,
            Err(_) => continue, // skip unreadable files silently
        };

        if entries.is_empty() {
            continue;
        }

        // ── Session start: prefer session_meta.payload.timestamp, fall back to
        //    the entry timestamp, then the first entry with any timestamp ─────
        let session_start = session_start_timestamp(&entries);
        let session_start = match session_start {
            Some(ts) => ts,
            None => continue,
        };
        let session_date = match date_from_timestamp(&session_start) {
            Some(d) => d,
            None => continue,
        };

        // Apply date range filter on session start date
        if let Some(from) = from_date {
            if session_date.as_str() < from {
                continue;
            }
        }
        if let Some(to) = to_date {
            if session_date.as_str() > to {
                continue;
            }
        }

        // ── Walk entries: track current model, attribute token deltas ─────────
        let mut current_model: Option<String> = None;
        let mut last_entry_ts: Option<String> = Some(session_start.clone());
        let mut session_messages: u64 = 0;

        for entry in &entries {
            if let Some(ts) = &entry.timestamp {
                last_entry_ts = Some(ts.clone());
            }

            match entry.entry_type.as_str() {
                "turn_context" => {
                    if let Some(payload) = &entry.payload {
                        if let Ok(p) = serde_json::from_value::<TurnContextPayload>(payload.clone())
                        {
                            if let Some(m) = p.model.filter(|m| !m.is_empty()) {
                                current_model = Some(m);
                            }
                        }
                    }
                }
                "event_msg" => {
                    let Some(payload) = &entry.payload else {
                        continue;
                    };
                    let Ok(p) = serde_json::from_value::<TokenCountPayload>(payload.clone()) else {
                        continue;
                    };
                    if p.payload_type.as_deref() != Some("token_count") {
                        continue;
                    }
                    let Some(usage) = p.info.and_then(|i| i.last_token_usage) else {
                        continue;
                    };
                    let Some(model) = current_model.clone() else {
                        // Token count before any turn_context — skip; we'd
                        // mis-attribute it. Realistically rare since the first
                        // turn_context always precedes the first token_count.
                        continue;
                    };

                    let new_input = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
                    let cache_read = usage.cached_input_tokens;
                    let output = usage.output_tokens;

                    let model_accum = accum.model_usage.entry(model.clone()).or_default();
                    model_accum.input_tokens += new_input;
                    model_accum.output_tokens += output;
                    model_accum.cache_read_tokens += cache_read;
                    // Codex doesn't expose a "creation" bucket; leave at 0.

                    session_messages += 1;
                    *accum
                        .daily_message_counts
                        .entry(session_date.clone())
                        .or_insert(0) += 1;

                    let day_model = accum
                        .daily_model_tokens
                        .entry(session_date.clone())
                        .or_default();
                    *day_model.entry(model).or_insert(0) += new_input + output;
                }
                _ => {}
            }
        }

        // ── Session stats (only if we recorded any assistant turns) ───────────
        if session_messages > 0 {
            let last_ts = last_entry_ts.as_deref().unwrap_or(&session_start);
            let duration_secs = ts_diff_secs(&session_start, last_ts);

            accum.sessions.push(super::scan::SessionStat {
                duration_secs,
                message_count: session_messages,
                start_timestamp: session_start.clone(),
                // Per-project / per-session insights are Claude-Code-only; Codex
                // sessions leave the new fields at their defaults.
                ..Default::default()
            });

            accum.total_messages += session_messages;

            *accum
                .daily_session_counts
                .entry(session_date.clone())
                .or_insert(0) += 1;

            if let Some(hour) = hour_from_timestamp(&session_start) {
                *accum.hour_counts.entry(hour).or_insert(0) += 1;
            }
        }
    }

    Ok(accum)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn session_start_timestamp(entries: &[RawEntry]) -> Option<String> {
    // Prefer the inner `payload.timestamp` of the first session_meta entry —
    // that's the real session start. Fall back to the outer timestamp on that
    // entry, or to the first entry with any timestamp.
    for entry in entries {
        if entry.entry_type == "session_meta" {
            if let Some(payload) = &entry.payload {
                if let Ok(p) = serde_json::from_value::<SessionMetaPayload>(payload.clone()) {
                    if let Some(ts) = p.timestamp {
                        return Some(ts);
                    }
                }
            }
            if let Some(ts) = entry.timestamp.clone() {
                return Some(ts);
            }
        }
    }
    entries.iter().find_map(|e| e.timestamp.clone())
}

fn read_jsonl_entries(path: &Path) -> anyhow::Result<Vec<RawEntry>> {
    let content = fs::read_to_string(path)?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<RawEntry>(line) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn file_mtime_date(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let secs = mtime.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)?;
    Some(dt.format("%Y-%m-%d").to_string())
}

fn ts_diff_secs(from: &str, to: &str) -> u64 {
    use chrono::{DateTime, NaiveDateTime};
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.timestamp())
            .or_else(|_| {
                NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                    .map(|d| d.and_utc().timestamp())
            })
            .unwrap_or(0)
    };
    let diff = parse(to) - parse(from);
    diff.max(0) as u64
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

    fn session_meta(outer_ts: &str, inner_ts: &str) -> String {
        serde_json::json!({
            "timestamp": outer_ts,
            "type": "session_meta",
            "payload": { "id": "abc", "timestamp": inner_ts }
        })
        .to_string()
    }

    fn turn_context(ts: &str, model: &str) -> String {
        serde_json::json!({
            "timestamp": ts,
            "type": "turn_context",
            "payload": { "model": model }
        })
        .to_string()
    }

    fn token_count(ts: &str, input: u64, cached: u64, output: u64) -> String {
        serde_json::json!({
            "timestamp": ts,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                    }
                }
            }
        })
        .to_string()
    }

    fn setup_codex_day(base: &Path, year: &str, month: &str, day: &str) -> PathBuf {
        let dir = base.join("sessions").join(year).join(month).join(day);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn list_session_files_walks_year_month_day() {
        let dir = tempdir().unwrap();
        let day1 = setup_codex_day(dir.path(), "2026", "05", "21");
        let day2 = setup_codex_day(dir.path(), "2026", "05", "22");

        write_jsonl(&day1, "rollout-a.jsonl", &[]);
        write_jsonl(&day1, "rollout-b.jsonl", &[]);
        write_jsonl(&day2, "rollout-c.jsonl", &[]);
        // Non-jsonl should be ignored
        write_jsonl(&day2, "notes.txt", &[]);

        let files = list_session_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[test]
    fn list_session_files_missing_root_returns_empty() {
        let dir = tempdir().unwrap();
        let files = list_session_files(dir.path()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn scan_attributes_tokens_to_current_model_and_splits_cache() {
        let dir = tempdir().unwrap();
        let day = setup_codex_day(dir.path(), "2026", "05", "21");

        let file = write_jsonl(
            &day,
            "rollout-1.jsonl",
            &[
                &session_meta("2026-05-21T23:00:04.647Z", "2026-05-21T22:56:58.259Z"),
                &turn_context("2026-05-21T23:00:04.677Z", "gpt-5.5"),
                // input=27109 (4480 cached), output=336
                &token_count("2026-05-21T23:00:13.127Z", 27109, 4480, 336),
                // delta turn 2: input=27967 (27008 cached), output=281
                &token_count("2026-05-21T23:00:19.904Z", 27967, 27008, 281),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();

        let m = accum.model_usage.get("gpt-5.5").unwrap();
        // new input = (27109 - 4480) + (27967 - 27008) = 22629 + 959 = 23588
        assert_eq!(m.input_tokens, 23588);
        // output = 336 + 281 = 617
        assert_eq!(m.output_tokens, 617);
        // cache_read = 4480 + 27008 = 31488
        assert_eq!(m.cache_read_tokens, 31488);
        assert_eq!(m.cache_write_tokens, 0);

        // daily_model_tokens for the session date (UTC -> 2026-05-21)
        let day_tokens = accum.daily_model_tokens.get("2026-05-21").unwrap();
        // = new_input + output across turns = 23588 + 617 = 24205
        assert_eq!(*day_tokens.get("gpt-5.5").unwrap(), 24205);

        assert_eq!(accum.sessions.len(), 1);
        assert_eq!(accum.total_messages, 2);
    }

    #[test]
    fn scan_handles_mid_session_model_switch() {
        let dir = tempdir().unwrap();
        let day = setup_codex_day(dir.path(), "2026", "05", "21");

        let file = write_jsonl(
            &day,
            "rollout-1.jsonl",
            &[
                &session_meta("2026-05-21T23:00:00Z", "2026-05-21T23:00:00Z"),
                &turn_context("2026-05-21T23:00:01Z", "gpt-5.5"),
                &token_count("2026-05-21T23:00:02Z", 100, 0, 50),
                &turn_context("2026-05-21T23:00:03Z", "gpt-5-codex"),
                &token_count("2026-05-21T23:00:04Z", 200, 0, 80),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        assert_eq!(accum.model_usage.get("gpt-5.5").unwrap().input_tokens, 100);
        assert_eq!(
            accum.model_usage.get("gpt-5-codex").unwrap().input_tokens,
            200
        );
    }

    #[test]
    fn scan_skips_token_count_before_first_turn_context() {
        let dir = tempdir().unwrap();
        let day = setup_codex_day(dir.path(), "2026", "05", "21");

        let file = write_jsonl(
            &day,
            "rollout-1.jsonl",
            &[
                &session_meta("2026-05-21T23:00:00Z", "2026-05-21T23:00:00Z"),
                // No turn_context yet -> this token_count is unattributable
                &token_count("2026-05-21T23:00:02Z", 100, 0, 50),
                &turn_context("2026-05-21T23:00:03Z", "gpt-5.5"),
                &token_count("2026-05-21T23:00:04Z", 200, 0, 80),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        assert_eq!(accum.model_usage.get("gpt-5.5").unwrap().input_tokens, 200);
        assert_eq!(accum.total_messages, 1);
    }

    #[test]
    fn scan_filters_by_date_range_on_session_start() {
        let dir = tempdir().unwrap();
        let old_day = setup_codex_day(dir.path(), "2026", "04", "01");
        let new_day = setup_codex_day(dir.path(), "2026", "05", "21");

        let old = write_jsonl(
            &old_day,
            "rollout-old.jsonl",
            &[
                &session_meta("2026-04-01T10:00:00Z", "2026-04-01T10:00:00Z"),
                &turn_context("2026-04-01T10:00:01Z", "gpt-5.5"),
                &token_count("2026-04-01T10:00:02Z", 100, 0, 100),
            ],
        );
        let new = write_jsonl(
            &new_day,
            "rollout-new.jsonl",
            &[
                &session_meta("2026-05-21T10:00:00Z", "2026-05-21T10:00:00Z"),
                &turn_context("2026-05-21T10:00:01Z", "gpt-5.5"),
                &token_count("2026-05-21T10:00:02Z", 50, 0, 50),
            ],
        );

        let accum = scan_files(&[old, new], Some("2026-05-01"), None).unwrap();
        let m = accum.model_usage.get("gpt-5.5").unwrap();
        assert_eq!(m.input_tokens, 50);
        assert_eq!(accum.sessions.len(), 1);
    }

    #[test]
    fn scan_ignores_other_event_msg_payload_types() {
        let dir = tempdir().unwrap();
        let day = setup_codex_day(dir.path(), "2026", "05", "21");

        let other_event = serde_json::json!({
            "timestamp": "2026-05-21T23:00:05Z",
            "type": "event_msg",
            "payload": { "type": "task_started", "turn_id": "x" }
        })
        .to_string();

        let file = write_jsonl(
            &day,
            "rollout-1.jsonl",
            &[
                &session_meta("2026-05-21T23:00:00Z", "2026-05-21T23:00:00Z"),
                &turn_context("2026-05-21T23:00:01Z", "gpt-5.5"),
                &other_event,
                &token_count("2026-05-21T23:00:06Z", 10, 0, 5),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        assert_eq!(accum.model_usage.get("gpt-5.5").unwrap().input_tokens, 10);
        assert_eq!(accum.total_messages, 1);
    }
}
