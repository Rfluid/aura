---
title: CLI surface contract
status: current
version: 0.1.0
last_updated: 2026-05-24
last_verified: 2026-05-24
source_refs: ["crates/aura/src/cli/", "docs/cli.md"]
owner: "@rfluid"
tags: [context, cli, conventions]
---

# CLI surface contract

Aura is both a tray app and a Clap-driven CLI. Every user-facing feature
should be reachable from the shell, not just the modal. When adding or
refactoring a feature, plan for both surfaces from the start.

## When a new feature touches user state

Default to exposing it via the CLI. The bar is low: a single subcommand
under `aura <noun> <verb>` that prints a result or mutates state is
enough. Read commands should accept `--format text|json` from day one
so the feature works in `jq`/status-bar pipelines without scraping.

Concrete examples already in tree (`crates/aura/src/cli/`):

| Module       | Pattern it demonstrates                                                 |
| ------------ | ----------------------------------------------------------------------- |
| `config.rs`  | Subcommand group with text+JSON read paths and an `edit` shell-out      |
| `state.rs`   | Read + mutate + validate-against-config (`set-profile`)                 |
| `plugin.rs`  | Mixed mutating commands (`add`, `remove`) and read commands (`list`)    |
| `usage.rs`   | Profile resolution (`--profile`/state/first agent) + period enum reuse  |
| `quota.rs`   | AgentKind dispatch for source-specific behavior                         |
| `doctor.rs`  | Aggregated diagnostic with both text and JSON output                    |

## Hard rules

1. **`aura-core` types must derive `Serialize`** when they're read by the
   UI. The CLI's `--format json` is the proof. If you add a new struct
   that the modal renders, derive `Serialize` at the same time — don't
   leave the CLI side as a TODO.
2. **No new hand-rolled arg parsing.** Everything goes through `clap`
   derive. If you find yourself reaching for `std::env::args`, reach for
   a new module under `cli/` instead.
3. **Backward-compatible spellings are hidden aliases, not first-class
   commands.** `aura setup-config` is the model: `#[command(hide = true,
   name = "setup-config")]` on a top-level variant that calls the
   canonical handler. Installer scripts and old muscle memory keep
   working; help output stays clean.
4. **Profile resolution goes through `cli::resolve::resolve_profile`.**
   `--profile` flag → `state.active_profile` → first agent in config.
   Don't reinvent this — `usage`, `quota`, and `plugin run` all use it.
5. **Tray dispatch lives in `main.rs`.** The CLI is the entry point;
   `main.rs` only falls through to the tray when `cli.command` is
   `None`. Headless subcommands must never spin up GPUI.

## Adding a new subcommand

1. Create `crates/aura/src/cli/<noun>.rs` with a `<Noun>Cli` struct
   (`#[derive(Args)]`) and a `run(self) -> Result<()>` method.
2. Wire it into `cli/mod.rs`: `mod <noun>;` plus a `Command::<Noun>`
   variant.
3. If the subcommand reads data: take `--format` and route through
   `cli::format::print_json` for the JSON arm.
4. If it shells out to an editor: use `cli::theme::open_in_editor`.
5. Update `docs/cli.md` and the README CLI section.
6. Add tests if the logic is non-trivial. Pure formatters can be tested
   inline in the module.

## Smoke-testing a new command

The `--help` ladder is the fastest sanity check during development:

```bash
cargo run -p aura -- --help
cargo run -p aura -- <noun> --help
cargo run -p aura -- <noun> <verb> --help
```

For commands that read live data, run them with both formats:

```bash
cargo run -p aura -- <noun> <verb>
cargo run -p aura -- <noun> <verb> --format json | jq .
```

## When not to expose something on the CLI

- Tray IPC commands (`show`, `toggle`, `quit`) — these need a control
  socket aura doesn't have yet. Track in roadmap, not as a half-built
  CLI command.
- Anything that requires GPUI to render. Diagnostics about render state
  belong in `aura doctor`'s text output, not as a runtime command.
