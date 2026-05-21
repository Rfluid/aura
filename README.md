# Aura

> **Agent Usage Reporter & Analyzer** — know exactly what your AI agents are spending.

Aura is a lightweight Rust desktop widget that lives in your taskbar and gives you instant visibility into AI agent usage: tokens consumed, estimated costs, and custom optimizer metrics via a plugin system. Switch between agent profiles (Claude Code Personal, Enterprise, Codex, and more) with one click.

---

## Why Aura?

Modern development workflows run on AI agents. But usage is invisible until the bill arrives. Aura surfaces that data where you already live — your taskbar — without switching context, opening a browser, or running a CLI command.

```
┌────────────────────────────────────────────────────┐
│  ◉ Aura                    Claude (Personal)  ▾    │
├────────────────────────────────────────────────────┤
│  All time · Last 7 days · Last 30 days             │
├────────────────────────────────────────────────────┤
│  [ Overview ]  [ Models ]                          │
│                                                    │
│  Favorite model:  Claude Opus 4.7                  │
│  Total tokens:    2,847,391                        │
│                                                    │
│  Sessions:      94     Longest:  2h 15m            │
│  Active days:   42/90  Peak hr:  15:00–16:00       │
│  Cur. streak:   3 days Longest:  12 days           │
├────────────────────────────────────────────────────┤
│  ⚡ RTK Gains — saved 1.2M tokens today            │
└────────────────────────────────────────────────────┘
```

---

## Features

- **Multi-agent support** — Claude Code and Codex out of the box; custom command agents on the roadmap
- **Agent profiles** — configure multiple instances of the same agent (e.g., personal vs. enterprise workspaces) and toggle between them; last selection is persisted across sessions
- **Plugin system** — extend Aura with custom metrics panels; anyone can author a plugin; ships with the RTK Gains plugin
- **RTK Gains plugin** — surfaces token savings from the [RTK](https://github.com/rtk) optimizer directly alongside your usage stats
- **Minimal footprint** — a small modal anchored near your taskbar widget; appears on click, disappears on blur

---

## Plugins

Plugins live outside the core codebase and load at runtime. Aura ships with:

| Plugin | Description |
|---|---|
| **RTK Gains** | Shows tokens saved by the Rust Token Killer (RTK) optimizer — how much you spent vs. how much you would have spent |

Plugins expose a simple trait interface. Authors can package them as shared libraries and distribute them independently of Aura.

---

## Configuration

Aura is configured via a single TOML file (`~/.config/aura/config.toml`). Define as many profiles as you need:

```toml
[[agents]]
name = "Claude Code (Personal)"
kind = "claude-code"
config_path = "~/.claude"

[[agents]]
name = "Claude Code (Enterprise)"
kind = "claude-code"
config_path = "~/.claude-enterprise"

[[agents]]
name = "Codex"
kind = "codex"
```

---

## Roadmap

- [ ] Claude Code usage integration
- [ ] Codex usage integration
- [ ] RTK Gains plugin (built-in)
- [ ] Custom command agents (BYOA — Bring Your Own Agent)
- [ ] Plugin authoring guide + example plugin
- [ ] macOS support
- [ ] Plugin registry

---

## Installation

Aura builds from source via Cargo. Rust 1.80+ is required.

### System dependencies (Linux)

```bash
# Debian/Ubuntu
sudo apt install build-essential pkg-config libgtk-3-dev \
                 libxkbcommon-x11-dev libxcb1-dev libxcb-render0-dev \
                 libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig-dev
```

### One-shot install

```bash
./install.sh        # builds + installs to ~/.local/bin + sets up systemd unit
```

Or, with [`just`](https://github.com/casey/just):

```bash
just install                 # build + install everything
systemctl --user enable --now aura
```

### Manual

```bash
cargo build --release --workspace
cp target/release/aura            ~/.local/bin/
cp target/release/aura-plugin-rtk ~/.local/bin/
cp packaging/aura.service         ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now aura
```

### Common commands

| Command | What it does |
|---|---|
| `just run` | Launch Aura (debug build) without installing |
| `just status` / `just logs` | Service status / tail journal |
| `just uninstall` | Remove binaries + unit (keeps config & state) |

## Contributing

_Coming soon — see `CONTRIBUTING.md`_

## License

MIT
