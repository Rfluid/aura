#!/usr/bin/env bash
# Aura installation script — supports two modes:
#
#   * source   — run from a cloned repo: builds with cargo, then installs.
#   * release  — run via `curl | bash`: downloads the latest GitHub release
#                archive for the host, verifies its checksum, then installs.
#
# Aura is a tray-indicator app: the icon next to wifi/volume is the
# entire UI, so the process needs to be running for there to be anything
# to click. The installer therefore wires up autostart by default:
#
#   Linux:  ~/.local/bin/aura + systemd user unit (enable --now) +
#           XDG .desktop entry (for app-menu discoverability / first run)
#   macOS:  /Applications/Aura.app + launchd LaunchAgent (loaded now,
#           and at every login)
#   Windows (see scripts/install.ps1): aura.exe + Startup-folder shortcut
#           (runs at sign-in) + Start Menu shortcut
#
# Override detection with AURA_INSTALL_MODE=source|release.
# Pin a specific release with AURA_VERSION=v1.2.3.

set -euo pipefail

BIN_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
UNIT_DIR="${HOME}/.config/systemd/user"
LAUNCHD_DIR="${HOME}/Library/LaunchAgents"
APP_DIR="/Applications"            # falls back to ~/Applications if not writable
LAUNCHD_LABEL="com.aura.agent-usage"
RELEASE_BASE_URL="https://github.com/Rfluid/aura/releases"

OS="$(uname -s)"
ARCH="$(uname -m)"

# Detect Git Bash / MSYS / Cygwin under Windows and redirect users to the
# PowerShell installer — bash autostart paths (systemd / launchd) don't
# apply, and forking a .ps1 from here is more brittle than just asking.
case "$OS" in
    MINGW*|MSYS*|CYGWIN*)
        cat >&2 <<'EOF'
error: this script is for Linux/macOS only.

Run the PowerShell installer instead (from PowerShell, not Git Bash):
    powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
EOF
        exit 1
        ;;
esac

# Resolve script directory when invoked from a file; tolerate `curl | bash`,
# where BASH_SOURCE is empty / not a real file path.
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
else
    ROOT=""
fi

INSTALL_MODE="${AURA_INSTALL_MODE:-}"
AURA_VERSION="${AURA_VERSION:-}"

if [ -z "$INSTALL_MODE" ]; then
    if [ -n "$ROOT" ] && [ -f "$ROOT/Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
        INSTALL_MODE="source"
    else
        INSTALL_MODE="release"
    fi
fi

# ── Helpers ───────────────────────────────────────────────────────────────────

asset_name_for_host() {
    local version="$1"
    case "$OS-$ARCH" in
        Linux-x86_64)  printf 'aura-%s-x86_64-unknown-linux-gnu\n'  "$version" ;;
        Linux-aarch64) printf 'aura-%s-aarch64-unknown-linux-gnu\n' "$version" ;;
        Darwin-x86_64) printf 'aura-%s-x86_64-apple-darwin\n'        "$version" ;;
        Darwin-arm64)  printf 'aura-%s-aarch64-apple-darwin\n'       "$version" ;;
        *) return 1 ;;
    esac
}

resolve_latest_version() {
    curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "${RELEASE_BASE_URL}/latest" | sed 's:.*/::'
}

verify_checksum() {
    local asset="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "${asset}.sha256"
    else
        shasum -a 256 -c "${asset}.sha256"
    fi
}

# ── Stage binaries + launcher assets ─────────────────────────────────────────
#
# After this block STAGING_DIR holds the binaries (and Aura.app on macOS), and
# PACKAGING_DIR holds the .desktop / icon source files.

if [ "$INSTALL_MODE" = "source" ]; then
    command -v cargo >/dev/null 2>&1 || {
        echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
        exit 1
    }

    echo "▸ Building Aura (release)…"
    (cd "$ROOT" && cargo build --release --workspace)

    STAGING_DIR="$ROOT/target/release"
    PACKAGING_DIR="$ROOT/packaging"

    if [ "$OS" = "Darwin" ]; then
        echo "▸ Assembling Aura.app…"
        "$ROOT/scripts/build-macos-app.sh" "$STAGING_DIR/aura" "$STAGING_DIR"
    fi
else
    command -v curl >/dev/null 2>&1 || {
        echo "error: curl not found — required to download release artifacts" >&2
        exit 1
    }
    command -v tar >/dev/null 2>&1 || {
        echo "error: tar not found" >&2
        exit 1
    }

    if [ -z "$AURA_VERSION" ]; then
        echo "▸ Resolving latest release…"
        AURA_VERSION="$(resolve_latest_version)"
        [ -n "$AURA_VERSION" ] || {
            echo "error: failed to determine the latest GitHub release version" >&2
            exit 1
        }
    fi

    ASSET="$(asset_name_for_host "$AURA_VERSION")" || {
        echo "error: no published release artifact for ${OS}-${ARCH}" >&2
        exit 1
    }

    echo "▸ Installing ${AURA_VERSION} (${ASSET})"

    DL_DIR="$(mktemp -d)"
    trap 'rm -rf "$DL_DIR"' EXIT

    (
        cd "$DL_DIR"
        curl -fsSL -O "${RELEASE_BASE_URL}/download/${AURA_VERSION}/${ASSET}.tar.gz"
        curl -fsSL -O "${RELEASE_BASE_URL}/download/${AURA_VERSION}/${ASSET}.sha256"
        verify_checksum "$ASSET"
        tar -xzf "${ASSET}.tar.gz"
    )

    STAGING_DIR="$DL_DIR/$ASSET"
    PACKAGING_DIR="$STAGING_DIR"
fi

# ── Install binaries ──────────────────────────────────────────────────────────

mkdir -p "$BIN_DIR"
install -m 755 "$STAGING_DIR/aura"            "$BIN_DIR/aura"
install -m 755 "$STAGING_DIR/aura-plugin-rtk" "$BIN_DIR/aura-plugin-rtk"
echo "▸ Installed binaries to $BIN_DIR"

# ── Detect agents and seed/merge config ──────────────────────────────────────
# Runs before autostart so the service picks up the populated config on its
# first launch. Failure here is non-fatal: AppConfig::load() will write a
# default config on first app launch as a fallback.

echo "▸ Configuring agents…"
if ! "$BIN_DIR/aura" setup-config; then
    echo "warning: 'aura setup-config' failed; defaults will be written on first launch" >&2
fi

# ── OS-specific autostart + launcher ──────────────────────────────────────────
#
# Aura is a tray indicator: its UI is the icon next to wifi/volume. The
# installer therefore enables autostart by default so the icon is present
# the moment the user logs in. App-menu / Launchpad entries are also
# installed for discoverability and first-run.

# ─ KDE Plasma: KWin rule to hide the keepalive surface from the task manager ─
#
# Aura keeps an off-screen, locked-shut 1×1 "keepalive" window open at
# all times so GPUI's Wayland event loop (which stops the moment
# `state.windows.is_empty()`) doesn't terminate when the user closes
# the modal. Wayland has no client-settable "skip taskbar" hint that
# GPUI exposes, so KDE happily renders that keepalive as a regular
# entry in the task manager. Fix it at the WM layer with a KWin rule
# matching `app_id == aura-keepalive` and forcing skip-taskbar /
# skip-pager / skip-switcher.
install_kwin_skip_taskbar_rule() {
    [ "${XDG_CURRENT_DESKTOP:-}" = "KDE" ] || return 0
    command -v kwriteconfig6 >/dev/null 2>&1 || return 0
    command -v kreadconfig6 >/dev/null 2>&1 || return 0

    local rule_id="aura-keepalive-skip-taskbar"
    local current_rules
    current_rules=$(kreadconfig6 --file kwinrulesrc --group General --key rules 2>/dev/null || true)

    if ! echo "${current_rules}" | tr ',' '\n' | grep -qFx "$rule_id"; then
        local new_rules
        if [ -z "$current_rules" ]; then
            new_rules="$rule_id"
        else
            new_rules="${current_rules},${rule_id}"
        fi
        local new_count
        new_count=$(echo "$new_rules" | tr ',' '\n' | grep -c .)
        kwriteconfig6 --file kwinrulesrc --group General --key rules  "$new_rules"
        kwriteconfig6 --file kwinrulesrc --group General --key count  "$new_count"
    fi

    # Rule properties — overwrite-safe. wmclassmatch=1 is exact match;
    # wmclasscomplete=false makes it match against the class portion of
    # WM_CLASS only (Wayland app_id maps cleanly to that on KWin).
    # skiptaskbarrule=2 is the "Force" enforcement mode (vs Don't Affect,
    # Apply, Remember).
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key Description       "Hide Aura keepalive surface from taskbar / pager / switcher"
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key wmclass           "aura-keepalive"
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key wmclasscomplete --type bool false
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key wmclassmatch      1
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skiptaskbar     --type bool true
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skiptaskbarrule   2
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skippager       --type bool true
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skippagerrule     2
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skipswitcher    --type bool true
    kwriteconfig6 --file kwinrulesrc --group "$rule_id" --key skipswitcherrule  2

    # Reload KWin so the rule applies without a logout.
    command -v qdbus6 >/dev/null 2>&1 && \
        qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1 || true

    echo "▸ Installed KWin rule: aura keepalive hidden from taskbar"
}

case "$OS" in
    Linux)
        # ─ App-menu / hicolor icon (discoverability + first-run launcher) ─
        mkdir -p "$ICON_DIR"
        ICON_SRC="$PACKAGING_DIR/aura.svg"
        if [ -f "$ICON_SRC" ]; then
            install -m 644 "$ICON_SRC" "$ICON_DIR/aura.svg"
        elif [ -n "$ROOT" ] && [ -f "$ROOT/assets/icons/aura.svg" ]; then
            sed 's/currentColor/#8b5cf6/g' "$ROOT/assets/icons/aura.svg" \
                > "$ICON_DIR/aura.svg"
            chmod 644 "$ICON_DIR/aura.svg"
        else
            echo "warning: aura.svg not found in packaging or assets — icon will be missing" >&2
        fi

        mkdir -p "$DESKTOP_DIR"
        DESKTOP_SRC="$PACKAGING_DIR/aura.desktop"
        if [ -f "$DESKTOP_SRC" ]; then
            sed "s|AURA_EXEC|${BIN_DIR}/aura|" "$DESKTOP_SRC" \
                > "$DESKTOP_DIR/aura.desktop"
            chmod 644 "$DESKTOP_DIR/aura.desktop"
            echo "▸ Installed launcher to $DESKTOP_DIR/aura.desktop"

            # Refresh desktop / icon caches if the helpers are present.
            command -v update-desktop-database >/dev/null 2>&1 && \
                update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
            command -v gtk-update-icon-cache >/dev/null 2>&1 && \
                gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" \
                >/dev/null 2>&1 || true
            for kbs in kbuildsycoca6 kbuildsycoca5 kbuildsycoca; do
                if command -v "$kbs" >/dev/null 2>&1; then
                    "$kbs" >/dev/null 2>&1 || true
                    break
                fi
            done
        else
            echo "warning: aura.desktop not found in ${PACKAGING_DIR} — skipping launcher" >&2
        fi

        # ─ systemd user unit (autostart at every login + start now) ─
        SERVICE_SRC="$PACKAGING_DIR/aura.service"
        if [ ! -f "$SERVICE_SRC" ]; then
            echo "warning: aura.service not found in ${PACKAGING_DIR} — skipping autostart" >&2
        elif ! command -v systemctl >/dev/null 2>&1; then
            echo "warning: systemctl not found — skipping systemd integration" >&2
        else
            mkdir -p "$UNIT_DIR"
            install -m 644 "$SERVICE_SRC" "$UNIT_DIR/aura.service"
            systemctl --user daemon-reload
            echo "▸ Installed systemd unit to $UNIT_DIR"

            # `enable --now` schedules autostart at login AND starts the
            # service in the current session — the tray icon should appear
            # immediately. From-source users get the same convenience so
            # `just install` is a one-shot operation.
            if systemctl --user enable --now aura >/dev/null 2>&1; then
                echo "▸ Service enabled and started — tray icon should appear"
            else
                echo "warning: 'systemctl --user enable --now aura' failed; start it manually" >&2
            fi
        fi

        # ─ KDE-only polish: hide aura's keepalive surface from the taskbar ─
        install_kwin_skip_taskbar_rule
        ;;

    Darwin)
        # ─ Aura.app (Dock-pinnable + the menu-bar app body) ─
        APP_SRC="$STAGING_DIR/Aura.app"
        if [ -d "$APP_SRC" ]; then
            # Pick an install location we can actually write to.
            if [ -w "$APP_DIR" ] || sudo -n true 2>/dev/null; then
                DEST_APP_DIR="$APP_DIR"
                if [ ! -w "$DEST_APP_DIR" ]; then
                    echo "▸ /Applications requires sudo to write"
                    sudo rm -rf "$DEST_APP_DIR/Aura.app"
                    sudo cp -R "$APP_SRC" "$DEST_APP_DIR/Aura.app"
                else
                    rm -rf "$DEST_APP_DIR/Aura.app"
                    cp -R "$APP_SRC" "$DEST_APP_DIR/Aura.app"
                fi
            else
                DEST_APP_DIR="${HOME}/Applications"
                mkdir -p "$DEST_APP_DIR"
                rm -rf "$DEST_APP_DIR/Aura.app"
                cp -R "$APP_SRC" "$DEST_APP_DIR/Aura.app"
            fi
            # Release tarballs are unsigned — strip Gatekeeper quarantine.
            xattr -dr com.apple.quarantine "$DEST_APP_DIR/Aura.app" 2>/dev/null || true
            APP_EXEC="${DEST_APP_DIR}/Aura.app/Contents/MacOS/aura"
            echo "▸ Installed Aura.app to ${DEST_APP_DIR}"
        else
            echo "warning: Aura.app missing from staging dir — falling back to bare binary" >&2
            APP_EXEC="$BIN_DIR/aura"
        fi

        # ─ launchd LaunchAgent (autostart at every login + load now) ─
        PLIST_SRC="$PACKAGING_DIR/com.aura.agent-usage.plist"
        if [ ! -f "$PLIST_SRC" ]; then
            echo "warning: com.aura.agent-usage.plist not found in ${PACKAGING_DIR} — skipping" >&2
        else
            mkdir -p "$LAUNCHD_DIR"
            PLIST_DEST="${LAUNCHD_DIR}/${LAUNCHD_LABEL}.plist"
            # Rewrite the plist's ProgramArguments to the actual install
            # path (handles /Applications vs ~/Applications + bare binary).
            sed "s|/Applications/Aura.app/Contents/MacOS/aura|${APP_EXEC}|" \
                "$PLIST_SRC" > "$PLIST_DEST"
            chmod 644 "$PLIST_DEST"
            echo "▸ Installed LaunchAgent to $PLIST_DEST"

            if command -v launchctl >/dev/null 2>&1; then
                launchctl bootout "gui/$(id -u)/${LAUNCHD_LABEL}" 2>/dev/null || true
                launchctl bootstrap "gui/$(id -u)" "$PLIST_DEST"
                launchctl kickstart -k "gui/$(id -u)/${LAUNCHD_LABEL}"
                echo "▸ LaunchAgent loaded — tray icon should appear in the menu bar"
            fi
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

# ── Per-DE / per-OS next-step hints ──────────────────────────────────────────
#
# Aura is a tray indicator: success depends on the user's desktop env
# actually surfacing StatusNotifierItem icons. The defaults are right on
# KDE Plasma; GNOME and minimal Wayland WMs need a one-time per-user
# step. We print a tip instead of failing — the install itself is done.

print_hint() { printf '\n  → %s\n' "$*"; }

case "$OS" in
    Linux)
        echo ""
        echo "Next steps:"
        case "${XDG_CURRENT_DESKTOP:-}" in
            *KDE*)
                print_hint "Tray icon: should already be next to wifi/volume. If it landed in the '^' overflow group, right-click the tray → Configure System Tray → Entries → Aura → set Visibility to 'Always shown'."
                print_hint "Modal placement: KWin centers it on Wayland (protocol limit). To pin a position, add a Window Rule for class 'aura' under System Settings → Window Management → Window Rules."
                ;;
            *GNOME*)
                print_hint "GNOME hides StatusNotifierItem icons by default. Install the AppIndicator extension once: https://extensions.gnome.org/extension/615/appindicator-support/  then log out & back in."
                ;;
            *sway*|*Hyprland*|*wlroots*)
                print_hint "Make sure your status bar speaks StatusNotifierItem (Waybar with the 'tray' module, sway-systray, etc.). Without it the icon will not be visible."
                ;;
            *)
                print_hint "Tray icon: should appear in any desktop env that supports StatusNotifierItem. If it doesn't, check that your panel / bar speaks the SNI protocol."
                ;;
        esac
        print_hint "Right-click the tray icon for Show / Quit. Left-click toggles the modal."
        ;;

    Darwin)
        echo ""
        echo "Next steps:"
        print_hint "Tray icon: should appear at the right end of your menu bar."
        print_hint "If macOS quarantined Aura.app, the launchd plist won't load — strip the quarantine: xattr -dr com.apple.quarantine /Applications/Aura.app"
        print_hint "To pin Aura.app to the Dock: right-click its Dock icon → Options → Keep in Dock."
        ;;

    *)
        ;;
esac
