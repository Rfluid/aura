use std::path::PathBuf;

use anyhow::Result;

use super::{
    claude_code::build_snapshot,
    dates::{n_days_ago, today},
    gemini_scan::{list_session_files, scan_files},
    AgentReader, Period, UsageSnapshot,
};

/// Reader for the Gemini CLI (`~/.gemini/`). Walks session files under
/// `tmp/<project>/chats/session-*.jsonl` and attributes per-turn `tokens`
/// deltas to the turn's `model`. Gemini has no on-disk stats cache, so
/// `AllTime` is a full scan.
pub struct GeminiReader {
    /// Path to the Gemini config directory (the folder containing `tmp/`).
    pub config_path: PathBuf,
}

impl GeminiReader {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl AgentReader for GeminiReader {
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

    fn setup_chats_dir(base: &Path, project: &str) -> PathBuf {
        let dir = base.join("tmp").join(project).join("chats");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn alltime_snapshot_aggregates_across_sessions() {
        let dir = tempdir().unwrap();
        let chats_a = setup_chats_dir(dir.path(), "proj-a");
        let chats_b = setup_chats_dir(dir.path(), "proj-b");

        let session_a = vec![
            r#"{"sessionId":"a","startTime":"2026-03-01T10:00:00Z","lastUpdated":"2026-03-01T10:00:00Z","kind":"main"}"#,
            r#"{"id":"g1","timestamp":"2026-03-01T10:00:02Z","type":"gemini","model":"gemini-3-flash","tokens":{"input":200,"output":100,"cached":50,"thoughts":0,"tool":0,"total":300}}"#,
        ];
        let session_b = vec![
            r#"{"sessionId":"b","startTime":"2026-03-01T11:00:00Z","lastUpdated":"2026-03-01T11:00:00Z","kind":"main"}"#,
            r#"{"id":"g2","timestamp":"2026-03-01T11:00:02Z","type":"gemini","model":"gemini-3-flash","tokens":{"input":300,"output":150,"cached":0,"thoughts":0,"tool":0,"total":450}}"#,
        ];

        write_jsonl(&chats_a, "session-a.jsonl", &session_a);
        write_jsonl(&chats_b, "session-b.jsonl", &session_b);

        let reader = GeminiReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();

        // new_input: (200-50) + (300-0) = 450; output: 100 + 150 = 250
        assert_eq!(snap.total_tokens, 700);
        assert_eq!(snap.total_input_tokens, 450);
        assert_eq!(snap.total_output_tokens, 250);
        assert_eq!(snap.total_cache_read_tokens, 50);
        assert_eq!(snap.total_sessions, 2);
        assert_eq!(snap.favorite_model.as_deref(), Some("gemini-3-flash"));
    }

    #[test]
    fn last7days_snapshot_filters_old_sessions() {
        let dir = tempdir().unwrap();
        let chats = setup_chats_dir(dir.path(), "proj");

        // Today's session — falls inside Last7Days
        let today_str = today();
        let session_today = [format!(
            r#"{{"sessionId":"t","startTime":"{0}T10:00:00Z","lastUpdated":"{0}T10:00:00Z","kind":"main"}}
{{"id":"g1","timestamp":"{0}T10:00:02Z","type":"gemini","model":"gemini-3-flash","tokens":{{"input":100,"output":50,"cached":0,"thoughts":0,"tool":0,"total":150}}}}"#,
            today_str
        )];
        let refs: Vec<&str> = session_today.iter().map(String::as_str).collect();
        write_jsonl(&chats, "session-today.jsonl", &refs);

        // Old session — well outside the window
        let session_old = vec![
            r#"{"sessionId":"o","startTime":"2025-01-01T10:00:00Z","lastUpdated":"2025-01-01T10:00:00Z","kind":"main"}"#,
            r#"{"id":"g0","timestamp":"2025-01-01T10:00:02Z","type":"gemini","model":"gemini-3-flash","tokens":{"input":9999,"output":9999,"cached":0,"thoughts":0,"tool":0,"total":19998}}"#,
        ];
        write_jsonl(&chats, "session-old.jsonl", &session_old);

        let reader = GeminiReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::Last7Days).unwrap();
        assert_eq!(snap.total_tokens, 150);
        assert_eq!(snap.total_sessions, 1);
    }

    #[test]
    fn empty_gemini_dir_returns_default_snapshot() {
        let dir = tempdir().unwrap();
        let reader = GeminiReader::new(dir.path().to_path_buf());
        let snap = reader.snapshot(Period::AllTime).unwrap();
        assert_eq!(snap.total_tokens, 0);
        assert_eq!(snap.total_sessions, 0);
        assert!(snap.favorite_model.is_none());
    }
}
