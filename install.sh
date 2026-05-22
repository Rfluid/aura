#!/usr/bin/env bash
# Aura installation script — builds binaries and wires up autostart.
# Supports Linux (systemd user unit) and macOS (.app bundle + launchd agent).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
UNIT_DIR="${HOME}/.config/systemd/user"
LAUNCHD_DIR="${HOME}/Library/LaunchAgents"
APP_DIR="/Applications"            # falls back to ~/Applications if not writable
LAUNCHD_LABEL="com.aura.agent-usage"

OS="$(uname -s)"

# ── Prerequisite checks ───────────────────────────────────────────────────────

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
}

# ── Build ─────────────────────────────────────────────────────────────────────

echo "▸ Building Aura (release)…"
(cd "$ROOT" && cargo build --release --workspace)

# ── Install binaries ──────────────────────────────────────────────────────────

mkdir -p "$BIN_DIR"
install -m 755 "$ROOT/target/release/aura"            "$BIN_DIR/aura"
install -m 755 "$ROOT/target/release/aura-plugin-rtk" "$BIN_DIR/aura-plugin-rtk"
echo "▸ Installed binaries to $BIN_DIR"

# ── OS-specific autostart ─────────────────────────────────────────────────────

case "$OS" in
    Linux)
        if command -v systemctl >/dev/null 2>&1; then
            mkdir -p "$UNIT_DIR"
            install -m 644 "$ROOT/packaging/aura.service" "$UNIT_DIR/aura.service"
            systemctl --user daemon-reload
            echo "▸ Installed systemd unit to $UNIT_DIR"
            echo ""
            echo "Enable autostart with:"
            echo "    systemctl --user enable --now aura"
        else
            echo "warning: systemctl not found — skipping systemd integration" >&2
        fi
        ;;

    Darwin)
        # Build the .app bundle so launchd has a proper target and Finder
        # gets a real icon. Falls back to running the bare binary when the
        # build script's prerequisites aren't met (rsvg-convert/iconutil).
        BUILD_APP_SCRIPT="$ROOT/scripts/build-macos-app.sh"
        STAGING_DIR="$ROOT/target/release"

        echo "▸ Assembling Aura.app…"
        "$BUILD_APP_SCRIPT" "$ROOT/target/release/aura" "$STAGING_DIR"

        # Pick an install location we can actually write to.
        if [ -w "$APP_DIR" ] || sudo -n true 2>/dev/null; then
            DEST_APP_DIR="$APP_DIR"
            if [ ! -w "$DEST_APP_DIR" ]; then
                echo "▸ /Applications requires sudo to write"
                sudo rm -rf "$DEST_APP_DIR/Aura.app"
                sudo cp -R "$STAGING_DIR/Aura.app" "$DEST_APP_DIR/Aura.app"
            else
                rm -rf "$DEST_APP_DIR/Aura.app"
                cp -R "$STAGING_DIR/Aura.app" "$DEST_APP_DIR/Aura.app"
            fi
        else
            DEST_APP_DIR="${HOME}/Applications"
            mkdir -p "$DEST_APP_DIR"
            rm -rf "$DEST_APP_DIR/Aura.app"
            cp -R "$STAGING_DIR/Aura.app" "$DEST_APP_DIR/Aura.app"
        fi
        APP_EXEC="${DEST_APP_DIR}/Aura.app/Contents/MacOS/aura"
        echo "▸ Installed Aura.app to ${DEST_APP_DIR}"

        # Drop the launchd plist (rewriting ProgramArguments to the actual
        # install path so the user can move the .app without re-running us
        # if they update both at once).
        mkdir -p "$LAUNCHD_DIR"
        PLIST_DEST="${LAUNCHD_DIR}/${LAUNCHD_LABEL}.plist"
        sed "s|/Applications/Aura.app/Contents/MacOS/aura|${APP_EXEC}|" \
            "$ROOT/packaging/com.aura.agent-usage.plist" > "$PLIST_DEST"
        chmod 644 "$PLIST_DEST"
        echo "▸ Installed LaunchAgent to $PLIST_DEST"

        # Reload the agent if launchctl is present (CI may skip this).
        if command -v launchctl >/dev/null 2>&1; then
            launchctl bootout "gui/$(id -u)/${LAUNCHD_LABEL}" 2>/dev/null || true
            launchctl bootstrap "gui/$(id -u)" "$PLIST_DEST"
            launchctl kickstart -k "gui/$(id -u)/${LAUNCHD_LABEL}"
            echo "▸ LaunchAgent loaded — Aura will autostart at login"
        fi
        ;;

    *)
        echo "warning: unsupported OS '$OS' — installing binaries only" >&2
        ;;
esac

# ── PATH check ────────────────────────────────────────────────────────────────

case ":$PATH:" in
    *":$BIN_DIR:"*)
        ;;
    *)
        echo ""
        echo "note: $BIN_DIR is not on your PATH. Add to your shell rc:"
        echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac

echo ""
echo "✔ Aura installed."
