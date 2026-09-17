//! Reader for Google's Antigravity CLI (`agy`, `~/.gemini/antigravity-cli/`).
//!
//! Activity only. Antigravity keeps its trajectories in
//! `conversations/<uuid>.db` as schema-less protobuf blobs with no plaintext
//! model names and no field names, so per-model attribution and token counts
//! are not recoverable without guessing undocumented field numbers. What *is*
//! readable is `conversation_summaries.db`: one row per conversation, with a
//! step count and timestamps. That gives sessions, messages, active days,
//! streaks and peak hour; [`UsageSnapshot::tokens_unreported`] tells the UI to
//! show "not reported" rather than a misleading `0` for the rest.
//!
//! The DB covers both the CLI and the Antigravity IDE — they share one account
//! and one summaries table, distinguished by `app_data_dir` (`antigravity-cli`
//! vs `antigravity`). Every row counts: the alternative scopes a user's whole
//! Antigravity history down to whatever they have typed into `agy` since
//! installing it. (Reading the IDE's *trajectories* — the flat-protobuf
//! `~/.gemini/antigravity/conversations/<uuid>.pb` files — remains out of
//! scope; these are its summary rows, which `agy` itself writes and reads.)

use std::path::{Path, PathBuf};

use anyhow::Result;
use diesel::prelude::*;

use crate::config::AgentKind;

use super::{
    claude_code::build_snapshot,
    dates::{date_from_timestamp, hour_from_timestamp, n_days_ago, today},
    scan::{ScanAccum, SessionStat},
    AgentReader, Period, UsageSnapshot,
};

/// The summaries DB, relative to the agent's config dir.
const SUMMARIES_DB: &str = "conversation_summaries.db";

/// `agy` writes Go's zero `time.Time` when a conversation has no recorded user
/// input — every row created before it started tracking that column, and every
/// row imported from the IDE. Detected by the year rather than the full string
/// so a differently-formatted zero value is still caught.
const ZERO_TIME_PREFIX: &str = "0001-01-01";

// ── Schema ───────────────────────────────────────────────────────────────────

diesel::table! {
    /// Mirrors what `agy` 1.2.4's gorm model creates. Only the columns Aura
    /// reads are declared — the table also carries `title`, `preview`,
    /// `workspace_uris`, `raw_summary` and a dozen more we have no use for.
    ///
    /// Both timestamps are `Text`, not `Timestamp`: gorm writes them as
    /// `2026-09-16 22:57:06.976586196+00:00`, and diesel's chrono
    /// deserializer expects `%Y-%m-%d %H:%M:%S%.f` with no UTC offset. They
    /// are normalised in [`to_rfc3339`] instead.
    conversation_summaries (conversation_id) {
        conversation_id -> Text,
        step_count -> BigInt,
        last_modified_time -> Text,
        last_user_input_time -> Text,
        app_data_dir -> Text,
    }
}

#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = conversation_summaries)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
struct SummaryRow {
    #[allow(dead_code)] // selected as the primary key; not used in the rollup
    conversation_id: String,
    /// Turns in the conversation — what Aura counts as messages.
    step_count: i64,
    last_modified_time: String,
    last_user_input_time: String,
    #[allow(dead_code)] // `antigravity-cli` vs `antigravity`; every row counts
    app_data_dir: String,
}

impl SummaryRow {
    /// When this conversation was last touched, as RFC 3339.
    ///
    /// Prefers the last user input, which is what "a session happened" means
    /// to the rest of Aura, and falls back to the last modification for the
    /// rows that carry Go's zero time.
    fn activity_timestamp(&self) -> Option<String> {
        if !self.last_user_input_time.starts_with(ZERO_TIME_PREFIX) {
            if let Some(ts) = to_rfc3339(&self.last_user_input_time) {
                return Some(ts);
            }
        }
        to_rfc3339(&self.last_modified_time)
    }
}

/// Turn gorm's `2026-09-16 22:57:06.976586196+00:00` into the RFC 3339 form
/// `dates::hour_from_timestamp` can parse. A value already using `T` passes
/// through untouched.
fn to_rfc3339(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(match raw.find(' ') {
        Some(i) => format!("{}T{}", &raw[..i], &raw[i + 1..]),
        None => raw.to_string(),
    })
}

// ── Connection ───────────────────────────────────────────────────────────────

/// Build a SQLite URI for `path` with the given query string.
///
/// Verified against SQLite's own `sqlite3ParseUri`: while parsing the filename
/// portion it treats exactly three bytes specially — `%` (escape), `?` (start
/// of query) and `#` (end of URI). `&` and `=` are literal there, so they need
/// no encoding. Windows separators are flipped to `/`; the parser has no
/// drive-letter special case, so `file:C:/dir/x.db` yields the filename
/// `C:/dir/x.db` unchanged, which is what the Windows VFS wants.
///
/// The one trap is that the authority check (`zUri[5]=='/' && zUri[6]=='/'`)
/// runs on the *raw* string, before any percent-decoding. A UNC path such as
/// `\\server\share\x.db` normalises to `//server/share/x.db` and would be
/// rejected as `invalid uri authority: server`. Encoding the second slash
/// sidesteps that check and decodes back to the same path.
fn sqlite_uri(path: &Path, query: &str) -> String {
    let mut encoded = String::new();
    for ch in path.to_string_lossy().chars() {
        match ch {
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            '%' => encoded.push_str("%25"),
            '\\' if cfg!(windows) => encoded.push('/'),
            other => encoded.push(other),
        }
    }
    if let Some(rest) = encoded.strip_prefix("//") {
        return format!("file:/%2f{rest}?{query}");
    }
    format!("file:{encoded}?{query}")
}

/// Open the summaries DB without disturbing a running `agy`.
///
/// `mode=ro` is tried first. The DB is in WAL mode, so a read-only connection
/// still consults the `-wal` file and therefore sees commits `agy` has made
/// but not yet checkpointed. That needs the `-shm` file, which SQLite creates
/// in the same directory; when it cannot (a read-only directory, a filesystem
/// with no shared-memory support), the open fails and `immutable=1` is tried
/// instead. That second form reads the main DB file alone: never blocked,
/// never blocking, but blind to anything still sitting in the WAL — a
/// deliberate last resort rather than the default.
fn connect(db_path: &Path) -> Result<SqliteConnection> {
    let ro = sqlite_uri(db_path, "mode=ro");
    match SqliteConnection::establish(&ro) {
        Ok(conn) => Ok(conn),
        Err(first) => {
            let immutable = sqlite_uri(db_path, "mode=ro&immutable=1");
            SqliteConnection::establish(&immutable).map_err(|second| {
                anyhow::anyhow!(
                    "open {} read-only: {first}; retry with immutable=1: {second}",
                    db_path.display()
                )
            })
        }
    }
}

// ── AntigravityReader ────────────────────────────────────────────────────────

pub struct AntigravityReader {
    /// Path to the Antigravity CLI data directory — the folder holding
    /// `conversation_summaries.db`.
    pub config_path: PathBuf,
}

impl AntigravityReader {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }

    fn db_path(&self) -> PathBuf {
        self.config_path.join(SUMMARIES_DB)
    }

    fn load_rows(&self) -> Result<Vec<SummaryRow>> {
        use self::conversation_summaries::dsl::conversation_summaries as summaries;

        let mut conn = connect(&self.db_path())?;
        Ok(summaries.select(SummaryRow::as_select()).load(&mut conn)?)
    }
}

impl AgentReader for AntigravityReader {
    fn snapshot(&self, period: Period) -> Result<UsageSnapshot> {
        // No DB yet means `agy` has never run here. An empty snapshot is the
        // honest answer, and matches how the other readers treat a missing
        // sessions directory.
        if !self.db_path().is_file() {
            return Ok(UsageSnapshot {
                tokens_unreported: !AgentKind::Antigravity.reports_tokens(),
                ..Default::default()
            });
        }

        let (from, to) = match period {
            Period::Last7Days => (Some(n_days_ago(6)), Some(today())),
            Period::Last30Days => (Some(n_days_ago(29)), Some(today())),
            Period::AllTime => (None, None),
        };

        Ok(build_activity_snapshot(
            self.load_rows()?,
            from.as_deref(),
            to.as_deref(),
        ))
    }
}

/// Roll a set of summary rows up into a snapshot, keeping only rows whose
/// activity date falls inside `[from, to]` (inclusive, "YYYY-MM-DD").
fn build_activity_snapshot(
    rows: Vec<SummaryRow>,
    from: Option<&str>,
    to: Option<&str>,
) -> UsageSnapshot {
    let mut accum = ScanAccum::default();

    for row in rows {
        let Some(ts) = row.activity_timestamp() else {
            continue;
        };
        let Some(date) = date_from_timestamp(&ts) else {
            continue;
        };
        if from.is_some_and(|f| date.as_str() < f) || to.is_some_and(|t| date.as_str() > t) {
            continue;
        }

        let messages = row.step_count.max(0) as u64;
        accum.total_messages += messages;
        *accum.daily_message_counts.entry(date.clone()).or_insert(0) += messages;
        *accum.daily_session_counts.entry(date).or_insert(0) += 1;
        if let Some(hour) = hour_from_timestamp(&ts) {
            *accum.hour_counts.entry(hour).or_insert(0) += 1;
        }
        accum.sessions.push(SessionStat {
            // Summaries record when a conversation was last touched, not how
            // long it ran — see `longest_session_secs` below.
            duration_secs: 0,
            message_count: messages,
            start_timestamp: ts,
        });
    }

    let mut snap = build_snapshot(accum, None);
    // `build_snapshot` reads this off the max session duration, which is
    // uniformly 0 here. Antigravity doesn't publish session durations, and
    // "0s" reads as a measurement rather than an absence.
    snap.longest_session_secs = None;
    snap.tokens_unreported = !AgentKind::Antigravity.reports_tokens();
    snap
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// The exact `CREATE TABLE` gorm emits in `agy` 1.2.4, so the diesel
    /// schema above is checked against the real column set and types.
    const CREATE: &str = r#"CREATE TABLE `conversation_summaries` (
        `conversation_id` text,
        `title` text NOT NULL DEFAULT "",
        `preview` text NOT NULL DEFAULT "",
        `step_count` integer NOT NULL DEFAULT 0,
        `last_modified_time` datetime NOT NULL,
        `workspace_uris` text NOT NULL,
        `status` text NOT NULL DEFAULT "",
        `source` text NOT NULL DEFAULT "",
        `project_id` text NOT NULL DEFAULT "",
        `agent_name` text NOT NULL DEFAULT "",
        `parent_conversation_id` text NOT NULL DEFAULT "",
        `nesting_depth` integer NOT NULL DEFAULT 0,
        `battle_id` text NOT NULL DEFAULT "",
        `winning_conversation_id` text NOT NULL DEFAULT "",
        `not_fully_idle` numeric NOT NULL DEFAULT false,
        `killed` numeric NOT NULL DEFAULT false,
        `last_user_input_time` datetime NOT NULL,
        `last_user_input_step_index` integer NOT NULL DEFAULT -1,
        `app_data_dir` text NOT NULL DEFAULT "",
        `raw_summary` blob,
        `group_id` text NOT NULL DEFAULT "",
        PRIMARY KEY (`conversation_id`))"#;

    /// Build a summaries DB at `dir/conversation_summaries.db`.
    /// Each row is `(id, step_count, last_modified, last_user_input, app_data_dir)`.
    fn seed(dir: &Path, rows: &[(&str, i64, &str, &str, &str)]) -> PathBuf {
        let path = dir.join(SUMMARIES_DB);
        let mut conn = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        diesel::sql_query(CREATE).execute(&mut conn).unwrap();
        for (id, steps, modified, input, app) in rows {
            diesel::sql_query(
                "INSERT INTO conversation_summaries \
                 (conversation_id, title, preview, step_count, last_modified_time, \
                  workspace_uris, last_user_input_time, app_data_dir) \
                 VALUES (?, '', '', ?, ?, '', ?, ?)",
            )
            .bind::<diesel::sql_types::Text, _>(*id)
            .bind::<diesel::sql_types::BigInt, _>(*steps)
            .bind::<diesel::sql_types::Text, _>(*modified)
            .bind::<diesel::sql_types::Text, _>(*input)
            .bind::<diesel::sql_types::Text, _>(*app)
            .execute(&mut conn)
            .unwrap();
        }
        path
    }

    /// gorm's wire format for a UTC instant.
    fn gorm(day: &str, time: &str) -> String {
        format!("{day} {time}.123456789+00:00")
    }

    const ZERO: &str = "0001-01-01 00:00:00+00:00";

    #[test]
    fn reads_sessions_and_messages_from_the_summaries_db() {
        let dir = tempdir().unwrap();
        seed(
            dir.path(),
            &[
                (
                    "a",
                    2,
                    &gorm("2026-03-01", "10:00:00"),
                    &gorm("2026-03-01", "10:00:00"),
                    "antigravity-cli",
                ),
                (
                    "b",
                    137,
                    &gorm("2026-03-02", "11:00:00"),
                    &gorm("2026-03-02", "11:00:00"),
                    "antigravity-cli",
                ),
            ],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        assert_eq!(snap.total_sessions, 2);
        assert_eq!(snap.total_messages, 139);
        assert_eq!(snap.active_days, 2);
        assert_eq!(snap.first_session_date.as_deref(), Some("2026-03-01"));
        assert_eq!(snap.last_session_date.as_deref(), Some("2026-03-02"));
        assert_eq!(snap.total_days, 2);
    }

    #[test]
    fn token_fields_stay_empty_and_are_flagged_unreported() {
        let dir = tempdir().unwrap();
        seed(
            dir.path(),
            &[(
                "a",
                9,
                &gorm("2026-03-01", "10:00:00"),
                &gorm("2026-03-01", "10:00:00"),
                "antigravity-cli",
            )],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        assert!(
            snap.tokens_unreported,
            "the UI must distinguish this from a genuine zero"
        );
        assert_eq!(snap.total_tokens, 0);
        assert!(snap.per_model.is_empty());
        assert!(snap.favorite_model.is_none());
        assert!(snap.daily_tokens.is_empty());
        // No session durations in the summaries — not a zero-length session.
        assert!(snap.longest_session_secs.is_none());
    }

    #[test]
    fn zero_user_input_time_falls_back_to_last_modified() {
        let dir = tempdir().unwrap();
        // The shape every IDE-sourced row has: a real `last_modified_time`
        // and Go's zero `time.Time` for `last_user_input_time`.
        seed(
            dir.path(),
            &[(
                "ide",
                137,
                &gorm("2026-03-29", "01:47:15"),
                ZERO,
                "antigravity",
            )],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        assert_eq!(snap.total_sessions, 1);
        assert_eq!(snap.first_session_date.as_deref(), Some("2026-03-29"));
    }

    #[test]
    fn ide_rows_are_counted_alongside_cli_rows() {
        let dir = tempdir().unwrap();
        seed(
            dir.path(),
            &[
                (
                    "cli",
                    2,
                    &gorm("2026-03-02", "10:00:00"),
                    &gorm("2026-03-02", "10:00:00"),
                    "antigravity-cli",
                ),
                (
                    "ide",
                    40,
                    &gorm("2026-03-01", "10:00:00"),
                    ZERO,
                    "antigravity",
                ),
            ],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        assert_eq!(snap.total_sessions, 2);
        assert_eq!(snap.total_messages, 42);
    }

    #[test]
    fn period_filtering_drops_rows_outside_the_window() {
        let dir = tempdir().unwrap();
        let recent = n_days_ago(1);
        let stale = "2025-01-01";
        seed(
            dir.path(),
            &[
                (
                    "recent",
                    3,
                    &gorm(&recent, "10:00:00"),
                    &gorm(&recent, "10:00:00"),
                    "antigravity-cli",
                ),
                (
                    "stale",
                    999,
                    &gorm(stale, "10:00:00"),
                    &gorm(stale, "10:00:00"),
                    "antigravity-cli",
                ),
            ],
        );

        let reader = AntigravityReader::new(dir.path().to_path_buf());

        let last7 = reader.snapshot(Period::Last7Days).unwrap();
        assert_eq!(last7.total_sessions, 1);
        assert_eq!(last7.total_messages, 3);

        let all = reader.snapshot(Period::AllTime).unwrap();
        assert_eq!(all.total_sessions, 2);
        assert_eq!(all.total_messages, 1002);
    }

    #[test]
    fn streaks_and_peak_hour_are_derived_from_activity() {
        let dir = tempdir().unwrap();
        // Three consecutive days, two of them starting in the same hour.
        let d0 = n_days_ago(2);
        let d1 = n_days_ago(1);
        let d2 = n_days_ago(0);
        seed(
            dir.path(),
            &[
                (
                    "a",
                    1,
                    &gorm(&d0, "14:00:00"),
                    &gorm(&d0, "14:00:00"),
                    "antigravity-cli",
                ),
                (
                    "b",
                    1,
                    &gorm(&d1, "14:30:00"),
                    &gorm(&d1, "14:30:00"),
                    "antigravity-cli",
                ),
                (
                    "c",
                    1,
                    &gorm(&d2, "09:00:00"),
                    &gorm(&d2, "09:00:00"),
                    "antigravity-cli",
                ),
            ],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        assert_eq!(snap.streaks.current, 3);
        assert_eq!(snap.streaks.longest, 3);
        assert_eq!(snap.active_days, 3);
        // Two 14:00 UTC starts vs one at 09:00 — the peak, whatever local
        // hour that lands on for the machine running the test.
        let expected = hour_from_timestamp(&to_rfc3339(&gorm(&d0, "14:00:00")).unwrap());
        assert_eq!(snap.peak_hour, expected);
    }

    #[test]
    fn daily_activity_is_sorted_and_aggregated_per_day() {
        let dir = tempdir().unwrap();
        seed(
            dir.path(),
            &[
                (
                    "b",
                    5,
                    &gorm("2026-03-02", "09:00:00"),
                    &gorm("2026-03-02", "09:00:00"),
                    "antigravity-cli",
                ),
                (
                    "a1",
                    3,
                    &gorm("2026-03-01", "09:00:00"),
                    &gorm("2026-03-01", "09:00:00"),
                    "antigravity-cli",
                ),
                (
                    "a2",
                    4,
                    &gorm("2026-03-01", "18:00:00"),
                    &gorm("2026-03-01", "18:00:00"),
                    "antigravity-cli",
                ),
            ],
        );

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        let days: Vec<(&str, u64, u64)> = snap
            .daily_activity
            .iter()
            .map(|d| (d.date.as_str(), d.session_count, d.message_count))
            .collect();
        assert_eq!(days, [("2026-03-01", 2, 7), ("2026-03-02", 1, 5)]);
    }

    #[test]
    fn the_snapshot_flag_tracks_the_agent_kind_capability() {
        // Two spellings of one fact: the tab row reads the kind (available
        // before any read), the renderers read the snapshot. They must agree.
        let dir = tempdir().unwrap();
        seed(
            dir.path(),
            &[(
                "a",
                1,
                &gorm("2026-03-01", "10:00:00"),
                &gorm("2026-03-01", "10:00:00"),
                "antigravity-cli",
            )],
        );
        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();
        assert_eq!(
            snap.tokens_unreported,
            !AgentKind::Antigravity.reports_tokens()
        );
    }

    #[test]
    fn missing_database_is_an_empty_snapshot_not_an_error() {
        let dir = tempdir().unwrap();
        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();
        assert_eq!(snap.total_sessions, 0);
        assert_eq!(snap.total_messages, 0);
        assert!(snap.tokens_unreported);
    }

    #[test]
    fn empty_database_reads_clean() {
        let dir = tempdir().unwrap();
        seed(dir.path(), &[]);
        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();
        assert_eq!(snap.total_sessions, 0);
        assert!(snap.first_session_date.is_none());
    }

    #[test]
    fn opening_read_only_does_not_create_a_database() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(SUMMARIES_DB);
        assert!(connect(&path).is_err());
        assert!(
            !path.exists(),
            "mode=ro must never create the file the way a default open would"
        );
    }

    #[test]
    fn reads_a_database_while_another_connection_holds_it_open() {
        let dir = tempdir().unwrap();
        let path = seed(
            dir.path(),
            &[(
                "a",
                7,
                &gorm("2026-03-01", "10:00:00"),
                &gorm("2026-03-01", "10:00:00"),
                "antigravity-cli",
            )],
        );

        // Stand in for a running `agy`: a live WAL-mode writer connection.
        let mut writer = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        diesel::sql_query("PRAGMA journal_mode=WAL")
            .execute(&mut writer)
            .unwrap();
        diesel::sql_query(
            "INSERT INTO conversation_summaries \
             (conversation_id, title, preview, step_count, last_modified_time, \
              workspace_uris, last_user_input_time, app_data_dir) \
             VALUES ('b', '', '', 11, '2026-03-02 10:00:00+00:00', '', \
                     '2026-03-02 10:00:00+00:00', 'antigravity-cli')",
        )
        .execute(&mut writer)
        .unwrap();

        let snap = AntigravityReader::new(dir.path().to_path_buf())
            .snapshot(Period::AllTime)
            .unwrap();

        // `mode=ro` consults the WAL, so the uncheckpointed row is visible.
        assert_eq!(snap.total_sessions, 2);
        assert_eq!(snap.total_messages, 18);
    }

    #[test]
    fn gorm_timestamps_are_normalised_to_rfc3339() {
        assert_eq!(
            to_rfc3339("2026-09-16 22:57:06.976586196+00:00").as_deref(),
            Some("2026-09-16T22:57:06.976586196+00:00")
        );
        // Already RFC 3339 — unchanged.
        assert_eq!(
            to_rfc3339("2026-09-16T22:57:06Z").as_deref(),
            Some("2026-09-16T22:57:06Z")
        );
        assert_eq!(to_rfc3339("  "), None);
    }

    #[test]
    fn sqlite_uri_escapes_the_characters_sqlite_reads_as_syntax() {
        let uri = sqlite_uri(Path::new("/tmp/a?b#c%d/x.db"), "mode=ro");
        assert_eq!(uri, "file:/tmp/a%3fb%23c%25d/x.db?mode=ro");
    }

    #[test]
    fn sqlite_uri_does_not_let_a_unc_path_parse_as_an_authority() {
        // `//server/...` after separator normalisation would otherwise hit
        // SQLite's authority check and fail with "invalid uri authority".
        let uri = sqlite_uri(Path::new("//server/share/x.db"), "mode=ro");
        assert_eq!(uri, "file:/%2fserver/share/x.db?mode=ro");
        // Still only two leading slashes once SQLite decodes it.
        assert!(!uri.starts_with("file://"));
    }

    #[test]
    fn sqlite_uri_leaves_ampersand_and_equals_alone() {
        // Literal in the filename state of SQLite's parser — encoding them
        // would corrupt the path rather than protect it.
        let uri = sqlite_uri(Path::new("/tmp/a&b=c/x.db"), "mode=ro");
        assert_eq!(uri, "file:/tmp/a&b=c/x.db?mode=ro");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_with_an_ampersand_still_opens() {
        let dir = tempdir().unwrap();
        let odd = dir.path().join("a&b=c");
        std::fs::create_dir_all(&odd).unwrap();
        seed(
            &odd,
            &[(
                "a",
                6,
                &gorm("2026-03-01", "10:00:00"),
                &gorm("2026-03-01", "10:00:00"),
                "antigravity-cli",
            )],
        );
        let snap = AntigravityReader::new(odd)
            .snapshot(Period::AllTime)
            .unwrap();
        assert_eq!(snap.total_messages, 6);
    }

    #[test]
    fn a_path_with_a_question_mark_still_opens() {
        let dir = tempdir().unwrap();
        let odd = dir.path().join("we?rd#dir");
        std::fs::create_dir_all(&odd).unwrap();
        seed(
            &odd,
            &[(
                "a",
                4,
                &gorm("2026-03-01", "10:00:00"),
                &gorm("2026-03-01", "10:00:00"),
                "antigravity-cli",
            )],
        );

        let snap = AntigravityReader::new(odd)
            .snapshot(Period::AllTime)
            .unwrap();
        assert_eq!(snap.total_sessions, 1);
        assert_eq!(snap.total_messages, 4);
    }
}
