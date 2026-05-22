---
title: Configuration
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [configuration, docs]
---

# Configuration

## File location

`~/.config/aura/config.toml`

Aura creates a default config on first run if none exists.

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

[[plugins]]
name = "RTK Gains"
# Binary on $PATH or absolute path
command = "aura-plugin-rtk"

# ── Display ──────────────────────────────────────────────────────────────────

[display]
# Which usage period to show by default when opening the modal
# Options: "today" | "this_month" | "all_time"
default_period = "today"

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
