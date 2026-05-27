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

# Install Aura — tray-indicator-style (autostart by default). Dispatches
# by host:
# Linux:   binaries → ~/.local/bin + systemd user unit (enable --now) +
#          XDG .desktop entry (app-menu discoverability)
# macOS:   binaries → ~/.local/bin + Aura.app in /Applications + launchd
#          LaunchAgent (loaded now + at every login)
# Windows: use `just install-windows` from PowerShell — installs aura.exe
#          + a Startup-folder shortcut (autostart) + a Start Menu shortcut.
install:
    ./install.sh

install-windows:
    powershell -ExecutionPolicy Bypass -File scripts/install.ps1

# Install only the RTK plugin binary (useful when iterating on plugin code)
install-plugin-rtk: build-rtk
    install -m 755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
    @echo "✔ Installed aura-plugin-rtk to ~/.local/bin/"

# ── Update ────────────────────────────────────────────────────────────────────

# Reinstall Aura on top of an existing install. Runs `uninstall` first so the
# running tray is stopped, the autostart unit is torn down, and the previous
# binary (+ Aura.app on macOS) is removed — nothing stale survives into the
# new install. Then `install` lays down the fresh binary at the same path the
# systemd unit / launchd agent points at, so the tray icon comes back bound
# to the updated binary.
update: uninstall install

update-windows: uninstall-windows install-windows

# ── Uninstall ─────────────────────────────────────────────────────────────────

# Linux + macOS uninstall — disables autostart, kills the running tray,
# removes binaries / launchers / units, clears the KDE keepalive rule.
# The actual logic lives in uninstall.sh so the README's "two-curl
# update" instructions can fetch it directly without a checkout.
uninstall:
    ./uninstall.sh

# Windows-only uninstall: removes binaries + Start Menu shortcut. Also
# cleans up the legacy Startup-folder shortcut from older installs. The
# script lives in scripts/uninstall.ps1 so the README's `iex (irm ...)`
# update instructions can fetch it directly.
uninstall-windows:
    powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1

# ── Process control (convenience wrappers) ────────────────────────────────────
#
# Aura is a launcher-style tray app — these recipes operate on the running
# process directly. If you opted into systemd / launchd autostart, use the
# native tooling instead (systemctl --user … / launchctl …).

start:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)
            # Detach from this shell so the tray icon survives the recipe exit.
            nohup ~/.local/bin/aura >/dev/null 2>&1 &
            disown $! 2>/dev/null || true
            echo "▸ Aura started (PID $!)"
            ;;
        Darwin)
            open -a Aura
            echo "▸ Aura launched"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just start-windows' from PowerShell." >&2; exit 1 ;;
    esac

stop:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux)  pkill -x aura && echo "▸ Aura stopped" || echo "(aura not running)" ;;
        Darwin) pkill -x aura && echo "▸ Aura stopped" || echo "(aura not running)" ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just stop-windows' from PowerShell." >&2; exit 1 ;;
    esac

status:
    #!/usr/bin/env bash
    case "$(uname -s)" in
        Linux|Darwin)
            if pgrep -x aura >/dev/null; then
                pgrep -x -l aura
            else
                echo "(aura not running)"
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            echo "Use 'just status-windows' from PowerShell." >&2; exit 1 ;;
    esac

# Aura logs to stderr — when launched from the .desktop / Aura.app there is
# no terminal capturing it. Run `aura` directly from a terminal to see logs.
logs:
    #!/usr/bin/env bash
    echo "Aura is a launcher-style tray app; there's no service log to tail."
    echo "Run it from a terminal to see stderr:"
    case "$(uname -s)" in
        Linux)  echo "    ~/.local/bin/aura" ;;
        Darwin) echo "    /Applications/Aura.app/Contents/MacOS/aura" ;;
    esac

# ── Windows service control (PowerShell) ──────────────────────────────────────

start-windows:
    powershell -ExecutionPolicy Bypass -Command "Start-Process -WindowStyle Hidden (Join-Path $env:LOCALAPPDATA 'Programs\\Aura\\aura.exe')"

stop-windows:
    powershell -ExecutionPolicy Bypass -Command "Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force"

status-windows:
    powershell -ExecutionPolicy Bypass -Command "Get-Process aura -ErrorAction SilentlyContinue | Format-Table Id, ProcessName, StartTime"
