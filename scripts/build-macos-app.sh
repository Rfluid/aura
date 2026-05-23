#!/usr/bin/env bash
# Assemble a macOS .app bundle around the `aura` binary.
#
# Usage:
#     scripts/build-macos-app.sh <binary-path> <output-dir>
#
# Example:
#     scripts/build-macos-app.sh target/release/aura dist/
#     # → dist/Aura.app
#
# Optional code signing:
#     CODESIGN_IDENTITY="Developer ID Application: Acme (TEAMID)" \
#         scripts/build-macos-app.sh ...
#
# When CODESIGN_IDENTITY is unset the bundle is left unsigned. Gatekeeper
# will quarantine it on first launch; users can right-click → Open or
# `xattr -dr com.apple.quarantine Aura.app` to bypass.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <binary-path> <output-dir>" >&2
    exit 2
fi

BIN="$1"
OUT_DIR="$2"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ ! -x "$BIN" ]; then
    echo "error: binary not found or not executable: $BIN" >&2
    exit 1
fi

APP_DIR="${OUT_DIR}/Aura.app"
CONTENTS="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS}/MacOS"
RES_DIR="${CONTENTS}/Resources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RES_DIR"

# ── Executable ────────────────────────────────────────────────────────────────
install -m 755 "$BIN" "${MACOS_DIR}/aura"

# ── Info.plist ────────────────────────────────────────────────────────────────
# Substitute the version token from the aura crate manifest so CFBundleVersion
# and CFBundleShortVersionString match the release tag (otherwise Finder's
# "Get Info" and `defaults read` report whatever the template was last set to).
AURA_VERSION="$(
    sed -nE 's/^version[[:space:]]*=[[:space:]]*"(.*)"[[:space:]]*$/\1/p' \
        "${ROOT}/crates/aura/Cargo.toml" | head -n1
)"
if [ -z "$AURA_VERSION" ]; then
    echo "error: could not read version from crates/aura/Cargo.toml" >&2
    exit 1
fi
sed "s/__AURA_VERSION__/${AURA_VERSION}/g" \
    "${ROOT}/packaging/macos/Info.plist" > "${CONTENTS}/Info.plist"
chmod 644 "${CONTENTS}/Info.plist"

# ── Icon ──────────────────────────────────────────────────────────────────────
# Convert the brand SVG → .icns when the toolchain is available; otherwise
# ship the SVG so the app still launches (Finder shows the generic icon).
# Use packaging/aura.svg (brand purple #8b5cf6) instead of assets/icons/aura.svg
# (which uses stroke="currentColor" and renders black without a CSS context).
ICON_SRC="${ROOT}/packaging/aura.svg"
if command -v rsvg-convert >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    ICONSET_DIR="$(mktemp -d)/Aura.iconset"
    mkdir -p "$ICONSET_DIR"
    # macOS .iconset expects a fixed set of sizes.
    for size in 16 32 64 128 256 512 1024; do
        rsvg-convert -w "$size" -h "$size" "$ICON_SRC" \
            -o "${ICONSET_DIR}/icon_${size}x${size}.png"
    done
    # Retina variants (@2x).
    for base in 16 32 128 256 512; do
        retina=$((base * 2))
        cp "${ICONSET_DIR}/icon_${retina}x${retina}.png" \
            "${ICONSET_DIR}/icon_${base}x${base}@2x.png"
    done
    iconutil -c icns "$ICONSET_DIR" -o "${RES_DIR}/Aura.icns"
    rm -rf "$(dirname "$ICONSET_DIR")"
else
    echo "warning: rsvg-convert / iconutil not on PATH — shipping SVG icon only" >&2
    cp "$ICON_SRC" "${RES_DIR}/Aura.svg"
fi

# ── Code signing (optional) ───────────────────────────────────────────────────
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    echo "▸ Signing bundle with identity '${CODESIGN_IDENTITY}'"
    codesign --force --deep --options runtime \
        --sign "$CODESIGN_IDENTITY" \
        "$APP_DIR"
else
    echo "note: CODESIGN_IDENTITY unset — bundle will be unsigned (Gatekeeper will quarantine)"
fi

echo "✔ Built ${APP_DIR}"
