#!/usr/bin/env bash
# Aura uninstall script — Linux + macOS.
#
# Mirrors the `just uninstall` recipe so the README can curl this file
# directly (`curl -fsSL .../uninstall.sh | bash`) without requiring the
# user to have `just` / `cargo` / a cloned checkout. The `just` recipe
# itself is now a thin wrapper that invokes this script.
#
# Tears down:
#
#   Linux:  disables the systemd --user autostart, kills the running
#           tray, removes ~/.local/bin/aura + .desktop + icon + unit,
#           clears the KDE keepalive skip-taskbar rule, refreshes the
#           desktop/icon caches.
#   macOS:  unloads the launchd LaunchAgent, removes the plist, removes
#           /Applications/Aura.app + ~/Applications/Aura.app + the bare
#           binary in ~/.local/bin.
#
# Config (`~/.config/aura/config.toml`) and state
# (`~/.local/share/aura/state.json`) are preserved by design — `just
# update` calls this script before re-installing.

set -euo pipefail

case "$(uname -s)" in
    Linux)
        # Disable autostart, stop the running tray, then clear the
        # binaries + launcher + systemd unit.
        systemctl --user disable --now aura 2>/dev/null || true
        pkill -x aura 2>/dev/null || true
        rm -f ~/.local/bin/aura
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
        rm -f ~/.local/bin/aura
        ;;
    MINGW*|MSYS*|CYGWIN*)
        cat >&2 <<'EOF'
error: this script is for Linux/macOS only.

Run the PowerShell uninstaller instead (from PowerShell, not Git Bash):
    iex (irm https://raw.githubusercontent.com/Rfluid/aura/main/scripts/uninstall.ps1)
EOF
        exit 1
        ;;
    *)
        echo "warning: unknown OS, removing binaries only" >&2
        rm -f ~/.local/bin/aura
        ;;
esac

echo "✔ Removed binaries and launcher (config + state preserved)"
