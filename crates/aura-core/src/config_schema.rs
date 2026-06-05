//! Field registry for [`AppConfig`] — the single source of truth that powers
//! the self-documenting config surfaces:
//!
//! - `aura config describe` (and `--format json`)
//! - the commented `config.toml` template ([`render_commented`])
//! - `aura config get` / `set` ([`get_value`] / [`set_value`])
//! - `aura config wizard`
//!
//! Every settable scalar field under `[display]` / `[update]` has a
//! [`FieldDescriptor`] here. A unit test (`registry_covers_every_field`)
//! serializes a default config and asserts each leaf key is described, so
//! adding a struct field without documenting it breaks the build.
//!
//! The repeatable `[[agents]]` / `[[plugins]]` tables are *documented* via
//! [`agent_fields`] / [`plugin_fields`] but are not get/set targets — they're
//! managed with `aura agents`, `aura plugin`, or `config edit`.

use anyhow::{Context, Result};

use crate::config::AppConfig;

// ── Descriptors ────────────────────────────────────────────────────────────────

/// One settable scalar config field.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct FieldDescriptor {
    /// Dotted key path, e.g. `"display.anchor"`.
    pub key: &'static str,
    /// Human type label: `string`, `string?`, `string[]`, `bool`, `u32?`.
    pub type_label: &'static str,
    /// Constrained value set, or `&[]` when free-form.
    pub allowed: &'static [&'static str],
    /// Default value, rendered for `describe`.
    pub default: &'static str,
    /// One-line summary; also used as the comment above the key in the template.
    pub summary: &'static str,
    /// Full description (lifted from the doc comments in `config.rs`).
    pub description: &'static str,
    /// Example value, used by the template for optional/unset fields.
    pub example: &'static str,
}

/// One field of a repeatable `[[agents]]` / `[[plugins]]` table — documentation
/// only (these are not get/set targets).
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SectionField {
    /// Bare field name, e.g. `"config_path"`.
    pub key: &'static str,
    pub type_label: &'static str,
    pub allowed: &'static [&'static str],
    pub summary: &'static str,
}

/// All settable scalar fields, in template-emission order (`display.*` then
/// `update.*`).
pub fn fields() -> &'static [FieldDescriptor] {
    &[
        FieldDescriptor {
            key: "display.default_period",
            type_label: "string",
            allowed: &["all", "7d", "30d"],
            default: "all",
            summary: "Which usage period tab is selected on open.",
            description: "Which period to show by default: \"all\", \"7d\" (last 7 days), or \
                \"30d\" (last 30 days). Unrecognised values fall back to \"all\".",
            example: "all",
        },
        FieldDescriptor {
            key: "display.anchor",
            type_label: "string",
            allowed: &["none", "bottom", "top"],
            default: "\"none\" (macOS/Linux), \"bottom\" (Windows)",
            summary: "How the modal anchors as it auto-fits its content height.",
            description: "How the modal anchors as it auto-fits its content height. \
                \"none\": open at the platform's natural tray corner and grow downward; \
                never reposition after a resize (safe on Wayland, where the compositor \
                owns placement). \"bottom\": pin the bottom edge above a bottom taskbar so \
                it grows upward (the tray-popup feel). \"top\": pin the top edge below a top \
                panel / menu bar and grow downward. Default is per-OS: \"bottom\" on Windows, \
                \"none\" on macOS and Linux. Unrecognised values (incl. the legacy \"auto\") \
                fall back to the per-OS default.",
            example: "none",
        },
        FieldDescriptor {
            key: "display.plugin_order",
            type_label: "string[]",
            allowed: &[],
            default: "[] (config-then-discovered order)",
            summary: "Display order for plugin pills (comma-separated names on `set`).",
            description: "Display order for plugin pills. Plugins whose display name appears \
                here render in the listed order; anything not named keeps its natural order \
                (config-then-discovered-alphabetical) and appends after the explicitly-ordered \
                prefix. Match is case-insensitive. On `set`, pass a comma-separated list.",
            example: "RTK Gains, Hello",
        },
        FieldDescriptor {
            key: "display.show_in_app_switcher",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Show the modal in Alt+Tab / Cmd+Tab / dock surfaces.",
            description: "Whether the modal appears in the OS's \"where are my windows\" \
                surfaces — Cmd+Tab + Dock on macOS, Alt+Tab + taskbar on Windows, panel + \
                window switcher on Linux. Default false so Aura behaves like a true \
                tray-indicator. Set true if you want to alt-tab to the modal. On macOS this \
                also drives the process-wide NSApplication activation policy.",
            example: "false",
        },
        FieldDescriptor {
            key: "display.dismiss_on_focus_loss",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "true",
            summary: "Auto-close the modal when it loses focus.",
            description: "Auto-close the modal when it loses focus (click outside, switch \
                app). Default true — matches typical menu-bar / tray-popup behaviour. Set \
                false if you'd rather the modal stay open until you click the tray icon again \
                (useful when copy-pasting from the modal into another window).",
            example: "true",
        },
        FieldDescriptor {
            key: "display.window_chrome",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Show the native window title bar.",
            description: "Show the OS-native window chrome (title bar + minimize / maximize / \
                close buttons). Default false: Aura behaves like a tray popup with no title \
                bar. This only controls the title bar — whether the modal auto-resizes to fit \
                its content is the separate display.auto_resize knob.",
            example: "false",
        },
        FieldDescriptor {
            key: "display.auto_resize",
            type_label: "bool?",
            allowed: &["true", "false"],
            default: "unset (auto-fit)",
            summary: "Auto-resize the modal to fit its content height.",
            description: "Whether the modal auto-resizes to fit its content height. On every \
                layout pass a content-fit callback grows / shrinks the window to match (capped \
                at the screen work area and display.max_height). Unset (default) means auto-fit \
                is on. Set false for a fixed-size window. This is not user drag-to-resize (the \
                window manager owns that) — only Aura's content-fit. Independent of \
                window_chrome, so the auto-fit works the same with or without the title bar.",
            example: "true",
        },
        FieldDescriptor {
            key: "display.max_height",
            type_label: "u32?",
            allowed: &[],
            default: "unset (only the screen work-area cap applies)",
            summary: "Upper bound, in logical pixels, on the modal's auto-fit height.",
            description: "Optional upper bound (in logical pixels) on the modal's auto-fit \
                height. The content-fit callback already caps growth at the screen's available \
                work area; this lets you impose a tighter ceiling so the modal never grows \
                past, say, 500 px even on a tall display. Unset means \"only the work-area cap \
                applies\". Ignored when window_chrome is true (auto-fit is off then).",
            example: "500",
        },
        FieldDescriptor {
            key: "display.goblin_mode",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Swap UI copy for the aggressive \"Goblin Mode\" variant.",
            description: "Swap the modal's user-facing copy for an aggressive / unhinged \
                variant (\"Goblin Mode\"). Default false. Toggling reloads on the next refresh \
                — no restart. See docs/goblin-mode.md.",
            example: "false",
        },
        FieldDescriptor {
            key: "fleet.enabled",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Enable the Fleet tab and cross-machine usage sync.",
            description: "Master switch for the Fleet feature. When false (default) the Fleet \
                tab is hidden and the background publish/subscribe task is never started — no \
                network, no keychain access, no cost. When true, paired machines sync \
                end-to-end-encrypted usage heartbeats over the broker. See docs/fleet.md.",
            example: "false",
        },
        FieldDescriptor {
            key: "fleet.broker_url",
            type_label: "string",
            allowed: &[],
            default: "https://ntfy.sh",
            summary: "Base URL of the ntfy pub/sub broker (no trailing slash).",
            description: "Base URL of the ntfy pub/sub broker. Defaults to the free public \
                server https://ntfy.sh. Point this at a self-hosted ntfy instance for full \
                privacy or heavier setups. No trailing slash. The broker only ever sees \
                ciphertext and an opaque, secret-derived topic name.",
            example: "https://ntfy.sh",
        },
        FieldDescriptor {
            key: "fleet.machine_label",
            type_label: "string",
            allowed: &[],
            default: "(empty → system hostname)",
            summary: "Label for this machine in peer rows; empty uses the hostname.",
            description: "Human-friendly label for this machine in the Fleet peer list. Empty \
                (default) means \"use the system hostname\". Two machines with the same hostname \
                are still disambiguated by a random per-install machine id.",
            example: "Pedros-MacBook-Air",
        },
        FieldDescriptor {
            key: "fleet.heartbeat_secs",
            type_label: "u64",
            allowed: &[],
            default: "45",
            summary: "Seconds between encrypted heartbeat publishes.",
            description: "How often (seconds) this machine publishes an encrypted heartbeat to \
                the broker. The public broker's rate limit is far above this; the default keeps \
                well under it. Values below 10 are clamped up at runtime to stay polite.",
            example: "45",
        },
        FieldDescriptor {
            key: "fleet.stale_secs",
            type_label: "u64",
            allowed: &[],
            default: "120",
            summary: "Seconds before a silent peer is treated as stale.",
            description: "A peer with no fresh heartbeat for this many seconds is treated as \
                stale: its row dims and it is excluded from the per-machine share math. The \
                replay window (messages older than 2 × heartbeat_secs are dropped) is derived \
                from heartbeat_secs, not from this value.",
            example: "120",
        },
        FieldDescriptor {
            key: "update.dismissed_version",
            type_label: "string?",
            allowed: &[],
            default: "unset (never dismissed)",
            summary: "Last release version dismissed via the update button's \u{00d7}.",
            description: "The last release version the user dismissed via the \"x\" on the \
                update button. Stored as the bare semver string (\"0.1.18\"). Any newer release \
                re-shows the button. Unset means \"never dismissed\".",
            example: "0.1.18",
        },
        FieldDescriptor {
            key: "update.dismiss_all",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Master mute: never show the update button or check for updates.",
            description: "Master switch. When true, the update button is never rendered and \
                the background check is skipped entirely (so no GitHub request fires). Off by \
                default.",
            example: "false",
        },
        FieldDescriptor {
            key: "insights.enabled",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Show the Insights tab (top projects / sessions / mode mix).",
            description: "Show the Insights tab, which surfaces the top projects and sessions \
                by token spend plus a model-tier / ultracode mode distribution. Off by default \
                — the tab is hidden until you opt in. Mode (ultracode) is inferred from session \
                content and is a heuristic.",
            example: "true",
        },
        FieldDescriptor {
            key: "insights.top_n",
            type_label: "usize",
            allowed: &[],
            default: "5",
            summary: "How many rows each Insights ranked list shows.",
            description: "How many rows the Insights tab's top-projects and top-sessions lists \
                render. Default 5. Capped by the number of projects/sessions actually scanned in \
                the active period.",
            example: "5",
        },
        FieldDescriptor {
            key: "pacing.enabled",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Show the per-session budget gauge on the Forecast tab.",
            description: "Master switch for the budget-pacing gauge (F2). Off by default — \
                opt-in until the active_session_min_tokens heuristic is tuned against real \
                data. When on, the Forecast tab gains a gauge recommending how much of the \
                current 5h session to spend so the weekly window lands at/under 100% at reset.",
            example: "false",
        },
        FieldDescriptor {
            key: "pacing.active_session_min_tokens",
            type_label: "u64",
            allowed: &[],
            default: "50000",
            summary: "Token threshold for a session to count as \"active\".",
            description: "A session counts as \"active\" (real coding, not an idle renewal) \
                when its input+output tokens reach this threshold. Sessions below it are \
                excluded from the learned active-session pattern. Default 50000 — excludes \
                trivial one-message sessions while keeping real coding ones. Tune against \
                your own usage.",
            example: "50000",
        },
        FieldDescriptor {
            key: "pacing.history_days",
            type_label: "u32",
            allowed: &[],
            default: "14",
            summary: "Trailing window, in days, used to learn the active-session pattern.",
            description: "Trailing window, in days, over which the active-session pattern is \
                learned. Active days only contribute (zero-usage days are ignored), and the \
                per-day counts are trimmed-mean averaged so a marathon day doesn't skew the \
                estimate. Default 14.",
            example: "14",
        },
    ]
}

/// Fields of a `[[agents]]` block (documentation only).
pub fn agent_fields() -> &'static [SectionField] {
    &[
        SectionField {
            key: "name",
            type_label: "string",
            allowed: &[],
            summary: "Display name for this agent profile.",
        },
        SectionField {
            key: "kind",
            type_label: "string",
            allowed: &["claude-code", "codex", "gemini"],
            summary: "Which agent this profile reads.",
        },
        SectionField {
            key: "config_path",
            type_label: "string?",
            allowed: &[],
            summary: "Agent config dir; defaults to ~/.claude, ~/.codex, ~/.gemini per kind.",
        },
        SectionField {
            key: "color",
            type_label: "string?",
            allowed: &[],
            summary: "Accent color override, hex like #rrggbb or #rgb.",
        },
    ]
}

/// Fields of a `[[plugins]]` block (documentation only).
pub fn plugin_fields() -> &'static [SectionField] {
    &[
        SectionField {
            key: "name",
            type_label: "string",
            allowed: &[],
            summary: "Display name for the plugin pill.",
        },
        SectionField {
            key: "command",
            type_label: "string",
            allowed: &[],
            summary: "Binary name on $PATH or absolute path.",
        },
        SectionField {
            key: "color",
            type_label: "string?",
            allowed: &[],
            summary: "Accent color override, hex like #rrggbb or #rgb.",
        },
        SectionField {
            key: "icon",
            type_label: "string?",
            allowed: &[],
            summary: "SVG icon: embedded asset name, absolute path, or ~/ path.",
        },
    ]
}

/// Look up a descriptor by its dotted key.
pub fn field(key: &str) -> Option<&'static FieldDescriptor> {
    fields().iter().find(|f| f.key == key)
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SchemaError {
    UnknownKey {
        key: String,
        suggestion: Option<&'static str>,
    },
    InvalidValue {
        key: String,
        value: String,
        allowed: Vec<&'static str>,
    },
    InvalidType {
        key: String,
        expected: &'static str,
        value: String,
    },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::UnknownKey { key, suggestion } => {
                write!(f, "unknown config key `{key}`")?;
                if let Some(s) = suggestion {
                    write!(f, " (did you mean `{s}`?)")?;
                }
                write!(
                    f,
                    "\n  run `aura config describe` to list settable keys; \
                     agents and plugins are managed via `aura agents` / `aura plugin` / `config edit`"
                )
            }
            SchemaError::InvalidValue {
                key,
                value,
                allowed,
            } => write!(
                f,
                "invalid value `{value}` for `{key}`\n  allowed: {}",
                allowed.join(" | ")
            ),
            SchemaError::InvalidType {
                key,
                expected,
                value,
            } => write!(
                f,
                "invalid value `{value}` for `{key}`: expected {expected}"
            ),
        }
    }
}

impl std::error::Error for SchemaError {}

fn unknown_key(key: &str) -> SchemaError {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    // Suggest the nearest key: exact, or one leaf is a substring of the other
    // (catches typos like `anchorr` → `anchor` and bare leaves like `anchor`).
    let suggestion = fields().iter().map(|f| f.key).find(|k| {
        let kl = k.rsplit('.').next().unwrap_or(k);
        *k == key || kl == leaf || kl.contains(leaf) || leaf.contains(kl)
    });
    SchemaError::UnknownKey {
        key: key.to_string(),
        suggestion,
    }
}

// ── Read / write a single field ────────────────────────────────────────────────

/// Read a field's current value as a human-readable string. Optional fields
/// that are unset render as `(unset)`.
pub fn get_value(cfg: &AppConfig, key: &str) -> Result<String, SchemaError> {
    let v = match key {
        "display.default_period" => cfg.display.default_period.clone(),
        "display.anchor" => cfg.display.anchor.clone(),
        "display.plugin_order" => cfg.display.plugin_order.join(", "),
        "display.show_in_app_switcher" => cfg.display.show_in_app_switcher.to_string(),
        "display.dismiss_on_focus_loss" => cfg.display.dismiss_on_focus_loss.to_string(),
        "display.window_chrome" => cfg.display.window_chrome.to_string(),
        "display.auto_resize" => cfg
            .display
            .auto_resize
            .map(|b| b.to_string())
            .unwrap_or_else(|| "(unset)".to_string()),
        "display.max_height" => cfg
            .display
            .max_height
            .map(|n| n.to_string())
            .unwrap_or_else(|| "(unset)".to_string()),
        "display.goblin_mode" => cfg.display.goblin_mode.to_string(),
        "fleet.enabled" => cfg.fleet.enabled.to_string(),
        "fleet.broker_url" => cfg.fleet.broker_url.clone(),
        "fleet.machine_label" => {
            if cfg.fleet.machine_label.is_empty() {
                "(unset)".to_string()
            } else {
                cfg.fleet.machine_label.clone()
            }
        }
        "fleet.heartbeat_secs" => cfg.fleet.heartbeat_secs.to_string(),
        "fleet.stale_secs" => cfg.fleet.stale_secs.to_string(),
        "update.dismissed_version" => cfg
            .update
            .dismissed_version
            .clone()
            .unwrap_or_else(|| "(unset)".to_string()),
        "update.dismiss_all" => cfg.update.dismiss_all.to_string(),
        "insights.enabled" => cfg.insights.enabled.to_string(),
        "insights.top_n" => cfg.insights.top_n.to_string(),
        "pacing.enabled" => cfg.pacing.enabled.to_string(),
        "pacing.active_session_min_tokens" => cfg.pacing.active_session_min_tokens.to_string(),
        "pacing.history_days" => cfg.pacing.history_days.to_string(),
        _ => return Err(unknown_key(key)),
    };
    Ok(v)
}

/// Parse `raw` against the field's type and assign it. Validates enums against
/// `allowed`; clears optional fields when `raw` is empty / `none` / `null`.
pub fn set_value(cfg: &mut AppConfig, key: &str, raw: &str) -> Result<(), SchemaError> {
    let raw = raw.trim();
    match key {
        "display.default_period" => {
            cfg.display.default_period = parse_enum(key, raw, &["all", "7d", "30d"])?
        }
        "display.anchor" => cfg.display.anchor = parse_enum(key, raw, &["none", "bottom", "top"])?,
        "display.plugin_order" => cfg.display.plugin_order = parse_list(raw),
        "display.show_in_app_switcher" => cfg.display.show_in_app_switcher = parse_bool(key, raw)?,
        "display.dismiss_on_focus_loss" => {
            cfg.display.dismiss_on_focus_loss = parse_bool(key, raw)?
        }
        "display.window_chrome" => cfg.display.window_chrome = parse_bool(key, raw)?,
        "display.auto_resize" => cfg.display.auto_resize = parse_opt_bool(key, raw)?,
        "display.max_height" => cfg.display.max_height = parse_opt_u32(key, raw)?,
        "display.goblin_mode" => cfg.display.goblin_mode = parse_bool(key, raw)?,
        "fleet.enabled" => cfg.fleet.enabled = parse_bool(key, raw)?,
        "fleet.broker_url" => cfg.fleet.broker_url = raw.trim_end_matches('/').to_string(),
        "fleet.machine_label" => cfg.fleet.machine_label = raw.to_string(),
        "fleet.heartbeat_secs" => cfg.fleet.heartbeat_secs = parse_u64(key, raw)?,
        "fleet.stale_secs" => cfg.fleet.stale_secs = parse_u64(key, raw)?,
        "update.dismissed_version" => cfg.update.dismissed_version = parse_opt_string(raw),
        "update.dismiss_all" => cfg.update.dismiss_all = parse_bool(key, raw)?,
        "insights.enabled" => cfg.insights.enabled = parse_bool(key, raw)?,
        "insights.top_n" => cfg.insights.top_n = parse_usize(key, raw)?,
        "pacing.enabled" => cfg.pacing.enabled = parse_bool(key, raw)?,
        "pacing.active_session_min_tokens" => {
            cfg.pacing.active_session_min_tokens = parse_u64(key, raw)?
        }
        "pacing.history_days" => cfg.pacing.history_days = parse_u32(key, raw)?,
        _ => return Err(unknown_key(key)),
    }
    Ok(())
}

fn parse_usize(key: &str, raw: &str) -> Result<usize, SchemaError> {
    raw.parse::<usize>().map_err(|_| SchemaError::InvalidType {
        key: key.to_string(),
        expected: "a non-negative integer",
        value: raw.to_string(),
    })
}

fn parse_enum(
    key: &str,
    raw: &str,
    allowed: &'static [&'static str],
) -> Result<String, SchemaError> {
    match allowed.iter().find(|a| a.eq_ignore_ascii_case(raw)) {
        Some(canonical) => Ok((*canonical).to_string()),
        None => Err(SchemaError::InvalidValue {
            key: key.to_string(),
            value: raw.to_string(),
            allowed: allowed.to_vec(),
        }),
    }
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, SchemaError> {
    match raw.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(SchemaError::InvalidType {
            key: key.to_string(),
            expected: "a boolean (true/false)",
            value: raw.to_string(),
        }),
    }
}

fn is_clear(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "" | "none" | "null" | "unset"
    )
}

fn parse_opt_bool(key: &str, raw: &str) -> Result<Option<bool>, SchemaError> {
    if is_clear(raw) {
        return Ok(None);
    }
    parse_bool(key, raw).map(Some)
}

fn parse_opt_u32(key: &str, raw: &str) -> Result<Option<u32>, SchemaError> {
    if is_clear(raw) {
        return Ok(None);
    }
    raw.parse::<u32>()
        .map(Some)
        .map_err(|_| SchemaError::InvalidType {
            key: key.to_string(),
            expected: "a non-negative integer or `none`",
            value: raw.to_string(),
        })
}

fn parse_u32(key: &str, raw: &str) -> Result<u32, SchemaError> {
    raw.parse::<u32>().map_err(|_| SchemaError::InvalidType {
        key: key.to_string(),
        expected: "a non-negative integer",
        value: raw.to_string(),
    })
}

fn parse_u64(key: &str, raw: &str) -> Result<u64, SchemaError> {
    raw.parse::<u64>().map_err(|_| SchemaError::InvalidType {
        key: key.to_string(),
        expected: "a non-negative integer",
        value: raw.to_string(),
    })
}

fn parse_opt_string(raw: &str) -> Option<String> {
    if is_clear(raw) {
        None
    } else {
        Some(raw.to_string())
    }
}

fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

// ── Commented template renderer ─────────────────────────────────────────────────

/// Render `cfg` as a `config.toml` whose `[display]` / `[update]` keys each
/// carry a `#` comment, preceded by section docs for `[[agents]]` /
/// `[[plugins]]`. Round-trips: parsing the output yields `cfg` again.
pub fn render_commented(cfg: &AppConfig) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Aura configuration.\n");
    out.push_str("# Run `aura config describe` for full field docs, or\n");
    out.push_str("# `aura config set <key> <value>` to change a value from the CLI.\n\n");

    push_section_docs(&mut out, "agents", agent_fields());
    for agent in &cfg.agents {
        out.push_str("[[agents]]\n");
        out.push_str(&toml::to_string(agent).context("serialize agent entry")?);
        out.push('\n');
    }

    push_section_docs(&mut out, "plugins", plugin_fields());
    for plugin in &cfg.plugins {
        out.push_str("[[plugins]]\n");
        out.push_str(&toml::to_string(plugin).context("serialize plugin entry")?);
        out.push('\n');
    }

    push_scalar_table(&mut out, cfg, "display");
    out.push('\n');
    push_scalar_table(&mut out, cfg, "update");
    out.push('\n');
    push_scalar_table(&mut out, cfg, "insights");
    push_scalar_table(&mut out, cfg, "pacing");
    push_scalar_table(&mut out, cfg, "fleet");

    Ok(out)
}

fn push_section_docs(out: &mut String, name: &str, fields: &[SectionField]) {
    out.push_str(&format!(
        "# ── [[{name}]] ── repeatable; one block per {name} entry.\n"
    ));
    for f in fields {
        out.push_str(&format!(
            "#   {} ({}){} — {}\n",
            f.key,
            f.type_label,
            allowed_suffix(f.allowed),
            f.summary
        ));
    }
    out.push('\n');
}

fn push_scalar_table(out: &mut String, cfg: &AppConfig, section: &str) {
    out.push_str(&format!("[{section}]\n"));
    let prefix = format!("{section}.");
    for f in fields().iter().filter(|f| f.key.starts_with(&prefix)) {
        let leaf = &f.key[prefix.len()..];
        for line in wrap_text(f.summary, 72) {
            out.push_str("# ");
            out.push_str(&line);
            out.push('\n');
        }
        match toml_rhs(cfg, f.key) {
            Some(rhs) => out.push_str(&format!("{leaf} = {rhs}\n")),
            // Optional + unset: emit a commented example so the key is discoverable.
            None => out.push_str(&format!("# {leaf} = {}\n", example_rhs(f))),
        }
    }
}

/// TOML-formatted right-hand side for a field, or `None` when an optional field
/// is unset (so the renderer emits a commented example instead).
fn toml_rhs(cfg: &AppConfig, key: &str) -> Option<String> {
    Some(match key {
        "display.default_period" => quote(&cfg.display.default_period),
        "display.anchor" => quote(&cfg.display.anchor),
        "display.plugin_order" => str_array(&cfg.display.plugin_order),
        "display.show_in_app_switcher" => cfg.display.show_in_app_switcher.to_string(),
        "display.dismiss_on_focus_loss" => cfg.display.dismiss_on_focus_loss.to_string(),
        "display.window_chrome" => cfg.display.window_chrome.to_string(),
        "display.auto_resize" => return cfg.display.auto_resize.map(|b| b.to_string()),
        "display.max_height" => return cfg.display.max_height.map(|n| n.to_string()),
        "display.goblin_mode" => cfg.display.goblin_mode.to_string(),
        "fleet.enabled" => cfg.fleet.enabled.to_string(),
        "fleet.broker_url" => quote(&cfg.fleet.broker_url),
        "fleet.machine_label" => quote(&cfg.fleet.machine_label),
        "fleet.heartbeat_secs" => cfg.fleet.heartbeat_secs.to_string(),
        "fleet.stale_secs" => cfg.fleet.stale_secs.to_string(),
        "update.dismissed_version" => return cfg.update.dismissed_version.as_deref().map(quote),
        "update.dismiss_all" => cfg.update.dismiss_all.to_string(),
        "insights.enabled" => cfg.insights.enabled.to_string(),
        "insights.top_n" => cfg.insights.top_n.to_string(),
        "pacing.enabled" => cfg.pacing.enabled.to_string(),
        "pacing.active_session_min_tokens" => cfg.pacing.active_session_min_tokens.to_string(),
        "pacing.history_days" => cfg.pacing.history_days.to_string(),
        _ => return None,
    })
}

fn example_rhs(f: &FieldDescriptor) -> String {
    match f.type_label {
        "string" | "string?" => quote(f.example),
        _ => f.example.to_string(),
    }
}

fn allowed_suffix(allowed: &[&str]) -> String {
    if allowed.is_empty() {
        String::new()
    } else {
        format!(" [{}]", allowed.join(" | "))
    }
}

/// Quote a string as a TOML basic string (with proper escaping).
fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn str_array(items: &[String]) -> String {
    let inner = items
        .iter()
        .map(|s| quote(s))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, AgentKind, DisplayConfig, FleetConfig, InsightsConfig, PacingConfig,
        PluginConfig, UpdateConfig,
    };

    /// Walk a serialized default config and assert every leaf key under
    /// `[display]` / `[update]` has a `FieldDescriptor`. Fails if a struct
    /// field is added without a descriptor — the anti-drift guard.
    /// A config with every optional field populated, so `None`-valued fields
    /// (which serde omits from TOML) still appear when checking coverage.
    fn fully_populated() -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.display.auto_resize = Some(false);
        cfg.display.max_height = Some(500);
        cfg.update.dismissed_version = Some("0.0.0".to_string());
        cfg
    }

    #[test]
    fn registry_covers_every_field() {
        let cfg = fully_populated();
        let value = toml::Value::try_from(&cfg).unwrap();
        let table = value.as_table().unwrap();

        for section in ["display", "update", "insights", "pacing", "fleet"] {
            let sub = table
                .get(section)
                .and_then(|v| v.as_table())
                .unwrap_or_else(|| panic!("section [{section}] missing from serialized config"));
            for leaf in sub.keys() {
                let key = format!("{section}.{leaf}");
                assert!(
                    field(&key).is_some(),
                    "config key `{key}` has no FieldDescriptor in config_schema::fields()"
                );
            }
        }

        // ...and no descriptor names a key that doesn't exist on the struct.
        for f in fields() {
            let (section, leaf) = f.key.split_once('.').unwrap();
            let present = table
                .get(section)
                .and_then(|v| v.as_table())
                .map(|t| t.contains_key(leaf))
                .unwrap_or(false);
            assert!(present, "descriptor `{}` names a non-existent field", f.key);
        }
    }

    #[test]
    fn get_and_set_round_trip_every_descriptor() {
        for f in fields() {
            let mut cfg = AppConfig::default_config();
            // get_value must handle the key.
            get_value(&cfg, f.key).unwrap_or_else(|e| panic!("get `{}`: {e}", f.key));
            // set_value(example) then get_value should reflect the example.
            set_value(&mut cfg, f.key, f.example)
                .unwrap_or_else(|e| panic!("set `{}` = `{}`: {e}", f.key, f.example));
            let got = get_value(&cfg, f.key).unwrap();
            // For list/optional types the rendered form differs from the raw
            // example, so just assert it's non-empty and not the unset marker.
            assert_ne!(got, "(unset)", "`{}` still unset after set", f.key);
        }
    }

    #[test]
    fn set_value_validates() {
        let mut cfg = AppConfig::default_config();

        // Bad enum value lists the allowed set.
        let err = set_value(&mut cfg, "display.anchor", "sideways").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("none | bottom | top"), "{msg}");

        // Bad bool.
        let err = set_value(&mut cfg, "display.goblin_mode", "maybe").unwrap_err();
        assert!(err.to_string().contains("boolean"));

        // Bad int.
        assert!(set_value(&mut cfg, "display.max_height", "tall").is_err());

        // Unknown key suggests a real one.
        let err = set_value(&mut cfg, "display.anchorr", "top").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown config key"), "{msg}");
        assert!(msg.contains("did you mean"), "{msg}");

        // Clearing an optional.
        cfg.display.max_height = Some(400);
        set_value(&mut cfg, "display.max_height", "none").unwrap();
        assert_eq!(cfg.display.max_height, None);

        // Enum is case-insensitive and canonicalized.
        set_value(&mut cfg, "display.anchor", "TOP").unwrap();
        assert_eq!(cfg.display.anchor, "top");

        // List parsing.
        set_value(&mut cfg, "display.plugin_order", "A, B ,C").unwrap();
        assert_eq!(cfg.display.plugin_order, vec!["A", "B", "C"]);
    }

    fn assert_round_trips(cfg: &AppConfig) {
        let rendered = render_commented(cfg).unwrap();
        let parsed: AppConfig = toml::from_str(&rendered)
            .unwrap_or_else(|e| panic!("re-parse rendered config: {e}\n---\n{rendered}"));

        assert_eq!(parsed.agents.len(), cfg.agents.len());
        for (a, b) in parsed.agents.iter().zip(&cfg.agents) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.config_path, b.config_path);
            assert_eq!(a.color, b.color);
        }
        assert_eq!(parsed.plugins.len(), cfg.plugins.len());
        for (a, b) in parsed.plugins.iter().zip(&cfg.plugins) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.command, b.command);
            assert_eq!(a.color, b.color);
            assert_eq!(a.icon, b.icon);
        }
        assert_eq!(parsed.display, cfg.display);
        assert_eq!(parsed.update, cfg.update);
        assert_eq!(parsed.insights, cfg.insights);
        assert_eq!(parsed.pacing, cfg.pacing);
        assert_eq!(parsed.fleet, cfg.fleet);
    }

    #[test]
    fn render_commented_round_trips_default() {
        assert_round_trips(&AppConfig::default_config());
    }

    #[test]
    fn render_commented_round_trips_populated() {
        let cfg = AppConfig {
            agents: vec![AgentConfig {
                name: "Work Claude".to_string(),
                kind: AgentKind::ClaudeCode,
                config_path: Some("~/.claude-work".to_string()),
                color: Some("#abcdef".to_string()),
            }],
            plugins: vec![PluginConfig {
                name: "RTK Gains".to_string(),
                command: "aura-plugin-rtk".to_string(),
                color: Some("#123".to_string()),
                icon: Some("icons/blocks.svg".to_string()),
            }],
            display: DisplayConfig {
                default_period: "7d".to_string(),
                anchor: "top".to_string(),
                plugin_order: vec!["RTK Gains".to_string(), "Hello".to_string()],
                show_in_app_switcher: true,
                dismiss_on_focus_loss: false,
                window_chrome: true,
                auto_resize: Some(false),
                max_height: Some(500),
                goblin_mode: true,
            },
            update: UpdateConfig {
                dismissed_version: Some("0.1.18".to_string()),
                dismiss_all: true,
            },
            insights: InsightsConfig {
                enabled: true,
                top_n: 8,
            },
            pacing: PacingConfig {
                enabled: true,
                active_session_min_tokens: 40_000,
                history_days: 21,
            },
            fleet: FleetConfig {
                enabled: true,
                broker_url: "https://ntfy.example.com".to_string(),
                machine_label: "Work-Linux".to_string(),
                heartbeat_secs: 30,
                stale_secs: 90,
            },
        };
        assert_round_trips(&cfg);
    }
}
