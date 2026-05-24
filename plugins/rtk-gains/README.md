# RTK Gains plugin for Aura

Surfaces token savings from the [RTK (Rust Token Killer)][rtk]
optimizer inside Aura's tray modal. Shows what you actually spent vs.
what you *would* have spent without RTK rewriting your shell commands.

## What it shows

| Section          | Content                                                                 |
| ---------------- | ----------------------------------------------------------------------- |
| **Overview**     | Total commands intercepted, input/output tokens, tokens saved + rate, total/avg exec time, saved today, saved this month, projected monthly savings, efficiency meter (0.0–1.0) |
| **By Command**   | Per-command table of count, saved tokens, savings %, and an impact bar |

All figures come from `rtk gain -a --format json`; this plugin is a
thin presentation layer over that subcommand.

## Requirements

- [RTK][rtk] installed and on `$PATH` (the plugin shells out to `rtk`
  at every modal open). Verify with `rtk --version`.
- Aura ≥ 0.1.9 (the version that ships the `aura plugin add` CLI).

If `rtk` is not on `PATH`, the plugin renders an error message in the
panel — Aura keeps working, only this plugin's tab shows the error.

## Install

### Option A — `aura plugin add` (recommended)

Build from this workspace and hand the binary to Aura:

```bash
# From the repo root
cargo build --release -p aura-plugin-rtk
aura plugin add ./target/release/aura-plugin-rtk \
    --name "RTK Gains" \
    --color "#f59e0b" \
    --icon "icons/rtk.svg"
```

That copies the binary into `~/.config/aura/plugins/` and writes a
sidecar TOML next to it so the display name, accent color, and icon
persist. The next modal open picks up the new plugin automatically —
no restart needed.

For active development (rebuild → refresh):

```bash
aura plugin add ./target/release/aura-plugin-rtk --link \
    --name "RTK Gains" --color "#f59e0b" --icon "icons/rtk.svg"
```

`--link` symlinks instead of copying (Unix only), so re-running
`cargo build` updates the live plugin in place.

### Option B — drop-in

```bash
cargo build --release -p aura-plugin-rtk
mkdir -p ~/.config/aura/plugins
install -m 755 target/release/aura-plugin-rtk \
    ~/.config/aura/plugins/aura-plugin-rtk

# Optional metadata sidecar
cat > ~/.config/aura/plugins/aura-plugin-rtk.toml <<'EOF'
name  = "RTK Gains"
color = "#f59e0b"
icon  = "icons/rtk.svg"
EOF
```

Without the sidecar, the display name is derived from the binary
filename (`aura-plugin-rtk` → "Rtk").

### Option C — `[[plugins]]` in `config.toml`

If you've installed `aura-plugin-rtk` system-wide (e.g. `/usr/local/bin/`):

```toml
# ~/.config/aura/config.toml

[[plugins]]
name    = "RTK Gains"
command = "aura-plugin-rtk"  # or absolute path
color   = "#f59e0b"
icon    = "icons/rtk.svg"
```

## Verify

```bash
aura plugin list
# NAME                     SOURCE       COMMAND
# RTK Gains                discovered   /home/you/.config/aura/plugins/aura-plugin-rtk

# Manual run — same way Aura invokes it:
~/.config/aura/plugins/aura-plugin-rtk --period all | jq .
```

Click the Aura tray icon → switch to the **Plugins** mode → the
**RTK Gains** pill should be present with the orange accent.

## Uninstall

```bash
aura plugin remove "RTK Gains"
```

Or manually:

```bash
rm ~/.config/aura/plugins/aura-plugin-rtk \
   ~/.config/aura/plugins/aura-plugin-rtk.toml
```

## Per-OS plugins directory

| Platform | Path                                                |
| -------- | --------------------------------------------------- |
| Linux    | `~/.config/aura/plugins/`                           |
| macOS    | `~/Library/Application Support/aura/plugins/`       |
| Windows  | `%APPDATA%\aura\plugins\`                           |

## Authoring your own plugin

See [`docs/plugin-authoring.md`](../../docs/plugin-authoring.md) for
the full wire contract. This plugin is also a useful real-world
reference — it parses external JSON, formats numbers compactly, and
emits both `lines` and `table` sections.

[rtk]: https://github.com/rtk
