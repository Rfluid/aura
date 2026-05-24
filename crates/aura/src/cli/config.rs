//! `aura config …` subcommands.
//!
//! Wraps `AppConfig` from `aura-core`. `config setup` is also reachable
//! via the hidden top-level `setup-config` alias defined in `cli/mod.rs`
//! so the existing installer scripts keep working.

use anyhow::{Context, Result};
use aura_core::config::{AgentStatus, AppConfig};
use clap::{Args, Subcommand};

use super::format::{print_json, OutputFormat};
use super::theme::open_in_editor;

#[derive(Debug, Args)]
pub struct ConfigCli {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Detect installed agents and write/update `~/.config/aura/config.toml`.
    Setup,
    /// Print the resolved config file path.
    Path,
    /// Print the loaded config (text or JSON).
    Show {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Open `config.toml` in `$EDITOR` (creates defaults if missing).
    Edit,
    /// Parse the config file and report errors.
    Validate,
}

impl ConfigCli {
    pub fn run(self) -> Result<()> {
        match self.command {
            ConfigCommand::Setup => run_setup(),
            ConfigCommand::Path => {
                println!("{}", AppConfig::default_path().display());
                Ok(())
            }
            ConfigCommand::Show { format } => run_show(format),
            ConfigCommand::Edit => run_edit(),
            ConfigCommand::Validate => run_validate(),
        }
    }
}

/// Public so the hidden `setup-config` top-level command can call it
/// without going through `ConfigCli::run`.
pub fn run_setup() -> Result<()> {
    let path = AppConfig::default_path();
    let report = AppConfig::run_setup(&path)?;

    if report.created {
        println!("Created {}", path.display());
    } else {
        println!("Updated {}", path.display());
    }

    for (agent, status) in &report.agents {
        let resolved = agent.resolved_config_path();
        match status {
            AgentStatus::Added => {
                println!("  + {} ({})", agent.name, resolved.display());
            }
            AgentStatus::AlreadyConfigured => {
                println!(
                    "  · {} ({}) — already configured",
                    agent.name,
                    resolved.display()
                );
            }
            AgentStatus::NotInstalled => {
                println!(
                    "  - {} ({}) — not installed, skipping",
                    agent.name,
                    resolved.display()
                );
            }
        }
    }

    Ok(())
}

fn run_show(format: OutputFormat) -> Result<()> {
    let path = AppConfig::default_path();
    let config = AppConfig::load_with_discovery(&path)
        .with_context(|| format!("load config from {}", path.display()))?;
    match format {
        OutputFormat::Json => print_json(&config),
        OutputFormat::Text => {
            let toml = toml::to_string_pretty(&config).context("re-serialize config")?;
            print!("{toml}");
            Ok(())
        }
    }
}

fn run_edit() -> Result<()> {
    let path = AppConfig::default_path();
    // `AppConfig::load` writes defaults if the file is missing, so editing
    // a fresh install behaves like editing an existing config.
    let _ = AppConfig::load(&path).with_context(|| format!("load {}", path.display()))?;
    open_in_editor(&path)
}

fn run_validate() -> Result<()> {
    let path = AppConfig::default_path();
    if !path.exists() {
        println!(
            "{} does not exist (will be created on next launch).",
            path.display()
        );
        return Ok(());
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let _: AppConfig =
        toml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    println!("{} is valid.", path.display());
    Ok(())
}
