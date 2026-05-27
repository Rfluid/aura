---
title: Forecast tab
status: design
version: 0.1.0
last_updated: 2026-05-27
last_verified: 2026-05-27
source_refs:
    - crates/aura-core/src/quota/mod.rs
    - crates/aura-core/src/quota/api.rs
    - crates/aura-core/src/reader/mod.rs
    - crates/aura/src/app.rs
owner: "@rfluid"
tags: [forecast, quota, roadmap, design]
---

# Forecast tab

## Problem

The Quota tab tells the user **what they have used so far** in each
rate-limit window (5h session, 7d weekly, 7d Opus, 7d Sonnet, overage).
It does not tell them **whether the current burn rate will exhaust the
window before it resets**. A user halfway through a 5h session at 60%
used has no way to know, without manual math, that they are on track to
overshoot.

The Quota tab answers _"where am I?"_ — the Forecast tab answers
_"where am I headed?"_.

## Scope

A new modal tab named **Forecast**, sitting alongside Quota / Summary /
Models / Plugins. For every rate-limit window the Quota tab renders,
the Forecast tab renders a matching projection:

- **Projected end-of-window %** — what `used_percentage` is expected to
  be at `resets_at` if the current pace continues.
- **Projected absolute tokens** (when the source provides counts).
- **Overshoot indicator** — green / amber / red badge keyed off the
  projected percentage (`< 90%`, `90–100%`, `> 100%`).
- **Time until overshoot** (red windows only) — when the linear
  extrapolation crosses 100%.

Rollout is per agent:

1. **Claude Code** (first) — has the richest data: API-reported
   `utilization` + JSONL-derived per-timestamp token counts.
2. **Codex** — second.
3. **Gemini** — third.

Out of scope for v1:

- Forecast for plugin-supplied panels.
- Cost (dollar) forecasts — v1 projects token usage only. The same
  engine can later multiply by per-model pricing once that data is
  modeled; tracked as a follow-up below.
- Anomaly / spike detection — only smoothed rate extrapolation.
- Proactive alerts / notifications when a projection crosses
  100% — Forecast v1 is a passive tab. A tray-icon badge / desktop
  notification is a natural follow-up once the engine is proven.

## Non-goals

- Predicting future _content_ of usage (which model, which project).
- Replacing the Quota tab — Forecast is additive.
- Sub-minute precision — windows are hours-to-days; the user only needs
  to know "you will / will not overshoot".

## Design

### Forecasting model

For each `QuotaWindow`, we know:

- `used_percentage` (or `used_tokens`) at "now".
- `resets_at` — when the window rolls over.
- The **window length** (5h, 7d, …) — currently _not_ in `QuotaWindow`;
  see [Required plumbing](#required-plumbing).

From window length and `resets_at` we derive `started_at = resets_at -
length`. Then:

```text
elapsed_fraction  = (now - started_at) / length          # 0.0 … 1.0
projected_at_end  = used_now / elapsed_fraction          # linear extrapolation
```

Edge cases:

- `elapsed_fraction < 0.05` (window barely started) — display
  "insufficient signal" instead of a wild projection.
- `elapsed_fraction > 0.95` — projection ≈ current value; flag as
  "near reset" rather than forecasting.
- `used_now == 0` — show "no activity yet".

v1 uses **plain linear extrapolation** (uniform pace). Two follow-ups
are scoped but explicitly deferred to keep v1 shippable:

- **Recency-weighted rate** — sum tokens in the last _N_ minutes from
  `UsageSnapshot.daily_tokens` / session JSONL and divide by _N_,
  instead of treating the whole elapsed window as uniform. Better
  for bursty workloads; needs a tunable window. The relevant data is
  already in `UsageSnapshot` (see
  `crates/aura-core/src/reader/mod.rs:93`).
- **Confidence band** — derive a min/max projection from rate variance
  across recent sample buckets; render as a faded range on the bar.

### Data shape

New types in `crates/aura-core/src/quota/forecast.rs`:

```rust
pub struct ForecastWindow {
    pub label: String,                   // mirrors QuotaWindow.label
    pub used_percentage_now: Option<f64>,
    pub projected_percentage: Option<f64>,
    pub projected_tokens: Option<u64>,
    pub overshoot_at: Option<DateTime<Utc>>, // when projection crosses 100%
    pub status: ForecastStatus,          // Ok | Watch | Overshoot | Insufficient
    pub resets_at: Option<DateTime<Utc>>,
}

pub struct ForecastSnapshot {
    pub windows: Vec<ForecastWindow>,
    pub computed_at: DateTime<Utc>,
    pub note: Option<String>,            // e.g. "session has 8 min elapsed"
}

pub enum ForecastStatus { Ok, Watch, Overshoot, Insufficient }
```

A pure function takes a `QuotaSnapshot` plus optional `UsageSnapshot`
(for recency weighting in a later version) and returns a
`ForecastSnapshot`:

```rust
pub fn forecast(
    quota: &QuotaSnapshot,
    now: DateTime<Utc>,
) -> ForecastSnapshot;
```

Agent-agnostic — both Codex and Gemini already feed a `QuotaSnapshot`
from their own quota providers, so the same function serves all three.

### Required plumbing

`QuotaWindow` (`crates/aura-core/src/quota/mod.rs:22`) does **not**
currently expose a window length or start time. The
`/api/oauth/usage` body only carries `utilization` and `resets_at` /
`resetsAt` (see `crates/aura-core/src/quota/api.rs:78`). Three options;
recommendation is **(a)**:

(a) **Derive length from label / window kind**. The five labels are a
closed set ("Current session" → 5h, "Current week …" → 7d, "Overage"
→ skip). Encode the mapping in `forecast.rs` and keep `QuotaWindow`
unchanged. Pro: zero plumbing. Con: brittle if Anthropic adds a new
window label.

(b) Add `length: Option<chrono::Duration>` to `QuotaWindow` and have
each backend (`QuotaApi`, `CodexQuota`, `GeminiQuota`) populate it.
Cleaner, but touches three providers and the wire format.

(c) Add a `WindowKind` enum (`Session5h`, `Week7d`, `Week7dOpus`,
`Week7dSonnet`, `Overage`) alongside `label`. Best long-term but
the largest diff.

Start with **(a)**, migrate to **(c)** when a second consumer needs
window length (e.g. the budget-warnings roadmap item).

### UI

Tab plumbing (all in `crates/aura/src/app.rs`):

1. Add `Forecast` variant to `AgentSection` enum at line 38.
2. Add label (`"Forecast"`) and id (`"forecast"`) at lines 44–59.
3. Add to the tab array in `render_tab_row()` at line 1028.
4. Add render dispatch in `render_body()` at line 1109 →
   `render_forecast(cx)`.
5. Implement `render_forecast()` mirroring `render_quota()`
   (line 1223) and `render_quota_window()` (line 1333), but for
   `ForecastWindow` instead of `QuotaWindow`.

Visual:

- One row per window, same vertical rhythm as the Quota tab.
- The progress bar shows **two segments**: solid (used now) +
  hashed/translucent (projected delta to `projected_percentage`).
  Capped at 100%; anything beyond renders as an overflow marker.
- Right-aligned: `projected_percentage` and the status badge.
- Subtext: `"Projected at reset"` or `"Will hit 100% at 14:32"` for
  overshoots.
- `Insufficient` rows render a muted placeholder ("warming up — check
  back in a few minutes") so the tab is never empty on a fresh session.

### Refresh integration

In `crates/aura/src/app.rs`:

1. Add `forecast: Option<ForecastSnapshot>` to `RefreshResult` (line
   122).
2. In `do_refresh()` (line 295), after the quota snapshot loads, call
   `forecast::forecast(&quota, Utc::now())`.
3. In `apply_refresh_result()` (line 224), assign to `self.forecast`.
4. Same refresh button / hot-reload behavior — no new code paths.

No background timer in v1: the forecast is recomputed only on
modal-open / refresh-click / profile-switch / period-change, matching
how Quota itself refreshes today. If users want a "live" forecast we
can add a 60s tick later.

## Rollout

### Phase 1 — Claude Code (target: same PR)

- Add `forecast` module under `crates/aura-core/src/quota/`.
- Wire up `Forecast` tab and `render_forecast()`.
- Hook into `RefreshResult` / `do_refresh()`.
- Tests in `crates/aura-core/src/quota/forecast.rs`:
    - linear extrapolation at 25%, 50%, 75% elapsed
    - `Insufficient` when elapsed < 5%
    - `Overshoot` + `overshoot_at` correctness
    - empty `QuotaSnapshot` → empty `ForecastSnapshot` (no panic)

### Phase 2 — Codex

- Verify Codex's `QuotaSnapshot` populates `resets_at` and percentages
  in the same shape (it should — same `QuotaWindow` type).
- Confirm the label-→-length map in `forecast.rs` covers Codex's
  window labels; extend if Codex uses different strings.
- No UI changes — agent-agnostic dispatch already works.

### Phase 3 — Gemini

- Same as Phase 2 for Gemini.
- After this lands, the **Forecast tab** roadmap item closes.

## Open questions

- **Do Codex / Gemini even have meaningful rate-limit windows today?**
  If their `QuotaSnapshot` is currently `Unavailable`, the Forecast tab
  for those agents will just show "no quota source". That is fine —
  it tracks the Quota tab's behavior.
- **Should Forecast pre-empt the Quota tab when the user is about to
  overshoot?** Out of scope for v1; a tray-icon badge / desktop
  notification when a projection crosses 100% is the natural
  follow-up once the engine is proven.
- **Do we want a "history" sparkline of past forecasts vs. actuals?**
  Useful for tuning the model, but adds storage. Defer.

## Tracking

Roadmap entry: [`README.md` → Roadmap → Next up → Forecast tab](../README.md#roadmap).
