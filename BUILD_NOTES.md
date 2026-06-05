# F3 — Insights tab — Build Notes

## Crates / versions added to Cargo.toml

**None.** The feature uses only crates already present in the workspace:

- `serde` (derive `Serialize`/`Deserialize`) — for the new `InsightsSnapshot`,
  `ProjectStat`, `SessionInsight`, `ModeDistribution`, `ModelTier`, and
  `InsightsConfig` types.
- `std` only for the ultracode substring scan (`str::contains`) — the spec's
  "memchr-style" phrasing is descriptive; no `memchr` dependency was added to
  keep the dep tree unchanged and avoid a non-additive Cargo.toml edit.
- `tempfile` (already a dev-dependency) for the new unit tests.

No `Cargo.toml` files were modified.

## Things I could NOT verify without compiling

The hard rule forbids building, so the following are reasoned-not-compiled:

1. **GPUI style methods** used in `render_insights` (`flex_wrap`, `overflow_hidden`,
   `h_full`, `flex_1`, `gpui::relative`, `Theme::blend`, `Theme::on_accent_text`).
   All are already used elsewhere in `app.rs`/the codebase (verified by grep), so
   they exist — but the exact return-type chaining of the new
   `impl IntoElement` helpers was not type-checked. The early-return + final-value
   branches in `render_insights_projects` / `_sessions` / `render_mode_distribution`
   all return `gpui::Div` (since `insights_card()` returns `Div` and `Div::child`
   returns `Self`), so they should unify. If the compiler complains, wrap the
   branches in `.into_any_element()`.
2. **TOML round-trip ordering** for the new `[insights]` table. `AppConfig` has no
   top-level scalar fields, so all members serialize as tables/arrays; `[insights]`
   mirrors the already-round-tripping `[update]` table shape, so it should be fine.
3. **clippy** lints (e.g. `mode_badge`'s `accent` param is only read in one match
   arm — that is allowed, not dead code).

## Checklist to run when build is allowed

Run in order; stop at the first failure.

```sh
# 1. Core logic + all the new unit tests (insights, scan, config, lexicon, format-less).
cargo build -p aura-core
cargo test  -p aura-core

# Targeted test modules of interest:
cargo test -p aura-core reader::scan::tests
cargo test -p aura-core reader::insights::tests
cargo test -p aura-core config::tests
cargo test -p aura-core config_schema::tests
cargo test -p aura-core lexicon::tests

# 2. The GPUI binary (UI: render_insights + tab wiring + compact_tokens).
cargo build -p aura
cargo test  -p aura format::tests   # compact_tokens test lives here

# 3. Full workspace lint (optional but recommended).
cargo clippy --workspace --all-targets
```

### New tests added (written, not run)

- `reader/scan.rs`: `ultracode_detection_*` (Workflow tool_use → true, literal
  "ultracode" → true, plain → false), `scan_marks_ultracode_session`,
  `scan_picks_dominant_model_and_session_total`,
  `scan_accumulates_tokens_per_project`,
  `scan_folds_subagent_tokens_into_parent_project`.
- `reader/insights.rs`: `top_projects_*`, `top_sessions_sorted_desc_and_skips_empty`,
  `model_tier_from_model_matches_substrings`, `mode_distribution_tiers_and_counts`
  (percentages sum to 100 ± rounding), `mode_distribution_empty_is_safe`,
  `humanize_project_*`, `build_insights_assembles_all_three`.
- `config.rs`: `insights_config_defaults_when_block_missing`,
  `insights_config_round_trips_populated_block`.
- `config_schema.rs`: existing `registry_covers_every_field` /
  `get_and_set_round_trip_every_descriptor` / `render_commented_round_trips_*`
  now cover the `[insights]` section.
- `aura/src/format.rs`: `compact_tokens_scales`.

## Known limitations (by design / documented in code)

- **Ultracode detection is heuristic** (`ULTRACODE_MARKERS` in `scan.rs`) and
  Claude-Code-version-dependent. A UI footnote ("ⓘ mode is inferred from session
  content") and rustdoc make this explicit. If the marker disappears the chip
  simply stops showing — no crash, tier badge still works.
- **AllTime insights cover only the delta scan.** The `StatsCache` baseline used
  for the AllTime period carries no per-project / per-session breakdown, so
  Insights for AllTime reflect sessions after `lastComputedDate`. Documented in a
  comment in `build_snapshot`. 7d / 30d are exact (full scan).
- **`humanize_project` returns the trimmed slug, not just the last path segment.**
  Claude Code slugifies a cwd by replacing every `/` with `-`, which is lossy —
  the original segment boundaries are unrecoverable. The UI truncates long names
  via `flex_1 + overflow_hidden`. (Minor deviation from the spec sketch, which
  shows just the project name.)
