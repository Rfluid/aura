//! Field registry for [`AppConfig`] — the single source of truth that powers
//! the self-documenting config surfaces:
//!
//! - `aura config describe` (and `--format json`)
//! - the commented `config.toml` template ([`render_commented`])
//! - `aura config get` / `set` ([`get_value`] / [`set_value`])
//! - `aura config wizard`
//!
//! Every settable scalar field under `[window]` / `[tray]` / `[content]` /
//! `[update]` has a [`FieldDescriptor`] here. A unit test
//! (`registry_covers_every_field`) serializes a default config and asserts
//! each leaf key is described, so adding a struct field without documenting
//! it breaks the build.
//!
//! Keys from an older layout still resolve: every lookup here runs through
//! [`crate::config_migrate::resolve_key`] first, so `aura config get
//! display.anchor` answers with `window.anchor`.
//!
//! The repeatable `[[agents]]` / `[[plugins]]` tables are *documented* via
//! [`agent_fields`] / [`plugin_fields`] but are not get/set targets — they're
//! managed with `aura agents`, `aura plugin`, or `config edit`.

use anyhow::{Context, Result};

use crate::config::AppConfig;
use crate::config_migrate;

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

/// The scalar `[section]`s of the config, in template-emission order. The
/// repeatable `[[agents]]` / `[[plugins]]` tables are not here — they are
/// documented by [`agent_fields`] / [`plugin_fields`] and edited elsewhere.
pub const SECTIONS: &[&str] = &["window", "tray", "content", "update"];

/// All settable scalar fields, in template-emission order ([`SECTIONS`]).
pub fn fields() -> &'static [FieldDescriptor] {
    &[
        // ── [window] ──
        FieldDescriptor {
            key: "window.anchor",
            type_label: "string",
            allowed: &["none", "bottom", "top"],
            default: "\"none\" (macOS/Linux), \"bottom\" (Windows)",
            summary: "How the modal anchors as it auto-fits its content height.",
            description: "How the modal anchors as it auto-fits its content height. \
                \"none\": open at the platform's natural tray corner and grow downward; \
                never reposition after a resize (all a native Wayland surface can do, \
                since the compositor owns placement there \u{2014} see window.linux_backend). \
                \"bottom\": pin the bottom edge above a bottom taskbar so \
                it grows upward (the tray-popup feel). \"top\": pin the top edge below a top \
                panel / menu bar and grow downward. Default is per-OS: \"bottom\" on \
                Windows, \"none\" on macOS and Linux. Unrecognised values (incl. the legacy \
                \"auto\") fall back to the per-OS default.",
            example: "none",
        },
        FieldDescriptor {
            key: "window.linux_backend",
            type_label: "string",
            allowed: &["auto", "x11", "wayland"],
            default: "\"auto\"",
            summary: "Which display server GPUI talks to on Linux / BSD.",
            description: "Which display server GPUI talks to on Linux / BSD. Wayland forbids a \
                client from positioning its own toplevel, so a native Wayland session silently \
                disables window.anchor, taskbar avoidance, and centring the modal under the tray \
                icon \u{2014} which is why this knob lives under [window] at all. \"auto\" (default) \
                uses X11 whenever $DISPLAY is set \u{2014} via XWayland on a \
                Wayland session \u{2014} which restores placement control. \"x11\" is the same but \
                warns when $DISPLAY is missing. \"wayland\" keeps the native backend and leaves \
                placement to the compositor (use a compositor window rule instead); pick it if \
                XWayland looks blurry on a fractional-scale display. Ignored on macOS and Windows.",
            example: "auto",
        },
        FieldDescriptor {
            key: "window.show_in_app_switcher",
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
            key: "window.dismiss_on_focus_loss",
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
            key: "window.chrome",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Show the native window title bar.",
            description: "Show the OS-native window chrome (title bar + minimize / maximize / \
                close buttons). Default false: Aura behaves like a tray popup with no title \
                bar. This only controls the title bar — whether the modal auto-resizes to fit \
                its content is the separate window.auto_resize knob.",
            example: "false",
        },
        FieldDescriptor {
            key: "window.auto_resize",
            type_label: "bool?",
            allowed: &["true", "false"],
            default: "unset (auto-fit)",
            summary: "Auto-resize the modal to fit its content height.",
            description: "Whether the modal auto-resizes to fit its content height. On every \
                layout pass a content-fit callback grows / shrinks the window to match (capped \
                at the screen work area and window.max_height). Unset (default) means auto-fit \
                is on. Set false for a fixed-size window. This is not user drag-to-resize (the \
                window manager owns that) — only Aura's content-fit. Independent of \
                window.chrome, so the auto-fit works the same with or without the title bar.",
            example: "true",
        },
        FieldDescriptor {
            key: "window.max_height",
            type_label: "u32?",
            allowed: &[],
            default: "unset (only the screen work-area cap applies)",
            summary: "Upper bound, in logical pixels, on the modal's auto-fit height.",
            description: "Optional upper bound (in logical pixels) on the modal's auto-fit \
                height. The content-fit callback already caps growth at the screen's available \
                work area; this lets you impose a tighter ceiling so the modal never grows \
                past, say, 500 px even on a tall display. Unset means \"only the work-area cap \
                applies\". Ignored when window.auto_resize is false (no auto-fit to cap).",
            example: "500",
        },
        // ── [tray] ──
        FieldDescriptor {
            key: "tray.indicator",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "true",
            summary: "Whether the tray icon reports quota, or is just a button that opens the modal.",
            description: "Whether the tray icon is a live indicator or a plain button. Default \
                true: the icon keeps its tooltip and gauge in sync with the active profile's \
                quota, which is what makes it an indicator rather than something you click to \
                find out. Set false and it becomes a static button whose only job is opening \
                the modal. Master switch for tray.progress, tray.color and tray.pulse. While \
                the modal is closed this costs one quota lookup every tray.refresh_secs, which \
                for API-backed agents is a network request.",
            example: "true",
        },
        FieldDescriptor {
            key: "tray.progress",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "true",
            summary: "Fill the tray icon's ring in proportion to quota usage.",
            description: "Fill the tray icon's ring in proportion to peak quota usage. Default \
                true. Set false to draw the complete ring at full opacity and leave usage to \
                the tooltip. Ignored when tray.indicator is false.",
            example: "true",
        },
        FieldDescriptor {
            key: "tray.color",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "true",
            summary: "Move the tray icon up a purple → yellow → orange → red ramp as usage climbs.",
            description: "Move the tray icon's color up the purple → yellow → orange → red \
                ramp as usage climbs (50% / 75% / 90%). Default true. Set false to keep the \
                icon Aura purple at every level — on macOS it then also keeps the menu bar's \
                own foreground color, like every other status item. Ignored when \
                tray.indicator is false.",
            example: "true",
        },
        FieldDescriptor {
            key: "tray.pulse",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Ask the desktop to draw attention to the tray icon at 90% usage.",
            description: "Ask the desktop to draw attention to the tray icon once usage reaches \
                90%. Default false, unlike the other tray visuals: this is a request to the \
                desktop rather than Aura drawing its own icon, and hosts answer it loudly — \
                Plasma pulls the item out of the overflow group and animates it. Linux only in \
                practice (StatusNotifierItem NeedsAttention); macOS and Windows have no \
                equivalent, where the red end of tray.color is the whole signal. Ignored when \
                tray.indicator is false.",
            example: "false",
        },
        FieldDescriptor {
            key: "tray.refresh_secs",
            type_label: "u64",
            allowed: &[],
            default: "1200",
            summary: "Seconds between background refreshes of the tray indicator.",
            description: "Seconds between background refreshes of the tray indicator. Ignored \
                when tray.indicator is false. Default 1200 (20 minutes). Values below 30 are \
                clamped up to 30 so a typo cannot turn the indicator into a hot loop against a \
                rate-limited quota endpoint.",
            example: "1200",
        },
        // ── [content] ──
        FieldDescriptor {
            key: "content.default_period",
            type_label: "string",
            allowed: &["all", "7d", "30d"],
            default: "all",
            summary: "Which usage period tab is selected on open.",
            description: "Which period to show by default: \"all\", \"7d\" (last 7 days), or \
                \"30d\" (last 30 days). Unrecognised values fall back to \"all\".",
            example: "all",
        },
        FieldDescriptor {
            key: "content.plugin_order",
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
            key: "content.goblin_mode",
            type_label: "bool",
            allowed: &["true", "false"],
            default: "false",
            summary: "Swap UI copy for the aggressive \"Goblin Mode\" variant.",
            description: "Swap the modal's user-facing copy for an aggressive / unhinged \
                variant (\"Goblin Mode\"). Default false. Toggling reloads on the next refresh \
                — no restart. See docs/goblin-mode.md.",
            example: "false",
        },
        // ── [update] ──
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
        SectionField {
            key: "tray_progress_source",
            type_label: "u32?",
            allowed: &[],
            summary: "Quota window that fills the tray ring, by position. Unset = 0, the session.",
        },
        SectionField {
            key: "tray_color_source",
            type_label: "u32?",
            allowed: &[],
            summary: "Quota window that drives the tray color ramp, by position. \
                Unset = 1, the week.",
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

/// Look up a descriptor by its dotted key. A key from an older layout
/// resolves to the descriptor it moved to, so old spellings keep working.
pub fn field(key: &str) -> Option<&'static FieldDescriptor> {
    let key = canonical_key(key);
    fields().iter().find(|f| f.key == key)
}

/// The current name of `key`, following any section reorganization it has
/// been through. Returns `key` unchanged when it never moved.
pub fn canonical_key(key: &str) -> &str {
    config_migrate::resolve_key(key).unwrap_or(key)
}

/// `Some(current_name)` when `key` is an old spelling that has since moved —
/// what the CLI prints as a deprecation note. `None` for a current key.
pub fn renamed_from(key: &str) -> Option<&'static str> {
    config_migrate::resolve_key(key)
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
    let leaf = canonical_key(key).rsplit('.').next().unwrap_or(key);
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
    let v = match canonical_key(key) {
        "window.anchor" => cfg.window.anchor.clone(),
        "window.linux_backend" => cfg.window.linux_backend.clone(),
        "window.show_in_app_switcher" => cfg.window.show_in_app_switcher.to_string(),
        "window.dismiss_on_focus_loss" => cfg.window.dismiss_on_focus_loss.to_string(),
        "window.chrome" => cfg.window.chrome.to_string(),
        "window.auto_resize" => cfg
            .window
            .auto_resize
            .map(|b| b.to_string())
            .unwrap_or_else(|| "(unset)".to_string()),
        "window.max_height" => cfg
            .window
            .max_height
            .map(|n| n.to_string())
            .unwrap_or_else(|| "(unset)".to_string()),
        "tray.indicator" => cfg.tray.indicator.to_string(),
        "tray.progress" => cfg.tray.progress.to_string(),
        "tray.color" => cfg.tray.color.to_string(),
        "tray.pulse" => cfg.tray.pulse.to_string(),
        "tray.refresh_secs" => cfg.tray.refresh_secs.to_string(),
        "content.default_period" => cfg.content.default_period.clone(),
        "content.plugin_order" => cfg.content.plugin_order.join(", "),
        "content.goblin_mode" => cfg.content.goblin_mode.to_string(),
        "update.dismissed_version" => cfg
            .update
            .dismissed_version
            .clone()
            .unwrap_or_else(|| "(unset)".to_string()),
        "update.dismiss_all" => cfg.update.dismiss_all.to_string(),
        _ => return Err(unknown_key(key)),
    };
    Ok(v)
}

/// Parse `raw` against the field's type and assign it. Validates enums against
/// `allowed`; clears optional fields when `raw` is empty / `none` / `null`.
pub fn set_value(cfg: &mut AppConfig, key: &str, raw: &str) -> Result<(), SchemaError> {
    let raw = raw.trim();
    // Errors are reported against the key the user typed, not its canonical
    // form, so a legacy spelling still produces a message they recognise.
    match canonical_key(key) {
        "window.anchor" => cfg.window.anchor = parse_enum(key, raw, &["none", "bottom", "top"])?,
        "window.linux_backend" => {
            cfg.window.linux_backend = parse_enum(key, raw, &["auto", "x11", "wayland"])?
        }
        "window.show_in_app_switcher" => cfg.window.show_in_app_switcher = parse_bool(key, raw)?,
        "window.dismiss_on_focus_loss" => cfg.window.dismiss_on_focus_loss = parse_bool(key, raw)?,
        "window.chrome" => cfg.window.chrome = parse_bool(key, raw)?,
        "window.auto_resize" => cfg.window.auto_resize = parse_opt_bool(key, raw)?,
        "window.max_height" => cfg.window.max_height = parse_opt_u32(key, raw)?,
        "tray.indicator" => cfg.tray.indicator = parse_bool(key, raw)?,
        "tray.progress" => cfg.tray.progress = parse_bool(key, raw)?,
        "tray.color" => cfg.tray.color = parse_bool(key, raw)?,
        "tray.pulse" => cfg.tray.pulse = parse_bool(key, raw)?,
        "tray.refresh_secs" => cfg.tray.refresh_secs = parse_u64(key, raw)?,
        "content.default_period" => {
            cfg.content.default_period = parse_enum(key, raw, &["all", "7d", "30d"])?
        }
        "content.plugin_order" => cfg.content.plugin_order = parse_list(raw),
        "content.goblin_mode" => cfg.content.goblin_mode = parse_bool(key, raw)?,
        "update.dismissed_version" => cfg.update.dismissed_version = parse_opt_string(raw),
        "update.dismiss_all" => cfg.update.dismiss_all = parse_bool(key, raw)?,
        _ => return Err(unknown_key(key)),
    }
    Ok(())
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

fn parse_u64(key: &str, raw: &str) -> Result<u64, SchemaError> {
    raw.parse::<u64>().map_err(|_| SchemaError::InvalidType {
        key: key.to_string(),
        expected: "a non-negative integer",
        value: raw.to_string(),
    })
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

/// Render `cfg` as a `config.toml` whose [`SECTIONS`] keys each carry a `#`
/// comment, preceded by section docs for `[[agents]]` / `[[plugins]]`.
/// Round-trips: parsing the output yields `cfg` again.
pub fn render_commented(cfg: &AppConfig) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Aura configuration.\n");
    out.push_str("# Tutorial: https://github.com/Rfluid/aura/blob/main/docs/configuration.md\n");
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

    for (i, section) in SECTIONS.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_scalar_table(&mut out, cfg, section);
    }

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
        "window.anchor" => quote(&cfg.window.anchor),
        "window.linux_backend" => quote(&cfg.window.linux_backend),
        "window.show_in_app_switcher" => cfg.window.show_in_app_switcher.to_string(),
        "window.dismiss_on_focus_loss" => cfg.window.dismiss_on_focus_loss.to_string(),
        "window.chrome" => cfg.window.chrome.to_string(),
        "window.auto_resize" => return cfg.window.auto_resize.map(|b| b.to_string()),
        "window.max_height" => return cfg.window.max_height.map(|n| n.to_string()),
        "tray.indicator" => cfg.tray.indicator.to_string(),
        "tray.progress" => cfg.tray.progress.to_string(),
        "tray.color" => cfg.tray.color.to_string(),
        "tray.pulse" => cfg.tray.pulse.to_string(),
        "tray.refresh_secs" => cfg.tray.refresh_secs.to_string(),
        "content.default_period" => quote(&cfg.content.default_period),
        "content.plugin_order" => str_array(&cfg.content.plugin_order),
        "content.goblin_mode" => cfg.content.goblin_mode.to_string(),
        "update.dismissed_version" => return cfg.update.dismissed_version.as_deref().map(quote),
        "update.dismiss_all" => cfg.update.dismiss_all.to_string(),
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
        AgentConfig, AgentKind, ContentConfig, PluginConfig, TrayConfig, UpdateConfig, WindowConfig,
    };

    /// Walk a serialized default config and assert every leaf key under each
    /// of `SECTIONS` has a `FieldDescriptor`. Fails if a struct field is added
    /// without a descriptor — the anti-drift guard.
    /// A config with every optional field populated, so `None`-valued fields
    /// (which serde omits from TOML) still appear when checking coverage.
    fn fully_populated() -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.window.auto_resize = Some(false);
        cfg.window.max_height = Some(500);
        cfg.update.dismissed_version = Some("0.0.0".to_string());
        cfg
    }

    #[test]
    fn registry_covers_every_field() {
        let cfg = fully_populated();
        let value = toml::Value::try_from(&cfg).unwrap();
        let table = value.as_table().unwrap();

        for section in SECTIONS {
            let sub = table
                .get(*section)
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
        let err = set_value(&mut cfg, "window.anchor", "sideways").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("none | bottom | top"), "{msg}");

        // Bad bool.
        let err = set_value(&mut cfg, "content.goblin_mode", "maybe").unwrap_err();
        assert!(err.to_string().contains("boolean"));

        // Bad int.
        assert!(set_value(&mut cfg, "window.max_height", "tall").is_err());

        // Unknown key suggests a real one.
        let err = set_value(&mut cfg, "window.anchorr", "top").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown config key"), "{msg}");
        assert!(msg.contains("did you mean"), "{msg}");

        // Clearing an optional.
        cfg.window.max_height = Some(400);
        set_value(&mut cfg, "window.max_height", "none").unwrap();
        assert_eq!(cfg.window.max_height, None);

        // Enum is case-insensitive and canonicalized.
        set_value(&mut cfg, "window.anchor", "TOP").unwrap();
        assert_eq!(cfg.window.anchor, "top");

        // List parsing.
        set_value(&mut cfg, "content.plugin_order", "A, B ,C").unwrap();
        assert_eq!(cfg.content.plugin_order, vec!["A", "B", "C"]);
    }

    #[test]
    fn legacy_keys_still_get_and_set() {
        let mut cfg = AppConfig::default_config();

        // A key from the pre-split [display] layout resolves to its new home.
        set_value(&mut cfg, "display.window_chrome", "true").unwrap();
        assert!(cfg.window.chrome);
        assert_eq!(get_value(&cfg, "display.window_chrome").unwrap(), "true");

        set_value(&mut cfg, "display.tray_status_interval_secs", "600").unwrap();
        assert_eq!(cfg.tray.refresh_secs, 600);

        // ...and it describes as the field it became.
        assert_eq!(field("display.tray_color").unwrap().key, "tray.color");
        assert_eq!(renamed_from("display.tray_color"), Some("tray.color"));
        assert_eq!(renamed_from("tray.color"), None);

        // Validation still names the key the user actually typed.
        let err = set_value(&mut cfg, "display.anchor", "sideways").unwrap_err();
        assert!(err.to_string().contains("display.anchor"), "{err}");
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
        assert_eq!(parsed.window, cfg.window);
        assert_eq!(parsed.tray, cfg.tray);
        assert_eq!(parsed.content, cfg.content);
        assert_eq!(parsed.update, cfg.update);
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
                tray_progress_source: None,
                tray_color_source: None,
            }],
            plugins: vec![PluginConfig {
                name: "RTK Gains".to_string(),
                command: "aura-plugin-rtk".to_string(),
                color: Some("#123".to_string()),
                icon: Some("icons/blocks.svg".to_string()),
            }],
            window: WindowConfig {
                anchor: "top".to_string(),
                linux_backend: "wayland".to_string(),
                show_in_app_switcher: true,
                dismiss_on_focus_loss: false,
                chrome: true,
                auto_resize: Some(false),
                max_height: Some(500),
            },
            tray: TrayConfig {
                indicator: false,
                progress: false,
                color: false,
                pulse: true,
                refresh_secs: 900,
            },
            content: ContentConfig {
                default_period: "7d".to_string(),
                plugin_order: vec!["RTK Gains".to_string(), "Hello".to_string()],
                goblin_mode: true,
            },
            update: UpdateConfig {
                dismissed_version: Some("0.1.18".to_string()),
                dismiss_all: true,
            },
        };
        assert_round_trips(&cfg);
    }
}
