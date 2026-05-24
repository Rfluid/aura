//! Shared profile-resolution + agent-kind formatting helpers.

use anyhow::{anyhow, Result};
use aura_core::{
    config::{AgentConfig, AgentKind},
    state::AppState,
};

/// Pick the agent profile to operate on. Precedence:
///
/// 1. `--profile <name>` (case-insensitive)
/// 2. `state.active_profile` (whatever the modal last selected)
/// 3. The first entry in `config.agents`
pub fn resolve_profile<'a>(
    agents: &'a [AgentConfig],
    state: &AppState,
    requested: Option<&str>,
) -> Result<&'a AgentConfig> {
    if agents.is_empty() {
        return Err(anyhow!(
            "no agent profiles configured; run `aura config setup`"
        ));
    }
    let name = requested
        .map(str::to_string)
        .or_else(|| state.active_profile.clone());
    if let Some(name) = name {
        return agents
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| anyhow!("no agent profile named '{name}' in config"));
    }
    Ok(&agents[0])
}

/// Stable kebab-case name for an `AgentKind`. Matches the on-disk TOML
/// representation (`kind = "claude-code"`).
pub fn agent_kind_str(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "claude-code",
        AgentKind::Codex => "codex",
        AgentKind::Gemini => "gemini",
    }
}
