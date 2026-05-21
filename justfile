# Aura — common dev/install tasks
# Run `just` with no args for a list.

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

# Install binaries (aura + rtk plugin) into ~/.local/bin and the systemd unit
install: build
    install -Dm755 target/release/aura            ~/.local/bin/aura
    install -Dm755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
    install -Dm644 packaging/aura.service         ~/.config/systemd/user/aura.service
    @echo ""
    @echo "✔ Installed binaries to ~/.local/bin/"
    @echo "✔ Installed systemd unit to ~/.config/systemd/user/"
    @echo ""
    @echo "Next steps:"
    @echo "  systemctl --user daemon-reload"
    @echo "  systemctl --user enable --now aura"

# Install only the RTK plugin binary (useful when iterating on plugin code)
install-plugin-rtk: build-rtk
    install -Dm755 target/release/aura-plugin-rtk ~/.local/bin/aura-plugin-rtk
    @echo "✔ Installed aura-plugin-rtk to ~/.local/bin/"

# ── Uninstall ─────────────────────────────────────────────────────────────────

uninstall:
    -systemctl --user disable --now aura 2>/dev/null
    rm -f ~/.local/bin/aura ~/.local/bin/aura-plugin-rtk
    rm -f ~/.config/systemd/user/aura.service
    @echo "✔ Removed binaries and systemd unit (config + state preserved)"

# ── Service control (convenience wrappers) ────────────────────────────────────

start:
    systemctl --user start aura

stop:
    systemctl --user stop aura

status:
    systemctl --user status aura

logs:
    journalctl --user -u aura -f
