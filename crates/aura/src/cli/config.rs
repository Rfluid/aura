//! `aura config …` subcommands.
//!
//! Wraps `AppConfig` from `aura-core`. `config setup` is also reachable
//! via the hidden top-level `setup-config` alias defined in `cli/mod.rs`
//! so the existing installer scripts keep working.
//!
//! Discoverability is driven by the field registry in
//! `aura_core::config_schema`: `describe` lists/explains every field, `get`
//! and `set` read/write a single key with validation, and `wizard` walks the
//! fields interactively. The on-disk `config.toml` is written with `#`
//! comments above each key (see `config_schema::render_commented`), so the
//! file itself documents what's available.

use std::io::{self, Write};

use anyhow::{Context, Result};
use aura_core::config::{AgentStatus, AppConfig};
use aura_core::config_schema::{self, FieldDescriptor, SectionField};
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
    /// List every config field with its type, default, and docs — or explain
    /// one field when a key is given (e.g. `display.anchor`).
    Describe {
        /// A dotted key to explain in full (omit to list everything).
        key: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Print the current value of a single key (e.g. `display.anchor`).
    Get {
        /// Dotted key, e.g. `display.anchor`.
        key: String,
    },
    /// Set a single key and save (e.g. `set display.anchor top`).
    Set {
        /// Dotted key, e.g. `display.anchor`.
        key: String,
        /// New value. Use `none` to clear an optional field.
        value: String,
    },
    /// Interactively walk every field, keeping the current value on blank input.
    Wizard,
    /// Write a fresh, fully-commented `config.toml`.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Rewrite the existing `config.toml` in place with inline docs, keeping
    /// every current value (adds the `#` comments an older config lacks).
    Document,
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
            ConfigCommand::Describe { key, format } => run_describe(key.as_deref(), format),
            ConfigCommand::Get { key } => run_get(&key),
            ConfigCommand::Set { key, value } => run_set(&key, &value),
            ConfigCommand::Wizard => run_wizard(),
            ConfigCommand::Init { force } => run_init(force),
            ConfigCommand::Document => run_document(),
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

// ── describe ────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct SchemaJson {
    fields: Vec<FieldDescriptor>,
    agents: Vec<SectionField>,
    plugins: Vec<SectionField>,
}

fn run_describe(key: Option<&str>, format: OutputFormat) -> Result<()> {
    match (key, format) {
        (_, OutputFormat::Json) => {
            // A single key still emits the whole schema in JSON — callers can
            // filter with jq; keeping one shape is simpler to consume.
            print_json(&SchemaJson {
                fields: config_schema::fields().to_vec(),
                agents: config_schema::agent_fields().to_vec(),
                plugins: config_schema::plugin_fields().to_vec(),
            })
        }
        (Some(key), OutputFormat::Text) => describe_one(key),
        (None, OutputFormat::Text) => {
            describe_all();
            Ok(())
        }
    }
}

fn describe_one(key: &str) -> Result<()> {
    let Some(f) = config_schema::field(key) else {
        anyhow::bail!(
            "unknown config key `{key}`. Run `aura config describe` to list settable keys."
        );
    };
    let current = AppConfig::load(&AppConfig::default_path())
        .ok()
        .and_then(|cfg| config_schema::get_value(&cfg, key).ok());

    println!("{}  ({})", f.key, f.type_label);
    if !f.allowed.is_empty() {
        println!("  allowed: {}", f.allowed.join(" | "));
    }
    println!("  default: {}", f.default);
    if let Some(cur) = current {
        println!("  current: {cur}");
    }
    println!();
    println!("{}", f.description);
    Ok(())
}

fn describe_all() {
    let current = AppConfig::load(&AppConfig::default_path()).ok();

    println!("Settable keys — `aura config set <key> <value>`:\n");
    for section in ["display", "update"] {
        println!("[{section}]");
        let prefix = format!("{section}.");
        for f in config_schema::fields()
            .iter()
            .filter(|f| f.key.starts_with(&prefix))
        {
            let leaf = &f.key[prefix.len()..];
            let allowed = if f.allowed.is_empty() {
                String::new()
            } else {
                format!("  [{}]", f.allowed.join(" | "))
            };
            println!("  {leaf:<22} {}{allowed}", f.type_label);
            println!("      {}", f.summary);
            let cur = current
                .as_ref()
                .and_then(|c| config_schema::get_value(c, f.key).ok());
            match cur {
                Some(cur) => println!("      default: {}   current: {cur}", f.default),
                None => println!("      default: {}", f.default),
            }
        }
        println!();
    }

    println!("Repeatable tables — edit via `aura config edit`, `aura agents`, `aura plugin`:\n");
    print_section("[[agents]]", config_schema::agent_fields());
    print_section("[[plugins]]", config_schema::plugin_fields());

    println!("Explain one field with `aura config describe <key>` (e.g. display.anchor).");
}

fn print_section(header: &str, fields: &[SectionField]) {
    println!("{header}");
    for f in fields {
        let allowed = if f.allowed.is_empty() {
            String::new()
        } else {
            format!(" [{}]", f.allowed.join(" | "))
        };
        println!(
            "  {:<14} ({}){allowed} — {}",
            f.key, f.type_label, f.summary
        );
    }
    println!();
}

// ── get / set / wizard / init ────────────────────────────────────────────────

fn run_get(key: &str) -> Result<()> {
    let path = AppConfig::default_path();
    let cfg = AppConfig::load(&path).with_context(|| format!("load {}", path.display()))?;
    let value = config_schema::get_value(&cfg, key).map_err(anyhow::Error::msg)?;
    println!("{value}");
    Ok(())
}

fn run_set(key: &str, value: &str) -> Result<()> {
    let path = AppConfig::default_path();
    let mut cfg = AppConfig::load(&path).with_context(|| format!("load {}", path.display()))?;
    config_schema::set_value(&mut cfg, key, value).map_err(anyhow::Error::msg)?;
    cfg.save(&path)
        .with_context(|| format!("write config to {}", path.display()))?;
    let stored = config_schema::get_value(&cfg, key).unwrap_or_else(|_| value.to_string());
    println!("set {key} = {stored}");
    Ok(())
}

fn run_wizard() -> Result<()> {
    let path = AppConfig::default_path();
    let mut cfg = AppConfig::load(&path).with_context(|| format!("load {}", path.display()))?;

    println!("Editing {}", path.display());
    println!("Press Enter to keep the current value; type `none` to clear an optional field.\n");

    let stdin = io::stdin();
    let mut changed = false;
    for f in config_schema::fields() {
        let current = config_schema::get_value(&cfg, f.key).unwrap_or_default();
        let hint = if f.allowed.is_empty() {
            String::new()
        } else {
            format!(" ({})", f.allowed.join("/"))
        };
        loop {
            print!("{}{hint} [{current}]: ", f.key);
            io::stdout().flush().ok();
            let mut line = String::new();
            if stdin.read_line(&mut line)? == 0 {
                // EOF (piped/empty input): stop walking, keep what we have.
                println!();
                break;
            }
            let input = line.trim();
            if input.is_empty() {
                break;
            }
            match config_schema::set_value(&mut cfg, f.key, input) {
                Ok(()) => {
                    changed = true;
                    break;
                }
                Err(e) => println!("  {e}\n"),
            }
        }
    }

    if changed {
        cfg.save(&path)
            .with_context(|| format!("write config to {}", path.display()))?;
        println!("\nSaved {}", path.display());
    } else {
        println!("\nNo changes.");
    }
    Ok(())
}

fn run_init(force: bool) -> Result<()> {
    let path = AppConfig::default_path();
    if path.exists() && !force {
        println!(
            "{} already exists (pass --force to overwrite).",
            path.display()
        );
        return Ok(());
    }
    // `save` serializes through the commented renderer.
    AppConfig::default_config()
        .save(&path)
        .with_context(|| format!("write config to {}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}

fn run_document() -> Result<()> {
    let path = AppConfig::default_path();
    let existed = path.exists();
    // Load parses the current values; save re-serializes through the commented
    // renderer, so the rewrite preserves values and (re)applies the inline docs.
    let cfg = AppConfig::load(&path).with_context(|| format!("load {}", path.display()))?;
    cfg.save(&path)
        .with_context(|| format!("write config to {}", path.display()))?;
    if existed {
        println!("Rewrote {} with inline documentation.", path.display());
    } else {
        println!("Created {} with inline documentation.", path.display());
    }
    Ok(())
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
