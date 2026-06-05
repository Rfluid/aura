# F2 — Budget pacing · Build notes

## Crates / versions added
None. The feature uses only crates already in the workspace:
- `chrono` (date/duration math) — same usage pattern as `quota/forecast.rs`.
- `serde` / `serde_json` (serializable output + JSONL parsing) — already deps of `aura-core`.

No `Cargo.toml` changes were required.

## What was added / changed

### New
- `crates/aura-core/src/quota/pacing.rs` — pure pacing logic + the F2-local JSONL
  token scan. Public surface: `SessionBudget`, `PacingStatus`, `ActivityPattern`,
  `SessionTokens`, `session_budget()`, `learn_pattern()`, `collect_session_tokens()`.
  Re-exported from `quota/mod.rs`.

### Modified (additive)
- `crates/aura-core/src/quota/mod.rs` — module decl + re-exports; new
  `QuotaSnapshot.pacing_pattern: Option<ActivityPattern>` field
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`, so the wire
  format is unchanged when absent).
- `crates/aura-core/src/quota/{api,codex,gemini}.rs` — added `pacing_pattern: None`
  to the explicit `QuotaSnapshot { … }` constructors (5 sites). Mechanical.
- `crates/aura-core/src/config.rs` — new `PacingConfig { enabled, active_session_min_tokens,
  history_days }` + `AppConfig.pacing` field; updated `default_config()` and the
  test literals.
- `crates/aura-core/src/config_schema.rs` — 3 `FieldDescriptor`s for `pacing.*`,
  get/set/toml_rhs arms, `push_scalar_table(…, "pacing")`, new `parse_u32`/`parse_u64`,
  and test coverage (`["display","update","pacing"]`, round-trip assertion + literal).
- `crates/aura-core/src/lexicon.rs` — new `pacing_*` strings + `pacing_spend_up_to_fmt`
  in both POLITE and GOBLIN, plus the test `pairs()` entries (goblin length budget honoured).
- `crates/aura/src/app.rs` — refresh now learns the pattern (Claude Code + Api source +
  `pacing.enabled`) and attaches it to the snapshot; `render_forecast` gained
  `quota` + `pacing_enabled` params and appends `render_session_budget(…)`.
- `crates/aura/src/cli/config.rs` — `aura config describe` lists the `[pacing]` section.

## F3 / SessionStat dependency status — IMPORTANT
F2 does **not** depend on the F3 branch. The base `SessionStat` on this branch has
`{ duration_secs, message_count, start_timestamp }` — **no per-session token total**.
Rather than enrich `SessionStat` (which would collide with F3), F2:
- defines its own minimal `pacing::SessionTokens { total_tokens, start_timestamp }`, and
- does its own tiny token-per-session JSONL pass in `pacing::collect_session_tokens()`
  (reusing `reader::scan::list_session_files`, skipping `isSidechain`).

`learn_pattern()` therefore takes `&[SessionTokens]`, **not** `&[SessionStat]` (a
deliberate deviation from the spec's literal signature — the base `SessionStat` has no
token field, so the spec's own "compute your own minimal per-session token tally locally"
fallback applies). At upstream merge time, whichever of F2/F3 lands second can map the
shared `SessionStat` into `SessionTokens` at the call boundary in `app.rs`; the pacing
math is untouched.

## Unverifiable without compiling
- This worktree was **not** built (hard rule). The code mirrors the conventions of
  `forecast.rs` / existing `app.rs` render helpers, but the following need a real build:
  - gpui builder-method availability on the new gauge (`.h(px(...))`, `.w(relative(...))`,
    `.py_0p5()` — all copied from `render_forecast_window`, so expected fine).
  - the `#[allow(clippy::too_many_arguments)]` on `render_forecast` (8 args now).
- No network/API calls in tests (all `pacing.rs` tests are headless, fixed inputs).

## Build checklist for later (parent runs)
```
cargo build -p aura-core
cargo test  -p aura-core        # includes pacing.rs unit tests + config_schema round-trips
cargo build -p aura
# optional: cargo clippy --workspace
```

## Tuning note
`pacing.active_session_min_tokens` default (50_000) is a guess — exposed in config and
documented in `config_schema` so users can tune it against real usage.
