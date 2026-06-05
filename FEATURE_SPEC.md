---
title: Weekly budget pacing (F2)
status: design
version: 0.1.0
last_updated: 2026-06-05
owner: "@pedro (fork) → upstream rfluid/aura"
tags: [forecast, quota, pacing, design]
---

# F2 — Budget pacing

## Problem

The Forecast tab (`quota/forecast.rs`) already answers *"if I keep this pace, where does each
window land?"*. It does **not** answer the question a Max user actually has mid-week:

> "How much can I spend **in this 5h session** without running out of my **weekly** limit
> before it resets?"

Naively dividing weekly-remaining by remaining-5h-windows is wrong: a Max plan renews the 5h
window ~4–5×/day, but the user is only *actively coding* in a few of them. Pacing against all
renewals understates how much each active session can use. Pacing must be against the user's
**actual active-session pattern**.

## Scope

Extend the existing **Forecast** tab (do not add a new tab) with one new element per render:
a **session budget gauge** for Claude Code:

- **"You can use ~X% of this 5h session"** — the share of a full 5h window this session may burn
  so the weekly window lands at/under 100% at `resets_at`.
- A live gauge: current 5h usage vs the recommended ceiling (green under, amber near, red over).
- A one-line rationale ("~3 active sessions left this week · 42% weekly remaining").

Claude Code only (richest data, matches forecast rollout order). Codex/Gemini show nothing new.

## Ground truth: pace on **percentage**, not token caps

The exact weekly token cap is **not** published and Aura must not guess it. But
`QuotaWindow.used_percentage` (in `quota/mod.rs`) is the **API-reported** real utilization for
both the 5h and weekly windows (`quota/api.rs` → `https://api.anthropic.com/api/oauth/usage`).
**All pacing math is in percentage of the weekly window.** This sidesteps the unknown cap
entirely and stays correct across plan tiers.

## Algorithm

Inputs (all already available):
- `weekly = ` the 7d `QuotaWindow` (`used_percentage`, `resets_at`, `length_minutes`).
- `session = ` the 5h `QuotaWindow` (`used_percentage`, `resets_at`, `length_minutes`).
- History: per-day **active-session count** derived from JSONL (see below).

Steps:

```
1. weekly_remaining_pct = 100 - weekly.used_percentage
2. days_left            = (weekly.resets_at - now) as fractional days
3. active_per_day       = learned typical active-sessions/day   (see "Learning")
4. sessions_left        = active_per_day * days_left
                          - (active sessions already used today)      // floor at ~0.5
5. weekly_pct_per_session = weekly_remaining_pct / sessions_left
6. // express as a share of ONE full 5h window:
   //   a "full" session historically moves weekly by `avg_weekly_pct_per_active_session`
   session_budget_pct = min(100,
        100 * weekly_pct_per_session / avg_weekly_pct_per_active_session)
7. headroom_in_current = session_budget_pct - session.used_percentage  // for the gauge color
```

`avg_weekly_pct_per_active_session` is itself learned: total weekly-% consumed over the trailing
window ÷ number of active sessions in it. If history is too thin (< INSUFFICIENT threshold,
reuse `forecast::INSUFFICIENT_BELOW` spirit), show a "warming up" state instead of a number —
**never** show a fabricated budget.

### Learning the active-session pattern

An "active session" = a 5h window (by session start, bucketed) whose token usage exceeds a
threshold, so idle/one-message renewals don't count. Concretely, reuse the scan:

- Use `ScanAccum.sessions` (from F3's enriched `SessionStat`, or compute independently if F2
  lands first — see "Dependency") to get per-session `total_tokens` and `start_timestamp`.
- `active` if `total_tokens >= ACTIVE_SESSION_MIN_TOKENS` (config, default e.g. 50_000) — tune
  against real data; the default should exclude trivial sessions but keep real coding ones.
- `active_per_day` = trimmed mean of active-session counts over **active days only** in the
  trailing 14 days (ignore zero-usage days so vacations don't deflate the estimate). Trimmed
  mean (drop top/bottom) resists the occasional 8-session marathon day.

This is the "intelligent" part the user asked for: the budget is anchored to *how much they
typically actually use*, not the raw renewal cadence.

## Architecture

- **`aura-core/src/quota/pacing.rs`** (new) — pure functions + serializable output, sibling to
  `forecast.rs`:
  - `struct SessionBudget { recommended_pct: f64, headroom_pct: f64, status: PacingStatus,
       active_per_day: f64, sessions_left: f64, note: Option<String> }`
  - `enum PacingStatus { Ok, Watch, Over, Insufficient }` (reuse the green/amber/red mapping
    style from `ForecastStatus`).
  - `fn session_budget(weekly: &QuotaWindow, session: &QuotaWindow,
       pattern: &ActivityPattern, now) -> SessionBudget`
  - `struct ActivityPattern { active_per_day: f64, avg_weekly_pct_per_active_session: f64,
       used_active_sessions_today: f64 }`
  - `fn learn_pattern(sessions: &[SessionStat], now, cfg) -> ActivityPattern`
  - Export from `quota/mod.rs` next to the `forecast` re-exports.
- **`aura/src/app.rs`** — in `render_forecast` (line ~1621), append a budget gauge block under
  the existing forecast windows. New helper `render_session_budget(theme, &SessionBudget)`.
  Reuse the bar/badge primitives already used by `render_forecast_window`.
- **`aura-core/src/config.rs`** — `[pacing]` section: `enabled: bool` (default `false`),
  `active_session_min_tokens: u64` (default 50_000), `history_days: u32` (default 14).
- **Live update:** the existing `ProjectsWatcher` already triggers re-reads; the gauge recomputes
  on each snapshot refresh — no new timer/thread, protecting RAM.

## UI sketch (terminal mock — appended below forecast windows)

```
┌ Forecast ──────────────────────────────────────────┐
│ (existing projected-window rows…)                   │
│                                                     │
│ SESSION BUDGET                                      │
│  Spend up to ~38% of this 5h session                │
│  [███████████░░░░░░░░░░░░░░░]  used 22% · ceiling 38%│
│  ~3 active sessions left this week · 42% wk remaining│
└─────────────────────────────────────────────────────┘
```

States: `Insufficient` → "Warming up — need more history to pace." `Over` → red bar + "Over your
session budget; ease off to protect the weekly window."

## Testing

Unit tests in `pacing.rs` (headless, no API/network):
- `learn_pattern`: synthetic `SessionStat` lists → assert `active_per_day` excludes sub-threshold
  sessions, ignores zero days, trimmed mean drops outliers.
- `session_budget`: fixed inputs → known `recommended_pct`; weekly near 100% → `Over`; thin
  history → `Insufficient` (no number).
- Boundary: `sessions_left` floored so we never divide by ~0 and emit absurd budgets.

## Dependency / sequencing note

F2 needs per-session `total_tokens` + `start_timestamp` (the F3 `SessionStat` enrichment). Since
all three features build in parallel, **F2 must not depend on F3's branch**. Resolution: F2
computes its own minimal session list from `scan.rs` (the base `SessionStat` already has
timestamps; F2 adds a tiny local token-per-session pass if F3's field isn't merged yet). At
upstream-merge time, whichever lands second rebases onto the shared `SessionStat` shape. Call
this out in the PR description so the maintainer can sequence the merges.

## Risks / notes

- `ACTIVE_SESSION_MIN_TOKENS` default is a guess; expose it in config and document tuning.
- If the API quota source is unavailable (`QuotaSource::Fallback`/`Unavailable`), pacing shows
  "Needs live quota data" rather than pacing on approximate local token counts — the whole point
  is API-percentage accuracy.
