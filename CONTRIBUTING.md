# Contributing to Aura

Thanks for your interest in Aura! This guide covers everything you need to get a
local checkout building, run the app, validate changes before opening a PR, and
find your way around the codebase.

By participating you agree to license your contributions under the project's
[MIT license](./README.md#license).

---

## Table of contents

- [Getting started](#getting-started)
- [Repository layout](#repository-layout)
- [Running the project](#running-the-project)
- [Development workflow](#development-workflow)
- [Before opening a PR — `scripts/pre-pr.sh`](#before-opening-a-pr--scriptspre-prsh)
- [Commit & PR conventions](#commit--pr-conventions)
- [Reporting bugs & requesting features](#reporting-bugs--requesting-features)
- [Documentation index](#documentation-index)

---

## Getting started

### Prerequisites

- **Rust 1.94+** (the toolchain pinned by CI). Install via [rustup](https://rustup.rs).
  Components `rustfmt` and `clippy` are required and come with a default install.
- **Linux system libraries** — GPUI + `tray-icon` (gtk feature) need a handful of
  `-dev` packages at build time:

  ```bash
  # Debian / Ubuntu
  sudo apt install build-essential pkg-config libgtk-3-dev \
                   libxkbcommon-x11-dev libxcb1-dev libxcb-render0-dev \
                   libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig-dev
  ```

- **Optional but recommended**:
  - [`just`](https://github.com/casey/just) — task runner used by the project's `justfile`
  - [`cargo-audit`](https://github.com/rustsec/rustsec) — installed automatically by `pre-pr.sh` if missing
  - [`gitleaks`](https://github.com/gitleaks/gitleaks) — secret scanning, mirrors CI

### Clone & build

```bash
git clone https://github.com/Rfluid/aura.git
cd aura
cargo build --workspace
```

A first build pulls a lot of GPUI dependencies — give it a few minutes.

---

## Repository layout

```
aura/
├── crates/
│   ├── aura/           # Binary crate — GPUI app, tray, modal UI
│   └── aura-core/      # Library — config, readers, plugin runner, quota logic
├── plugins/
│   └── rtk-gains/      # First-party plugin: RTK token savings panel
├── docs/               # User & architecture docs (see "Documentation index" below)
├── .design/            # Visual design system source-of-truth (tokens, components)
├── packaging/          # systemd unit + distribution metadata
├── scripts/
│   ├── pre-pr.sh       # Local mirror of CI checks (run this before pushing)
│   └── prepare-release.sh
├── .github/workflows/  # CI: quality-and-security, codeql, sbom, release, secrets
├── install.sh          # One-shot build + install + systemd wiring
└── justfile            # `just <task>` — see `just --list`
```

---

## Running the project

### Quick run (debug build)

```bash
just run
# or
cargo run -p aura
```

This launches the tray widget without touching `~/.local/bin` or systemd.

### Install locally

```bash
./install.sh                       # build + install + systemd unit
# or
just install                       # same, via the justfile
systemctl --user enable --now aura # start at login
```

### Iterating on the RTK plugin

```bash
just install-plugin-rtk            # rebuild + reinstall just the plugin binary
```

### Service control (after install)

| Command         | What it does                          |
| --------------- | ------------------------------------- |
| `just start`    | `systemctl --user start aura`         |
| `just stop`     | `systemctl --user stop aura`          |
| `just status`   | `systemctl --user status aura`        |
| `just logs`     | `journalctl --user -u aura -f`        |
| `just uninstall`| remove binaries + unit (keeps config) |

---

## Development workflow

### Format & lint

```bash
just fix     # cargo fmt
just lint    # cargo clippy -D warnings && cargo fmt --check
```

The project uses default rustfmt with `edition = "2021"` (see `.rustfmt.toml`).
Clippy is run with `-D warnings` in CI — warnings are errors on PRs.

### Tests

```bash
just test
# or
cargo test --workspace
```

### Adding a dependency

- Add the dep to `[workspace.dependencies]` in the root `Cargo.toml` if it is
  used by more than one crate, then reference it as `foo = { workspace = true }`
  in each member's `Cargo.toml`.
- Keep internal crate versions (`aura-core`, `aura-plugin-rtk`) in sync — they
  are managed by `scripts/prepare-release.sh`.

### Working on the UI

The `.design/` directory is the source-of-truth for visual tokens. If you change
a color, spacing value, or component layout, update the matching entry there in
the same PR. See `.design/README.md`.

### Writing a plugin

Plugins are independent binaries that print a single JSON payload to stdout.
See [`docs/plugin-system.md`](./docs/plugin-system.md) for the full contract,
the 500ms timeout, and configuration via `~/.config/aura/config.toml`.

---

## Before opening a PR — `scripts/pre-pr.sh`

`scripts/pre-pr.sh` runs the **same checks as CI**, locally, in parallel where
possible. Always run it before pushing a PR.

```bash
./scripts/pre-pr.sh
```

### What it runs

The script mirrors `.github/workflows/quality-and-security.yml` and
`.github/workflows/secret_scanning.yml` (CodeQL and SBOM are GitHub-only and
intentionally skipped).

| Stage  | Check          | Action                                                    |
| ------ | -------------- | --------------------------------------------------------- |
| Wave 1 | `rustfmt`      | **Auto-fixes** with `cargo fmt --all`                     |
| Wave 1 | `clippy`       | `cargo clippy --workspace --all-targets --locked -D warnings` |
| Wave 1 | `test`         | `cargo test --workspace --locked`                         |
| Wave 1 | `gitleaks`     | Secret scan (skipped if `gitleaks` is not installed)      |
| Wave 2 | `cargo_audit`  | `cargo audit` — runs only if Wave 1 passes                |
| Wave 3 | `build_release`| `cargo build --workspace --release --locked` — runs only if `cargo_audit` passes |

### Reading the output

- Each check prints `PASS`, `FAIL`, or `SKIP` in a colored block, followed by a
  summary at the end.
- On failure, the script prints a hint pointing at the exact command to fix the
  issue. Exit code is non-zero so this composes well in scripts and pre-push hooks.
- `rustfmt` **auto-applies** formatting changes — after a run that touches
  formatting, `git status` will show modified files. Review and stage them.

### When checks are skipped

- `gitleaks` skips if the binary is missing. Install it from
  [gitleaks/gitleaks](https://github.com/gitleaks/gitleaks#installing) to mirror CI fully.
- `cargo_audit` and `build_release` skip if an earlier gate failed — fix the gate
  first, then rerun.

### Pre-push hook (optional)

To run the script automatically before every push:

```bash
echo '#!/usr/bin/env bash
exec ./scripts/pre-pr.sh' > .git/hooks/pre-push
chmod +x .git/hooks/pre-push
```

---

## Commit & PR conventions

- **Commit style** — match the existing history: a short imperative subject,
  optionally prefixed with a tag (`feat:`, `fix:`, `docs:`) and/or a phase
  marker (`Phase 4: …`). Run `git log` for examples.
- **One logical change per PR.** Split unrelated work into separate PRs.
- **Update docs alongside code.** If you change behavior, configuration, or
  visual design, update the matching file under `docs/` or `.design/` in the
  same PR.
- **Run `./scripts/pre-pr.sh` before pushing.** PRs that fail CI on lint, tests,
  audit, or secrets will not be merged until green.
- **Target `main`** unless coordinating a longer-running branch with a maintainer.

---

## Reporting bugs & requesting features

Open an issue at <https://github.com/Rfluid/aura/issues>. Helpful bug reports include:

- Aura version (`aura --version` once available, or commit SHA for source builds)
- OS + desktop environment (e.g., Ubuntu 24.04, GNOME on Wayland)
- Output of `journalctl --user -u aura -n 200` if the service is misbehaving
- Minimal reproduction steps

Security issues — please **do not** file a public issue. See `SECURITY.md`
(coming soon) or contact the maintainer directly.

---

## Documentation index

User-facing and architectural docs live under `docs/`. Visual design lives under
`.design/`.

### `docs/` — architecture, configuration, plugins

| Doc                                                    | What's in it                                                              |
| ------------------------------------------------------ | ------------------------------------------------------------------------- |
| [`architecture.md`](./docs/architecture.md)            | High-level system diagram: data collection, UI, plugin orchestration      |
| [`configuration.md`](./docs/configuration.md)          | `~/.config/aura/config.toml` schema, agent kinds, state file              |
| [`plugin-system.md`](./docs/plugin-system.md)          | Plugin contract, JSON IPC, timeout, authoring guide                       |
| [`ui-design.md`](./docs/ui-design.md)                  | UI behavior and interaction notes                                         |
| [`roadmap.md`](./docs/roadmap.md)                      | Planned features and ordering                                             |

### `.design/` — visual design system

| Doc                                                     | What's in it                                                              |
| ------------------------------------------------------- | ------------------------------------------------------------------------- |
| [`README.md`](./.design/README.md)                      | Design philosophy and index                                               |
| [`tokens.md`](./.design/tokens.md)                      | Canonical color, type, spacing, radius, shadow tokens                     |
| [`agents.md`](./.design/agents.md)                      | Per-agent brand colors and luminance fallback rule                        |
| [`components.md`](./.design/components.md)              | Visual primitives (stat-card, pill, tab, progress bar, plugin panel, modal) |
| [`customization.md`](./.design/customization.md)        | Schema for user-overridable theme (`~/.config/aura/theme.toml`)           |
| [`loading.md`](./.design/loading.md)                    | Spinner spec for fetch-triggering actions                                 |

### Other top-level docs

| File                                  | What's in it                                          |
| ------------------------------------- | ----------------------------------------------------- |
| [`README.md`](./README.md)            | Project overview, install, common commands            |
| [`PLAN.md`](./PLAN.md)                | Phased implementation plan                            |
| [`SPONSOR.md`](./SPONSOR.md)          | Sponsorship info                                      |

---

Thanks for contributing! If anything in this guide is unclear or out of date,
open a PR fixing it — that counts as a contribution too.
