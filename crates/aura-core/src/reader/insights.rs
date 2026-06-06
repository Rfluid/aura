//! Insights: pure aggregations over a single [`ScanAccum`] pass that answer the
//! "interesting" questions a heavy user asks — which project burned the most
//! tokens, which single session was the most expensive, whether `ultracode`
//! runs are actually heavier, and how much context is served from cache.
//!
//! Everything here is derived from data the existing JSONL scan already
//! collects (`tokens_by_project`, `sessions`); no extra file I/O happens. The
//! results hang off [`UsageSnapshot::insights`](super::UsageSnapshot), so the
//! All / 7d / 30d period plumbing in the UI is reused unchanged.

use serde::Serialize;

use super::scan::{ScanAccum, SessionStat};

// ── Output types ────────────────────────────────────────────────────────────

/// One project's total token spend, for the "top projects" table.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectStat {
    /// Display name — `basename(cwd)` (e.g. `reconhecimento`), or the trimmed
    /// slug when the log carried no `cwd`.
    pub name: String,
    /// Raw slugified project-dir as stored on disk (`-Users-pedro-…`).
    pub dir: String,
    /// Full working-directory path when known (`/Users/pedro/Downloads/aura`),
    /// for a tooltip. `None` on older logs that predate the `cwd` field.
    pub path: Option<String>,
    /// `input + output` tokens across every session under this project.
    pub tokens: u64,
}

/// A single session in the "top sessions" table, plus its inferred mode.
///
/// Tier (`opus` / `sonnet` / `haiku` / other) is exact, taken from the
/// dominant model. `is_ultracode` is a heuristic — see
/// [`ULTRACODE_MARKERS`](super::scan::ULTRACODE_MARKERS).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionInsight {
    pub session_id: String,
    /// Display name of the project this session ran under — `basename(cwd)`, or
    /// the trimmed slug when no `cwd` was logged.
    pub project: String,
    pub tokens: u64,
    /// Full dominant-model id (e.g. `claude-opus-4-7`), or `None`.
    pub dominant_model: Option<String>,
    /// Coarse model tier derived from `dominant_model`.
    pub tier: ModelTier,
    /// Heuristic high-effort / `ultracode` flag.
    pub is_ultracode: bool,
}

/// Coarse Claude model tier, derived from a model id by substring match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Opus,
    Sonnet,
    Haiku,
    /// Anything that doesn't match a known tier (or no dominant model).
    Other,
}

impl Default for ModelTier {
    fn default() -> Self {
        Self::Other
    }
}

impl ModelTier {
    /// Map a model id to its tier by case-insensitive substring. Returns
    /// [`ModelTier::Other`] for unknown or `None` model ids.
    pub fn from_model(model: Option<&str>) -> Self {
        let Some(m) = model else {
            return Self::Other;
        };
        let m = m.to_ascii_lowercase();
        if m.contains("opus") {
            Self::Opus
        } else if m.contains("sonnet") {
            Self::Sonnet
        } else if m.contains("haiku") {
            Self::Haiku
        } else {
            Self::Other
        }
    }

    /// Short lowercase label for the mode badge (`opus`, `sonnet`, …).
    pub fn label(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Haiku => "haiku",
            Self::Other => "other",
        }
    }
}

/// Whether `ultracode` sessions are actually heavier than normal ones, by
/// average token spend. `ultracode` detection is a **heuristic** — see
/// [`ULTRACODE_MARKERS`](super::scan::ULTRACODE_MARKERS).
#[derive(Debug, Clone, Default, Serialize)]
pub struct UltracodeRoi {
    /// Number of sessions flagged `ultracode`.
    pub ultracode_sessions: u32,
    /// Number of sessions not flagged `ultracode`.
    pub normal_sessions: u32,
    /// Mean `input + output` tokens across `ultracode` sessions.
    pub ultracode_avg_tokens: u64,
    /// Mean `input + output` tokens across normal sessions.
    pub normal_avg_tokens: u64,
}

impl UltracodeRoi {
    /// How many times heavier the average `ultracode` session is vs the average
    /// normal one. `None` when there are no normal sessions to divide by (avoids
    /// divide-by-zero) — the UI then omits the multiplier.
    pub fn multiplier(&self) -> Option<f64> {
        if self.normal_avg_tokens == 0 {
            return None;
        }
        Some(self.ultracode_avg_tokens as f64 / self.normal_avg_tokens as f64)
    }
}

/// How much of the model's context is served from cache — a proxy for prompt
/// reuse efficiency. Claude Code re-reads large cached contexts, so a high hit
/// ratio means little is being re-sent fresh.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CacheEfficiency {
    /// Cache *read* tokens (context served from cache).
    pub read_tokens: u64,
    /// Cache *write* tokens (context newly written to cache).
    pub write_tokens: u64,
}

impl CacheEfficiency {
    pub fn new(read_tokens: u64, write_tokens: u64) -> Self {
        Self {
            read_tokens,
            write_tokens,
        }
    }

    /// Whether there is any cache activity to report.
    pub fn has_activity(&self) -> bool {
        self.read_tokens + self.write_tokens > 0
    }

    /// Hit ratio `read / (read + write) * 100`, the share of cache traffic that
    /// was a reuse rather than a fresh write. `None` when there is no activity
    /// (guards divide-by-zero).
    pub fn hit_ratio_pct(&self) -> Option<f64> {
        let total = self.read_tokens + self.write_tokens;
        if total == 0 {
            return None;
        }
        Some(self.read_tokens as f64 / total as f64 * 100.0)
    }
}

/// All Insights-tab data for one period. Hangs off
/// [`UsageSnapshot`](super::UsageSnapshot).
#[derive(Debug, Clone, Default, Serialize)]
pub struct InsightsSnapshot {
    pub top_projects: Vec<ProjectStat>,
    pub top_sessions: Vec<SessionInsight>,
    pub ultracode_roi: UltracodeRoi,
}

// ── Aggregations ────────────────────────────────────────────────────────────

/// Top `n` projects by `input + output` tokens, descending. Ties break on the
/// raw dir name for determinism.
pub(crate) fn top_projects(accum: &ScanAccum, n: usize) -> Vec<ProjectStat> {
    let mut projects: Vec<ProjectStat> = accum
        .tokens_by_project
        .iter()
        .map(|(dir, acc)| {
            let path = accum.cwd_by_project.get(dir).cloned();
            ProjectStat {
                name: project_name(path.as_deref(), dir),
                dir: dir.clone(),
                path,
                tokens: acc.input_tokens + acc.output_tokens,
            }
        })
        .collect();
    projects.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.dir.cmp(&b.dir)));
    projects.truncate(n);
    projects
}

/// Top `n` sessions by token spend, descending, each annotated with its mode
/// (tier + ultracode). Sessions with zero tokens are dropped — they carry no
/// signal for the "most expensive" view. Ties break on `session_id`.
pub(crate) fn top_sessions(accum: &ScanAccum, n: usize) -> Vec<SessionInsight> {
    let mut sessions: Vec<SessionInsight> = accum
        .sessions
        .iter()
        .filter(|s| s.total_tokens > 0)
        .map(session_insight)
        .collect();
    sessions.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    sessions.truncate(n);
    sessions
}

/// Average token spend of `ultracode` sessions vs normal ones, over every
/// session in the accumulator. Averages are integer-truncated; groups with no
/// sessions report a `0` average.
pub(crate) fn ultracode_roi(accum: &ScanAccum) -> UltracodeRoi {
    let mut ultra_sum = 0u64;
    let mut ultra_count = 0u32;
    let mut normal_sum = 0u64;
    let mut normal_count = 0u32;

    for s in &accum.sessions {
        if s.is_ultracode {
            ultra_sum += s.total_tokens;
            ultra_count += 1;
        } else {
            normal_sum += s.total_tokens;
            normal_count += 1;
        }
    }

    let avg = |sum: u64, count: u32| if count == 0 { 0 } else { sum / count as u64 };

    UltracodeRoi {
        ultracode_sessions: ultra_count,
        normal_sessions: normal_count,
        ultracode_avg_tokens: avg(ultra_sum, ultra_count),
        normal_avg_tokens: avg(normal_sum, normal_count),
    }
}

/// Assemble the full [`InsightsSnapshot`] for a scan, taking the top `n` of
/// each ranked list.
pub(crate) fn build_insights(accum: &ScanAccum, n: usize) -> InsightsSnapshot {
    InsightsSnapshot {
        top_projects: top_projects(accum, n),
        top_sessions: top_sessions(accum, n),
        ultracode_roi: ultracode_roi(accum),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn session_insight(s: &SessionStat) -> SessionInsight {
    SessionInsight {
        session_id: s.session_id.clone(),
        project: project_name(s.cwd.as_deref(), &s.project_dir),
        tokens: s.total_tokens,
        tier: ModelTier::from_model(s.dominant_model.as_deref()),
        dominant_model: s.dominant_model.clone(),
        is_ultracode: s.is_ultracode,
    }
}

/// Resolve a project's display name.
///
/// Claude Code stores each project's dir as the cwd slugified by replacing every
/// `/` with `-`, which is lossy — the original path segments can't be recovered.
/// So we prefer the real `cwd` Claude Code writes on every entry and display its
/// `basename` (`/Users/pedro/Downloads/reconhecimento` → `reconhecimento`).
///
/// `slug` is the fallback for older logs that predate the `cwd` field: we trim
/// the leading/trailing dashes and surface the whole slug (`(unknown)` if empty).
pub fn project_name(cwd: Option<&str>, slug: &str) -> String {
    if let Some(cwd) = cwd {
        let base = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if !base.is_empty() {
            return base.to_string();
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        return "(unknown)".to_string();
    }
    trimmed.to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::scan::{ModelAccum, SessionStat};

    fn accum_with_projects(pairs: &[(&str, u64)]) -> ScanAccum {
        let mut accum = ScanAccum::default();
        for (dir, tokens) in pairs {
            accum.tokens_by_project.insert(
                dir.to_string(),
                ModelAccum {
                    input_tokens: *tokens,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                },
            );
        }
        accum
    }

    fn session(
        id: &str,
        project: &str,
        tokens: u64,
        model: Option<&str>,
        ultracode: bool,
    ) -> SessionStat {
        SessionStat {
            duration_secs: 60,
            message_count: 2,
            start_timestamp: "2026-05-10T10:00:00Z".to_string(),
            session_id: id.to_string(),
            project_dir: project.to_string(),
            cwd: None,
            total_tokens: tokens,
            dominant_model: model.map(str::to_string),
            is_ultracode: ultracode,
        }
    }

    #[test]
    fn top_projects_ranks_by_tokens_and_truncates() {
        let accum = accum_with_projects(&[("a", 300), ("b", 900), ("c", 100)]);
        let top = top_projects(&accum, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].dir, "b");
        assert_eq!(top[0].tokens, 900);
        assert_eq!(top[1].dir, "a");
    }

    #[test]
    fn top_projects_sums_input_and_output() {
        let mut accum = ScanAccum::default();
        accum.tokens_by_project.insert(
            "proj".to_string(),
            ModelAccum {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 999, // cache excluded
                cache_write_tokens: 999,
            },
        );
        let top = top_projects(&accum, 5);
        assert_eq!(top[0].tokens, 150);
    }

    #[test]
    fn top_sessions_sorted_desc_and_skips_empty() {
        let mut accum = ScanAccum::default();
        accum.sessions = vec![
            session("low", "p", 100, Some("claude-opus-4-7"), false),
            session("high", "p", 5000, Some("claude-opus-4-7"), true),
            session("zero", "p", 0, None, false), // dropped
            session("mid", "p", 1200, Some("claude-sonnet-4-7"), false),
        ];
        let top = top_sessions(&accum, 10);
        let ids: Vec<&str> = top.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["high", "mid", "low"]);
        assert_eq!(top[0].tier, ModelTier::Opus);
        assert!(top[0].is_ultracode);
    }

    #[test]
    fn model_tier_from_model_matches_substrings() {
        assert_eq!(ModelTier::from_model(Some("claude-opus-4-7")), ModelTier::Opus);
        assert_eq!(
            ModelTier::from_model(Some("claude-sonnet-4-5")),
            ModelTier::Sonnet
        );
        assert_eq!(ModelTier::from_model(Some("claude-haiku-4")), ModelTier::Haiku);
        assert_eq!(ModelTier::from_model(Some("gpt-4o")), ModelTier::Other);
        assert_eq!(ModelTier::from_model(None), ModelTier::Other);
    }

    #[test]
    fn ultracode_roi_averages_and_multiplier() {
        let mut accum = ScanAccum::default();
        accum.sessions = vec![
            // ultracode: 1000 + 3000 → avg 2000
            session("u1", "p", 1000, Some("claude-opus-4-7"), true),
            session("u2", "p", 3000, Some("claude-opus-4-7"), true),
            // normal: 400 + 600 → avg 500
            session("n1", "p", 400, Some("claude-opus-4-7"), false),
            session("n2", "p", 600, Some("claude-opus-4-7"), false),
        ];
        let roi = ultracode_roi(&accum);
        assert_eq!(roi.ultracode_sessions, 2);
        assert_eq!(roi.normal_sessions, 2);
        assert_eq!(roi.ultracode_avg_tokens, 2000);
        assert_eq!(roi.normal_avg_tokens, 500);
        assert_eq!(roi.multiplier(), Some(4.0));
    }

    #[test]
    fn ultracode_roi_multiplier_none_without_normal_sessions() {
        let mut accum = ScanAccum::default();
        accum.sessions = vec![session("u1", "p", 1000, Some("claude-opus-4-7"), true)];
        let roi = ultracode_roi(&accum);
        assert_eq!(roi.normal_sessions, 0);
        assert_eq!(roi.normal_avg_tokens, 0);
        assert_eq!(roi.multiplier(), None); // divide-by-zero guarded
    }

    #[test]
    fn cache_efficiency_hit_ratio_and_zero_guard() {
        // 95 read of 100 total → 95%.
        let eff = CacheEfficiency::new(95, 5);
        assert!(eff.has_activity());
        assert!((eff.hit_ratio_pct().unwrap() - 95.0).abs() < 1e-9);

        // No activity → guarded.
        let empty = CacheEfficiency::new(0, 0);
        assert!(!empty.has_activity());
        assert_eq!(empty.hit_ratio_pct(), None);
    }

    #[test]
    fn project_name_prefers_cwd_basename() {
        // Real cwd → basename, even though the slug is lossy.
        assert_eq!(
            project_name(
                Some("/Users/pedro/Downloads/reconhecimento"),
                "-Users-pedro-Downloads-reconhecimento"
            ),
            "reconhecimento"
        );
        // Trailing slash is tolerated.
        assert_eq!(
            project_name(Some("/Users/pedro/Downloads/aura/"), "-x"),
            "aura"
        );
    }

    #[test]
    fn project_name_falls_back_to_slug_when_cwd_missing() {
        assert_eq!(
            project_name(None, "-Users-pedro-Downloads-aura"),
            "Users-pedro-Downloads-aura"
        );
        assert_eq!(project_name(None, ""), "(unknown)");
        assert_eq!(project_name(None, "---"), "(unknown)");
        // Empty cwd also falls back.
        assert_eq!(project_name(Some(""), "-x"), "x");
    }

    #[test]
    fn top_projects_uses_cwd_basename() {
        let mut accum = accum_with_projects(&[("-Users-pedro-Downloads-aura", 500)]);
        accum.cwd_by_project.insert(
            "-Users-pedro-Downloads-aura".to_string(),
            "/Users/pedro/Downloads/aura".to_string(),
        );
        let top = top_projects(&accum, 5);
        assert_eq!(top[0].name, "aura");
        assert_eq!(top[0].path.as_deref(), Some("/Users/pedro/Downloads/aura"));
    }

    #[test]
    fn build_insights_assembles_lists_and_roi() {
        let mut accum = accum_with_projects(&[("p1", 500)]);
        accum.sessions = vec![
            session("s1", "p1", 500, Some("claude-opus-4-7"), false),
            session("s2", "p1", 1500, Some("claude-opus-4-7"), true),
        ];
        let ins = build_insights(&accum, 5);
        assert_eq!(ins.top_projects.len(), 1);
        assert_eq!(ins.top_sessions.len(), 2);
        assert_eq!(ins.ultracode_roi.ultracode_sessions, 1);
        assert_eq!(ins.ultracode_roi.normal_sessions, 1);
    }
}
