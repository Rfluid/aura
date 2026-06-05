---
title: Insights stats tab (F3)
status: design
version: 0.1.0
last_updated: 2026-06-05
owner: "@pedro (fork) → upstream rfluid/aura"
tags: [stats, reader, ui, design]
---

# F3 — Insights tab

## Problem

The current modal shows aggregate token totals and per-model breakdowns, but never answers the
"interesting" questions a heavy user actually asks:

- **Which project burned the most tokens?**
- **Which single session was the most expensive**, and **what mode was it running in**
  (model tier + whether it was an `ultracode` / high-effort session)?

The data to answer these is already scanned but thrown away (`SessionStat` in
`reader/scan.rs:58` is tagged `#[allow(dead_code)] // used in future UI for session detail`).
This feature lights it up.

## Scope

A new modal tab named **Insights**, alongside Quota / Summary / Models / Forecast / Plugins.
Honors the existing **period selector** (All / 7d / 30d). Three cards:

1. **Top projects** — table of project dir → total tokens (input+output), top ~5, with a bar.
2. **Top sessions** — table of the most expensive sessions: tokens, duration, project, and a
   **mode badge group**: model tier (`opus` / `sonnet` / `haiku`) + an `ultracode` chip when
   detected. Top ~5.
3. **Mode distribution** — share of tokens by model tier, and count of `ultracode` sessions vs
   normal, for the period.

Out of scope: editing/filtering, per-message drilldown, non-Claude agents.

## Data sources & computation

All from the existing JSONL scan — **no new file formats**.

### Per-project tokens
`scan.rs::list_session_files()` already returns `(PathBuf, is_subagent)` and walks
`projects/<project-dir>/`. The project dir name is the slugified cwd
(e.g. `-Users-pedro-Downloads-cambrian-api-key-dashboard`). Extend `ScanAccum` to accumulate
`tokens_by_project: HashMap<String, ModelAccum>` keyed by project-dir, filled in `scan_files()`
where it already parses each entry. Unslug the dir for display (replace leading `-`/`-` → `/`)
— add a small `humanize_project(dir) -> String` helper (last path segment is enough for the UI).

### Per-session tokens + duration
`SessionStat` already exists. Ensure it carries: `session_id`, `project_dir`, `total_tokens`
(input+output), `start_timestamp`, `last_timestamp` (duration = last − start),
`dominant_model` (model with most tokens in that file). Most of these already accrue in the
`scan_files()` per-file loop; add `dominant_model` and `total_tokens` if missing. Remove the
`#[allow(dead_code)]`.

### `ultracode` detection (per session)
`ultracode` is **not** a structured field. It is detectable by content markers in the session
JSONL (verified on real data: 82 sessions contain the `ultracode` string, 23 contain a
`Workflow` tool_use). Detection rule for a session file:

```
is_ultracode = file contains a tool_use entry with "name":"Workflow"
            OR file contains the literal token "ultracode" in a user/system message
```

Implement as a cheap byte-substring check while the file is already being read in
`scan_files()` (one `memchr`-style scan per file; do **not** re-open the file). Store
`is_ultracode: bool` on `SessionStat`. This is a heuristic — document it as such in rustdoc and
in the tab's footnote ("mode is inferred from session content").

> Design note: keep the markers in a `const ULTRACODE_MARKERS: &[&str]` so the rule is
> auditable and unit-testable. Model tier comes straight from `dominant_model`.

## Architecture

- **`aura-core/src/reader/scan.rs`** — extend `ScanAccum` (`tokens_by_project`) and `SessionStat`
  (`total_tokens`, `dominant_model`, `is_ultracode`). All accumulation happens in the existing
  single pass — **no second scan, no extra file I/O**, protecting the RAM/CPU budget.
- **`aura-core/src/reader/insights.rs`** (new) — pure functions over the scan output:
  - `top_projects(accum, n) -> Vec<ProjectStat>`
  - `top_sessions(accum, n) -> Vec<SessionStat>` (sorted by tokens desc)
  - `mode_distribution(accum) -> ModeDistribution` (tokens by tier, ultracode count)
  - New serializable structs: `ProjectStat { name, dir, tokens, message_count }`,
    `ModeDistribution { by_tier: Vec<(String,u64)>, ultracode_sessions: u32, normal_sessions: u32 }`.
  - Wire these into `UsageSnapshot` (add `insights: InsightsSnapshot` or expose via a new
    `ClaudeCodeReader::read_insights(period)` — prefer extending `UsageSnapshot` so the existing
    period plumbing in `app.rs` is reused).
- **`aura/src/app.rs`** — add `Insights` to the tab enum, a tab button in `render_tab_row`, a
  `render_insights(theme, snap, accent)` fn following `render_models` as the structural
  template (cards, bars, rows). Reuse `format.rs` token/number formatting.
- **`aura-core/src/config.rs`** — `[insights]` section with `enabled: bool` (default `false`)
  and `top_n: usize` (default 5). Tab hidden unless enabled.

## UI sketch (terminal mock)

```
┌ Insights ──────────────────────  [All] [7d] [30d] ┐
│ TOP PROJECTS                                       │
│  cambrian-api-key-dashboard  ████████████  18.2M   │
│  aura                        ██████        9.1M    │
│  reconhecimento              ███           4.4M    │
│                                                    │
│ TOP SESSIONS                                       │
│  18.2M · 2h14m · cambrian…   [opus] [ultracode]    │
│   9.0M · 1h02m · aura        [opus]                │
│   3.1M · 22m   · api-docs    [sonnet]              │
│                                                    │
│ MODE DISTRIBUTION                                  │
│  opus 92%  sonnet 6%  haiku 2%                     │
│  ultracode: 23 sessions · normal: 412              │
│  ⓘ mode inferred from session content              │
└────────────────────────────────────────────────────┘
```

Badges use existing theme accent tokens — no hardcoded colors.

## Testing

- Unit tests in `scan.rs` / `insights.rs` using the existing `tempfile`-based fixtures pattern
  (`assistant_entry()` helper already in `claude_code.rs` tests). Cases:
  - two projects, assert ranking + token sums.
  - a session containing a `Workflow` tool_use → `is_ultracode == true`; one without → `false`;
    one with the literal `ultracode` string → `true`.
  - `dominant_model` picks the higher-token model in a mixed-model session.
  - `mode_distribution` percentages sum to 100 (± rounding) and counts are correct.
- No GPUI render tests (repo doesn't have them); keep all logic in `aura-core` so it's testable
  headless.

## Risks / notes

- `ultracode` detection is heuristic and Claude-Code-version-dependent. Footnote + rustdoc make
  this explicit. If the marker disappears in a future CC version the chip simply stops showing —
  no crash, no wrong totals (tier badge still works).
- Substring scan adds a few µs per file; negligible vs JSON parse already happening.
