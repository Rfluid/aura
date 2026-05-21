---
id: 2026-05-21-claude-usage-display-format
type: fact
title: Exact fields and computation logic of `claude /usage` (a.k.a. /stats, /cost)
status: current
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
discovered: 2026-05-21
source_task: ~
source_refs:
  - path: ~/.local/share/claude/versions/2.1.146
owner: "@rfluid"
confidence: high
tags: [claude-code, data-source, ui, architecture]
supersedes: ~
---

# Exact fields and computation logic of `claude /usage`

## What

Reverse-engineered from the Claude Code 2.1.146 binary (embedded minified JS).

## Display: Overview tab

- Favorite model (model with highest input+output tokens)
- Total tokens = `inputTokens + outputTokens` across all models (cache NOT included)
- Sessions count
- Longest session (formatted duration)
- Current streak / Longest streak (consecutive active days)
- Active days / total days in period
- Peak hour (most session starts)
- Fun fact comparing token count to a book

## Display: Models tab

- "Tokens per Day" ASCII line chart (input+output only, up to 3 models)
- Per-model card: name, % of total, `In: N · Out: N`

Cache tokens (`cacheReadInputTokens`, `cacheCreationInputTokens`) are tracked internally in `modelUsage` but not shown anywhere in the `/usage` display.

## Computation (from `ML8` function)

For each `assistant` entry in each JSONL file in the date range:
- Accumulate `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens` per model
- `dailyModelTokens[date][model]` = input + output only (no cache)
- Session stats derived from first/last entry timestamps per file

## All-time period strategy

1. Load `stats-cache.json` as baseline
2. Find JSONL files newer than `lastComputedDate`
3. Scan those files, merge into cached totals
4. Write updated cache back to `stats-cache.json`
5. Always append TODAY's JSONL data on top (regardless of cache date)

## 7d/30d period strategy

Ignore cache entirely. Scan all JSONL files, filter by mtime first (cheap), then by entry timestamp. No cache write.

## Date ranges

`wL8 = ["all", "7d", "30d"]` — labeled "All time", "Last 7 days", "Last 30 days". Cycle with 'r' key.

## How to apply

Aura's Claude Code adapter must replicate this logic exactly. "Total tokens" in the tray widget and modal = input+output only. Show the same 6 summary fields in the Overview panel. Models panel shows per-model In/Out (not cache). Cache breakdown can be an optional extra detail row.
