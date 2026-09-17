---
title: Technical stack
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [context, architecture]
---

# Technical stack

## Status

Decisions locked (2026-05-21). See below.

## Language

**Rust** (stable toolchain). Chosen for performance, binary size, and RTK ecosystem fit.

## UI framework

**GPUI** — Zed's GPU-accelerated UI framework. Rust-native; visually polished. Trade-off: early ecosystem, sparse docs, API changes frequently. Accepted given the project's Rust-first ethos.

Key crates: `gpui` from the [zed-industries/zed](https://github.com/zed-industries/zed) monorepo (published as `gpui` on crates.io).

## Taskbar integration

**System tray icon** — cross-platform tray icon; click opens a floating GPUI window (the modal). Crate: `tray-icon` (or `ksni` for KDE/GNOME on Linux).

The modal is a borderless GPUI window anchored near the tray icon click position.

## Claude Code data source

Two sources, used together:

**Primary (Today / ThisMonth):** `~/.claude/projects/<project>/<session>.jsonl`
Real-time per-message records. Each `assistant` entry has `timestamp` + `message.usage` (input/output/cache tokens, model). Updated live as messages complete. Filter by file mtime to skip old files cheaply, then by `timestamp` for the period.

**Baseline (AllTime fast path):** `~/.claude/stats-cache.json`
Periodic rollup. Stale by months in practice (observed: last updated 2026-02-16 despite active use). Use only as a cumulative baseline for all-time totals; add JSONL files newer than `lastComputedDate` on top.

**Live updates:** `inotify` (Linux) / `kqueue` (macOS) watcher on the `projects/` directory. On JSONL modification, re-read that file's new tail and update the running token sum. Keeps tray + modal current without polling.

`costUSD` in `stats-cache.json` is always 0 (subscription billing). Estimated cost computed from token counts × Anthropic published pricing.

_No `claude usage` CLI subcommand exists._

## Antigravity data source

Two sources, neither of them shaped like the others.

**Activity:** `~/.gemini/antigravity-cli/conversation_summaries.db` — SQLite,
one row per conversation (`step_count`, `last_modified_time`,
`last_user_input_time`, `app_data_dir`). Covers both the CLI and the
Antigravity IDE; Aura counts every row. Opened read-only through a
`file:…?mode=ro` URI so a running `agy` is never blocked, falling back to
`immutable=1` when the `-shm` file can't be created.

**Quota:** `agy -p "/usage" --output-format json`, a documented public flag.
The slash command is answered locally from a cached backend reading, so it
starts no LLM turn and consumes no quota; it costs ~3 s of Go-binary startup,
which a 20 s TTL cache absorbs. The underlying RPC
(`v1internal:retrieveUserQuotaSummary`) is not usable directly: `agy` keeps
its OAuth token in the OS keyring and the call is license-gated on the CLI's
own client identity.

_No token counts are recoverable._ The trajectories in
`conversations/<uuid>.db` are schema-less protobuf with no plaintext model
names, so `UsageSnapshot::tokens_unreported` marks the token fields absent
rather than zero.

## Plugin loading

**Subprocess + JSON IPC** — Aura spawns the plugin binary and reads a JSON panel payload from stdout. Any language can author plugins. 500ms timeout; plugins that exceed it are shown in an error state.

## Serialization

`serde` + `toml` for config; `serde` + `serde_json` for state persistence and plugin IPC.

## SQLite

`diesel` (sqlite backend, no default features) with `libsqlite3-sys/bundled`, for
Antigravity's `conversation_summaries.db`. Bundled so no host `libsqlite3` is
required on any release target. Five of the six build natively; only
`aarch64-pc-windows-msvc` cross-compiles, on an x86_64 `windows-latest` runner
that carries `Microsoft.VisualStudio.Component.VC.Tools.ARM64` — and that
target is already `experimental: true`. The workspace already compiles C
(`cc` arrives via the gpui tree), so this adds no new class of dependency.

## Error handling

`anyhow` for binary crates; `thiserror` for library crates.
