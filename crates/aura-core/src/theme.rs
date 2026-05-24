//! User-customizable theme. Loaded from `~/.config/aura/theme.toml` and
//! applied as an override layer on top of the built-in defaults. See
//! `.design/customization.md` for the user-facing schema and
//! `.agent/workflows/customizable-themes.md` for the implementation plan.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::{parse_hex_color, AgentConfig, AgentKind, PluginConfig};

// ── Public surface ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Theme {
    pub colors: ThemeColors,
    pub typography: ThemeTypography,
    pub spinner: ThemeSpinner,
    /// Per-agent overrides keyed by `AgentConfig.name`.
    pub agents: HashMap<String, AgentTheme>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeColors {
    pub bg: u32,
    pub surface: u32,
    pub surface_hi: u32,
    pub border: u32,
    pub text: u32,
    pub text_dim: u32,
    pub accent: u32,
    pub accent_dim: u32,
    pub error: u32,
    pub on_accent: u32,
    /// Used by the quota-fallback chip and similar advisory surfaces.
    pub warning: u32,
    /// Used in place of an agent's brand accent when its relative luminance
    /// exceeds 0.85 (i.e. would wash out against `bg`).
    pub agent_fallback: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeTypography {
    pub font_family: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    Braille,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeSpinner {
    pub style: SpinnerStyle,
    pub color: u32,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AgentTheme {
    pub accent: Option<u32>,
}

// ── Defaults ─────────────────────────────────────────────────────────────────

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            bg: 0x0e0e10,
            surface: 0x1a1a1f,
            surface_hi: 0x252530,
            border: 0x2d2d36,
            text: 0xe6e6ee,
            text_dim: 0x8a8a9a,
            accent: 0x8b5cf6,
            accent_dim: 0x4c1d95,
            error: 0xff6b6b,
            on_accent: 0xffffff,
            warning: 0xe0a96d,
            agent_fallback: 0xb8b8c0,
        }
    }
}

impl Default for ThemeTypography {
    fn default() -> Self {
        Self {
            font_family: "monospace".to_string(),
        }
    }
}

impl Default for ThemeSpinner {
    fn default() -> Self {
        let colors = ThemeColors::default();
        Self {
            style: SpinnerStyle::Braille,
            color: colors.accent,
            interval_ms: 80,
        }
    }
}

// ── Loading ──────────────────────────────────────────────────────────────────

impl Theme {
    /// Default on-disk location: `$XDG_CONFIG_HOME/aura/theme.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("aura")
            .join("theme.toml")
    }

    /// Read the theme file at `path` and merge it on top of `Theme::default()`.
    /// Returns the default theme unchanged if `path` does not exist; any other
    /// failure (read or parse) is bubbled up so the caller can decide whether
    /// to log and continue or surface to the UI.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("read theme file {}", path.display()))?;
        Self::from_toml(&content).with_context(|| format!("parse theme file {}", path.display()))
    }

    /// Parse a TOML string into a `Theme`. Unknown keys are accepted and
    /// reported on stderr (forward-compat over strictness — the spec's design
    /// goal). Invalid hex strings fall through to the default for that field.
    pub fn from_toml(content: &str) -> Result<Self> {
        // First pass over the raw `toml::Value` lets us warn on unknown
        // keys without making serde reject the document.
        if let Ok(value) = content.parse::<toml::Value>() {
            warn_unknown_keys(&value);
        }
        let file: ThemeFile = toml::from_str(content).context("parse theme TOML")?;
        Ok(file.into_theme())
    }

    /// A canned TOML document matching `Theme::default()`. Used by the
    /// settings modal to seed a new `theme.toml` when the user clicks
    /// "Themes" for the first time, so they have something to edit instead
    /// of a blank file.
    pub const DEFAULT_TOML: &'static str = include_str!("theme_default.toml");
}

// ── Color math + lookups (methods on Theme) ──────────────────────────────────

impl Theme {
    /// Brand accent for an agent, with the following precedence:
    ///
    /// 1. `theme.agents[name].accent` — `theme.toml` `[agents."Name"]` block.
    /// 2. `AgentConfig.color` — the `[[agents]] color` field in `config.toml`.
    /// 3. Per-kind brand color (Claude orange, OpenAI white, Gemini blue).
    ///
    /// The luminance fallback applies after all of the above: any resolved
    /// color whose relative luminance exceeds 0.85 is replaced with
    /// `theme.colors.agent_fallback` so it stays legible on the dark surface.
    ///
    /// When both `theme.toml` and `config.toml` set an override for the
    /// same agent, `theme.toml` wins — per `.design/customization.md`.
    pub fn agent_accent(&self, agent: &AgentConfig) -> u32 {
        let resolved = self
            .agents
            .get(&agent.name)
            .and_then(|a| a.accent)
            .or_else(|| agent.color.as_deref().and_then(parse_hex_color))
            .unwrap_or_else(|| agent_kind_default_color(agent.kind));
        self.with_luminance_fallback(resolved)
    }

    /// Brand accent for a plugin. Precedence: `PluginConfig.color` →
    /// `theme.colors.accent`. The same luminance fallback as `agent_accent`
    /// applies.
    pub fn plugin_accent(&self, plugin: &PluginConfig) -> u32 {
        let resolved = plugin
            .color
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or(self.colors.accent);
        self.with_luminance_fallback(resolved)
    }

    /// Pick a legible text color to render on top of `bg`. White over light
    /// pastels (e.g. the agent-fallback grey) reads as a smudge — flip to
    /// the dark app background instead when the accent is too light.
    pub fn on_accent_text(&self, bg: u32) -> u32 {
        if Self::relative_luminance(bg) > 0.55 {
            self.colors.bg
        } else {
            self.colors.on_accent
        }
    }

    fn with_luminance_fallback(&self, color: u32) -> u32 {
        if Self::relative_luminance(color) > 0.85 {
            self.colors.agent_fallback
        } else {
            color
        }
    }

    /// WCAG relative luminance, 0.0–1.0.
    pub fn relative_luminance(rgb_hex: u32) -> f64 {
        let r = ((rgb_hex >> 16) & 0xff) as f64 / 255.0;
        let g = ((rgb_hex >> 8) & 0xff) as f64 / 255.0;
        let b = (rgb_hex & 0xff) as f64 / 255.0;
        let lin = |c: f64| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// Blend `a` toward `b` by `t` (0.0 = pure `a`, 1.0 = pure `b`).
    pub fn blend(a: u32, b: u32, t: f64) -> u32 {
        let ch = |shift: u32| {
            let av = ((a >> shift) & 0xff) as f64;
            let bv = ((b >> shift) & 0xff) as f64;
            ((av * (1.0 - t) + bv * t).round() as u32) & 0xff
        };
        (ch(16) << 16) | (ch(8) << 8) | ch(0)
    }
}

/// Default brand tint for an agent kind, used as the final fallback in
/// `Theme::agent_accent`. These aren't in `[colors]` because they're brand
/// constants — overriding them belongs in the per-agent `[agents."Name"]`
/// block, not in a global token list.
pub fn agent_kind_default_color(kind: AgentKind) -> u32 {
    match kind {
        AgentKind::ClaudeCode => 0xd97757,
        AgentKind::Codex => 0xffffff,
        AgentKind::Gemini => 0x4285f4,
    }
}

// ── Wire format (TOML schema) ────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
struct ThemeFile {
    #[serde(default)]
    colors: ThemeColorsFile,
    #[serde(default)]
    typography: ThemeTypographyFile,
    #[serde(default)]
    spinner: ThemeSpinnerFile,
    #[serde(default)]
    agents: HashMap<String, AgentThemeFile>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ThemeColorsFile {
    bg: Option<String>,
    surface: Option<String>,
    surface_hi: Option<String>,
    border: Option<String>,
    text: Option<String>,
    text_dim: Option<String>,
    accent: Option<String>,
    accent_dim: Option<String>,
    error: Option<String>,
    on_accent: Option<String>,
    warning: Option<String>,
    agent_fallback: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ThemeTypographyFile {
    font_family: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ThemeSpinnerFile {
    style: Option<String>,
    color: Option<String>,
    interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AgentThemeFile {
    accent: Option<String>,
}

impl ThemeFile {
    fn into_theme(self) -> Theme {
        let mut colors = ThemeColors::default();
        apply_hex(&mut colors.bg, &self.colors.bg, "colors.bg");
        apply_hex(&mut colors.surface, &self.colors.surface, "colors.surface");
        apply_hex(
            &mut colors.surface_hi,
            &self.colors.surface_hi,
            "colors.surface_hi",
        );
        apply_hex(&mut colors.border, &self.colors.border, "colors.border");
        apply_hex(&mut colors.text, &self.colors.text, "colors.text");
        apply_hex(
            &mut colors.text_dim,
            &self.colors.text_dim,
            "colors.text_dim",
        );
        apply_hex(&mut colors.accent, &self.colors.accent, "colors.accent");
        apply_hex(
            &mut colors.accent_dim,
            &self.colors.accent_dim,
            "colors.accent_dim",
        );
        apply_hex(&mut colors.error, &self.colors.error, "colors.error");
        apply_hex(
            &mut colors.on_accent,
            &self.colors.on_accent,
            "colors.on_accent",
        );
        apply_hex(&mut colors.warning, &self.colors.warning, "colors.warning");
        apply_hex(
            &mut colors.agent_fallback,
            &self.colors.agent_fallback,
            "colors.agent_fallback",
        );

        let mut typography = ThemeTypography::default();
        if let Some(family) = self.typography.font_family {
            typography.font_family = family;
        }

        // Spinner color defaults to the (possibly-overridden) accent.
        let mut spinner = ThemeSpinner {
            color: colors.accent,
            ..ThemeSpinner::default()
        };
        if let Some(style) = self.spinner.style.as_deref() {
            match style {
                "braille" => spinner.style = SpinnerStyle::Braille,
                "dot" => spinner.style = SpinnerStyle::Dot,
                other => eprintln!(
                    "aura: theme.toml: unknown spinner.style {other:?}; \
                     using {:?}",
                    spinner.style
                ),
            }
        }
        apply_hex(&mut spinner.color, &self.spinner.color, "spinner.color");
        if let Some(ms) = self.spinner.interval_ms {
            spinner.interval_ms = ms;
        }

        let agents = self
            .agents
            .into_iter()
            .map(|(name, raw)| {
                let mut at = AgentTheme::default();
                if let Some(s) = raw.accent.as_deref() {
                    match parse_hex_color(s) {
                        Some(v) => at.accent = Some(v),
                        None => eprintln!(
                            "aura: theme.toml: agents[{name:?}].accent {s:?} \
                             is not a 3- or 6-digit hex color; using the default"
                        ),
                    }
                }
                (name, at)
            })
            .collect();

        Theme {
            colors,
            typography,
            spinner,
            agents,
        }
    }
}

fn apply_hex(target: &mut u32, raw: &Option<String>, label: &str) {
    let Some(s) = raw else {
        return;
    };
    match parse_hex_color(s) {
        Some(v) => *target = v,
        None => eprintln!(
            "aura: theme.toml: {label} = {s:?} is not a 3- or 6-digit hex color; \
             using the default"
        ),
    }
}

// ── Unknown-key warnings ─────────────────────────────────────────────────────

fn warn_unknown_keys(value: &toml::Value) {
    let toml::Value::Table(root) = value else {
        return;
    };
    let known_top: HashSet<&str> = ["colors", "typography", "spinner", "agents"]
        .into_iter()
        .collect();
    for (k, v) in root {
        if !known_top.contains(k.as_str()) {
            eprintln!("aura: theme.toml: unknown top-level key {k:?}; ignoring");
            continue;
        }
        match k.as_str() {
            "colors" => warn_subtable_keys(
                k,
                v,
                &[
                    "bg",
                    "surface",
                    "surface_hi",
                    "border",
                    "text",
                    "text_dim",
                    "accent",
                    "accent_dim",
                    "error",
                    "on_accent",
                    "warning",
                    "agent_fallback",
                ],
            ),
            "typography" => warn_subtable_keys(k, v, &["font_family"]),
            "spinner" => warn_subtable_keys(k, v, &["style", "color", "interval_ms"]),
            "agents" => {
                if let toml::Value::Table(t) = v {
                    for (name, agent_val) in t {
                        warn_subtable_keys(&format!("agents.{name:?}"), agent_val, &["accent"]);
                    }
                }
            }
            _ => {}
        }
    }
}

fn warn_subtable_keys(section: &str, value: &toml::Value, known: &[&str]) {
    let toml::Value::Table(t) = value else {
        return;
    };
    let known: HashSet<&str> = known.iter().copied().collect();
    for k in t.keys() {
        if !known.contains(k.as_str()) {
            eprintln!("aura: theme.toml: unknown key {section}.{k}; ignoring");
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_legacy_constants() {
        let t = Theme::default();
        // These are the exact values the binary used to hard-code in
        // `crates/aura/src/app.rs` (see `.design/customization.md`).
        assert_eq!(t.colors.bg, 0x0e0e10);
        assert_eq!(t.colors.surface, 0x1a1a1f);
        assert_eq!(t.colors.surface_hi, 0x252530);
        assert_eq!(t.colors.border, 0x2d2d36);
        assert_eq!(t.colors.text, 0xe6e6ee);
        assert_eq!(t.colors.text_dim, 0x8a8a9a);
        assert_eq!(t.colors.accent, 0x8b5cf6);
        assert_eq!(t.colors.warning, 0xe0a96d);
        assert_eq!(t.colors.agent_fallback, 0xb8b8c0);
        assert_eq!(t.colors.error, 0xff6b6b);
        assert_eq!(t.colors.on_accent, 0xffffff);
    }

    #[test]
    fn partial_override_leaves_other_keys_at_default() {
        let t = Theme::from_toml(
            r##"
[colors]
bg = "#123456"
"##,
        )
        .unwrap();
        assert_eq!(t.colors.bg, 0x123456);
        // Untouched defaults survive.
        assert_eq!(t.colors.surface, 0x1a1a1f);
        assert_eq!(t.colors.accent, 0x8b5cf6);
    }

    #[test]
    fn bad_hex_falls_through_without_panic() {
        let t = Theme::from_toml(
            r##"
[colors]
bg = "not a color"
accent = "#xyz"
"##,
        )
        .unwrap();
        // Both fields fall through to default.
        assert_eq!(t.colors.bg, 0x0e0e10);
        assert_eq!(t.colors.accent, 0x8b5cf6);
    }

    #[test]
    fn alpha_hex_falls_through_to_default() {
        // 4- and 8-digit (with alpha) hex isn't supported by parse_hex_color;
        // we expect it to fall through to the default and not crash.
        let t = Theme::from_toml(
            r##"
[colors]
bg = "#0e0e10ff"
"##,
        )
        .unwrap();
        assert_eq!(t.colors.bg, 0x0e0e10);
    }

    #[test]
    fn per_agent_map_parses_quoted_keys_with_spaces() {
        let t = Theme::from_toml(
            r##"
[agents."Claude Code (Personal)"]
accent = "#d97757"

[agents."Codex"]
accent = "#ffffff"
"##,
        )
        .unwrap();
        assert_eq!(
            t.agents
                .get("Claude Code (Personal)")
                .and_then(|a| a.accent),
            Some(0xd97757)
        );
        assert_eq!(t.agents.get("Codex").and_then(|a| a.accent), Some(0xffffff));
    }

    #[test]
    fn unknown_keys_are_accepted_silently_at_the_type_level() {
        // The warn path lives in `warn_unknown_keys` and writes to stderr;
        // we just need to confirm the parse itself doesn't error.
        let t = Theme::from_toml(
            r##"
[colors]
bg = "#000000"
made_up = "#ffffff"

[novel_section]
key = 1
"##,
        )
        .unwrap();
        assert_eq!(t.colors.bg, 0x000000);
    }

    #[test]
    fn spinner_style_round_trip() {
        let t = Theme::from_toml(
            r##"
[spinner]
style = "dot"
color = "#aabbcc"
interval_ms = 120
"##,
        )
        .unwrap();
        assert_eq!(t.spinner.style, SpinnerStyle::Dot);
        assert_eq!(t.spinner.color, 0xaabbcc);
        assert_eq!(t.spinner.interval_ms, 120);
    }

    #[test]
    fn spinner_color_defaults_to_overridden_accent() {
        let t = Theme::from_toml(
            r##"
[colors]
accent = "#112233"
"##,
        )
        .unwrap();
        // No explicit spinner.color → inherits the (overridden) accent.
        assert_eq!(t.spinner.color, 0x112233);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        let t = Theme::load(&path).unwrap();
        assert_eq!(t, Theme::default());
    }

    #[test]
    fn agent_accent_precedence_theme_beats_config() {
        let mut t = Theme::default();
        t.agents.insert(
            "Claude Code".to_string(),
            AgentTheme {
                accent: Some(0x111111),
            },
        );
        let agent = AgentConfig {
            name: "Claude Code".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: None,
            // config.toml override should LOSE to theme.toml override.
            color: Some("#222222".to_string()),
        };
        assert_eq!(t.agent_accent(&agent), 0x111111);
    }

    #[test]
    fn agent_accent_falls_back_to_config_color_then_kind() {
        let t = Theme::default();
        // No theme override, but config.toml has a color.
        let agent_with_color = AgentConfig {
            name: "Claude Code".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: None,
            color: Some("#333333".to_string()),
        };
        assert_eq!(t.agent_accent(&agent_with_color), 0x333333);

        // Neither override → per-kind brand color.
        let agent_bare = AgentConfig {
            name: "Claude Code".to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: None,
            color: None,
        };
        assert_eq!(t.agent_accent(&agent_bare), 0xd97757);
    }

    #[test]
    fn agent_accent_applies_luminance_fallback() {
        let t = Theme::default();
        // OpenAI's brand white trips the >0.85 luminance gate and falls
        // back to the neutral grey instead of washing out the UI.
        let codex = AgentConfig {
            name: "Codex".to_string(),
            kind: AgentKind::Codex,
            config_path: None,
            color: None,
        };
        assert_eq!(t.agent_accent(&codex), t.colors.agent_fallback);
    }

    #[test]
    fn plugin_accent_uses_global_accent_when_unset() {
        let t = Theme::default();
        let plugin = PluginConfig {
            name: "p".to_string(),
            command: "x".to_string(),
            color: None,
            icon: None,
        };
        assert_eq!(t.plugin_accent(&plugin), t.colors.accent);
    }

    #[test]
    fn on_accent_text_flips_for_light_backgrounds() {
        let t = Theme::default();
        // White is bright → text should be the dark app bg.
        assert_eq!(t.on_accent_text(0xffffff), t.colors.bg);
        // Purple accent is dark → text stays on_accent (white).
        assert_eq!(t.on_accent_text(0x8b5cf6), t.colors.on_accent);
    }
}
