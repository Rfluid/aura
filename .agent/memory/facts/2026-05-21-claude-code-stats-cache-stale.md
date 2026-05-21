---
id: 2026-05-21-claude-code-stats-cache-stale
type: fact
title: stats-cache.json is a stale periodic rollup; live token data lives in per-session JSONL files
status: current
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
discovered: 2026-05-21
source_task: ~
source_refs:
  - path: ~/.claude/stats-cache.json
  - path: ~/.claude/projects/
owner: "@rfluid"
confidence: high
tags: [claude-code, data-source, architecture]
supersedes: ~
---

# stats-cache.json is a stale periodic rollup

## What

`~/.claude/stats-cache.json` is a versioned JSON rollup of all-time usage. Its `lastComputedDate` field was observed to be `2026-02-16` despite active Claude Code use through May 2026 — months of data are missing from it.

The live source is `~/.claude/projects/<project-dir>/<session-id>.jsonl`. Each file is appended in real time. Every `assistant`-type entry has a `timestamp` (ISO 8601) and `message.usage` (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, model).

## Why it matters

Aura must NOT use `stats-cache.json` as the primary source for Today/ThisMonth views — it will show data from months ago. The JSONL files are the only reliable fresh source.

## How to apply

- **Today / ThisMonth**: scan `projects/**/*.jsonl`, filter by file mtime (cheap), then by `timestamp` in each entry.
- **AllTime**: use `stats-cache.json` as a fast baseline for historical totals + sum JSONL files newer than `lastComputedDate` for the delta.
- **Live updates**: `inotify` (Linux) / `kqueue` (macOS) watcher on the `projects/` dir; re-read only modified JSONL tails.
