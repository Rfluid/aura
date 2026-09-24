---
title: Memory index
status: current
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [memory, index]
---

# Memory index

Auto-grown list of memory entries. Agents append after each task that produced new learning.

## facts/

- [2026-05-21-claude-code-stats-cache-stale](facts/2026-05-21-claude-code-stats-cache-stale.md) — `stats-cache.json` is a stale periodic rollup; live token data lives in per-session JSONL files under `projects/`
- [2026-05-21-claude-usage-display-format](facts/2026-05-21-claude-usage-display-format.md) — Exact fields, computation logic, and date-range strategy of `claude /usage`; total tokens = input+output only (cache excluded)
- [2026-09-23-gpui-keymap-dispatch](facts/2026-09-23-gpui-keymap-dispatch.md) — GPUI bindings need a focused root element; same-node contexts tie on depth so insertion order decides; `NoAction` masks lower contexts

## patterns/

- [2026-09-16-issue-triage-before-publishing](patterns/2026-09-16-issue-triage-before-publishing.md) — Diagnose Aura support reports against docs/source first, then file or update GitHub issues only when evidence warrants it

## lessons/

_(empty — populate as you learn)_
