//! Insights: pure aggregations over a single [`ScanAccum`] pass that answer the
//! "interesting" questions a heavy user asks — which project burned the most
//! tokens, which single session was the most expensive (and what mode it ran
//! in), and how token spend splits across model tiers.
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
    /// Human-friendly project name (last path segment of the unslugged cwd).
    pub name: String,
    /// Raw slugified project-dir as stored on disk (`-Users-pedro-…`).
    pub dir: String,
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
    /// Human-friendly project name this session ran under.
    pub project: String,
    pub tokens: u64,
    pub duration_secs: u64,
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

/// Share of token spend by model tier plus the `ultracode` session split, for
/// the period in view.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModeDistribution {
    /// `(tier label, tokens)` pairs sorted by tokens descending. Only tiers
    /// with non-zero spend appear.
    pub by_tier: Vec<(String, u64)>,
    /// Sessions flagged `ultracode` by the content heuristic.
    pub ultracode_sessions: u32,
    /// Sessions not flagged `ultracode`.
    pub normal_sessions: u32,
}

impl ModeDistribution {
    /// Total tokens across all tiers (denominator for percentage display).
    pub fn total_tokens(&self) -> u64 {
        self.by_tier.iter().map(|(_, t)| *t).sum()
    }

    /// Percentage of total token spend for a given tier label, `0.0` when there
    /// is no spend. The returned percentages across all tiers sum to 100 (±
    /// floating-point rounding).
    pub fn tier_pct(&self, label: &str) -> f64 {
        let total = self.total_tokens();
        if total == 0 {
            return 0.0;
        }
        let tokens = self
            .by_tier
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, t)| *t)
            .unwrap_or(0);
        tokens as f64 / total as f64 * 100.0
    }
}

/// All Insights-tab data for one period. Hangs off
/// [`UsageSnapshot`](super::UsageSnapshot).
#[derive(Debug, Clone, Default, Serialize)]
pub struct InsightsSnapshot {
    pub top_projects: Vec<ProjectStat>,
    pub top_sessions: Vec<SessionInsight>,
    pub mode_distribution: ModeDistribution,
}

// ── Aggregations ────────────────────────────────────────────────────────────

/// Top `n` projects by `input + output` tokens, descending. Ties break on the
/// raw dir name for determinism.
pub(crate) fn top_projects(accum: &ScanAccum, n: usize) -> Vec<ProjectStat> {
    let mut projects: Vec<ProjectStat> = accum
        .tokens_by_project
        .iter()
        .map(|(dir, acc)| ProjectStat {
            name: humanize_project(dir),
            dir: dir.clone(),
            tokens: acc.input_tokens + acc.output_tokens,
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

/// Share of token spend by model tier plus the ultracode/normal session split,
/// over every session in the accumulator.
pub(crate) fn mode_distribution(accum: &ScanAccum) -> ModeDistribution {
    use std::collections::HashMap;

    let mut by_tier: HashMap<&'static str, u64> = HashMap::new();
    let mut ultracode_sessions = 0u32;
    let mut normal_sessions = 0u32;

    for s in &accum.sessions {
        let tier = ModelTier::from_model(s.dominant_model.as_deref());
        *by_tier.entry(tier.label()).or_insert(0) += s.total_tokens;
        if s.is_ultracode {
            ultracode_sessions += 1;
        } else {
            normal_sessions += 1;
        }
    }

    let mut by_tier: Vec<(String, u64)> = by_tier
        .into_iter()
        .filter(|(_, t)| *t > 0)
        .map(|(l, t)| (l.to_string(), t))
        .collect();
    by_tier.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    ModeDistribution {
        by_tier,
        ultracode_sessions,
        normal_sessions,
    }
}

/// Assemble the full [`InsightsSnapshot`] for a scan, taking the top `n` of
/// each ranked list.
pub(crate) fn build_insights(accum: &ScanAccum, n: usize) -> InsightsSnapshot {
    InsightsSnapshot {
        top_projects: top_projects(accum, n),
        top_sessions: top_sessions(accum, n),
        mode_distribution: mode_distribution(accum),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn session_insight(s: &SessionStat) -> SessionInsight {
    SessionInsight {
        session_id: s.session_id.clone(),
        project: humanize_project(&s.project_dir),
        tokens: s.total_tokens,
        duration_secs: s.duration_secs,
        tier: ModelTier::from_model(s.dominant_model.as_deref()),
        dominant_model: s.dominant_model.clone(),
        is_ultracode: s.is_ultracode,
    }
}

/// Turn a slugified project-dir (`-Users-pedro-Downloads-cambrian-api-key-dashboard`)
/// into a short human label — the last path segment of the original cwd.
///
/// Claude Code slugifies a project's cwd by replacing every `/` with `-`, so the
/// original segment boundaries are not recoverable in general. We take the
/// trailing run after the final separator, which is the working-directory name
/// in the common case (`…-cambrian-api-key-dashboard` → `cambrian-api-key-dashboard`).
/// Empty input yields `"(unknown)"`.
pub fn humanize_project(dir: &str) -> String {
    let trimmed = dir.trim_matches('-');
    if trimmed.is_empty() {
        return "(unknown)".to_string();
    }
    // The slug is `<segment>-<segment>-…`; without the original separator map we
    // can't split path segments, so surface the whole trimmed slug. It reads as
    // the project path with dashes, which is what users recognise.
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
    fn mode_distribution_tiers_and_counts() {
        let mut accum = ScanAccum::default();
        accum.sessions = vec![
            session("a", "p", 920, Some("claude-opus-4-7"), true),
            session("b", "p", 60, Some("claude-sonnet-4-7"), false),
            session("c", "p", 20, Some("claude-haiku-4"), false),
        ];
        let dist = mode_distribution(&accum);

        // Sorted by tokens desc.
        assert_eq!(dist.by_tier[0].0, "opus");
        assert_eq!(dist.by_tier[0].1, 920);
        assert_eq!(dist.ultracode_sessions, 1);
        assert_eq!(dist.normal_sessions, 2);

        // Percentages sum to ~100.
        let sum: f64 = dist
            .by_tier
            .iter()
            .map(|(l, _)| dist.tier_pct(l))
            .sum();
        assert!((sum - 100.0).abs() < 1e-6, "tier pct sum was {sum}");
        // opus is ~92%.
        assert!((dist.tier_pct("opus") - 92.0).abs() < 0.001);
    }

    #[test]
    fn mode_distribution_empty_is_safe() {
        let accum = ScanAccum::default();
        let dist = mode_distribution(&accum);
        assert_eq!(dist.total_tokens(), 0);
        assert_eq!(dist.tier_pct("opus"), 0.0);
        assert!(dist.by_tier.is_empty());
    }

    #[test]
    fn humanize_project_trims_leading_dashes() {
        assert_eq!(
            humanize_project("-Users-pedro-Downloads-aura"),
            "Users-pedro-Downloads-aura"
        );
        assert_eq!(humanize_project(""), "(unknown)");
        assert_eq!(humanize_project("---"), "(unknown)");
    }

    #[test]
    fn build_insights_assembles_all_three() {
        let mut accum = accum_with_projects(&[("p1", 500)]);
        accum.sessions = vec![session("s1", "p1", 500, Some("claude-opus-4-7"), false)];
        let ins = build_insights(&accum, 5);
        assert_eq!(ins.top_projects.len(), 1);
        assert_eq!(ins.top_sessions.len(), 1);
        assert_eq!(ins.mode_distribution.by_tier[0].0, "opus");
    }
}
