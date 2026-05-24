//! Clap-driven CLI surface for `aura`.
//!
//! With no subcommand `aura` falls through to the tray entry point in
//! `main.rs`. Every other invocation is headless: each subcommand variant
//! routes to a `run()` on its argument struct and exits without spinning
//! up GPUI.
//!
//! When adding a new feature that has a user-facing surface, prefer
//! exposing it here as a subcommand alongside the GUI affordance —
//! every read-only operation should be scriptable. See
//! `.agent/context/cli.md` for the contract.

mod agents;
mod completions;
mod config;
mod doctor;
mod format;
mod plugin;
mod quota;
mod resolve;
mod state;
mod theme;
mod usage;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "aura",
    version,
    about = "Tray status for AI coding-agent token usage",
    long_about = "Aura is a tray app that shows token usage for AI coding agents \
                  (Claude Code, Codex, Gemini). Running `aura` with no subcommand \
                  launches the tray; subcommands expose configuration, plugins, \
                  usage, and quota data for scripting."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage the on-disk config (`~/.config/aura/config.toml`).
    Config(config::ConfigCli),

    /// Inspect and modify state (`~/.local/share/aura/state.json`).
    State(state::StateCli),

    /// Inspect and seed the user theme (`~/.config/aura/theme.toml`).
    Theme(theme::ThemeCli),

    /// List configured agent profiles and their detection status.
    Agents(agents::AgentsCli),

    /// Manage user plugins.
    #[command(alias = "plugins")]
    Plugin(plugin::PluginCli),

    /// Print a token-usage snapshot for an agent profile.
    Usage(usage::UsageCli),

    /// Print subscription rate-limit windows for an agent profile.
    Quota(quota::QuotaCli),

    /// Diagnose your install: paths, agent detection, plugin discovery,
    /// theme load result.
    Doctor(doctor::DoctorCli),

    /// Emit a shell completion script.
    Completions(completions::CompletionsCli),

    /// Compatibility alias for `aura config setup`. Kept so the existing
    /// installer scripts (`install.sh`, `install.ps1`) keep working.
    #[command(hide = true, name = "setup-config")]
    SetupConfig,
}

pub fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Config(args) => args.run(),
        Command::State(args) => args.run(),
        Command::Theme(args) => args.run(),
        Command::Agents(args) => args.run(),
        Command::Plugin(args) => args.run(),
        Command::Usage(args) => args.run(),
        Command::Quota(args) => args.run(),
        Command::Doctor(args) => args.run(),
        Command::Completions(args) => args.run::<Cli>(),
        Command::SetupConfig => config::run_setup(),
    }
}
