#!/usr/bin/env bash
# Aura installation script — builds binaries, installs to ~/.local/bin,
# wires up the systemd user service.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
UNIT_DIR="${HOME}/.config/systemd/user"

# ── Prerequisite checks ───────────────────────────────────────────────────────

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
}

if ! command -v systemctl >/dev/null 2>&1; then
    echo "warning: systemctl not found — skipping systemd integration"
    INSTALL_SERVICE=false
else
    INSTALL_SERVICE=true
fi

# ── Build ─────────────────────────────────────────────────────────────────────

echo "▸ Building Aura (release)…"
(cd "$ROOT" && cargo build --release --workspace)

# ── Install binaries ──────────────────────────────────────────────────────────

mkdir -p "$BIN_DIR"
install -m755 "$ROOT/target/release/aura"            "$BIN_DIR/aura"
install -m755 "$ROOT/target/release/aura-plugin-rtk" "$BIN_DIR/aura-plugin-rtk"
echo "▸ Installed binaries to $BIN_DIR"

# ── Install systemd unit ──────────────────────────────────────────────────────

if [ "$INSTALL_SERVICE" = "true" ]; then
    mkdir -p "$UNIT_DIR"
    install -m644 "$ROOT/packaging/aura.service" "$UNIT_DIR/aura.service"
    systemctl --user daemon-reload
    echo "▸ Installed systemd unit to $UNIT_DIR"
    echo ""
    echo "Enable autostart with:"
    echo "    systemctl --user enable --now aura"
fi

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
