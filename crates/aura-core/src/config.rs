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
    Gemini,
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
                AgentKind::ClaudeCode => home_dir().join(".claude"),
                AgentKind::Codex => home_dir().join(".codex"),
                AgentKind::Gemini => home_dir().join(".gemini"),
            },
        }
    }
}

/// Resolve the user's home directory.
///
/// Prefers `HOME` (and `USERPROFILE` on Windows) over `dirs::home_dir()` so
/// tests can redirect agent detection at a tempdir. In production this
/// matches `dirs::home_dir()` on every platform we ship: `HOME` is always set
/// on Unix, and `USERPROFILE` mirrors `FOLDERID_Profile` on Windows.
fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
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
    /// Display order for plugin pills. Plugins whose display `name`
    /// appears here render in the listed order; anything not named
    /// keeps its natural order (config-then-discovered-alphabetical)
    /// and appends after the explicitly-ordered prefix. Match is
    /// case-insensitive.
    #[serde(default)]
    pub plugin_order: Vec<String>,
    /// Whether the modal appears in the OS's "where are my windows"
    /// surfaces — Cmd+Tab + Dock on macOS, Alt+Tab + taskbar on Windows,
    /// panel + window switcher on Linux. Default `false` so Aura behaves
    /// like a true tray-indicator (the icon by the clock is the entire
    /// UI surface). Set `true` if you want to alt-tab to the modal.
    ///
    /// On macOS this also drives the process-wide NSApplication
    /// activation policy (Accessory vs Regular); on Windows / Linux it
    /// only affects the modal's window kind, picked at open time.
    pub show_in_app_switcher: bool,
    /// Auto-close the modal when it loses focus (click outside, switch app).
    /// Default true — matches typical menu-bar / tray-popup behaviour.
    /// Set to false if you'd rather the modal stay open until you click the
    /// tray icon again (useful when copy-pasting from the modal into
    /// another window).
    pub dismiss_on_focus_loss: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_period: "all".to_string(),
            anchor: "auto".to_string(),
            plugin_order: Vec::new(),
            show_in_app_switcher: false,
            dismiss_on_focus_loss: true,
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

    /// Serialize and write `self` to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        let toml = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(path, toml).with_context(|| format!("write config to {}", path.display()))
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

    /// Sensible out-of-the-box config: detected agents only, no bundled
    /// plugins. Plugins are installed separately via `aura plugin add` or
    /// by dropping a binary into `~/.config/aura/plugins/`.
    pub fn default_config() -> Self {
        Self {
            agents: known_agent_profiles(),
            plugins: Vec::new(),
            display: DisplayConfig::default(),
        }
    }

    /// Same as [`Self::load`], plus any executable plugins discovered in
    /// `<config-dir>/plugins/` are merged into `self.plugins` (config-listed
    /// entries win on display-name collision). Discovered plugins are kept
    /// in-memory only — the on-disk config is not rewritten, so removing a
    /// binary from the plugins dir makes it disappear cleanly on the next
    /// launch. Finally, [`Self::apply_plugin_order`] reorders the merged
    /// list to honour `display.plugin_order`.
    pub fn load_with_discovery(path: &Path) -> Result<Self> {
        let mut cfg = Self::load(path)?;
        let plugins_dir = crate::plugin::plugins_dir_for_config(path);
        let discovered = crate::plugin::discover_plugins(&plugins_dir);
        cfg.merge_plugins(discovered);
        cfg.apply_plugin_order();
        Ok(cfg)
    }

    /// Reorder `self.plugins` according to `self.display.plugin_order`.
    /// Plugins whose display name appears in the order list move to the
    /// front in the listed order; anything not named keeps its relative
    /// position and is appended after the ordered prefix. Match is
    /// case-insensitive. No-op when `plugin_order` is empty.
    pub fn apply_plugin_order(&mut self) {
        if self.display.plugin_order.is_empty() {
            return;
        }
        let mut remaining = std::mem::take(&mut self.plugins);
        let mut ordered: Vec<PluginConfig> = Vec::with_capacity(remaining.len());
        for wanted in &self.display.plugin_order {
            if let Some(pos) = remaining
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(wanted))
            {
                ordered.push(remaining.remove(pos));
            }
        }
        ordered.extend(remaining);
        self.plugins = ordered;
    }

    /// Append `additions` to `self.plugins`, skipping any whose display
    /// `name` (case-insensitive) is already present. Existing entries in
    /// the on-disk config always win — this lets the user override the
    /// color or icon of a discovered plugin by adding a `[[plugins]]`
    /// block with the same name.
    pub fn merge_plugins(&mut self, additions: Vec<PluginConfig>) -> usize {
        let mut added = 0;
        for candidate in additions {
            let already = self
                .plugins
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(&candidate.name));
            if !already {
                self.plugins.push(candidate);
                added += 1;
            }
        }
        added
    }

    /// Append `additions` to `self.agents`, skipping any whose
    /// `(kind, resolved_config_path)` is already present. Returns the number
    /// of agents actually appended.
    pub fn merge_agents(&mut self, additions: Vec<AgentConfig>) -> usize {
        let mut added = 0;
        for candidate in additions {
            let candidate_path = candidate.resolved_config_path();
            let already = self
                .agents
                .iter()
                .any(|a| a.kind == candidate.kind && a.resolved_config_path() == candidate_path);
            if !already {
                self.agents.push(candidate);
                added += 1;
            }
        }
        added
    }

    /// Detect installed agents and either create a fresh config containing
    /// only the detected ones (plus the default plugins/display) or merge any
    /// newly-detected agents into an existing config without disturbing the
    /// user's other edits. Returns a report describing what changed.
    pub fn run_setup(path: &Path) -> Result<SetupReport> {
        let known = known_agent_profiles();
        let detected_keys: Vec<(AgentKind, PathBuf)> = known
            .iter()
            .filter(|a| a.resolved_config_path().is_dir())
            .map(|a| (a.kind, a.resolved_config_path()))
            .collect();

        let (mut config, created) = if path.exists() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("read config file {}", path.display()))?;
            let cfg: AppConfig = toml::from_str(&content)
                .with_context(|| format!("parse config file {}", path.display()))?;
            (cfg, false)
        } else {
            // Fresh config: only the detected agents get added below.
            let mut cfg = Self::default_config();
            cfg.agents.clear();
            (cfg, true)
        };

        let mut report_entries = Vec::with_capacity(known.len());
        for profile in known {
            let key = (profile.kind, profile.resolved_config_path());
            if !detected_keys.contains(&key) {
                report_entries.push((profile, AgentStatus::NotInstalled));
                continue;
            }
            let already = config
                .agents
                .iter()
                .any(|a| a.kind == key.0 && a.resolved_config_path() == key.1);
            if already {
                report_entries.push((profile, AgentStatus::AlreadyConfigured));
            } else {
                config.agents.push(profile.clone());
                report_entries.push((profile, AgentStatus::Added));
            }
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        let toml = toml::to_string_pretty(&config).context("serialize config")?;
        fs::write(path, toml).with_context(|| format!("write config to {}", path.display()))?;

        Ok(SetupReport {
            created,
            agents: report_entries,
        })
    }
}

// ── Setup ────────────────────────────────────────────────────────────────────

/// Agent profiles Aura knows how to detect. Each entry is the canonical
/// `AgentConfig` Aura will add to a user's config when the matching agent is
/// found on disk.
pub fn known_agent_profiles() -> Vec<AgentConfig> {
    vec![
        AgentConfig {
            name: "Claude Code".to_string(),
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
        AgentConfig {
            name: "Gemini".to_string(),
            kind: AgentKind::Gemini,
            config_path: None,
            color: None,
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Detected on disk and newly written into the config.
    Added,
    /// Detected on disk; an equivalent entry was already in the config.
    AlreadyConfigured,
    /// Not detected on disk; nothing changed.
    NotInstalled,
}

#[derive(Debug)]
pub struct SetupReport {
    /// True when the config file did not exist and was just created.
    pub created: bool,
    /// One entry per known agent profile, in `known_agent_profiles()` order.
    pub agents: Vec<(AgentConfig, AgentStatus)>,
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
        home_dir().join(rest)
    } else if path == "~" {
        home_dir()
    } else {
        PathBuf::from(path)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn plug(name: &str) -> PluginConfig {
        PluginConfig {
            name: name.to_string(),
            command: format!("aura-plugin-{}", name.to_ascii_lowercase()),
            color: None,
            icon: None,
        }
    }

    #[test]
    fn apply_plugin_order_moves_named_to_front_preserves_rest() {
        let mut cfg = AppConfig {
            agents: vec![],
            plugins: vec![plug("Alpha"), plug("Beta"), plug("Gamma"), plug("Delta")],
            display: DisplayConfig {
                plugin_order: vec!["Gamma".into(), "Alpha".into()],
                ..DisplayConfig::default()
            },
        };
        cfg.apply_plugin_order();
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        // Named entries first (in listed order), then unlisted in original order.
        assert_eq!(names, vec!["Gamma", "Alpha", "Beta", "Delta"]);
    }

    #[test]
    fn apply_plugin_order_case_insensitive_and_ignores_unknown() {
        let mut cfg = AppConfig {
            agents: vec![],
            plugins: vec![plug("RTK Gains"), plug("Hello")],
            display: DisplayConfig {
                plugin_order: vec!["hello".into(), "Nonexistent".into()],
                ..DisplayConfig::default()
            },
        };
        cfg.apply_plugin_order();
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Hello", "RTK Gains"]);
    }

    #[test]
    fn apply_plugin_order_is_noop_when_empty() {
        let mut cfg = AppConfig {
            agents: vec![],
            plugins: vec![plug("Alpha"), plug("Beta")],
            display: DisplayConfig::default(),
        };
        cfg.apply_plugin_order();
        let names: Vec<&str> = cfg.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
    }

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
        // Should have the four default profiles.
        assert_eq!(cfg.agents.len(), 4);
        assert_eq!(cfg.agents[0].kind, AgentKind::ClaudeCode);
        assert_eq!(cfg.agents[2].kind, AgentKind::Codex);
        assert_eq!(cfg.agents[3].kind, AgentKind::Gemini);
    }

    #[test]
    fn merge_agents_skips_existing_by_kind_and_path() {
        let mut cfg = AppConfig {
            agents: vec![AgentConfig {
                name: "User-renamed Claude".to_string(),
                kind: AgentKind::ClaudeCode,
                config_path: None, // resolves to ~/.claude
                color: Some("#abcdef".to_string()),
            }],
            plugins: vec![],
            display: DisplayConfig::default(),
        };

        let added = cfg.merge_agents(vec![
            AgentConfig {
                name: "Claude Code".to_string(),
                kind: AgentKind::ClaudeCode,
                config_path: None,
                color: None,
            },
            AgentConfig {
                name: "Codex".to_string(),
                kind: AgentKind::Codex,
                config_path: None,
                color: None,
            },
        ]);

        // Claude Code was skipped (same kind + resolved path), Codex was added.
        assert_eq!(added, 1);
        assert_eq!(cfg.agents.len(), 2);
        // User's customization is preserved.
        assert_eq!(cfg.agents[0].name, "User-renamed Claude");
        assert_eq!(cfg.agents[0].color.as_deref(), Some("#abcdef"));
        assert_eq!(cfg.agents[1].kind, AgentKind::Codex);
    }

    #[test]
    fn merge_agents_treats_distinct_paths_as_distinct() {
        let mut cfg = AppConfig {
            agents: vec![AgentConfig {
                name: "Claude Code".to_string(),
                kind: AgentKind::ClaudeCode,
                config_path: None,
                color: None,
            }],
            plugins: vec![],
            display: DisplayConfig::default(),
        };

        let added = cfg.merge_agents(vec![AgentConfig {
            name: "Claude Code (Enterprise)".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: Some("~/.claude-enterprise".to_string()),
            color: None,
        }]);

        assert_eq!(added, 1);
        assert_eq!(cfg.agents.len(), 2);
    }

    #[test]
    fn run_setup_creates_file_with_only_detected_agents() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        // Point HOME at the tempdir so detection is deterministic.
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        // No ~/.claude-enterprise, no ~/.gemini.

        let _guard = HomeGuard::set(&home);

        let report = AppConfig::run_setup(&config_path).unwrap();
        assert!(report.created);

        let detected: Vec<_> = report
            .agents
            .iter()
            .filter(|(_, s)| *s == AgentStatus::Added)
            .map(|(a, _)| a.kind)
            .collect();
        assert_eq!(detected, vec![AgentKind::ClaudeCode, AgentKind::Codex]);

        let cfg = AppConfig::load(&config_path).unwrap();
        assert_eq!(cfg.agents.len(), 2);
        // No plugins ship by default — they're installed separately
        // via `aura plugin add` or by dropping a binary into the user
        // plugins dir.
        assert!(cfg.plugins.is_empty());
    }

    #[test]
    fn run_setup_merges_into_existing_config_preserving_edits() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let home = dir.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();

        // Pre-seed an existing config that only has Claude Code (renamed).
        fs::write(
            &config_path,
            r##"
[[agents]]
name = "Work Claude"
kind = "claude-code"
color = "#123456"

[[plugins]]
name = "RTK Gains"
command = "aura-plugin-rtk"
"##,
        )
        .unwrap();

        let _guard = HomeGuard::set(&home);

        let report = AppConfig::run_setup(&config_path).unwrap();
        assert!(!report.created);

        let cfg = AppConfig::load(&config_path).unwrap();
        // Original entry preserved.
        assert_eq!(cfg.agents[0].name, "Work Claude");
        assert_eq!(cfg.agents[0].color.as_deref(), Some("#123456"));
        // Codex + Gemini appended.
        let kinds: Vec<_> = cfg.agents.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Gemini]
        );
        // Plugins untouched.
        assert_eq!(cfg.plugins.len(), 1);
        assert_eq!(cfg.plugins[0].name, "RTK Gains");
    }

    /// Set `HOME` (and `USERPROFILE` on Windows) for the duration of a test
    /// and restore the previous value on drop. Tests that touch this must run
    /// serially — Rust's default test runner already serializes per-test
    /// execution within a single binary by default for `cargo test --`, but
    /// these tests use `--test-threads=1` semantics implicitly via the
    /// process-wide env var; we serialize with a mutex to be safe.
    struct HomeGuard {
        prev_home: Option<std::ffi::OsString>,
        #[cfg(windows)]
        prev_userprofile: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev_home = std::env::var_os("HOME");
            // SAFETY: tests are serialized via LOCK; no other thread reads HOME concurrently.
            unsafe { std::env::set_var("HOME", home) };
            #[cfg(windows)]
            let prev_userprofile = std::env::var_os("USERPROFILE");
            #[cfg(windows)]
            unsafe {
                std::env::set_var("USERPROFILE", home)
            };
            Self {
                prev_home,
                #[cfg(windows)]
                prev_userprofile,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: still holding LOCK via _lock.
            unsafe {
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
            #[cfg(windows)]
            unsafe {
                match &self.prev_userprofile {
                    Some(v) => std::env::set_var("USERPROFILE", v),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }
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
