---
title: Plugin system
status: stable
version: 0.1.0
last_updated: 2026-05-23
source_refs:
  - crates/aura-core/src/plugin/mod.rs
  - crates/aura-core/src/plugin/runner.rs
  - crates/aura-core/src/plugin/discovery.rs
owner: "@rfluid"
tags: [plugins, docs]
---

# Plugin system

## Overview

Plugins extend Aura with custom metrics panels displayed in the modal
beneath the core usage stats. Any developer can author a plugin in any
language that can print JSON. **The core install ships no plugins** —
the repo includes first-party plugin sources (`plugins/rtk-gains/`,
`plugins/hello/`), but every plugin is installed separately via
`aura plugin add` or by dropping a binary into the user plugins dir.

This document describes the system itself. For the practical
how-to — wire schema, install flow, checklist — see
[`plugin-authoring.md`](plugin-authoring.md).

## Architecture

```
┌────────────────────┐   spawns        ┌──────────────────────┐
│ aura (host, GPUI)  │ ──────────────▶ │ aura-plugin-foo      │
│                    │ ◀────────────── │ (stand-alone binary) │
│ PluginRunner       │   JSON / stderr └──────────────────────┘
│ → PluginPanel      │   exit code
│ → modal section    │
└────────────────────┘
```

- **Loading strategy**: subprocess + JSON IPC (no dynamic linking).
  This keeps the ABI a stable wire format, so plugins survive Aura
  rebuilds and can be written in any language.
- **Lifetime**: spawned at modal open, output cached for the modal's
  lifetime, killed if it exceeds the budget.
- **Isolation**: plugins have no write access to Aura's config or state.
  They can read whatever the host user can read.

## Plugin sources

A plugin reaches the modal via one of three paths, merged at modal open:

1. **`[[plugins]]` entries in `config.toml`** — the classic path. Used
   for system-wide plugins on `$PATH` and any user override.
2. **Auto-discovery from `~/.config/aura/plugins/`** — every executable
   file in that directory is treated as a plugin. Sidecar TOML
   (`<binary>.toml`) supplies optional display metadata.
3. **`aura plugin add <path>`** — convenience wrapper that copies (or
   symlinks) a binary into the user plugins dir and writes the sidecar.

On display-name collision, config entries always win, so users can
override the color/icon of a discovered plugin without removing the
binary.

## Wire contract (summary)

Full schema and examples: [`plugin-authoring.md`](plugin-authoring.md#wire-contract).

| Aspect       | Contract                                                  |
| ------------ | --------------------------------------------------------- |
| Invocation   | `<binary> --period <all\|7d\|30d>`                        |
| Output       | One UTF-8 JSON object on stdout (`PluginPanel`)           |
| Timeout      | 500 ms; exceeded → error panel                            |
| Exit non-0   | stderr surfaced as the panel error                        |
| Soft errors  | `{"title": "...", "error": "..."}` with exit 0            |

The `PluginPanel` schema lives in
[`aura-core/src/plugin/mod.rs`](../crates/aura-core/src/plugin/mod.rs)
and is forward-compatible: the runner falls back to the legacy flat
`{title, lines, error}` shape when a plugin doesn't emit `sections`.

## First-party plugins (all opt-in)

### RTK Gains

Source: [`plugins/rtk-gains/`](../plugins/rtk-gains). Surfaces tokens
saved by the Rust Token Killer optimizer (today, this month, lifetime,
savings rate, command count). See
[`plugins/rtk-gains/README.md`](../plugins/rtk-gains/README.md) for
build + install instructions.

### Hello (reference)

Source: [`plugins/hello/`](../plugins/hello). Minimal demonstration of
the wire contract — emits both `lines` and `table` sections from
static data. Built by `cargo build --workspace`; install via
`aura plugin add ./target/release/aura-plugin-hello`.

## Future

- Plugin registry (`aura plugin install <name>`) — discover and pull
  community plugins from a central index. Out of scope for v0.1.
- Signed plugin manifests for trust on first use.
