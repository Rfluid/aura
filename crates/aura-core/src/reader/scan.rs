/// Core JSONL scanning logic — mirrors the `ML8` function from `claude /usage`.
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::Deserialize;

use super::dates::{add_one_day, date_from_timestamp, hour_from_timestamp, today};

// ── JSONL entry types ─────────────────────────────────────────────────────────

/// A single line from a session JSONL file.
#[derive(Deserialize)]
pub struct RawEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: bool,
    pub message: Option<RawMessage>,
    /// Present only on `speculation-accept` entries.
    #[serde(rename = "timeSavedMs", default)]
    pub time_saved_ms: u64,
}

#[derive(Deserialize)]
pub struct RawMessage {
    pub model: Option<String>,
    pub usage: Option<RawUsage>,
}

#[derive(Deserialize)]
pub struct RawUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

// ── Accumulator ───────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct ModelAccum {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug)]
pub struct SessionStat {
    pub duration_secs: u64,
    #[allow(dead_code)] // used in future UI for session detail display
    pub message_count: u64,
    pub start_timestamp: String,
}

/// Raw output of scanning a set of JSONL files.
#[derive(Default, Debug)]
pub struct ScanAccum {
    pub model_usage: HashMap<String, ModelAccum>,
    /// date → model → (input+output) tokens.
    pub daily_model_tokens: HashMap<String, HashMap<String, u64>>,
    /// date → message_count, session_count.
    pub daily_message_counts: HashMap<String, u64>,
    pub daily_session_counts: HashMap<String, u64>,
    pub sessions: Vec<SessionStat>,
    /// hour (0–23) → session start count.
    pub hour_counts: HashMap<u8, u64>,
    pub total_messages: u64,
    pub total_speculation_saved_ms: u64,
}

// ── File discovery ────────────────────────────────────────────────────────────

/// Returns all `*.jsonl` paths under `config_path/projects/`, tagged with
/// whether they live under a `subagents/` subdirectory.
pub fn list_session_files(config_path: &Path) -> anyhow::Result<Vec<(PathBuf, bool)>> {
    let projects_dir = config_path.join("projects");
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for project_entry in fs::read_dir(&projects_dir)? {
        let project_dir = project_entry?.path();
        if !project_dir.is_dir() {
            continue;
        }
        collect_jsonl_files(&project_dir, false, &mut files)?;

        // Check for subagents/ subdir
        let subagents_dir = project_dir.join("subagents");
        if subagents_dir.is_dir() {
            collect_jsonl_files(&subagents_dir, true, &mut files)?;
        }
    }
    Ok(files)
}

fn collect_jsonl_files(
    dir: &Path,
    is_subagent: bool,
    out: &mut Vec<(PathBuf, bool)>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push((path, is_subagent));
        }
    }
    Ok(())
}

// ── Main scanner ──────────────────────────────────────────────────────────────

/// Scan a list of JSONL session files, optionally restricting to entries whose
/// session date falls within `[from_date, to_date]` (inclusive, "YYYY-MM-DD").
///
/// Mirrors the `ML8` function from `claude /usage`.
pub fn scan_files(
    files: &[(PathBuf, bool)],
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> anyhow::Result<ScanAccum> {
    let mut accum = ScanAccum::default();

    for (path, is_subagent) in files {
        // ── Fast mtime check ──────────────────────────────────────────────────
        if let Some(from) = from_date {
            if let Some(mtime) = file_mtime_date(path) {
                if mtime.as_str() < from {
                    continue; // file not touched since before the window
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

        // For non-subagent files, skip sidechain entries; subagent files keep all.
        let relevant: Vec<&RawEntry> = if *is_subagent {
            entries.iter().collect()
        } else {
            entries.iter().filter(|e| !e.is_sidechain).collect()
        };

        if relevant.is_empty() {
            continue;
        }

        // ── Determine session date from first entry ───────────────────────────
        let first = match relevant.iter().find(|e| e.timestamp.is_some()) {
            Some(e) => e,
            None => continue,
        };
        let first_ts = first.timestamp.as_deref().unwrap();
        let session_date = match date_from_timestamp(first_ts) {
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

        // ── Session stats (non-subagent files only) ───────────────────────────
        if !is_subagent {
            let last_ts = relevant
                .iter()
                .rev()
                .find_map(|e| e.timestamp.as_deref())
                .unwrap_or(first_ts);

            let duration_secs = ts_diff_secs(first_ts, last_ts);
            let message_count = relevant.len() as u64;

            accum.sessions.push(SessionStat {
                duration_secs,
                message_count,
                start_timestamp: first_ts.to_string(),
            });

            *accum
                .daily_session_counts
                .entry(session_date.clone())
                .or_insert(0) += 1;

            if let Some(hour) = hour_from_timestamp(first_ts) {
                *accum.hour_counts.entry(hour).or_insert(0) += 1;
            }
        }

        // ── Per-entry accumulation ────────────────────────────────────────────
        for entry in &relevant {
            if entry.entry_type == "speculation-accept" {
                accum.total_speculation_saved_ms += entry.time_saved_ms;
                continue;
            }

            if entry.entry_type != "assistant" {
                continue;
            }

            let Some(ref message) = entry.message else {
                continue;
            };

            let model = match message.model.as_deref() {
                Some(m) if m != "<synthetic>" && !m.is_empty() => m.to_string(),
                _ => continue,
            };

            accum.total_messages += 1;
            *accum
                .daily_message_counts
                .entry(session_date.clone())
                .or_insert(0) += 1;

            let Some(ref usage) = message.usage else {
                continue;
            };

            let model_accum = accum.model_usage.entry(model.clone()).or_default();
            model_accum.input_tokens += usage.input_tokens;
            model_accum.output_tokens += usage.output_tokens;
            model_accum.cache_read_tokens += usage.cache_read_input_tokens;
            model_accum.cache_write_tokens += usage.cache_creation_input_tokens;

            // dailyModelTokens: input+output only (matches /usage)
            let day_model = accum
                .daily_model_tokens
                .entry(session_date.clone())
                .or_default();
            *day_model.entry(model).or_insert(0) += usage.input_tokens + usage.output_tokens;
        }
    }

    Ok(accum)
}

// ── Streak computation ────────────────────────────────────────────────────────

/// Compute current and longest streaks from a set of active dates.
/// Mirrors the `WM4` function from `claude /usage`.
pub fn compute_streaks(active_dates: &[String]) -> (u32, u32) {
    if active_dates.is_empty() {
        return (0, 0);
    }

    // Current streak: walk backwards from today.
    let today_str = today();
    let date_set: std::collections::HashSet<&str> =
        active_dates.iter().map(String::as_str).collect();

    let mut current = 0u32;
    let mut cursor = today_str.clone();
    loop {
        if date_set.contains(cursor.as_str()) {
            current += 1;
            match subtract_one_day(&cursor) {
                Some(prev) => cursor = prev,
                None => break,
            }
        } else {
            break;
        }
    }

    // Longest streak: scan the sorted dates.
    let mut sorted: Vec<&str> = active_dates.iter().map(String::as_str).collect();
    sorted.sort_unstable();

    let mut longest = 0u32;
    let mut run = 1u32;
    for i in 1..sorted.len() {
        let expected = add_one_day(sorted[i - 1]);
        if expected.as_deref() == Some(sorted[i]) {
            run += 1;
        } else {
            longest = longest.max(run);
            run = 1;
        }
    }
    longest = longest.max(run);

    (current, longest)
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

fn subtract_one_day(date: &str) -> Option<String> {
    use chrono::{Duration, NaiveDate};
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some((d - Duration::days(1)).format("%Y-%m-%d").to_string())
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

    fn session_entry(
        ts: &str,
        model: &str,
        input: u64,
        output: u64,
        cache_r: u64,
        cache_w: u64,
    ) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": ts,
            "isSidechain": false,
            "message": {
                "model": model,
                "usage": {
                    "input_tokens": input,
                    "output_tokens": output,
                    "cache_read_input_tokens": cache_r,
                    "cache_creation_input_tokens": cache_w,
                }
            }
        })
        .to_string()
    }

    fn user_entry(ts: &str) -> String {
        serde_json::json!({ "type": "user", "timestamp": ts }).to_string()
    }

    #[test]
    fn scan_accumulates_tokens_per_model() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj1");
        fs::create_dir_all(&project).unwrap();

        let file = write_jsonl(
            &project,
            "session1.jsonl",
            &[
                &user_entry("2026-05-10T10:00:00Z"),
                &session_entry("2026-05-10T10:01:00Z", "claude-opus-4-7", 100, 200, 50, 30),
                &session_entry("2026-05-10T10:02:00Z", "claude-opus-4-7", 80, 120, 10, 5),
            ],
        );

        let accum = scan_files(&[(file, false)], None, None).unwrap();

        let m = accum.model_usage.get("claude-opus-4-7").unwrap();
        assert_eq!(m.input_tokens, 180);
        assert_eq!(m.output_tokens, 320);
        assert_eq!(m.cache_read_tokens, 60);
        assert_eq!(m.cache_write_tokens, 35);
    }

    #[test]
    fn scan_skips_synthetic_model() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj1");
        fs::create_dir_all(&project).unwrap();

        let file = write_jsonl(
            &project,
            "s.jsonl",
            &[
                &user_entry("2026-05-10T10:00:00Z"),
                &session_entry("2026-05-10T10:01:00Z", "<synthetic>", 999, 999, 0, 0),
                &session_entry("2026-05-10T10:02:00Z", "claude-opus-4-7", 10, 20, 0, 0),
            ],
        );

        let accum = scan_files(&[(file, false)], None, None).unwrap();
        assert!(!accum.model_usage.contains_key("<synthetic>"));
        assert_eq!(
            accum
                .model_usage
                .get("claude-opus-4-7")
                .unwrap()
                .input_tokens,
            10
        );
    }

    #[test]
    fn scan_skips_sidechain_entries_in_non_subagent_files() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj1");
        fs::create_dir_all(&project).unwrap();

        let sidechain = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-05-10T10:01:00Z",
            "isSidechain": true,
            "message": {
                "model": "claude-opus-4-7",
                "usage": { "input_tokens": 500, "output_tokens": 500 }
            }
        })
        .to_string();

        let normal = session_entry("2026-05-10T10:02:00Z", "claude-opus-4-7", 10, 20, 0, 0);

        let file = write_jsonl(
            &project,
            "s.jsonl",
            &[&user_entry("2026-05-10T10:00:00Z"), &sidechain, &normal],
        );

        let accum = scan_files(&[(file, false)], None, None).unwrap();
        let m = accum.model_usage.get("claude-opus-4-7").unwrap();
        assert_eq!(m.input_tokens, 10); // sidechain not counted
    }

    #[test]
    fn scan_daily_model_tokens_uses_input_plus_output_only() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj1");
        fs::create_dir_all(&project).unwrap();

        let file = write_jsonl(
            &project,
            "s.jsonl",
            &[
                &user_entry("2026-05-10T10:00:00Z"),
                &session_entry("2026-05-10T10:01:00Z", "model-a", 100, 200, 999, 999),
            ],
        );

        let accum = scan_files(&[(file, false)], None, None).unwrap();
        let day = accum.daily_model_tokens.get("2026-05-10").unwrap();
        assert_eq!(*day.get("model-a").unwrap(), 300); // 100+200, no cache
    }

    #[test]
    fn scan_filters_by_date_range() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj1");
        fs::create_dir_all(&project).unwrap();

        // Two separate session files on different days
        let f1 = write_jsonl(
            &project,
            "old.jsonl",
            &[
                &user_entry("2026-04-01T10:00:00Z"),
                &session_entry("2026-04-01T10:01:00Z", "model-a", 100, 100, 0, 0),
            ],
        );
        let f2 = write_jsonl(
            &project,
            "new.jsonl",
            &[
                &user_entry("2026-05-10T10:00:00Z"),
                &session_entry("2026-05-10T10:01:00Z", "model-a", 50, 50, 0, 0),
            ],
        );

        let accum = scan_files(&[(f1, false), (f2, false)], Some("2026-05-01"), None).unwrap();
        let m = accum.model_usage.get("model-a").unwrap();
        assert_eq!(m.input_tokens, 50); // only the May file
    }

    #[test]
    fn compute_streaks_basic() {
        // Dates form one unbroken run
        let dates: Vec<String> = vec![
            "2026-05-18".to_string(),
            "2026-05-19".to_string(),
            "2026-05-20".to_string(),
        ];
        let (_current, longest) = compute_streaks(&dates);
        assert_eq!(longest, 3);
    }

    #[test]
    fn compute_streaks_with_gap() {
        let dates: Vec<String> = vec![
            "2026-05-10".to_string(),
            "2026-05-11".to_string(),
            // gap
            "2026-05-15".to_string(),
            "2026-05-16".to_string(),
            "2026-05-17".to_string(),
        ];
        let (_current, longest) = compute_streaks(&dates);
        assert_eq!(longest, 3);
    }

    #[test]
    fn list_session_files_finds_subagents() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("projects").join("my-proj");
        let subagents = project.join("subagents");
        fs::create_dir_all(&subagents).unwrap();

        write_jsonl(&project, "main.jsonl", &[]);
        write_jsonl(&subagents, "agent.jsonl", &[]);

        let files = list_session_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        let main_file = files
            .iter()
            .find(|(p, _)| p.ends_with("main.jsonl"))
            .unwrap();
        let sub_file = files
            .iter()
            .find(|(p, _)| p.ends_with("agent.jsonl"))
            .unwrap();
        assert!(!main_file.1); // not a subagent
        assert!(sub_file.1); // is a subagent
    }
}
