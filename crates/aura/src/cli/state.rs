//! `aura state …` subcommands.
//!
//! Wraps `AppState` from `aura-core`. `set-profile` cross-validates against
//! the loaded `AppConfig` so we never persist a name that won't resolve.

use anyhow::{anyhow, Context, Result};
use aura_core::{config::AppConfig, state::AppState};
use clap::{Args, Subcommand};

use super::format::{print_json, OutputFormat};

#[derive(Debug, Args)]
pub struct StateCli {
    #[command(subcommand)]
    command: StateCommand,
}

#[derive(Debug, Subcommand)]
enum StateCommand {
    /// Print the state file path.
    Path,
    /// Print current state.
    Show {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Set the active agent profile (case-insensitive name match).
    SetProfile { name: String },
    /// Reset state (remove active-profile selection).
    Clear,
}

impl StateCli {
    pub fn run(self) -> Result<()> {
        match self.command {
            StateCommand::Path => {
                println!("{}", AppState::state_path().display());
                Ok(())
            }
            StateCommand::Show { format } => {
                let state = AppState::load().context("load state")?;
                match format {
                    OutputFormat::Json => print_json(&state),
                    OutputFormat::Text => {
                        match &state.active_profile {
                            Some(name) => println!("active_profile = \"{name}\""),
                            None => println!("active_profile = <none>"),
                        }
                        Ok(())
                    }
                }
            }
            StateCommand::SetProfile { name } => {
                let config = AppConfig::load_with_discovery(&AppConfig::default_path())
                    .context("load config to validate profile name")?;
                let canonical = config
                    .agents
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(&name))
                    .map(|a| a.name.clone())
                    .ok_or_else(|| anyhow!("no agent profile named '{name}' in config"))?;
                let mut state = AppState::load().unwrap_or_default();
                state.active_profile = Some(canonical.clone());
                state.save().context("save state")?;
                println!("active_profile = \"{canonical}\"");
                Ok(())
            }
            StateCommand::Clear => {
                let mut state = AppState::load().unwrap_or_default();
                state.active_profile = None;
                state.save().context("save state")?;
                println!("state cleared");
                Ok(())
            }
        }
    }
}
