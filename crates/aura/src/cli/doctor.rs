//! `aura doctor` — print resolved paths, agent detection, plugin discovery,
//! theme load result. Pure local introspection; no network calls.

use anyhow::Result;
use aura_core::{config::AppConfig, plugin, state::AppState, theme::Theme};
use clap::Args;
use serde::Serialize;

use super::format::{print_json, OutputFormat};
use super::resolve::agent_kind_str;

#[derive(Debug, Args)]
pub struct DoctorCli {
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    paths: Paths,
    config: ConfigStatus,
    state: StateStatus,
    theme: ThemeStatus,
    agents: Vec<AgentRow>,
    plugins: PluginStatus,
}

#[derive(Debug, Serialize)]
struct Paths {
    config: String,
    state: String,
    theme: String,
    plugins_dir: String,
}

#[derive(Debug, Serialize)]
struct ConfigStatus {
    exists: bool,
    parse_ok: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct StateStatus {
    exists: bool,
    active_profile: Option<String>,
}

#[derive(Debug, Serialize)]
struct ThemeStatus {
    exists: bool,
    parse_ok: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentRow {
    name: String,
    kind: &'static str,
    config_path: String,
    exists: bool,
}

#[derive(Debug, Serialize)]
struct PluginStatus {
    configured: usize,
    discovered: usize,
    names: Vec<String>,
}

impl DoctorCli {
    pub fn run(self) -> Result<()> {
        let report = collect();
        match self.format {
            OutputFormat::Json => print_json(&report),
            OutputFormat::Text => {
                render_text(&report);
                Ok(())
            }
        }
    }
}

fn collect() -> DoctorReport {
    let config_path = AppConfig::default_path();
    let state_path = AppState::state_path();
    let theme_path = Theme::default_path();
    let plugins_dir = plugin::user_plugins_dir();

    let (config_status, agents, plugins) = match AppConfig::load_with_discovery(&config_path) {
        Ok(cfg) => {
            let agents: Vec<AgentRow> = cfg
                .agents
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
                .collect();
            let discovered = plugin::discover_plugins(&plugins_dir);
            let configured_count = cfg
                .plugins
                .iter()
                .filter(|p| {
                    !discovered
                        .iter()
                        .any(|d| d.name.eq_ignore_ascii_case(&p.name))
                })
                .count();
            let plugins = PluginStatus {
                configured: configured_count,
                discovered: discovered.len(),
                names: cfg.plugins.iter().map(|p| p.name.clone()).collect(),
            };
            (
                ConfigStatus {
                    exists: config_path.exists(),
                    parse_ok: true,
                    error: None,
                },
                agents,
                plugins,
            )
        }
        Err(e) => (
            ConfigStatus {
                exists: config_path.exists(),
                parse_ok: false,
                error: Some(format!("{e:#}")),
            },
            Vec::new(),
            PluginStatus {
                configured: 0,
                discovered: 0,
                names: Vec::new(),
            },
        ),
    };

    let state_status = match AppState::load_from(&state_path) {
        Ok(s) => StateStatus {
            exists: state_path.exists(),
            active_profile: s.active_profile,
        },
        Err(_) => StateStatus {
            exists: state_path.exists(),
            active_profile: None,
        },
    };

    let theme_status = if theme_path.exists() {
        match Theme::load(&theme_path) {
            Ok(_) => ThemeStatus {
                exists: true,
                parse_ok: true,
                error: None,
            },
            Err(e) => ThemeStatus {
                exists: true,
                parse_ok: false,
                error: Some(format!("{e:#}")),
            },
        }
    } else {
        ThemeStatus {
            exists: false,
            parse_ok: true,
            error: None,
        }
    };

    DoctorReport {
        paths: Paths {
            config: config_path.to_string_lossy().into_owned(),
            state: state_path.to_string_lossy().into_owned(),
            theme: theme_path.to_string_lossy().into_owned(),
            plugins_dir: plugins_dir.to_string_lossy().into_owned(),
        },
        config: config_status,
        state: state_status,
        theme: theme_status,
        agents,
        plugins,
    }
}

fn render_text(r: &DoctorReport) {
    println!("Paths:");
    println!("  config       {}", r.paths.config);
    println!("  state        {}", r.paths.state);
    println!("  theme        {}", r.paths.theme);
    println!("  plugins dir  {}", r.paths.plugins_dir);
    println!();
    println!(
        "Config: exists={}  parse_ok={}",
        r.config.exists, r.config.parse_ok
    );
    if let Some(err) = &r.config.error {
        println!("  error: {err}");
    }
    println!(
        "State:  exists={}  active_profile={}",
        r.state.exists,
        r.state.active_profile.as_deref().unwrap_or("<none>")
    );
    println!(
        "Theme:  exists={}  parse_ok={}",
        r.theme.exists, r.theme.parse_ok
    );
    if let Some(err) = &r.theme.error {
        println!("  error: {err}");
    }
    println!();
    println!("Agents:");
    if r.agents.is_empty() {
        println!("  (none configured)");
    } else {
        for a in &r.agents {
            println!(
                "  {:<28} kind={:<12} exists={:<3}  path={}",
                a.name,
                a.kind,
                if a.exists { "yes" } else { "no" },
                a.config_path
            );
        }
    }
    println!();
    println!(
        "Plugins: config-only={}, discovered={}",
        r.plugins.configured, r.plugins.discovered
    );
    for n in &r.plugins.names {
        println!("  - {n}");
    }
}
