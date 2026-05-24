//! `aura usage` — emit a token-usage snapshot from a profile's local data.

use anyhow::{Context, Result};
use aura_core::{
    config::AppConfig,
    reader::{make_reader, Period, UsageSnapshot},
    state::AppState,
};
use clap::{Args, ValueEnum};

use super::format::{print_json, OutputFormat};
use super::resolve::resolve_profile;

#[derive(Debug, Args)]
pub struct UsageCli {
    /// Agent profile name. Defaults to the active profile in state, falling
    /// back to the first agent in config.
    #[arg(long)]
    profile: Option<String>,

    /// Reporting period.
    #[arg(long, value_enum, default_value_t = PeriodArg::All)]
    period: PeriodArg,

    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

/// CLI shape for `Period`. Kept separate so the on-the-wire spellings
/// (`all`, `7d`, `30d`) match the values the plugin runner already passes
/// via `--period`.
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum PeriodArg {
    #[default]
    All,
    #[value(name = "7d")]
    Last7d,
    #[value(name = "30d")]
    Last30d,
}

impl From<PeriodArg> for Period {
    fn from(p: PeriodArg) -> Self {
        match p {
            PeriodArg::All => Period::AllTime,
            PeriodArg::Last7d => Period::Last7Days,
            PeriodArg::Last30d => Period::Last30Days,
        }
    }
}

impl UsageCli {
    pub fn run(self) -> Result<()> {
        let config =
            AppConfig::load_with_discovery(&AppConfig::default_path()).context("load config")?;
        let state = AppState::load().unwrap_or_default();
        let agent = resolve_profile(&config.agents, &state, self.profile.as_deref())?;
        let snapshot = make_reader(agent)
            .snapshot(self.period.into())
            .with_context(|| format!("read snapshot for '{}'", agent.name))?;
        match self.format {
            OutputFormat::Json => print_json(&snapshot),
            OutputFormat::Text => {
                render_text(&agent.name, &snapshot);
                Ok(())
            }
        }
    }
}

fn render_text(profile: &str, s: &UsageSnapshot) {
    println!("Profile: {profile}");
    println!(
        "Tokens:  {} total  ({} in, {} out)",
        s.total_tokens, s.total_input_tokens, s.total_output_tokens
    );
    println!(
        "Cache:   {} read, {} write",
        s.total_cache_read_tokens, s.total_cache_write_tokens
    );
    println!(
        "Sessions: {} ({} messages)",
        s.total_sessions, s.total_messages
    );
    println!("Active days: {} / {}", s.active_days, s.total_days);
    if let Some(model) = &s.favorite_model {
        println!("Favorite model: {model}");
    }
    println!(
        "Streak: current {} / longest {}",
        s.streaks.current, s.streaks.longest
    );
    if let Some(h) = s.peak_hour {
        println!("Peak hour: {h:02}:00");
    }
    if !s.per_model.is_empty() {
        println!();
        println!("Per model:");
        for m in &s.per_model {
            println!(
                "  {:<28} {:>10} tokens  ({:>10} in, {:>10} out)",
                m.model,
                m.total_tokens(),
                m.input_tokens,
                m.output_tokens
            );
        }
    }
}
