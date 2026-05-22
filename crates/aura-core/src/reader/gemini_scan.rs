/// Gemini-specific JSONL scanning logic.
///
/// Gemini's CLI writes per-project session files at
/// `{config_path}/tmp/<project-name-or-hash>/chats/session-*.jsonl`.
/// Each file starts with a metadata line carrying `startTime`/`lastUpdated`,
/// followed by `{type: "user"|"gemini", ...}` turn entries interleaved with
/// `{"$set": {...}}` mutation sentinels. Only `gemini` entries carry tokens.
///
/// Notable quirks vs. Codex / Claude Code:
///   - Gemini entries can be emitted twice with the same `id` (once when the
///     turn first lands, again after tool calls resolve). We dedupe by `id`
///     within each session file.
///   - The `tokens.input` value already includes the cached portion, mirroring
///     Codex semantics. We split it the same way:
///       input_tokens = input - cached
///       cache_read   = cached
///   - `tokens.thoughts` represents thinking/reasoning tokens. We add them to
///     `output_tokens` so the snapshot reports the full assistant-side cost,
///     matching how Codex folds reasoning into its output count.
use std::{
    collections::HashSet,
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
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    entry_type: Option<String>,
    model: Option<String>,
    tokens: Option<RawTokens>,
    // Session-header fields (present only on the first line)
    #[serde(rename = "startTime")]
    start_time: Option<String>,
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

// ── File discovery ────────────────────────────────────────────────────────────

/// Returns all `session-*.jsonl` paths under `{config_path}/tmp/*/chats/`.
pub fn list_session_files(config_path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let tmp_dir = config_path.join("tmp");
    if !tmp_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
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
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

// ── Scanner ───────────────────────────────────────────────────────────────────

/// Scan a list of Gemini session files, optionally restricting to entries
/// whose session start date falls within `[from_date, to_date]` (inclusive,
/// "YYYY-MM-DD"). Mirrors `codex_scan::scan_files` for the Gemini format.
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
            Err(_) => continue,
        };

        if entries.is_empty() {
            continue;
        }

        // ── Session start: prefer first-line `startTime`, fall back to first
        //    entry with any timestamp ────────────────────────────────────────
        let session_start = entries
            .iter()
            .find_map(|e| e.start_time.clone())
            .or_else(|| entries.iter().find_map(|e| e.timestamp.clone()));
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

        // ── Walk entries, dedupe by id, attribute tokens to model ─────────────
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut last_entry_ts: Option<String> = Some(session_start.clone());
        let mut session_messages: u64 = 0;

        for entry in &entries {
            if let Some(ts) = &entry.timestamp {
                last_entry_ts = Some(ts.clone());
            }

            if entry.entry_type.as_deref() != Some("gemini") {
                continue;
            }
            let Some(id) = entry.id.clone() else {
                continue;
            };
            if !seen_ids.insert(id) {
                continue; // already counted this gemini turn
            }
            let Some(model) = entry.model.clone().filter(|m| !m.is_empty()) else {
                continue;
            };
            let Some(tokens) = &entry.tokens else {
                continue;
            };

            let new_input = tokens.input.saturating_sub(tokens.cached);
            let cache_read = tokens.cached;
            // Reasoning is an assistant-side cost — fold it into output to
            // match how Codex reports it.
            let output = tokens.output + tokens.thoughts + tokens.tool;

            let model_accum = accum.model_usage.entry(model.clone()).or_default();
            model_accum.input_tokens += new_input;
            model_accum.output_tokens += output;
            model_accum.cache_read_tokens += cache_read;

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

        // ── Session stats (only if we recorded any assistant turns) ───────────
        if session_messages > 0 {
            let last_ts = last_entry_ts.as_deref().unwrap_or(&session_start);
            let duration_secs = ts_diff_secs(&session_start, last_ts);

            accum.sessions.push(super::scan::SessionStat {
                duration_secs,
                message_count: session_messages,
                start_timestamp: session_start.clone(),
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

fn read_jsonl_entries(path: &Path) -> anyhow::Result<Vec<RawEntry>> {
    let content = fs::read_to_string(path)?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip mutation sentinels like `{"$set": {...}}` cheaply.
        if line.starts_with("{\"$") {
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

    fn setup_chats_dir(base: &Path, project: &str) -> PathBuf {
        let dir = base.join("tmp").join(project).join("chats");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn header(start: &str) -> String {
        serde_json::json!({
            "sessionId": "abc",
            "projectHash": "xyz",
            "startTime": start,
            "lastUpdated": start,
            "kind": "main"
        })
        .to_string()
    }

    fn gemini_turn(
        id: &str,
        ts: &str,
        model: &str,
        input: u64,
        output: u64,
        cached: u64,
        thoughts: u64,
    ) -> String {
        serde_json::json!({
            "id": id,
            "timestamp": ts,
            "type": "gemini",
            "content": "",
            "tokens": {
                "input": input,
                "output": output,
                "cached": cached,
                "thoughts": thoughts,
                "tool": 0,
                "total": input + output + thoughts,
            },
            "model": model,
        })
        .to_string()
    }

    fn user_turn(id: &str, ts: &str) -> String {
        serde_json::json!({
            "id": id,
            "timestamp": ts,
            "type": "user",
            "content": [{"text": "hi"}]
        })
        .to_string()
    }

    fn set_sentinel(ts: &str) -> String {
        format!(r#"{{"$set":{{"lastUpdated":"{ts}"}}}}"#)
    }

    #[test]
    fn list_session_files_walks_tmp_chats() {
        let dir = tempdir().unwrap();
        let a = setup_chats_dir(dir.path(), "proj-a");
        let b = setup_chats_dir(dir.path(), "proj-b");

        write_jsonl(&a, "session-1.jsonl", &[]);
        write_jsonl(&a, "session-2.jsonl", &[]);
        write_jsonl(&b, "session-3.jsonl", &[]);
        write_jsonl(&b, "notes.txt", &[]);

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
    fn scan_dedupes_repeated_gemini_ids() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        let file = write_jsonl(
            &chats,
            "session-1.jsonl",
            &[
                &header("2026-05-22T10:00:00Z"),
                &user_turn("u1", "2026-05-22T10:00:01Z"),
                &set_sentinel("2026-05-22T10:00:01Z"),
                // Same `id` appears twice — the second after tool calls land.
                &gemini_turn(
                    "g1",
                    "2026-05-22T10:00:02Z",
                    "gemini-3-flash",
                    1000,
                    100,
                    0,
                    50,
                ),
                &set_sentinel("2026-05-22T10:00:02Z"),
                &gemini_turn(
                    "g1",
                    "2026-05-22T10:00:03Z",
                    "gemini-3-flash",
                    1000,
                    100,
                    0,
                    50,
                ),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        let m = accum.model_usage.get("gemini-3-flash").unwrap();
        // Counted exactly once: input 1000 - cached 0 = 1000; output 100 + thoughts 50 = 150
        assert_eq!(m.input_tokens, 1000);
        assert_eq!(m.output_tokens, 150);
        assert_eq!(m.cache_read_tokens, 0);
        assert_eq!(accum.total_messages, 1);
    }

    #[test]
    fn scan_splits_cached_input_and_folds_thoughts_into_output() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        let file = write_jsonl(
            &chats,
            "session-1.jsonl",
            &[
                &header("2026-05-22T10:00:00Z"),
                // input includes cached portion; thoughts are folded into output.
                &gemini_turn(
                    "g1",
                    "2026-05-22T10:00:02Z",
                    "gemini-3-flash",
                    16480,
                    424,
                    7718,
                    297,
                ),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        let m = accum.model_usage.get("gemini-3-flash").unwrap();
        // new_input = 16480 - 7718 = 8762
        assert_eq!(m.input_tokens, 8762);
        // output + thoughts = 424 + 297 = 721
        assert_eq!(m.output_tokens, 721);
        assert_eq!(m.cache_read_tokens, 7718);
        assert_eq!(m.cache_write_tokens, 0);

        let day_tokens = accum.daily_model_tokens.get("2026-05-22").unwrap();
        // daily = new_input + output_with_thoughts = 8762 + 721 = 9483
        assert_eq!(*day_tokens.get("gemini-3-flash").unwrap(), 9483);
    }

    #[test]
    fn scan_filters_by_date_range_on_session_start() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        let old = write_jsonl(
            &chats,
            "session-old.jsonl",
            &[
                &header("2026-04-01T10:00:00Z"),
                &gemini_turn(
                    "g1",
                    "2026-04-01T10:00:02Z",
                    "gemini-3-flash",
                    100,
                    100,
                    0,
                    0,
                ),
            ],
        );
        let new = write_jsonl(
            &chats,
            "session-new.jsonl",
            &[
                &header("2026-05-22T10:00:00Z"),
                &gemini_turn("g2", "2026-05-22T10:00:02Z", "gemini-3-flash", 50, 50, 0, 0),
            ],
        );

        let accum = scan_files(&[old, new], Some("2026-05-01"), None).unwrap();
        let m = accum.model_usage.get("gemini-3-flash").unwrap();
        assert_eq!(m.input_tokens, 50);
        assert_eq!(accum.sessions.len(), 1);
    }

    #[test]
    fn scan_ignores_set_sentinels_and_user_turns() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        let file = write_jsonl(
            &chats,
            "session-1.jsonl",
            &[
                &header("2026-05-22T10:00:00Z"),
                &user_turn("u1", "2026-05-22T10:00:01Z"),
                &set_sentinel("2026-05-22T10:00:01Z"),
                &gemini_turn("g1", "2026-05-22T10:00:02Z", "gemini-3-flash", 10, 5, 0, 0),
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        assert_eq!(accum.total_messages, 1);
        assert_eq!(
            accum
                .model_usage
                .get("gemini-3-flash")
                .unwrap()
                .input_tokens,
            10
        );
    }

    #[test]
    fn scan_skips_gemini_entries_with_no_tokens_or_model() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        let no_model = serde_json::json!({
            "id": "g0",
            "timestamp": "2026-05-22T10:00:01Z",
            "type": "gemini",
            "tokens": {"input": 5, "output": 1}
        })
        .to_string();
        let no_tokens = serde_json::json!({
            "id": "g1",
            "timestamp": "2026-05-22T10:00:02Z",
            "type": "gemini",
            "model": "gemini-3-flash"
        })
        .to_string();
        let good = gemini_turn("g2", "2026-05-22T10:00:03Z", "gemini-3-flash", 10, 5, 0, 0);

        let file = write_jsonl(
            &chats,
            "session-1.jsonl",
            &[
                &header("2026-05-22T10:00:00Z"),
                &no_model,
                &no_tokens,
                &good,
            ],
        );

        let accum = scan_files(&[file], None, None).unwrap();
        assert_eq!(accum.total_messages, 1);
        assert_eq!(
            accum
                .model_usage
                .get("gemini-3-flash")
                .unwrap()
                .input_tokens,
            10
        );
    }
}
