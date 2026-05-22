use std::path::PathBuf;

use anyhow::Result;

use super::{
    claude_code::build_snapshot,
    codex_scan::{list_session_files, scan_files},
    dates::{n_days_ago, today},
    AgentReader, Period, UsageSnapshot,
};

/// Reader for the Codex CLI (`~/.codex/`). Walks rollout files under
/// `sessions/{YYYY}/{MM}/{DD}/` and attributes `token_count` deltas to the
/// most-recent `turn_context.model`. Codex has no on-disk stats cache, so
/// `AllTime` is a full scan.
pub struct CodexReader {
    /// Path to the Codex config directory (the folder containing `sessions/`).
    pub config_path: PathBuf,
}

impl CodexReader {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl AgentReader for CodexReader {
    fn snapshot(&self, period: Period) -> Result<UsageSnapshot> {
        let files = list_session_files(&self.config_path)?;

        let (from, to) = match period {
            Period::Last7Days => (Some(n_days_ago(6)), Some(today())),
            Period::Last30Days => (Some(n_days_ago(29)), Some(today())),
            Period::AllTime => (None, None),
        };

        let accum = scan_files(&files, from.as_deref(), to.as_deref())?;
        Ok(build_snapshot(accum, None))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, path::Path};
    use tempfile::tempdir;

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        path
    }

    fn setup_codex_day(base: &Path, year: &str, month: &str, day: &str) -> PathBuf {
        let dir = base.join("sessions").join(year).join(month).join(day);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn alltime_snapshot_aggregates_across_sessions() {
        let dir = tempdir().unwrap();
        let day = setup_codex_day(dir.path(), "2026", "03", "01");

        let session_a = vec![
            r#"{"timestamp":"2026-03-01T10:00:00Z","type":"session_meta","payload":{"id":"a","timestamp":"2026-03-01T10:00:00Z"}}"#,
            r#"{"timestamp":"2026-03-01T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            r#"{"timestamp":"2026-03-01T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":100}}}}"#,
        ];
        let session_b = vec![
            r#"{"timestamp":"2026-03-01T11:00:00Z","type":"session_meta","payload":{"id":"b","timestamp":"2026-03-01T11:00:00Z"}}"#,
            r#"{"timestamp":"2026-03-01T11:00:01Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            r#"{"timestamp":"2026-03-01T11:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300,"cached_input_tokens":0,"output_tokens":150}}}}"#,
        ];

        write_jsonl(&day, "rollout-a.jsonl", &session_a);
        write_jsonl(&day, "rollout-b.jsonl", &session_b);

        let reader = CodexReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();

        // new_input: (200-50) + (300-0) = 450; output: 100 + 150 = 250
        // total = 450 + 250 = 700
        assert_eq!(snap.total_tokens, 700);
        assert_eq!(snap.total_input_tokens, 450);
        assert_eq!(snap.total_output_tokens, 250);
        assert_eq!(snap.total_cache_read_tokens, 50);
        assert_eq!(snap.total_sessions, 2);
        assert_eq!(snap.favorite_model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn last7days_snapshot_filters_old_sessions() {
        let dir = tempdir().unwrap();

        // Today's session — falls inside Last7Days
        let today_str = today();
        let parts: Vec<&str> = today_str.split('-').collect();
        let today_dir = setup_codex_day(dir.path(), parts[0], parts[1], parts[2]);
        let session_today = [
            format!(
                r#"{{"timestamp":"{0}T10:00:00Z","type":"session_meta","payload":{{"id":"t","timestamp":"{0}T10:00:00Z"}}}}"#,
                today_str
            ),
            format!(
                r#"{{"timestamp":"{0}T10:00:01Z","type":"turn_context","payload":{{"model":"gpt-5.5"}}}}"#,
                today_str
            ),
            format!(
                r#"{{"timestamp":"{0}T10:00:02Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50}}}}}}}}"#,
                today_str
            ),
        ];
        let refs: Vec<&str> = session_today.iter().map(String::as_str).collect();
        write_jsonl(&today_dir, "rollout-today.jsonl", &refs);

        // Old session — well outside the window
        let old_dir = setup_codex_day(dir.path(), "2025", "01", "01");
        let session_old = vec![
            r#"{"timestamp":"2025-01-01T10:00:00Z","type":"session_meta","payload":{"id":"o","timestamp":"2025-01-01T10:00:00Z"}}"#,
            r#"{"timestamp":"2025-01-01T10:00:01Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            r#"{"timestamp":"2025-01-01T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":9999,"cached_input_tokens":0,"output_tokens":9999}}}}"#,
        ];
        write_jsonl(&old_dir, "rollout-old.jsonl", &session_old);

        let reader = CodexReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::Last7Days).unwrap();
        assert_eq!(snap.total_tokens, 150);
        assert_eq!(snap.total_sessions, 1);
    }

    #[test]
    fn empty_codex_dir_returns_default_snapshot() {
        let dir = tempdir().unwrap();
        let reader = CodexReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();
        assert_eq!(snap.total_tokens, 0);
        assert_eq!(snap.total_sessions, 0);
        assert!(snap.favorite_model.is_none());
    }
}
