---
title: Configuration
status: current
version: 0.2.0
last_updated: 2026-09-24
last_verified: 2026-09-24
source_refs:
  - crates/aura-core/src/config.rs
  - crates/aura-core/src/config_schema.rs
  - crates/aura-core/src/keymap.rs
  - crates/aura-core/src/state.rs
  - crates/aura-core/src/sponsor.rs
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

## Quick tutorial

Most users should configure Aura with the CLI first, then use the config file
for fine tuning.

1. Run the setup wizard to detect installed agents and create/update the file:

   ```sh
   aura config setup
   ```

2. Use the interactive configuration wizard when you want to walk every
   supported setting without memorizing key names:

   ```sh
   aura config wizard
   ```

3. Use direct CLI edits for one setting at a time. The CLI validates values and
   rewrites the file with the inline comments preserved:

   ```sh
   aura config set window.anchor bottom
   aura config set tray.progress true
   aura config set content.default_period 7d
   ```

4. Open the config file when you want to edit multiple values together:

   ```sh
   aura config edit
   ```

   You can also right-click the tray icon and choose **Open config file**, or
   open Aura and use the settings button.

5. Keep this guide handy from the app: right-click the tray icon and choose
   **Configuration guide**, or use the **...** menu in the modal.

The generated `config.toml` starts with a link back to this tutorial and then
documents each field above the value it controls. Repeatable `[[agents]]` and
`[[plugins]]` blocks are ordinary TOML arrays of tables; scalar settings live
under `[window]`, `[tray]`, `[content]`, `[update]`, `[keybindings]`, and
`[sponsor]`.

## File locations

| File | Path | What it holds | Edited by |
|---|---|---|---|
| Config | `~/.config/aura/config.toml` | Agents, plugins, `[window]`, `[tray]`, `[content]`, `[update]`, `[keybindings]`, `[sponsor]` | You (CLI / editor) |
| Theme | `~/.config/aura/theme.toml` | Color / font / spinner overrides | You (CLI / editor) |
| Keybindings | `~/.config/aura/keybindings.toml` | Keyboard-shortcut overrides — see [keybindings.md](keybindings.md) | You (CLI / editor) |
| State | `~/.local/share/aura/state.json` | Active profile selection, modal height hint, first-run time, sponsor-nudge status | Aura (do not hand-edit) |
| Plugins dir | `~/.config/aura/plugins/` | Auto-discovered plugin binaries | `aura plugin add` |

Paths follow the XDG base-dir spec via the `dirs` crate, so the exact location
differs on macOS / Windows — always resolve it with `aura config path`. Aura
writes a fully-commented default `config.toml` on first run if none exists.

## The runtime model — config in layers

Config flows through five layers, top (authoring) to bottom (consumption):

1. **Typed structs** — `crates/aura-core/src/config.rs`. `AppConfig` is the
   root (`agents`, `plugins`, `window`, `tray`, `content`, `update`, `keybindings`, `sponsor`); each sub-struct derives
   `Serialize`/`Deserialize` and a `Default`, so the whole tree round-trips
   through TOML and an empty/partial file still parses (missing fields fall back
   to `Default`). This is the **source of truth** — the shape of a config is
   whatever these structs say it is.

2. **Field registry / schema** — `crates/aura-core/src/config_schema.rs`. A
   flat list of `FieldDescriptor`s (one per settable scalar in the
   `config_schema::SECTIONS` tables) plus `SectionField`s describing the
   repeatable `[[agents]]` /
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
   absent) and runs it through `config_migrate::normalize` first, so a config
   written against an older section layout parses as the current one;
   `load_with_discovery` additionally merges executable plugins found in the
   plugins dir (config-listed entries win on name collision) and applies
   `content.plugin_order`; `run_setup` detects installed agents and merges new
   ones without disturbing existing edits. See
   [Migrating an older config](#migrating-an-older-config).

5. **Runtime mirror** — `crates/aura/src/runtime.rs`. The tray poll loop in
   `main.rs` and the modal's async refresh task in `app.rs` each reload the
   config independently. To stop them drifting, a handful of `[window]` fields
   are mirrored into process atomics via `runtime::set_from_config`, and any
   platform state they drive (e.g. the macOS NSApp activation policy) is
   reapplied there. Add an atomic + accessor here when a new knob must be
   visible to *both* the background loop and the modal.

### Reload triggers (hot reload)

Most edits take effect **without a restart**. `set_from_config` is called, and
the config (and `theme.toml`) reloaded, at three moments:

- **Startup** — `main.rs` loads once before launching GPUI.
- **Every tray click** — the `Show` arm reloads via `load_with_discovery`, so a
  config edit (or a freshly-dropped plugin binary) is live on the next open.
- **Refresh button** — `app::do_refresh` reloads config + theme on a background
  thread; a malformed `theme.toml` logs a warning and falls back to defaults
  rather than blanking the UI.

`keybindings.toml` follows the same schedule: it is re-read, and the keymap
reinstalled, on every open and every refresh.

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
aura config set <key> <value>  # validate and set one field (e.g. set window.anchor top)
aura config wizard             # walk every field interactively; blank keeps current
aura config init [--force]     # write a fresh, fully-commented config.toml
aura config document           # rewrite the existing config in place with inline docs
aura config edit               # open in $EDITOR (creates defaults if missing)
aura config validate           # parse-check
```

Keys are dotted paths into `[window]` / `[tray]` / `[content]` / `[update]` /
`[keybindings]` / `[sponsor]`, e.g. `window.anchor`, `window.max_height`, `update.dismiss_all`. A key from an
older layout (`display.anchor`) still resolves — `get`, `set` and `describe`
answer with its current name and print a note. `set` rejects bad enums/booleans and
suggests near-miss keys; pass `none` (or empty) to clear an optional field. The
repeatable `[[agents]]` / `[[plugins]]` tables are *documented* by `describe`
but **edited** via `aura config edit`, `aura agents`, or `aura plugin` — they
are not `get`/`set` targets. The legacy `aura setup-config` is a hidden alias for
`aura config setup`. See [`docs/cli.md`](cli.md) for the full surface.

## Field reference

### `[window]`

Where the modal sits, how big it gets, and what kind of window it is.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `anchor` | string | `none` \| `bottom` \| `top` | `none` (macOS/Linux), `bottom` (Windows) | How the modal anchors as it auto-fits height. |
| `linux_backend` | string | `auto` \| `x11` \| `wayland` | `auto` | Which display server GPUI talks to on Linux/BSD. Ignored elsewhere. |
| `show_in_app_switcher` | bool | `true` \| `false` | `false` | Show the modal in Alt+Tab / Cmd+Tab / dock surfaces. |
| `dismiss_on_focus_loss` | bool | `true` \| `false` | `true` | Auto-close the modal when it loses focus. |
| `chrome` | bool | `true` \| `false` | `false` | Show the native window title bar (independent of `auto_resize`). |
| `auto_resize` | bool? | `true` \| `false` | unset (auto-fit) | Auto-resize the modal to fit its content height. `false` = fixed-size. Works with or without chrome. |
| `max_height` | u32? | — | unset | Upper bound (logical px) on auto-fit height; ignored when `auto_resize` is false. |

`linux_backend` lives here rather than in a platform section because it exists
entirely to decide whether Aura can place its own window — on a native Wayland
surface `anchor` has no effect at all. See
[Linux display backend](#linux-display-backend).

### `[tray]`

The icon by the clock. None of this reaches the modal.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `indicator` | bool | `true` \| `false` | `true` | Whether the icon reports quota, or is just a button that opens the modal. Master switch for the three below. |
| `progress` | bool | `true` \| `false` | `true` | Fill the tray icon's ring in proportion to peak quota usage. |
| `color` | bool | `true` \| `false` | `true` | Move the tray icon through the purple/yellow/orange/red usage ramp. |
| `pulse` | bool | `true` \| `false` | `false` | Ask the desktop to emphasize the tray icon at 90% usage. Effective on Linux SNI hosts. |
| `refresh_secs` | u64 | — | `1200` | Seconds between background refreshes of the indicator; clamped up to 30. |

### `[content]`

What the modal renders, as opposed to where the window sits.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `default_period` | string | `all` \| `7d` \| `30d` | `all` | Usage period tab selected on open. |
| `plugin_order` | string[] | — | `[]` | Display order for plugin pills (comma-separated names on `set`). |
| `goblin_mode` | bool | `true` \| `false` | `false` | Swap UI copy for the aggressive "Goblin Mode" variant. |

### `[update]`

Controls the "Update available" header button.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `dismissed_version` | string? | — | unset | Last release dismissed via the button's ×; a newer release re-shows it. |
| `dismiss_all` | bool | `true` \| `false` | `false` | Master mute: never render the button or fire the GitHub check. |

### `[keybindings]`

Master switch for the modal's keyboard shortcuts. The bindings themselves live
in `keybindings.toml` — see [keybindings.md](keybindings.md).

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `enabled` | bool | `true` \| `false` | `true` | Install the keymap (vim-style defaults + `keybindings.toml`). `false` leaves the modal mouse-only; Escape still closes it. |

### `[sponsor]`

Opt-out for the one-time sponsor card. Seven days after Aura first runs, the
modal shows a small card under the header asking you to consider sponsoring,
with **Sponsor on GitHub** (opens <https://github.com/sponsors/Rfluid>), **Pix
(BRL)** (opens <https://livepix.gg/rfluid>) and a **×** to dismiss. The sponsor
links leave the card open, so you can use both; only the × retires it for
good. The first-run time and the "dismissed"
flag live in `state.json`, not here — an install upgraded from a
build without them starts its week on the first launch after the upgrade.

| Key | Type | Allowed | Default | Summary |
|---|---|---|---|---|
| `nudge` | bool | `true` \| `false` | `true` | Show the one-time sponsor card a week after the first run. `false` never shows it. |

### `[[agents]]` (repeatable)

| Field | Type | Allowed | Summary |
|---|---|---|---|
| `name` | string | — | Display name for this agent profile. |
| `kind` | string | `claude-code` \| `codex` \| `gemini` \| `antigravity` | Which agent this profile reads. |
| `config_path` | string? | — | Agent config dir; defaults to `~/.claude`, `~/.codex`, `~/.gemini`, `~/.gemini/antigravity-cli` per kind. |
| `command` | string? | — | Executable for agents Aura reads by running them (`antigravity`). Unset = the agent's usual binary name on `$PATH`. |
| `color` | string? | — | Accent color override, hex like `#rrggbb` or `#rgb`. |
| `tray_progress_source` | u32? | — | Quota window that fills the tray ring, by position. Unset = `0`, the session. |
| `tray_color_source` | u32? | — | Quota window that drives the tray color ramp, by position. Unset = `1`, the week. |

The two `tray_*_source` selectors point the halves of the tray indicator at
different quota windows. Out of the box the ring is the session you're in and
the color is the week you're spending; set them to repoint either half:

```toml
[[agents]]
name = "Claude Code"
kind = "claude-code"
tray_progress_source = 0   # ring   ← Current session          (the default)
tray_color_source = 1      # color  ← Current week, all models (the default)
```

They live on the agent rather than under `[tray]` because the positions
index that agent's own window list: position 1 is Claude's all-models week and
Codex's weekly limit, and a Gemini profile reports no percentages at all.

Backends emit only the windows they actually have — an idle Claude session has
no 5h window, a plan without Opus has no Opus week — so positions shift. A
selector past the end, or one landing on a window with no percentage, falls
back to the peak across every window rather than blanking the icon; that is
also what puts the ramp on the only window a single-window agent reports. Note
that only the two selected windows reach the icon: a third window running out
shows up in the tooltip, not the ring.

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

# ── Window ────────────────────────────────────────────────────────

[window]
# How the modal anchors as it auto-fits height: "none" | "bottom" | "top".
# Default is per-OS and written at install (see "Modal anchoring" below).
anchor = "bottom"

# Which display server GPUI talks to on Linux/BSD: "auto" | "x11" | "wayland".
# "auto" (default) prefers X11 whenever $DISPLAY is set — via XWayland on a
# Wayland session — because Wayland forbids a client from positioning its own
# window, which disables `anchor` entirely. See "Linux display backend" below.
linux_backend = "auto"

# Appear in Alt+Tab / Cmd+Tab / dock / panel surfaces. Default false (tray-only).
# Reapplies on the next refresh or open — no restart needed.
show_in_app_switcher = false

# Auto-close the modal when it loses focus. Default true (tray-popup behaviour);
# set false to keep it open until the tray icon is clicked again.
dismiss_on_focus_loss = true

# Show the native window title bar. Default false — Aura is a chromeless tray
# popup. Turning this on also puts the modal in the taskbar / alt-tab list.
# Independent of `auto_resize`.
chrome = false

# Auto-resize the modal to fit its content height. Unset (default) = auto-fit on.
# Set false for a fixed-size modal. Works the same with or without `chrome`.
# (This is not user drag-to-resize — the window manager owns that.)
# auto_resize = false

# Optional upper bound (logical px) on auto-fit height. Already capped at the
# screen work area; this is a tighter ceiling. Ignored when auto_resize = false.
# max_height = 500

# ── Tray ──────────────────────────────────────────────────────────

[tray]
# Whether the icon reports quota or is just a button that opens the modal.
# Default true. While the modal is closed this costs one quota lookup per
# `refresh_secs` (a network request for the API-backed agents); set false to
# leave the icon static.
indicator = true

# Fill the icon's complete ring to the highest quota-window usage. Default true.
# Ignored when indicator is false.
progress = true

# Change the icon from purple to yellow at 50%, orange at 75%, and red at 90%.
# Default true. Ignored when indicator is false.
color = true

# Ask the desktop to emphasize the icon at 90%. Default false because Linux
# panels may animate it or pull it out of the overflow group. SNI/Linux only;
# the color remains the attention signal on macOS and Windows.
pulse = false

# Seconds between background refreshes of the indicator. Ignored when indicator
# is false. Values below 30 are clamped up to 30.
refresh_secs = 1200

# ── Content ────────────────────────────────────────────────────

[content]
# Which usage period tab is selected on open: "all" | "7d" | "30d".
default_period = "all"

# Explicit ordering for the plugin pill row. Named plugins render first in this
# order (case-insensitive match on `name`); the rest keep their natural order.
plugin_order = ["Hello", "RTK Gains"]

# Swap UI copy for the aggressive "Goblin Mode" variant. Default false.
goblin_mode = false

# ── Update ───────────────────────────────────────────────────────────────────

[update]
# Last release version dismissed via the update button's × (bare semver). A
# newer release re-shows the button. Omit for "never dismissed".
# dismissed_version = "0.1.18"

# Master mute: never render the update button, never call GitHub. Default false.
dismiss_all = false

[sponsor]
# Show the one-time sponsor card a week after the first run. Default true.
nudge = true
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
somewhere other than your platform's default. Linux defaults to `none` because
panel placement varies so much there — a top-panel GNOME session and a
bottom-panel Plasma one are equally normal — so a Linux desktop with a bottom
panel wants `anchor = "bottom"` and one with a top panel `anchor = "top"`.
Unrecognised values (including the legacy `"auto"`) fall back to the per-OS
default.

Horizontally the modal follows each platform's own tray popups: macOS and
Linux centre it on the tray icon (clamped to stay on screen), Windows
right-aligns it to the screen edge the way its volume / network flyouts do.
Opening from the tray menu's **Show Aura** entry carries no click position, so
that path falls back to the corner on every platform.

**Linux note:** `anchor = "bottom"` repositions live — after each resize Aura
asks the window manager to move the modal via an EWMH
`_NET_MOVERESIZE_WINDOW` request (a plain `ConfigureWindow` is ignored by KWin
for a managed top-level), so the modal hugs the bottom taskbar as it
grows/shrinks. This needs an X11 connection; see
[Linux display backend](#linux-display-backend-linux_backend) for how that is
arranged on a Wayland session, and what you lose if you opt out. We currently
detect only *bottom* panel reservations, so `top` on a Linux top-panel setup
approximates by sitting at the very top of the display.

> **KDE Plasma:** if the modal *visibly stretches/animates* over ~0.5s as it
> resizes, that is KWin's Morphing Popups effect, not Aura — see
> [Troubleshooting: modal stretches on resize](troubleshooting/modal-stretches-on-resize-kde.md).

### Linux display backend (`linux_backend`)

Aura's placement — `anchor`, keeping clear of the taskbar, centring the modal
under the tray icon — all depends on the app choosing its own window position.
**Wayland does not allow that.** An `xdg_toplevel` surface has no position in
the protocol, so on a native Wayland session the compositor puts the modal
wherever it likes, `anchor` does nothing, and an auto-hidden panel can slide
out on top of the window.

X11 has no such restriction, and every mainstream Wayland desktop ships
XWayland, so Aura prefers GPUI's X11 backend whenever `$DISPLAY` resolves:

| Value | Behavior |
|---|---|
| `auto` (default) | Use X11 whenever `$DISPLAY` is set — through XWayland on a Wayland session. Full placement control. |
| `x11` | Same, but also warns on stderr when there is no `$DISPLAY` to use. |
| `wayland` | Keep the native Wayland backend. The compositor owns placement; `anchor` and icon-centring stop having an effect. |

Pick `wayland` if XWayland output looks soft on a fractional-scale display,
and place the modal with a compositor window rule instead (KDE: **System
Settings → Window Management → Window Rules**, window class substring `aura`,
property **Position** → *Apply Initially* / *Force*).

Mechanically, GPUI picks its Linux backend in `guess_compositor()`, which
takes Wayland whenever `$WAYLAND_DISPLAY` is non-empty and offers no override.
Aura therefore hides that variable across the single `Application::new()` call
and restores it immediately after, so plugin commands and `xdg-open` still see
the real session environment. The field is ignored on macOS and Windows.

## Migrating an older config

Aura's section layout can change between releases. Reorganizing it must never
mean "everyone's settings silently revert to defaults", so every key that moves
is recorded as a declarative migration in
`crates/aura-core/src/config_migrate.rs`, and that one registry drives
everything:

- **`AppConfig::load` normalizes in memory on every launch.** An un-migrated
  `config.toml` keeps working exactly as before; Aura does not rewrite the
  user's file behind their back.
- **Old key names keep resolving in the CLI.** `aura config get
  display.tray_color` answers with `tray.color` and prints a note saying where
  the key went. Same for `set` and `describe`.
- **`aura config migrate` rewrites the file** into the current layout, carrying
  every value over. It is idempotent, so running it on a current config just
  says so. The installer runs it on every install and upgrade.
- **`aura doctor` and `aura config validate` report a pending migration**, so
  you find out without having to know the command exists.

```bash
aura config migrate --check   # report what would change; exit 1 if anything is pending
aura config migrate           # rewrite config.toml (also refreshes the inline docs)
```

A key the migration has no descriptor for — something you hand-wrote into a
section that moved — is reported and left alone in the file, but note that the
rewrite re-serializes from the parsed struct, so it does **not** survive
`migrate`. The `--check` output names any such key before you commit to the
rewrite.

### What moved in the `[display]` split

`[display]` had grown to cover four unrelated jobs. It now means only *where
the window goes*, under the clearer name `[window]`:

| Old key | New key |
|---|---|
| `display.anchor` | `window.anchor` |
| `display.linux_backend` | `window.linux_backend` |
| `display.show_in_app_switcher` | `window.show_in_app_switcher` |
| `display.dismiss_on_focus_loss` | `window.dismiss_on_focus_loss` |
| `display.window_chrome` | `window.chrome` |
| `display.auto_resize` | `window.auto_resize` |
| `display.max_height` | `window.max_height` |
| `display.tray_status` | `tray.indicator` |
| `display.tray_progress` | `tray.progress` |
| `display.tray_color` | `tray.color` |
| `display.tray_pulse` | `tray.pulse` |
| `display.tray_status_interval_secs` | `tray.refresh_secs` |
| `display.default_period` | `content.default_period` |
| `display.plugin_order` | `content.plugin_order` |
| `display.goblin_mode` | `content.goblin_mode` |

`tray_status` became `tray.indicator` because "enabled" would read as "is there
a tray icon at all"; what it actually toggles is whether the icon is a live
indicator or a plain button that opens the modal.

## Agent kinds

| `kind` | Description | Default `config_path` |
|---|---|---|
| `claude-code` | Claude Code CLI agent | `~/.claude` (dir containing `stats-cache.json` / `projects/`) |
| `codex` | OpenAI Codex CLI | `~/.codex` (dir containing `sessions/`) |
| `gemini` | Gemini CLI | `~/.gemini` |
| `antigravity` | Google Antigravity CLI (`agy`) | `~/.gemini/antigravity-cli` (dir containing `conversation_summaries.db`) |

A leading `~` in `config_path` is expanded to the user's home directory.

`antigravity` shares `~/.gemini` with the Gemini CLI but reads a disjoint
subtree, so the two profiles can both be configured without colliding.

### The `command` key

Most agents are read purely off disk. Antigravity is not: `agy` keeps its
OAuth credentials in the OS keyring and its quota RPC is gated on the CLI's
own client identity, so Aura gets quota by running
`agy -p "/usage" --output-format json` — a documented public flag. The call
is free (it starts no LLM turn and consumes no quota) and its result is
cached for 20 s so a tray tick and a modal open share one reading.

Aura resolves `agy` to an absolute path before spawning it, searching ahead
of the inherited `$PATH`:

| Platform | Searched |
|---|---|
| Linux / macOS | `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin`, `~/bin`, `/opt/homebrew/bin`, `/usr/local/bin` |
| Windows | `%LOCALAPPDATA%\agy\bin`, plus the same per-user directories where they exist |

That is deliberate. Aura runs from a GUI launcher, a systemd user unit or a
launchd agent, and none of those source your shell rc files:

- **Linux** — the systemd user manager's `PATH` typically omits
  `~/.local/bin` entirely, which is exactly where `agy`'s installer puts the
  binary.
- **macOS** — launchd hands a bundled app only
  `/usr/bin:/bin:/usr/sbin:/sbin`, so neither `~/.local/bin` (where `agy`
  lands) nor Homebrew is visible. The `agy` installer adds its directory by
  appending to your *shell profile*, which a launchd agent never reads.
- **Windows** — `agy` installs to `%LOCALAPPDATA%\agy\bin` and registers it
  in the user `PATH`, but a process started before the install, or a user who
  ran the installer with `--skip-path`, won't see it.

In every case the binary resolves fine from a terminal and is invisible to
the tray process, which makes this a confusing failure to hit.

`command` points that at an install outside all of those:

```toml
[[agents]]
name = "Antigravity"
kind = "antigravity"
command = "/opt/antigravity/bin/agy"
```

When `agy` is missing or you are not logged in, the Quota tab shows an
`unavailable` note explaining which — the tray keeps its last good reading
rather than degrading.

Antigravity reports no token counts at all (its trajectories are
schema-less protobuf). The modal drops the **Models** tab entirely for it —
both the tokens-per-day chart and the per-model bars are token-derived, so
the page would have nothing on it — and the token stat cards on Summary read
"not reported" rather than `0`. Sessions, messages, active days, streaks and
peak hour all work normally.

## State file

Aura writes the active profile selection — plus a few facts it records about
itself (the modal's last settled height, when it first ran, and whether the
sponsor card has been dismissed) — to `~/.local/share/aura/state.json`. This
file is managed automatically — `toggle_window` reloads it each time the modal
opens, so a profile change made in one session is visible the next time you
click the tray icon. **Do not edit by hand**; use `aura state set-profile <name>` (validated against
`config.agents`).

```json
{
  "active_profile": "Claude Code (Personal)",
  "modal_height": 409,
  "first_run": "2026-09-01T12:00:00Z",
  "sponsor_nudge_done": false
}
```

`aura state clear` resets only the profile selection; the first-run time and
sponsor-card flag are kept. To silence the sponsor card, use
`aura config set sponsor.nudge false` rather than editing this file.

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
