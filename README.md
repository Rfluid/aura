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

- **Multi-agent support** — Claude Code, Codex, and Gemini out of the box; custom command agents on the roadmap
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

Aura is configured via a single TOML file at the OS-standard config location:

| Platform | Config path |
|---|---|
| Linux | `~/.config/aura/config.toml` |
| macOS | `~/Library/Application Support/aura/config.toml` |
| Windows | `%APPDATA%\aura\config.toml` |

Define as many profiles as you need:

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

[[agents]]
name = "Gemini"
kind = "gemini"
```

## Roadmap

- [ ] Claude Code usage integration
- [ ] Codex usage integration
- [ ] RTK Gains plugin (built-in)
- [ ] Custom command agents (BYOA — Bring Your Own Agent)
- [ ] Plugin authoring guide + example plugin
- [x] macOS support (Apple Silicon + Intel; menu-bar app + launchd autostart)
- [x] Windows support (x86_64 + aarch64; tray app + Startup-folder autostart)
- [ ] Plugin registry

## Installation

Aura runs as a systemd user service on Linux, as a menu-bar app (launchd
LaunchAgent + `Aura.app`) on macOS, and as a tray app with a Startup-folder
shortcut on Windows. You can either grab a prebuilt release archive or build
from source with Cargo (Rust 1.80+).

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

### System dependencies (macOS)

```bash
xcode-select --install                  # AppKit / linker
brew install librsvg                    # only needed if you want a real .icns
```

The release tarball is unsigned, so on first launch macOS will quarantine it.
Either right-click `Aura.app` → **Open** and confirm, or strip the flag:

```bash
xattr -dr com.apple.quarantine /Applications/Aura.app
```

Claude Code OAuth tokens are stored in the macOS Keychain on Darwin (service
`Claude Code-credentials`); Aura reads them from there automatically. If you
get a "Keychain read failed" warning, run Claude Code at least once to
populate the entry, then restart Aura.

### System dependencies (Windows)

No system packages required — the MSVC C runtime ships with Windows. To
build from source you'll need:

```powershell
# Install Rust (https://rustup.rs) and the MSVC build tools
# (Visual Studio "Desktop development with C++" workload or VS Build Tools).
rustup target add x86_64-pc-windows-msvc      # default on Windows hosts
```

On Windows Claude Code stores OAuth tokens in **Credential Manager** (target
`Claude Code-credentials`); Aura reads them from there automatically, falling
back to `%USERPROFILE%\.claude\.credentials.json` for legacy installs. If you
get a "Credential Manager read failed" warning, run Claude Code at least once
to populate the entry, then restart Aura.

### Install from GitHub Releases

Published releases include tarballs (Linux/macOS) and a zip (Windows)
containing the `aura` and `aura-plugin-rtk` binaries plus the platform
autostart artifact:

- Linux x86_64 (gnu) — `x86_64-unknown-linux-gnu`
- Linux x86_64 (musl) — `x86_64-unknown-linux-musl`
- Linux aarch64 (gnu) — `aarch64-unknown-linux-gnu`
- macOS Intel — `x86_64-apple-darwin` (ships `Aura.app` + `com.aura.agent-usage.plist`)
- macOS Apple Silicon — `aarch64-apple-darwin`
- Windows x86_64 — `x86_64-pc-windows-msvc` (ships `aura.exe` + `install.ps1`)
- Windows aarch64 — `aarch64-pc-windows-msvc` (experimental)

> Note: the musl Linux artifact and the aarch64 Windows artifact are still
> experimental. macOS archives are built and signed best-effort — unsigned
> bundles will be quarantined by Gatekeeper; see the macOS deps section above.
> Windows binaries are unsigned: SmartScreen may prompt on first run.

Run the installer — it auto-detects your host, downloads the matching archive,
verifies its checksum, and wires up autostart:

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/Rfluid/aura/main/install.sh | bash
```

```powershell
# Windows (PowerShell)
iex (irm https://raw.githubusercontent.com/Rfluid/aura/main/scripts/install.ps1)
```

Make sure `~/.local/bin` (Linux/macOS) or `%LOCALAPPDATA%\Programs\Aura`
(Windows) is on `PATH`.

### Build from source

#### One-shot install

```bash
./install.sh        # auto-detects Linux/macOS and wires up autostart
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

Or, with [`just`](https://github.com/casey/just):

```bash
just install                 # build + install (Linux: systemd, macOS: launchd + .app)
# Linux only — the launchd agent is loaded automatically on macOS:
systemctl --user enable --now aura
```

```powershell
just install-windows         # build + install + Startup-folder shortcut
```

#### Manual (Linux)

```bash
cargo build --release --workspace
install -Dm755 target/release/aura            ~/.local/bin/aura
install -Dm755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
install -Dm644 packaging/aura.service         ~/.config/systemd/user/aura.service
systemctl --user daemon-reload
systemctl --user enable --now aura
```

#### Manual (macOS)

```bash
cargo build --release --workspace
install -m 755 target/release/aura            ~/.local/bin/aura
install -m 755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk

./scripts/build-macos-app.sh target/release/aura target/release
cp -R target/release/Aura.app /Applications/Aura.app

install -m 644 packaging/com.aura.agent-usage.plist \
    ~/Library/LaunchAgents/com.aura.agent-usage.plist
launchctl bootstrap "gui/$(id -u)" \
    ~/Library/LaunchAgents/com.aura.agent-usage.plist
```

#### Manual (Windows)

```powershell
cargo build --release --workspace

$dst = Join-Path $env:LOCALAPPDATA 'Programs\Aura'
New-Item -ItemType Directory -Force -Path $dst | Out-Null
Copy-Item -Force target\release\aura.exe            "$dst\aura.exe"
Copy-Item -Force target\release\aura-plugin-rtk.exe "$dst\aura-plugin-rtk.exe"

# Autostart at sign-in via Startup-folder shortcut.
$wsh = New-Object -ComObject WScript.Shell
$lnk = $wsh.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Startup')) 'Aura.lnk'))
$lnk.TargetPath       = "$dst\aura.exe"
$lnk.WorkingDirectory = $dst
$lnk.WindowStyle      = 7
$lnk.Save()
```

### Common commands

| Command                                           | What it does                                                                       |
| ------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `just run`                                        | Launch Aura (debug build) without installing                                       |
| `just status` / `just logs`                       | Service status / tail logs (systemd on Linux, launchd on macOS)                    |
| `just status-windows` / `just stop-windows`       | Show / stop the running `aura.exe` on Windows                                      |
| `just uninstall` / `just uninstall-windows`       | Remove binaries + unit / LaunchAgent / Startup shortcut (keeps config & state)     |

## Sponsor

See [SPONSOR.md](./SPONSOR.md) for ways to support Aura.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for local setup, development workflow,
and pull request guidance.

## License

MIT
