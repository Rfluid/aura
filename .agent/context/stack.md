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

## Plugin loading

**Subprocess + JSON IPC** — Aura spawns the plugin binary and reads a JSON panel payload from stdout. Any language can author plugins. 500ms timeout; plugins that exceed it are shown in an error state.

## Serialization

`serde` + `toml` for config; `serde` + `serde_json` for state persistence and plugin IPC.

## Error handling

`anyhow` for binary crates; `thiserror` for library crates.
