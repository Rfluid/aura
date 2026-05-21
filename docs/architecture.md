---
title: Architecture
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [architecture, docs]
---

# Architecture

## Overview

Aura is a single Rust binary with three main responsibilities:

1. **Data collection** — read usage snapshots from each configured agent's local data store
2. **UI rendering** — display a small modal near the taskbar when invoked
3. **Plugin orchestration** — load and query plugins that contribute additional metrics panels

```
┌──────────────────────────────────────────────────────┐
│  Aura binary                                         │
│                                                      │
│  ┌─────────────┐    ┌──────────────┐                 │
│  │  Config     │    │  State store │                 │
│  │  loader     │    │  (active     │                 │
│  │  (TOML)     │    │   profile)   │                 │
│  └──────┬──────┘    └──────┬───────┘                 │
│         │                  │                         │
│         ▼                  ▼                         │
│  ┌──────────────────────────────────┐                │
│  │  Agent registry                  │                │
│  │  (profile list + active pointer) │                │
│  └──────────────┬───────────────────┘                │
│                 │                                    │
│        ┌────────▼────────┐                           │
│        │  Usage reader   │  (per-agent adapter)      │
│        │  ┌───────────┐  │                           │
│        │  │ ClaudeCode│  │  reads ~/.claude/          │
│        │  ├───────────┤  │                           │
│        │  │  Codex    │  │  reads ~/.codex/ or API   │
│        │  └───────────┘  │                           │
│        └────────┬────────┘                           │
│                 │                                    │
│        ┌────────▼────────┐    ┌──────────────────┐  │
│        │  Plugin runner  │◄───│  Plugin registry │  │
│        │  (IPC or dlopen)│    │  (from config)   │  │
│        └────────┬────────┘    └──────────────────┘  │
│                 │                                    │
│        ┌────────▼────────┐                           │
│        │  UI renderer    │                           │
│        │  (modal window) │                           │
│        └─────────────────┘                           │
└──────────────────────────────────────────────────────┘
```

## Data flow

1. User clicks the taskbar widget → OS sends a signal / spawns `aura show`
2. Aura loads config from `~/.config/aura/config.toml`
3. Aura reads the active profile from `~/.local/share/aura/state.json`
4. Aura calls the usage reader for the active profile's agent kind
5. Aura spawns/queries each configured plugin for their panel data
6. Aura renders the modal with usage + plugin panels
7. User clicks a different profile → Aura swaps the active profile, re-reads usage, re-renders
8. User closes the modal → Aura writes the active profile to state, hides the window

## Agent adapters

Each agent kind implements the `AgentReader` trait:

```rust
trait AgentReader {
    fn read_snapshot(&self, config: &AgentConfig, period: Period) -> Result<UsageSnapshot>;
}

enum Period { Today, ThisMonth, AllTime }

struct UsageSnapshot {
    tokens_input: u64,
    tokens_output: u64,
    tokens_cache_read: u64,
    tokens_cache_write: u64,
    estimated_cost_usd: f64, // computed from token counts; CC subscription = $0 actual
    messages: u64,
    sessions: u64,
}
```

### Claude Code adapter — data sources and computation

The `/usage` (a.k.a. `/stats`) command implementation was reverse-engineered from the Claude Code binary. Aura replicates the same logic.

**Per-session JSONL files** — `{config_path}/projects/<project-dir>/<session-id>.jsonl`

The primary data source. Every `assistant`-type entry has a `timestamp` and `message.usage`:

```json
{
  "type": "assistant",
  "timestamp": "2026-05-16T19:53:11Z",
  "message": {
    "model": "claude-opus-4-7",
    "usage": {
      "input_tokens": 6,
      "output_tokens": 276,
      "cache_creation_input_tokens": 9176,
      "cache_read_input_tokens": 14768
    }
  }
}
```

**`stats-cache.json`** — `{config_path}/stats-cache.json`

Used as a baseline for "All time" only. Claude Code reads it, then adds a delta of JSONL files newer than `lastComputedDate`, then writes the updated cache back. For 7d/30d, the cache is ignored entirely — JSONL files are scanned directly.

### Stats computation (mirrors `/usage`)

```
For each JSONL file in projects/**/:
  1. Skip by file mtime if mtime < fromDate (cheap OS check)
  2. Parse assistant entries in date range
  3. Accumulate per model:
       inputTokens   += usage.input_tokens
       outputTokens  += usage.output_tokens
       cacheRead     += usage.cache_read_input_tokens
       cacheWrite    += usage.cache_creation_input_tokens
  4. dailyModelTokens[date][model] += input_tokens + output_tokens  ← cache NOT included
```

**"Total tokens"** = `inputTokens + outputTokens` across all models (cache tokens are NOT added).

### What `/usage` displays

**Overview panel:**

| Field | Source |
|---|---|
| Favorite model | model with highest `inputTokens + outputTokens` |
| Total tokens | sum of `inputTokens + outputTokens` across all models |
| Sessions | count of unique session files in period |
| Longest session | max `lastTimestamp - firstTimestamp` across sessions |
| Active days | count of calendar days with at least one message |
| Longest streak | longest consecutive-day run |
| Current streak | consecutive days up to today |
| Peak hour | hour with most session starts |

Date ranges: **All time** / **Last 7 days** / **Last 30 days**

**Models panel:**

- Tokens per Day ASCII chart (input+output only, up to 3 models)
- Per-model card: model name · percentage · `In: N · Out: N`

Cache token breakdown (`cacheReadInputTokens`, `cacheCreationInputTokens`) is tracked internally but **not shown** in the `/usage` display. Aura will show them as a secondary detail panel.

### Freshness

`stats-cache.json` is updated infrequently by Claude Code itself (observed: months stale). Aura must always scan JSONL files for the 7d/30d windows. For "All time", Aura replicates Claude Code's own strategy: load cache as baseline, scan only the JSONL files newer than `lastComputedDate`, merge and save.

**Live update:** `inotify` (Linux) / `kqueue` (macOS) watcher on `projects/`. On any JSONL append, re-read that file's new tail and update the running sums. Tray widget number updates within seconds of a message completing.

## State persistence

Active profile is written to disk on every profile change:

```json
// ~/.local/share/aura/state.json
{
  "active_profile": "Claude Code (Personal)",
  "last_updated": "2026-05-21T14:30:00Z"
}
```

## Decisions locked (2026-05-21)

| Decision | Choice | Rationale |
|---|---|---|
| UI framework | **GPUI** | Rust-native, GPU-accelerated; accepted API churn risk |
| Taskbar integration | **System tray icon** | Cross-platform; click opens a GPUI borderless window |
| Plugin loading | **Subprocess + JSON IPC** | Any language can author plugins; no ABI fragility |
| Claude Code data | **`~/.claude/stats-cache.json`** | No `usage` CLI subcommand exists; file is structured JSON with full token breakdown |

## Open questions

- `stats-cache.json` schema stability across Claude Code versions (file has a `version` field — watch for bumps)
- Codex usage data source (local files vs. OpenAI API usage endpoint)
- GPUI window positioning API for anchoring the modal near the tray icon
