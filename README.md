<h1 align="center">Aura</h1>

<p align="center">
  <img src="assets/icons/aura-mark.svg" alt="Aura" width="96" height="96"/>
</p>

<p align="center">
  <strong>Agent Usage Reporter &amp; Analyzer</strong><br/>
  <em>Know exactly what your AI agents are spending.</em>
</p>

<hr/>

Aura is a lightweight Rust desktop widget that lives in your taskbar and gives you instant visibility into AI agent usage: tokens consumed, estimated costs, and custom optimizer metrics via a plugin system. Switch between agent profiles (Claude Code Personal, Enterprise, Codex, and more) with one click.

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

## Features

- **Multi-agent support** — Claude Code and Codex out of the box; custom command agents on the roadmap
- **Agent profiles** — configure multiple instances of the same agent (e.g., personal vs. enterprise workspaces) and toggle between them; last selection is persisted across sessions
- **Plugin system** — extend Aura with custom metrics panels; anyone can author a plugin; ships with the RTK Gains plugin
- **RTK Gains plugin** — surfaces token savings from the [RTK](https://github.com/rtk) optimizer directly alongside your usage stats
- **Minimal footprint** — a small modal anchored near your taskbar widget; appears on click, disappears on blur

## Plugins

Plugins live outside the core codebase and load at runtime. Aura ships with:

| Plugin        | Description                                                                                                        |
| ------------- | ------------------------------------------------------------------------------------------------------------------ |
| **RTK Gains** | Shows tokens saved by the Rust Token Killer (RTK) optimizer — how much you spent vs. how much you would have spent |

Plugins expose a simple trait interface. Authors can package them as shared libraries and distribute them independently of Aura.

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

## Roadmap

- [ ] Claude Code usage integration
- [ ] Codex usage integration
- [ ] RTK Gains plugin (built-in)
- [ ] Custom command agents (BYOA — Bring Your Own Agent)
- [ ] Plugin authoring guide + example plugin
- [ ] macOS support
- [ ] Plugin registry

## Installation

Aura runs as a systemd user service on Linux. You can either grab a prebuilt
release archive or build from source with Cargo (Rust 1.80+).

### System dependencies (Linux)

```bash
# Debian/Ubuntu
sudo apt install build-essential pkg-config libgtk-3-dev \
                 libxkbcommon-x11-dev libxcb1-dev libxcb-render0-dev \
                 libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig-dev
```

The runtime dependencies (`libgtk-3-0`, `libxkbcommon-x11-0`, `libxcb1`,
`libxcb-render0`, `libxcb-shape0`, `libxcb-xfixes0`, `libfontconfig1`) are also
needed at install-from-release time — the `-dev` packages above cover them.

### Install from GitHub Releases

Published releases include tarballs containing the `aura` and `aura-plugin-rtk`
binaries plus the systemd unit (Linux archives only):

- Linux x86_64 (gnu) — `x86_64-unknown-linux-gnu`
- Linux x86_64 (musl) — `x86_64-unknown-linux-musl`
- Linux aarch64 (gnu) — `aarch64-unknown-linux-gnu`
- macOS Intel — `x86_64-apple-darwin`
- macOS Apple Silicon — `aarch64-apple-darwin`

> Note: macOS and musl Linux artifacts are currently published as experimental
> targets — until GPUI / `tray-icon` cross-platform support lands they may be
> missing from a given release. Use the `gnu` Linux archive for a known-good
> install.

Pick the archive that matches your host:

```bash
VERSION="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
  https://github.com/Rfluid/aura/releases/latest \
  | sed 's:.*/::')"

if [ -z "${VERSION}" ]; then
  echo "Failed to determine the latest GitHub release version" >&2
  exit 1
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  ASSET="aura-${VERSION}-x86_64-unknown-linux-gnu" ;;
  Linux-aarch64) ASSET="aura-${VERSION}-aarch64-unknown-linux-gnu" ;;
  Darwin-x86_64) ASSET="aura-${VERSION}-x86_64-apple-darwin" ;;
  Darwin-arm64)  ASSET="aura-${VERSION}-aarch64-apple-darwin" ;;
  *)
    echo "No published release artifact for $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

curl -LO "https://github.com/Rfluid/aura/releases/download/${VERSION}/${ASSET}.tar.gz"
curl -LO "https://github.com/Rfluid/aura/releases/download/${VERSION}/${ASSET}.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "${ASSET}.sha256"
else
  shasum -a 256 -c "${ASSET}.sha256"
fi

tar -xzf "${ASSET}.tar.gz"

install -Dm755 "${ASSET}/aura"            "${HOME}/.local/bin/aura"
install -Dm755 "${ASSET}/aura-plugin-rtk" "${HOME}/.local/bin/aura-plugin-rtk"

if [ "$(uname -s)" = "Linux" ] && [ -f "${ASSET}/aura.service" ]; then
  install -Dm644 "${ASSET}/aura.service" "${HOME}/.config/systemd/user/aura.service"
  systemctl --user daemon-reload
  systemctl --user enable --now aura
fi
```

Make sure `~/.local/bin` is on `PATH`.

### Build from source

#### One-shot install

```bash
./install.sh        # builds + installs to ~/.local/bin + sets up systemd unit
```

Or, with [`just`](https://github.com/casey/just):

```bash
just install                 # build + install everything
systemctl --user enable --now aura
```

#### Manual

```bash
cargo build --release --workspace
install -Dm755 target/release/aura            ~/.local/bin/aura
install -Dm755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
install -Dm644 packaging/aura.service         ~/.config/systemd/user/aura.service
systemctl --user daemon-reload
systemctl --user enable --now aura
```

### Common commands

| Command                     | What it does                                  |
| --------------------------- | --------------------------------------------- |
| `just run`                  | Launch Aura (debug build) without installing  |
| `just status` / `just logs` | Service status / tail journal                 |
| `just uninstall`            | Remove binaries + unit (keeps config & state) |

## Sponsor

See [SPONSOR.md](./SPONSOR.md) for ways to support Aura.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for local setup, development workflow,
and pull request guidance.

## License

MIT
