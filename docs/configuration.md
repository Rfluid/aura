---
title: Configuration
status: draft
version: 0.1.1
last_updated: 2026-05-24
last_verified: 2026-05-24
source_refs: ["crates/aura-core/src/config.rs", "crates/aura/src/cli/config.rs"]
owner: "@rfluid"
tags: [configuration, docs]
---

# Configuration

## File location

`~/.config/aura/config.toml`

Aura creates a default config on first run if none exists.

For scripted access, prefer the CLI:

```text
aura config path        # print resolved path
aura config show        # dump current config (--format text|json)
aura config edit        # open in $EDITOR (seeds defaults if missing)
aura config validate    # parse-check
aura config setup       # re-detect installed agents (alias: setup-config)
```

See `docs/cli.md` for the full subcommand surface.

## Full example

```toml
# Aura configuration

# ── Agent profiles ───────────────────────────────────────────────────────────
# Define as many profiles as you need. The active profile is tracked in state,
# not here — switching profiles in the UI does not touch this file.

[[agents]]
name = "Claude Code (Personal)"
kind = "claude-code"
# Path to the Claude Code config directory. Defaults to ~/.claude if omitted.
config_path = "~/.claude"

[[agents]]
name = "Claude Code (Enterprise)"
kind = "claude-code"
config_path = "~/.claude-enterprise"

[[agents]]
name = "Codex"
kind = "codex"
# Path to the Codex config directory. Defaults to ~/.codex if omitted.
config_path = "~/.codex"

# ── Plugins ──────────────────────────────────────────────────────────────────
#
# Plugins are typically installed via `aura plugin add <path>`, which writes
# the binary into ~/.config/aura/plugins/ and registers it via auto-discovery
# — no [[plugins]] block needed. Use the inline form below only for plugins
# that live outside the user plugins dir (e.g. system-wide binaries on $PATH).

[[plugins]]
name = "RTK Gains"
# Binary on $PATH or absolute path
command = "aura-plugin-rtk"

# ── Display ──────────────────────────────────────────────────────────────────

[display]
# Which usage period to show by default when opening the modal
# Options: "today" | "this_month" | "all_time"
default_period = "today"

# Optional explicit ordering for the plugin pill row. Plugins named here
# render in this order; anything not named keeps its natural order
# (config-then-discovered-alphabetical) and appends afterwards. Match is
# case-insensitive against each plugin's display `name`. Omit or set to
# [] to keep the default ordering.
plugin_order = ["Hello", "RTK Gains"]

# Where to anchor the modal relative to the widget click position
# Options: "auto" | "top" | "bottom" | "left" | "right"
# "auto" lets Aura pick based on available screen space
anchor = "auto"
```

## Agent kinds

| `kind` | Description | Config fields |
|---|---|---|
| `claude-code` | Claude Code CLI agent | `config_path` (path to the dir containing `stats-cache.json`; defaults to `~/.claude`) |
| `codex` | OpenAI Codex CLI | `config_path` (path to the dir containing `sessions/`; defaults to `~/.codex`) |

## State file

Aura writes the active profile selection to `~/.local/share/aura/state.json`. This file is managed automatically — do not edit by hand.

```json
{
  "active_profile": "Claude Code (Personal)",
  "last_updated": "2026-05-21T14:30:00Z"
}
```

## Themes

Aura ships with a built-in dark theme that you can override on a per-token basis
via `~/.config/aura/theme.toml`. Every key is optional — anything you don't set
falls back to the built-in default. Clicking the **Themes** entry in the more
menu (•••) opens the file in your editor, seeding it from the defaults on first
click.

```toml
[colors]
bg          = "#0e0e10"
surface     = "#1a1a1f"
accent      = "#8b5cf6"
error       = "#ff6b6b"
warning     = "#e0a96d"
agent_fallback = "#b8b8c0"   # used when a brand color would wash out on bg

[typography]
font_family = "JetBrains Mono"

[spinner]
style       = "braille"   # "braille" | "dot"
interval_ms = 80

# Per-agent overrides. Keys must match the agent's `name` from config.toml
# (quote names with spaces or parentheses).
[agents."Claude Code (Personal)"]
accent = "#d97757"
```

### Precedence

For an agent's accent color, the first match wins:

1. `[agents."<name>"].accent` in `theme.toml`
2. `[[agents]] color = "..."` in `config.toml`
3. Per-kind brand default (Claude orange, OpenAI white, Gemini blue)

A luminance fallback applies after all of the above: a resolved color whose
relative luminance exceeds 0.85 is silently swapped for `colors.agent_fallback`
so the accent never washes out against the dark surface.

### Hot reload

The refresh button in the header reloads `theme.toml` alongside `config.toml`
— no restart required. A malformed file logs a warning and falls back to the
built-in defaults rather than blanking the UI.

See `.design/customization.md` for the full schema reference.
