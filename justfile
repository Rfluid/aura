# Aura — common dev/install tasks
# Run `just` with no args for a list.

# Detect host OS so the install/start/stop targets dispatch correctly.
os := `uname -s`

# Default target
default:
    @just --list

# ── Build ─────────────────────────────────────────────────────────────────────

# Build everything in release mode
build:
    cargo build --release --workspace

# Build a single workspace member
build-app:
    cargo build --release -p aura

build-rtk:
    cargo build --release -p aura-plugin-rtk

# Assemble Aura.app around the release binary (macOS only)
build-macos-app: build-app
    ./scripts/build-macos-app.sh target/release/aura target/release

# ── Run ───────────────────────────────────────────────────────────────────────

# Run the GPUI app (debug)
run:
    cargo run -p aura

# ── Tests / lint ──────────────────────────────────────────────────────────────

test:
    cargo test --workspace

lint:
    cargo clippy --workspace -- -D warnings
    cargo fmt --check

fix:
    cargo fmt

# ── Install ───────────────────────────────────────────────────────────────────

# Install Aura — dispatches to the platform-appropriate flow.
# Linux:   binaries → ~/.local/bin + systemd user unit
# macOS:   binaries → ~/.local/bin + Aura.app + launchd LaunchAgent
# Windows: use `just install-windows` from PowerShell (bash flow does
#          not apply; this recipe will hand off if invoked under MSYS).
install:
    ./install.sh

# Windows-only: run from PowerShell. Builds + installs to
# %LOCALAPPDATA%\Programs\Aura and drops a Startup-folder shortcut.
install-windows:
    powershell -ExecutionPolicy Bypass -File scripts/install.ps1

# Install only the RTK plugin binary (useful when iterating on plugin code)
install-plugin-rtk: build-rtk
    install -m 755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
    @echo "✔ Installed aura-plugin-rtk to ~/.local/bin/"

# ── Uninstall ─────────────────────────────────────────────────────────────────

uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Linux)
            systemctl --user disable --now aura 2>/dev/null || true
            rm -f ~/.local/bin/aura ~/.local/bin/aura-plugin-rtk
            rm -f ~/.config/systemd/user/aura.service
            ;;
        Darwin)
            launchctl bootout "gui/$(id -u)/com.aura.agent-usage" 2>/dev/null || true
            rm -f ~/Library/LaunchAgents/com.aura.agent-usage.plist
            rm -rf /Applications/Aura.app ~/Applications/Aura.app
            rm -f ~/.local/bin/aura ~/.local/bin/aura-plugin-rtk
            ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Run 'just uninstall-windows' from PowerShell on Windows." >&2
            exit 1
            ;;
        *)
            echo "warning: unknown OS, removing binaries only" >&2
            rm -f ~/.local/bin/aura ~/.local/bin/aura-plugin-rtk
            ;;
    esac
    echo "✔ Removed binaries and service unit (config + state preserved)"

# Windows-only uninstall: removes binaries + Startup shortcut.
uninstall-windows:
    powershell -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; \
        $dir = Join-Path $env:LOCALAPPDATA 'Programs\\Aura'; \
        $lnk = Join-Path ([Environment]::GetFolderPath('Startup')) 'Aura.lnk'; \
        Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; \
        if (Test-Path $lnk) { Remove-Item $lnk }; \
        if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }; \
        Write-Host '✔ Removed binaries and Startup shortcut (config + state preserved)'"

# ── Service control (convenience wrappers) ────────────────────────────────────

start:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)  systemctl --user start aura ;;
        Darwin) launchctl kickstart "gui/$(id -u)/com.aura.agent-usage" ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just start-windows' from PowerShell." >&2; exit 1 ;;
    esac

stop:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)  systemctl --user stop aura ;;
        Darwin) launchctl kill TERM "gui/$(id -u)/com.aura.agent-usage" ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just stop-windows' from PowerShell." >&2; exit 1 ;;
    esac

status:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)  systemctl --user status aura ;;
        Darwin) launchctl print "gui/$(id -u)/com.aura.agent-usage" ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just status-windows' from PowerShell." >&2; exit 1 ;;
    esac

logs:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)  journalctl --user -u aura -f ;;
        Darwin) tail -F /tmp/aura.out.log /tmp/aura.err.log ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Windows: Aura runs as a foreground tray process — there is no service log to tail. Run aura.exe in a terminal to see output." >&2; exit 1 ;;
    esac

# ── Windows service control (PowerShell) ──────────────────────────────────────

start-windows:
    powershell -ExecutionPolicy Bypass -Command "Start-Process -WindowStyle Hidden (Join-Path $env:LOCALAPPDATA 'Programs\\Aura\\aura.exe')"

stop-windows:
    powershell -ExecutionPolicy Bypass -Command "Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force"

status-windows:
    powershell -ExecutionPolicy Bypass -Command "Get-Process aura -ErrorAction SilentlyContinue | Format-Table Id, ProcessName, StartTime"
