use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Agent ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub kind: AgentKind,
    /// Path to the agent's config directory. Falls back to the agent's default
    /// when absent (e.g. `~/.claude` for `claude-code`).
    pub config_path: Option<String>,
    /// Optional override for the agent's accent color. Hex string with a
    /// leading `#` (3- or 6-digit). When absent, the per-kind default applies.
    #[serde(default)]
    pub color: Option<String>,
}

impl AgentConfig {
    /// Resolved config directory for this agent. Expands a leading `~`.
    pub fn resolved_config_path(&self) -> PathBuf {
        match &self.config_path {
            Some(p) => expand_tilde(p),
            None => match self.kind {
                AgentKind::ClaudeCode => dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/"))
                    .join(".claude"),
                AgentKind::Codex => dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/"))
                    .join(".codex"),
            },
        }
    }
}

// ── Plugin ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    /// Binary name on `$PATH` or absolute path.
    pub command: String,
    /// Optional accent color for the plugin pill / active highlights. Hex
    /// string with a leading `#`. When absent, the global accent applies.
    #[serde(default)]
    pub color: Option<String>,
    /// Optional path to an SVG icon. Resolution order:
    /// 1. Embedded asset name (`"icons/foo.svg"` matching a baked-in file)
    /// 2. Absolute path on disk
    /// 3. Home-relative path beginning with `~/`
    ///
    /// When absent, a generic `blocks` glyph is used.
    #[serde(default)]
    pub icon: Option<String>,
}

// ── Display ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplayConfig {
    /// Which period to show by default: `"all"`, `"7d"`, or `"30d"`.
    pub default_period: String,
    /// Modal anchor relative to the tray icon: `"auto"`, `"top"`, `"bottom"`.
    pub anchor: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_period: "all".to_string(),
            anchor: "auto".to_string(),
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    #[serde(default)]
    pub display: DisplayConfig,
}

impl AppConfig {
    /// Default on-disk location: `$XDG_CONFIG_HOME/aura/config.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("aura")
            .join("config.toml")
    }

    /// Load from `path`. If the file does not exist, write a default config
    /// there and return it.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            let cfg = Self::default_config();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create config dir {}", parent.display()))?;
            }
            let toml = toml::to_string_pretty(&cfg).context("serialize default config")?;
            fs::write(path, toml)
                .with_context(|| format!("write default config to {}", path.display()))?;
            return Ok(cfg);
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("read config file {}", path.display()))?;
        toml::from_str(&content).with_context(|| format!("parse config file {}", path.display()))
    }

    /// Sensible out-of-the-box config: one Claude Code profile + RTK plugin.
    pub fn default_config() -> Self {
        Self {
            agents: vec![
                AgentConfig {
                    name: "Claude Code (Personal)".to_string(),
                    kind: AgentKind::ClaudeCode,
                    config_path: None,
                    color: None,
                },
                AgentConfig {
                    name: "Claude Code (Enterprise)".to_string(),
                    kind: AgentKind::ClaudeCode,
                    config_path: Some("~/.claude-enterprise".to_string()),
                    color: None,
                },
                AgentConfig {
                    name: "Codex".to_string(),
                    kind: AgentKind::Codex,
                    config_path: None,
                    color: None,
                },
            ],
            plugins: vec![PluginConfig {
                name: "RTK Gains".to_string(),
                command: "aura-plugin-rtk".to_string(),
                color: Some("#f59e0b".to_string()),
                icon: Some("icons/rtk.svg".to_string()),
            }],
            display: DisplayConfig::default(),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a hex color string like `"#rrggbb"`, `"#rgb"`, `"rrggbb"`, or
/// `"rgb"` into a `0x00rrggbb` u32. Returns `None` if the string isn't a
/// recognized hex shape — callers should fall back to a default.
pub fn parse_hex_color(s: &str) -> Option<u32> {
    let raw = s.trim().trim_start_matches('#');
    let expanded = match raw.len() {
        3 => {
            let chars: Vec<char> = raw.chars().collect();
            format!("{0}{0}{1}{1}{2}{2}", chars[0], chars[1], chars[2])
        }
        6 => raw.to_string(),
        _ => return None,
    };
    u32::from_str_radix(&expanded, 16).ok()
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(rest)
    } else if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else {
        PathBuf::from(path)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = AppConfig::default_config();
        let toml = toml::to_string_pretty(&cfg).unwrap();
        let parsed: AppConfig = toml::from_str(&toml).unwrap();

        assert_eq!(parsed.agents.len(), cfg.agents.len());
        assert_eq!(parsed.agents[0].name, cfg.agents[0].name);
        assert_eq!(parsed.plugins.len(), cfg.plugins.len());
        assert_eq!(parsed.display, cfg.display);
    }

    #[test]
    fn load_creates_default_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");

        // File and parent dir don't exist yet.
        assert!(!path.exists());

        let cfg = AppConfig::load(&path).unwrap();

        // File should now exist.
        assert!(path.exists());
        // Should have the three default profiles.
        assert_eq!(cfg.agents.len(), 3);
        assert_eq!(cfg.agents[0].name, "Claude Code (Personal)");
        assert_eq!(cfg.agents[2].kind, AgentKind::Codex);
    }

    #[test]
    fn load_reads_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        fs::write(
            &path,
            r#"
[[agents]]
name = "My Agent"
kind = "claude-code"
"#,
        )
        .unwrap();

        let cfg = AppConfig::load(&path).unwrap();

        assert_eq!(cfg.agents.len(), 1);
        assert_eq!(cfg.agents[0].name, "My Agent");
        assert_eq!(cfg.agents[0].kind, AgentKind::ClaudeCode);
    }

    #[test]
    fn display_config_defaults_on_missing_fields() {
        let cfg: AppConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.display.default_period, "all");
        assert_eq!(cfg.display.anchor, "auto");
    }

    #[test]
    fn resolved_config_path_expands_tilde() {
        let agent = AgentConfig {
            name: "test".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: Some("~/.claude-test".to_string()),
            color: None,
        };
        let resolved = agent.resolved_config_path();
        assert!(resolved.to_string_lossy().contains(".claude-test"));
        assert!(!resolved.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn resolved_config_path_defaults_for_claude_code() {
        let agent = AgentConfig {
            name: "test".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: None,
            color: None,
        };
        let resolved = agent.resolved_config_path();
        assert!(resolved.ends_with(".claude"));
    }
}
