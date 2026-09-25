//! `aura agents …` subcommands.

use anyhow::{Context, Result};
use aura_core::config::{AgentConfig, AppConfig};
use clap::{Args, Subcommand};
use serde::Serialize;

use super::format::{print_json, OutputFormat};
use super::resolve::agent_kind_str;

#[derive(Debug, Args)]
pub struct AgentsCli {
    #[command(subcommand)]
    command: AgentsCommand,
}

#[derive(Debug, Subcommand)]
enum AgentsCommand {
    /// List configured agent profiles and whether their config dirs exist on disk.
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
}

#[derive(Debug, Serialize)]
struct AgentRow {
    name: String,
    kind: &'static str,
    config_path: String,
    exists: bool,
}

fn rows(agents: &[AgentConfig]) -> Vec<AgentRow> {
    agents
        .iter()
        .map(|a| {
            let path = a.resolved_config_path();
            AgentRow {
                name: a.name.clone(),
                kind: agent_kind_str(a.kind),
                config_path: path.to_string_lossy().into_owned(),
                exists: path.is_dir(),
            }
        })
        .collect()
}

impl AgentsCli {
    pub fn run(self) -> Result<()> {
        let AgentsCommand::List { format } = self.command;
        let config =
            AppConfig::load_with_discovery(&AppConfig::default_path()).context("load config")?;
        let rows = rows(&config.agents);
        match format {
            OutputFormat::Json => print_json(&rows),
            OutputFormat::Text => {
                if rows.is_empty() {
                    println!("No agent profiles configured. Run `aura config setup`.");
                    return Ok(());
                }
                println!("{:<32} {:<14} {:<8} CONFIG_PATH", "NAME", "KIND", "EXISTS");
                for r in &rows {
                    println!(
                        "{:<32} {:<14} {:<8} {}",
                        r.name,
                        r.kind,
                        if r.exists { "yes" } else { "no" },
                        r.config_path
                    );
                }
                Ok(())
            }
        }
    }
}
