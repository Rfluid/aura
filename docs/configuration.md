---
title: Configuration
status: current
version: 0.2.0
last_updated: 2026-05-30
last_verified: 2026-05-30
source_refs:
  - crates/aura-core/src/config.rs
  - crates/aura-core/src/config_schema.rs
  - crates/aura-core/src/state.rs
  - crates/aura/src/cli/config.rs
  - crates/aura/src/runtime.rs
  - crates/aura/src/main.rs
  - crates/aura/src/app.rs
owner: "@rfluid"
tags: [configuration, docs]
---

# Configuration

Aura's configuration is layered: typed Rust structs are the source of truth, a
field registry documents and validates them, three on-disk files persist the
values, and a small runtime layer keeps the running tray app and modal in sync
as those files change. This page documents every field and how the layers fit
together. To **add or change** a config field as a developer, see
[`.agent/skills/add-or-change-config.md`](../.agent/skills/add-or-change-config.md),
which builds on this reference.

## File locations

| File | Path | What it holds | Edited by |
|---|---|---|---|
| Config | `~/.config/aura/config.toml` | Agents, plugins, `[display]`, `[update]` | You (CLI / editor) |
| Theme | `~/.config/aura/theme.toml` | Color / font / spinner overrides | You (CLI / editor) |
| State | `~/.local/share/aura/state.json` | Active profile selection | Aura (do not hand-edit) |
| Plugins dir | `~/.config/aura/plugins/` | Auto-discovered plugin binaries | `aura plugin add` |

Paths follow the XDG base-dir spec via the `dirs` crate, so the exact location
differs on macOS / Windows — always resolve it with `aura config path`. Aura
writes a fully-commented default `config.toml` on first run if none exists.

## The runtime model — config in layers

Config flows through five layers, top (authoring) to bottom (consumption):

1. **Typed structs** — `crates/aura-core/src/config.rs`. `AppConfig` is the
   root (`agents`, `plugins`, `display`, `update`); each sub-struct derives
   `Serialize`/`Deserialize` and a `Default`, so the whole tree round-trips
   through TOML and an empty/partial file still parses (missing fields fall back
   to `Default`). This is the **source of truth** — the shape of a config is
   whatever these structs say it is.

2. **Field registry / schema** — `crates/aura-core/src/config_schema.rs`. A
   flat list of `FieldDescriptor`s (one per settable scalar under `[display]` /
   `[update]`) plus `SectionField`s describing the repeatable `[[agents]]` /
   `[[plugins]]` tables. This registry powers everything self-documenting:
   `config describe`, `get`/`set` validation, the `wizard`, and the
   `#`-commented `config.toml` template (`render_commented`). A unit test
   (`registry_covers_every_field`) serializes a default config and asserts every
   leaf key has a descriptor — so the docs **cannot drift** from the structs
   without breaking the build.

3. **Persistence (disk)** — the four files above. `config.toml` is always
   written through `render_commented`, so every key carries a `#` comment lifted
   from the registry; those comments survive programmatic edits (`set`, `wizard`,
   `setup`).

4. **Load + merge** — `AppConfig::load` reads the file (writing defaults if
   absent); `load_with_discovery` additionally merges executable plugins found in
   the plugins dir (config-listed entries win on name collision) and applies
   `display.plugin_order`; `run_setup` detects installed agents and merges new
   ones without disturbing existing edits.

5. **Runtime mirror** — `crates/aura/src/runtime.rs`. The tray poll loop in
   `main.rs` and the modal's async refresh task in `app.rs` each reload the
   config independently. To stop them drifting, a handful of `[display]` fields
   are mirrored into process atomics via `runtime::set_from_config`, and any
   platform state they drive (e.g. the macOS NSApp activation policy) is
   reapplied there. Add an atomic + accessor here when a new `[display]` knob
   must be visible to *both* the background loop and the modal.

### Reload triggers (hot reload)

Most edits take effect **without a restart**. `set_from_config` is called, and
the config (and `theme.toml`) reloaded, at three moments:

- **Startup** — `main.rs` loads once before launching GPUI.
- **Every tray click** — the `Show` arm reloads via `load_with_discovery`, so a
  config edit (or a freshly-dropped plugin binary) is live on the next open.
- **Refresh button** — `app::do_refresh` reloads config + theme on a background
  thread; a malformed `theme.toml` logs a warning and falls back to defaults
  rather than blanking the UI.

A failed reload falls back to the last good in-memory snapshot, so a transient
I/O error never breaks the toggle.

### Precedence

- **Scalar fields:** struct `Default` (incl. the per-OS `default_anchor`) →
  value in `config.toml`.
- **Plugins:** a `[[plugins]]` block in `config.toml` overrides an
  auto-discovered binary of the same `name` (case-insensitive) — that's how you
  pin a color/icon onto a discovered plugin.
- **Agent accent color:** `[agents."<name>"].accent` in `theme.toml` → `[[agents]]
  color` in `config.toml` → per-kind brand default → luminance fallback (see
  [Themes](#themes)).

## CLI surface

Prefer the CLI over editing the file by hand — it validates values, suggests
near-miss keys, and keeps the inline docs intact.

```text
aura config setup              # detect installed agents, write/update config.toml
aura config path               # print resolved config path
aura config show               # print loaded config (--format text|json)
aura config describe [<key>]   # list every field (type/default/docs), or explain one
                               #   (--format json emits the full schema)
aura config get <key>          # print a single field's current value
aura config set <key> <value>  # validate and set one field (e.g. set display.anchor top)
aura config wizard             # walk every field interactively; blank keeps current
aura config init [--force]     # write a fresh, fully-commented config.toml
aura config document           # rewrite the existing config in place with inline docs
aura config edit               # open in $EDITOR (creates defaults if missing)
aura config validate           # parse-check
```

Keys are dotted paths into `[display]` / `[update]`, e.g. `display.anchor`,
`display.max_height`, `update.dismiss_all`. `set` rejects bad enums/booleans and
suggests near-miss keys; pass `none` (or empty) to clear an optional field. The
repeatable `[[agents]]` / `[[plugins]]` tables are *documented* by `describe`
but **edited** via `aura config edit`, `aura agents`, or `aura plugin` — they
are not `get`/`set` targets. The legacy `aura setup-config` is a hidden alias for
`aura config setup`. See [`docs/cli.md`](cli.md) for the full surface.

## Field reference

### `[display]`

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `default_period` | string | `all` \| `7d` \| `30d` | `all` | Usage period tab selected on open. |
| `anchor` | string | `none` \| `bottom` \| `top` | `none` (macOS/Linux), `bottom` (Windows) | How the modal anchors as it auto-fits height. |
| `plugin_order` | string[] | — | `[]` | Display order for plugin pills (comma-separated names on `set`). |
| `show_in_app_switcher` | bool | `true` \| `false` | `false` | Show the modal in Alt+Tab / Cmd+Tab / dock surfaces. |
| `dismiss_on_focus_loss` | bool | `true` \| `false` | `true` | Auto-close the modal when it loses focus. |
| `window_chrome` | bool | `true` \| `false` | `false` | Native window chrome + drag-to-resize (disables auto-fit). |
| `max_height` | u32? | — | unset | Upper bound (logical px) on auto-fit height; ignored when `window_chrome = true`. |
| `goblin_mode` | bool | `true` \| `false` | `false` | Swap UI copy for the aggressive "Goblin Mode" variant. |

### `[update]`

Controls the "Update available" header button.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `dismissed_version` | string? | — | unset | Last release dismissed via the button's ×; a newer release re-shows it. |
| `dismiss_all` | bool | `true` \| `false` | `false` | Master mute: never render the button or fire the GitHub check. |

### `[[agents]]` (repeatable)

| Field | Type | Allowed | Summary |
|---|---|---|---|
| `name` | string | — | Display name for this agent profile. |
| `kind` | string | `claude-code` \| `codex` \| `gemini` | Which agent this profile reads. |
| `config_path` | string? | — | Agent config dir; defaults to `~/.claude`, `~/.codex`, `~/.gemini` per kind. |
| `color` | string? | — | Accent color override, hex like `#rrggbb` or `#rgb`. |

### `[[plugins]]` (repeatable)

| Field | Type | Allowed | Summary |
|---|---|---|---|
| `name` | string | — | Display name for the plugin pill. |
| `command` | string | — | Binary name on `$PATH` or absolute path. |
| `color` | string? | — | Accent color override, hex like `#rrggbb` or `#rgb`. |
| `icon` | string? | — | SVG icon: embedded asset name, absolute path, or `~/` path. |

## Full example

```toml
# Aura configuration.
# Run `aura config describe` for full field docs, or
# `aura config set <key> <value>` to change a value from the CLI.

# ── Agent profiles ───────────────────────────────────────────────────────────
# Define as many profiles as you need. The active profile is tracked in state
# (state.json), not here — switching profiles in the UI does not touch this file.

[[agents]]
name = "Claude Code (Personal)"
kind = "claude-code"
# Path to the agent's config directory. Defaults to ~/.claude when omitted.
config_path = "~/.claude"

[[agents]]
name = "Claude Code (Enterprise)"
kind = "claude-code"
config_path = "~/.claude-enterprise"

[[agents]]
name = "Codex"
kind = "codex"
config_path = "~/.codex"

# ── Plugins ──────────────────────────────────────────────────────────────────
# Plugins are usually installed via `aura plugin add <path>`, which drops the
# binary into ~/.config/aura/plugins/ and registers it via auto-discovery — no
# [[plugins]] block needed. Use the inline form below only for plugins outside
# the user plugins dir, or to pin a color/icon onto a discovered plugin (a block
# with the same name wins over discovery).

[[plugins]]
name = "RTK Gains"
command = "aura-plugin-rtk"

# ── Display ──────────────────────────────────────────────────────────────────

[display]
# Which usage period tab is selected on open: "all" | "7d" | "30d".
default_period = "all"

# Explicit ordering for the plugin pill row. Named plugins render first in this
# order (case-insensitive match on `name`); the rest keep their natural order.
plugin_order = ["Hello", "RTK Gains"]

# How the modal anchors as it auto-fits height: "none" | "bottom" | "top".
# Default is per-OS and written at install (see "Modal anchoring" below).
anchor = "bottom"

# Native window chrome (title bar + drag-to-resize). Default false — Aura is a
# fixed-width tray popup that auto-fits its height. Turning this on disables the
# auto-fit and puts the modal in the taskbar / alt-tab list.
window_chrome = false

# Optional upper bound (logical px) on auto-fit height. Already capped at the
# screen work area; this is a tighter ceiling. Ignored when window_chrome = true.
# max_height = 500

# Auto-close the modal when it loses focus. Default true (tray-popup behaviour);
# set false to keep it open until the tray icon is clicked again.
dismiss_on_focus_loss = true

# Appear in Alt+Tab / Cmd+Tab / dock / panel surfaces. Default false (tray-only).
# Reapplies on the next refresh or open — no restart needed.
show_in_app_switcher = false

# Swap UI copy for the aggressive "Goblin Mode" variant. Default false.
goblin_mode = false

# ── Update ───────────────────────────────────────────────────────────────────

[update]
# Last release version dismissed via the update button's × (bare semver). A
# newer release re-shows the button. Omit for "never dismissed".
# dismissed_version = "0.1.18"

# Master mute: never render the update button, never call GitHub. Default false.
dismiss_all = false
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
**top** panel → `anchor = "top"`). Unrecognised values (including the legacy
`"auto"`) fall back to the per-OS default.

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

| `kind` | Description | Default `config_path` |
|---|---|---|
| `claude-code` | Claude Code CLI agent | `~/.claude` (dir containing `stats-cache.json` / `projects/`) |
| `codex` | OpenAI Codex CLI | `~/.codex` (dir containing `sessions/`) |
| `gemini` | Gemini CLI | `~/.gemini` |

A leading `~` in `config_path` is expanded to the user's home directory.

## State file

Aura writes the active profile selection to
`~/.local/share/aura/state.json`. This file is managed automatically —
`toggle_window` reloads it each time the modal opens, so a profile change made
in one session is visible the next time you click the tray icon. **Do not edit
by hand**; use `aura state set-profile <name>` (validated against
`config.agents`).

```json
{
  "active_profile": "Claude Code (Personal)"
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

### Color precedence

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

See `.design/customization.md` for the full theme schema reference.
