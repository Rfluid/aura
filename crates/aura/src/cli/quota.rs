//! `aura quota` — print subscription rate-limit windows.

use anyhow::{Context, Result};
use aura_core::{
    config::{AgentKind, AppConfig},
    quota::{CodexQuota, GeminiQuota, QuotaApi, QuotaSnapshot},
    state::AppState,
};
use clap::Args;

use super::format::{print_json, OutputFormat};
use super::resolve::resolve_profile;

#[derive(Debug, Args)]
pub struct QuotaCli {
    /// Agent profile name. Defaults to the active profile, then the first agent.
    #[arg(long)]
    profile: Option<String>,

    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

impl QuotaCli {
    pub fn run(self) -> Result<()> {
        let config =
            AppConfig::load_with_discovery(&AppConfig::default_path()).context("load config")?;
        let state = AppState::load().unwrap_or_default();
        let agent = resolve_profile(&config.agents, &state, self.profile.as_deref())?;
        let snapshot = match agent.kind {
            AgentKind::ClaudeCode => QuotaApi::new(agent.resolved_config_path()).snapshot(),
            AgentKind::Codex => CodexQuota::new(agent.resolved_config_path()).snapshot(),
            AgentKind::Gemini => GeminiQuota::new(agent.resolved_config_path()).snapshot(),
        };
        match self.format {
            OutputFormat::Json => print_json(&snapshot),
            OutputFormat::Text => {
                render_text(&agent.name, &snapshot);
                Ok(())
            }
        }
    }
}

fn render_text(profile: &str, q: &QuotaSnapshot) {
    println!("Profile: {profile}");
    if let Some(sub) = &q.subscription_type {
        println!("Subscription: {sub}");
    }
    println!("Source: {:?}", q.source);
    if let Some(note) = &q.note {
        println!("Note: {note}");
    }
    if q.windows.is_empty() {
        println!("(no windows reported)");
        return;
    }
    println!();
    println!("{:<14} {:>8} {:>12} RESETS_AT", "WINDOW", "USED%", "TOKENS");
    for w in &q.windows {
        let pct = w
            .used_percentage
            .map(|p| format!("{p:>5.1}%"))
            .unwrap_or_else(|| "    —".to_string());
        let toks = w
            .used_tokens
            .map(|t| t.to_string())
            .unwrap_or_else(|| "—".to_string());
        let reset = w
            .resets_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "—".to_string());
        println!("{:<14} {:>8} {:>12} {}", w.label, pct, toks, reset);
    }
}
