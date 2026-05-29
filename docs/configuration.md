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

# How the modal anchors as it auto-fits its content height.
# Options:
#   "none"   — open at the platform's natural tray corner and grow downward
#              from there; never reposition after a resize. Safe on Wayland,
#              where the compositor owns window placement.
#   "bottom" — pin the bottom edge above a bottom taskbar so the modal grows
#              *upward* (the tray-popup feel next to a bottom tray). Needs an
#              active window move after each resize: supported on Windows,
#              macOS, and Linux/X11. On Linux/Wayland it only applies at open
#              (the compositor owns placement — see note below).
#   "top"    — pin the top edge just below a top panel / menu bar and grow
#              downward.
# Default is OS-specific and written for you at install: "bottom" on Windows
# (bottom taskbar), "none" on macOS and Linux. Unrecognised values (including
# the legacy "auto") fall back to the per-OS default.
anchor = "bottom"

# Show OS-native window chrome (title bar + min/max/close) and let the user
# resize the modal by dragging its edges. Default false — Aura behaves like a
# fixed-width tray popup that auto-fits its height. Turning this on also
# disables the auto-fit (so a dragged size sticks) and puts the modal in the
# taskbar / alt-tab list.
window_chrome = false

# Optional upper bound (logical px) on the auto-fit height. The modal is
# already capped at the screen's available work area; this imposes a tighter
# ceiling. Omit (or leave unset) for "only the work-area cap applies".
# Ignored when window_chrome = true (auto-fit is off then).
# max_height = 500

# Auto-close the modal when it loses focus (click outside, switch app).
# Default true — matches typical menu-bar / tray-popup behaviour. Set
# to false to make the modal sticky (only the tray icon closes it),
# which is handy when copy-pasting from the modal into another window.
dismiss_on_focus_loss = true

# Appear in the OS's "where are my windows" surfaces:
# - macOS:   Cmd+Tab app switcher + Dock (NSApp activation policy)
# - Windows: Alt+Tab + taskbar (WS_EX_APPWINDOW vs WS_EX_TOOLWINDOW)
# - Linux:   panel / window switcher (WindowKind::Normal vs PopUp)
# Default false — Aura runs as a tray-only indicator. Reapplies on the
# next refresh (modal Refresh button) or modal open, so a config edit
# takes effect without restarting the service.
show_in_app_switcher = false
```

### Modal anchoring (`anchor`)

`anchor` controls which edge of the modal stays put as it auto-fits its
content height:

| Value | Behavior | Default on |
|---|---|---|
| `none` | Opens at the platform's natural tray corner and grows downward; never repositioned. | macOS, Linux |
| `bottom` | Bottom edge pinned above a bottom taskbar; grows upward. | Windows |
| `top` | Top edge pinned just below a top panel / menu bar; grows downward. | — |

The right default is written to your config at install time based on your OS,
so most people never need to set this. Change it if your taskbar/panel is
somewhere other than your platform's default (e.g. a Linux desktop with a
**top** panel → `anchor = "top"`).

**Linux note:** on **X11**, `anchor = "bottom"` repositions live — after each
resize Aura asks the window manager to move the modal via an EWMH
`_NET_MOVERESIZE_WINDOW` request (a plain `ConfigureWindow` is ignored by KWin
for a managed top-level), so the modal hugs the bottom taskbar as it
grows/shrinks. On **Wayland** the protocol forbids clients from positioning
their own toplevels, so `bottom` only applies at *open*; as the modal shrinks
it grows downward from there rather than hugging the taskbar. For exact
placement on KDE Plasma / Wayland, use a KWin window rule (see
[Modal placement on Wayland](../README.md#modal-placement-on-wayland) in the
README). We currently detect only *bottom* panel reservations, so `top` on a
Linux top-panel setup approximates by sitting at the very top of the display.

> **KDE Plasma:** if the modal *visibly stretches/animates* over ~0.5s as it
> resizes, that is KWin's Morphing Popups effect, not Aura — see
> [Troubleshooting: modal stretches on resize](troubleshooting/modal-stretches-on-resize-kde.md).

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
