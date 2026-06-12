//! `aura plugin …` subcommands.
//!
//! `add`, `list`, and `remove` preserve byte-identical output of the
//! pre-clap CLI so existing scripts and READMEs keep working. `dir` and
//! `run` are new.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use aura_core::{
    config::AppConfig,
    plugin::{self, AddOptions, PluginRunner},
};
use clap::{Args, Subcommand};
use serde::Serialize;

use super::format::{print_json, OutputFormat};
use super::usage::PeriodArg;

#[derive(Debug, Args)]
pub struct PluginCli {
    #[command(subcommand)]
    command: PluginCommand,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    /// Install a plugin binary into the user plugins dir.
    Add {
        /// Path to the plugin binary.
        path: PathBuf,
        /// Override destination filename inside the plugins dir.
        #[arg(long = "as")]
        as_name: Option<String>,
        /// Symlink instead of copying (Unix-only).
        #[arg(long, alias = "symlink")]
        link: bool,
        /// Display name shown in the tray modal.
        #[arg(long)]
        name: Option<String>,
        /// Accent color override (hex `#rrggbb` or `#rgb`).
        #[arg(long)]
        color: Option<String>,
        /// Icon path (embedded asset name, absolute path, or `~/`-relative).
        #[arg(long)]
        icon: Option<String>,
    },
    /// List configured + discovered plugins.
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Remove a discovered plugin by display name.
    #[command(alias = "rm")]
    Remove { name: String },
    /// Print the user plugins directory path.
    Dir,
    /// Run a plugin once and print its JSON payload (debug helper for plugin authors).
    Run {
        /// Plugin display name (matches the value in `aura plugin list`).
        name: String,
        /// Reporting period passed via `--period` to the plugin.
        #[arg(long, value_enum, default_value_t = PeriodArg::All)]
        period: PeriodArg,
        /// Fire a button action (`<cmd> action <id> --period <p>`) instead
        /// of a plain panel refresh.
        #[arg(long)]
        action: Option<String>,
    },
}

impl PluginCli {
    pub fn run(self) -> Result<()> {
        match self.command {
            PluginCommand::Add {
                path,
                as_name,
                link,
                name,
                color,
                icon,
            } => run_add(path, as_name, link, name, color, icon),
            PluginCommand::List { format } => run_list(format),
            PluginCommand::Remove { name } => run_remove(&name),
            PluginCommand::Dir => {
                println!("{}", plugin::user_plugins_dir().display());
                Ok(())
            }
            PluginCommand::Run {
                name,
                period,
                action,
            } => run_plugin(&name, period.into(), action.as_deref()),
        }
    }
}

fn run_add(
    path: PathBuf,
    as_name: Option<String>,
    link: bool,
    name: Option<String>,
    color: Option<String>,
    icon: Option<String>,
) -> Result<()> {
    let plugins_dir = plugin::user_plugins_dir();
    let outcome = plugin::add_plugin(
        &plugins_dir,
        AddOptions {
            source: path,
            dest_name: as_name,
            symlink: link,
            name,
            color,
            icon,
        },
    )?;
    println!("Installed {}", outcome.installed.display());
    if let Some(sidecar) = outcome.sidecar {
        println!("  sidecar  {}", sidecar.display());
    }
    Ok(())
}

fn run_remove(name: &str) -> Result<()> {
    let plugins_dir = plugin::user_plugins_dir();
    let outcome = plugin::remove_plugin(&plugins_dir, name)?;
    println!("Removed {}", outcome.removed_binary.display());
    if let Some(s) = outcome.removed_sidecar {
        println!("  sidecar {}", s.display());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct PluginRow {
    name: String,
    source: &'static str,
    command: String,
}

fn run_list(format: OutputFormat) -> Result<()> {
    let config_path = AppConfig::default_path();
    let config =
        AppConfig::load(&config_path).with_context(|| format!("load {}", config_path.display()))?;
    let plugins_dir = plugin::user_plugins_dir();
    let discovered = plugin::discover_plugins(&plugins_dir);

    let mut rows: Vec<PluginRow> = Vec::new();
    for p in &config.plugins {
        rows.push(PluginRow {
            name: p.name.clone(),
            source: "config",
            command: p.command.clone(),
        });
    }
    for p in &discovered {
        if config
            .plugins
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&p.name))
        {
            // Shadowed by a config entry; don't double-list.
            continue;
        }
        rows.push(PluginRow {
            name: p.name.clone(),
            source: "discovered",
            command: p.command.clone(),
        });
    }

    match format {
        OutputFormat::Json => print_json(&rows),
        OutputFormat::Text => {
            if rows.is_empty() {
                println!("No plugins configured.");
                println!(
                    "Drop a binary into {} or run `aura plugin add <path>`.",
                    plugins_dir.display()
                );
                return Ok(());
            }
            println!("{:<24} {:<12} COMMAND", "NAME", "SOURCE");
            for r in &rows {
                println!("{:<24} {:<12} {}", r.name, r.source, r.command);
            }
            Ok(())
        }
    }
}

fn run_plugin(name: &str, period: aura_core::reader::Period, action: Option<&str>) -> Result<()> {
    let config =
        AppConfig::load_with_discovery(&AppConfig::default_path()).context("load config")?;
    let plugin_cfg = config
        .plugins
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("no plugin named '{name}' (try `aura plugin list`)"))?;
    let panel = match action {
        Some(id) => PluginRunner::run_action(plugin_cfg, id, period),
        None => PluginRunner::run_with_period(plugin_cfg, period),
    };
    print_json(&panel)
}
