---
title: Activity
status: draft
version: 0.1.0
last_updated: 2026-06-05
last_verified: 2026-06-05
source_refs: ["crates/aura-core/src/activity.rs", "crates/aura/src/app.rs"]
owner: "@pedro"
---

The **Activity** tab is a live process monitor scoped to **Claude Code only**.
When you run many parallel Claude Code sessions and the machine bogs down, it
tells you — at a glance, refreshed live — which session (and which child
process inside it) is the CPU/RAM hog.

It is **opt-in and hidden by default**. Enable it with:

```
aura config set activity.enabled true
```

It only appears for the Claude Code agent (not Codex/Gemini), since the scope
is the `claude` CLI and its descendants.

## What it shows

One card per Claude Code **session** (a CLI root plus its full descendant
subtree), sorted by total CPU% descending:

```
ACTIVITY · live                                   ⟳ 3s
● reconhecimento · 7c51…      312% CPU · 2.1 GB
    ↳ node mcp-server-figma    180% · 0.8 GB
    ↳ bash cargo build         132% · 1.2 GB
● jp · a3d1…                    44% CPU · 0.6 GB
    ↳ claude (main)             44% · 0.6 GB
Total Claude Code: 356% CPU · 2.7 GB · 2 sessions
```

- **Total CPU%** per session is the sum of the subtree's `cpu_usage`. It can
  exceed 100% — that's a multi-core reading, not an error, which is why the
  label is "CPU", not "core".
- **RAM** is the sum of the subtree's resident set size (RSS).
- Each card surfaces the **heaviest 1–3 child processes** (the culprits) with a
  short readable label derived from the process command line (e.g.
  `node mcp-server-figma`, `bash cargo build`).
- **Empty state:** `No Claude Code processes running.`
- **Footer:** account-wide totals — CPU% · RAM · session count.

On the very first sample there is no prior point to compute a CPU delta
against, so CPU reads as `measuring…` for one tick; RAM is correct immediately.

## What counts as "Claude Code"

A **Claude Code root** is a process whose executable identifies the `claude`
CLI: `Process::name()` is `claude` (`.exe`-tolerant, case-insensitive), or the
first argv token's basename is `claude`. On this machine the signature is e.g.
`claude --dangerously-skip-permissions` and `claude --resume <uuid>`.

The desktop `Claude.app` (executable name `Claude`) and `aura` itself are
**excluded** — the scope is the CLI.

**Descendants** are collected by building a `ppid → children` map over all
processes and walking each root's full subtree. This captures the real hogs —
MCP servers (`node` / `python` children), and the `bash` / build commands the
CLI launched (`cargo build`, etc.). Those child processes are never treated as
roots themselves; they only appear as part of a session's subtree.

## Mapping a session to a window

Each `claude` root's working directory (`Process::cwd()`, reliable on
macOS + Linux) is the project directory; its basename is the **project name**
(e.g. `reconhecimento`).

The **active session id** is resolved from `~/.claude/projects/<slug>/`, where
`<slug>` is Claude Code's slugification of the cwd: every `/` **and** `.` is
replaced with `-`. For example
`/Users/x/AI-Outreach/.brand-research` →
`-Users-x-AI-Outreach--brand-research`. The newest `*.jsonl` file in that
directory is the live session; its short id (first 4 chars of the uuid) is
shown as `project · <short-id>…`. When no directory matches, the card shows
just the project (or `pid <n>` when the root has no cwd).

## Live refresh

Sampling runs **only while the modal is open and the Activity tab is active**.
It stops the instant you switch tabs (a generation token is bumped, which the
self-rescheduling tick checks) or the modal closes (the view drops, the
`update` call fails, the chain ends). There is **zero background cost**
otherwise — the monitor must never become a hog itself.

The cadence is `activity.refresh_secs` (default 3, clamped to ≥ 1 at use). The
loop mirrors aura's spinner-tick pattern: a `cx.spawn` + `background_executor`
timer chain that re-schedules itself only while still valid.

A single long-lived `sysinfo::System` is kept across ticks (inside
`ActivityMonitor`) so CPU% deltas compute correctly; the refresh interval
provides the spacing sysinfo needs (`MINIMUM_CPU_UPDATE_INTERVAL`).

## Configuration

| Key                     | Type   | Default | Meaning                                                        |
| ----------------------- | ------ | ------- | -------------------------------------------------------------- |
| `activity.enabled`      | `bool` | `false` | Show the Activity tab and sample while it's on screen.         |
| `activity.refresh_secs` | `u64`  | `3`     | Seconds between live re-samples. Clamped to a minimum of 1.    |

## Architecture

The pure logic lives in `crates/aura-core/src/activity.rs` and is unit-tested
headless against a synthetic process list (no real OS calls): `classify_roots`,
`build_subtrees`, `project_from_cwd` / `slugify_cwd`, `active_session_for`
(injected with a tempdir), the CPU/mem summation, and the heaviest-children
selection. The OS-touching `ActivityMonitor` (gated behind the default-on
`activity` Cargo feature) owns the `sysinfo::System` and is the only part that
reads the live process table.

The GUI (`crates/aura/src/app.rs`) holds the `ActivityMonitor`, ticks it on the
timer while the tab is active, and renders the cards with the same card / row /
badge primitives as the rest of the modal.
