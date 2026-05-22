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

# ── Uninstall ─────────────────────────────────────────────────────────────────

uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    case "$(uname -s)" in
        Linux)
            # Disable autostart, stop the running tray, then clear the
            # binaries + launcher + systemd unit.
            systemctl --user disable --now aura 2>/dev/null || true
            pkill -x aura 2>/dev/null || true
            rm -f ~/.local/bin/aura ~/.local/bin/aura-plugin-rtk
            rm -f ~/.local/share/applications/aura.desktop
            rm -f ~/.local/share/icons/hicolor/scalable/apps/aura.svg
            rm -f ~/.config/systemd/user/aura.service
            command -v update-desktop-database >/dev/null 2>&1 && \
                update-desktop-database ~/.local/share/applications >/dev/null 2>&1 || true
            command -v gtk-update-icon-cache >/dev/null 2>&1 && \
                gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor >/dev/null 2>&1 || true
            # KDE-only: remove the keepalive skip-taskbar rule installed by
            # install.sh, then ask KWin to reload its rules.
            if command -v kwriteconfig6 >/dev/null 2>&1 && command -v kreadconfig6 >/dev/null 2>&1; then
                rule_id="aura-keepalive-skip-taskbar"
                current=$(kreadconfig6 --file kwinrulesrc --group General --key rules 2>/dev/null || true)
                new=$(echo "$current" | tr ',' '\n' | grep -vFx "$rule_id" | paste -sd ',' -)
                kwriteconfig6 --file kwinrulesrc --group General --key rules "$new"
                kwriteconfig6 --file kwinrulesrc --group General --key count \
                    "$(echo "$new" | tr ',' '\n' | grep -c . || echo 0)"
                kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key Description --delete 2>/dev/null || true
                command -v qdbus6 >/dev/null 2>&1 && qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1 || true
            fi
            ;;
        Darwin)
            # Unload the LaunchAgent (stops the menu-bar tray) then remove
            # the agent plist + Aura.app + bare binaries.
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
    echo "✔ Removed binaries and launcher (config + state preserved)"

# Windows-only uninstall: removes binaries + Start Menu shortcut. Also
# cleans up the legacy Startup-folder shortcut from older installs.
uninstall-windows:
    powershell -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; \
        $dir = Join-Path $env:LOCALAPPDATA 'Programs\\Aura'; \
        $startMenu = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\\Aura.lnk'; \
        $startup   = Join-Path ([Environment]::GetFolderPath('Startup')) 'Aura.lnk'; \
        Get-Process aura -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; \
        foreach ($p in @($startMenu, $startup)) { if (Test-Path $p) { Remove-Item $p } }; \
        if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }; \
        Write-Host '✔ Removed binaries and Start Menu shortcut (config + state preserved)'"

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
