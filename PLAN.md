# Aura — Implementation Plan

> Phased checklist. Work top-to-bottom within each phase; check items off as they land.
> Stack reference: `.agent/context/stack.md` · Architecture: `docs/architecture.md`

---

## Phase 1 — Cargo workspace & CI

- [ ] Create `Cargo.toml` workspace at repo root with members: `crates/aura`, `crates/aura-core`, `plugins/rtk-gains`
- [ ] `crates/aura-core` — library crate (data layer, plugin runner, config, state); no UI deps
- [ ] `crates/aura` — binary crate (GPUI app, tray icon, modal); depends on `aura-core`
- [ ] `plugins/rtk-gains` — standalone binary crate (the RTK Gains plugin)
- [ ] Add `.rustfmt.toml` (defaults) and `clippy.toml`
- [ ] Add `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`
- [ ] Add `.gitignore` (standard Rust + `target/`)
- [ ] Smoke-test: `cargo build --workspace` passes

---

## Phase 2 — Configuration & state

**Crate: `aura-core`**

- [ ] Define `AgentConfig` and `AgentKind` (enum: `ClaudeCode`, `Codex`) in `config.rs`
- [ ] Define `PluginConfig` (name + command path) in `config.rs`
- [ ] Define `AppConfig` (agents list, plugins list, display defaults) — deserialize from TOML via `serde` + `toml`
- [ ] Implement `AppConfig::load(path)` — reads `~/.config/aura/config.toml`; creates default file on first run
- [ ] Write default config template (two Claude Code profiles + RTK plugin entry)
- [ ] Define `AppState` (active profile name) in `state.rs`
- [ ] Implement `AppState::load()` / `AppState::save()` — reads/writes `~/.local/share/aura/state.json` via `serde_json`
- [ ] Unit tests: round-trip serialize/deserialize for both config and state
- [ ] Smoke-test: binary loads config and prints active profile

---

## Phase 3 — JSONL data engine (Claude Code adapter)

**Crate: `aura-core`** — mirrors the `ML8` / `AT5` / `Wp6` logic from `claude /usage`

- [ ] Define `UsageSnapshot` struct: `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `sessions`, `active_days`, `longest_session_secs`, `current_streak`, `longest_streak`, `peak_hour`, `favorite_model`, `per_model: Vec<ModelUsage>`, `daily_tokens: Vec<DailyModelTokens>` — all fields needed to render Overview + Models panels
- [ ] Define `Period` enum: `AllTime`, `Last7Days`, `Last30Days`
- [ ] Implement `list_session_files(config_path)` — `readdir` on `projects/` subdirectories, collect all `*.jsonl` paths including `subagents/` subdirs
- [ ] Implement `scan_jsonl_files(files, from_date, to_date)` → raw aggregated data:
  - Skip file by `mtime < from_date` (cheap OS check before opening)
  - Parse each `assistant` entry: `timestamp`, `message.model`, `message.usage.*`
  - Skip synthetic models (model name `<synthetic>`)
  - Accumulate per-model: `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`
  - `dailyModelTokens[date][model]` += `input + output` only (matches `/usage`)
  - Track session stats: start/end timestamps per file → duration, message count
  - Track `hourCounts` from session start hours
- [ ] Implement streak computation from daily activity dates (matches `WM4` logic)
- [ ] Implement `ClaudeCodeReader::snapshot(config, period)`:
  - `Last7Days` / `Last30Days`: call `scan_jsonl_files` with date range; return `UsageSnapshot` from results
  - `AllTime`: load `stats-cache.json` as baseline, find JSONL files newer than `lastComputedDate`, merge delta, save updated cache, append today's JSONL on top
- [ ] Define `AgentReader` trait with `fn snapshot(&self, period: Period) -> Result<UsageSnapshot>`
- [ ] Implement `AgentReader` for `ClaudeCodeReader`
- [ ] Unit tests:
  - Fixture JSONL files covering multi-model, multi-day, cache tokens, synthetic model filtering
  - Assert `snapshot(Last7Days)` totals match hand-computed values
  - Assert `AllTime` merges cache + delta correctly
  - Assert streak computation for gaps, current streak reaching today
- [ ] Integration smoke-test: run against real `~/.claude/projects/` and compare output to `claude /usage` visually

---

## Phase 4 — File watcher (live updates)

**Crate: `aura-core`**

- [ ] Add `notify = "9"` dependency
- [ ] Implement `ProjectsWatcher` wrapping a `notify::RecommendedWatcher` on `{config_path}/projects/`
- [ ] Expose an async channel / callback: fires with `(file_path, new_tail_offset)` on `MODIFY` / `CREATE` events for `*.jsonl` files
- [ ] Implement incremental re-scan: given a path and a previous byte offset, read only new lines appended since offset; merge into the current `UsageSnapshot` for the active period
- [ ] Debounce: coalesce events within 500ms (rapid appends from a streaming response)
- [ ] Unit test: write lines to a temp JSONL file, assert watcher fires and snapshot updates within 1s

---

## Phase 5 — Plugin system

**Crate: `aura-core`**

- [ ] Define `PluginPanel` struct: `title: String`, `lines: Vec<PluginLine>`, `error: Option<String>`
- [ ] Define `PluginLine`: `label: String`, `value: String`, `highlight: bool`
- [ ] Implement `PluginRunner::run(config: &PluginConfig) -> Result<PluginPanel>`:
  - Spawn subprocess via `std::process::Command`
  - 500ms timeout (use `std::thread` + channel, or `tokio::time::timeout`)
  - Read stdout, parse JSON into `PluginPanel`
  - On non-zero exit or timeout: return `PluginPanel` with `error` set
- [ ] Unit tests: mock plugin binary that outputs valid JSON; mock that times out; mock with bad JSON

**Plugin: `plugins/rtk-gains`**

- [ ] Research RTK gains data location: check `~/.local/share/rtk/` or `rtk gain --format json` output
- [ ] Implement binary: read RTK gains data, print `PluginPanel` JSON to stdout, exit 0
- [ ] Output fields: `Tokens saved today`, `Tokens saved this month`, `Savings rate (%)`, `Commands intercepted`
- [ ] Handle missing RTK data gracefully (output `error` field, not panic)
- [ ] Integration test: run binary directly, assert valid JSON output

---

## Phase 6 — GPUI modal (UI)

**Crate: `aura`**

- [ ] Add `gpui` dependency; confirm it builds on Linux (X11 / Wayland)
- [ ] Create `AppModel` holding: `config`, `active_profile`, `snapshot: Option<UsageSnapshot>`, `plugin_panels: Vec<PluginPanel>`, `active_period: Period`, `active_tab: Tab`, `is_loading: bool`
- [ ] Scaffold a minimal GPUI window that opens, shows "Aura", closes on Escape — smoke-test
- [ ] Style: borderless window, dark background, monospace numbers, system UI for labels
- [ ] **Profile picker** (header row): active profile name + dropdown on click; selecting a profile triggers re-fetch
- [ ] **Period selector** row: three pills (All time / Last 7 days / Last 30 days); active pill highlighted
- [ ] **Overview panel**:
  - Row: Favorite model | Total tokens
  - Row: Sessions | Longest session
  - Row: Active days / total | Peak hour
  - Row: Current streak | Longest streak
- [ ] **Models panel**:
  - Tokens per Day chart (ASCII; reuse GPUI text rendering with monospace font)
  - Per-model cards (name, %, In · Out)
- [ ] **Loading state**: spinner on stats area while fetching; never blank labels
- [ ] **Plugin panels**: one panel per plugin below the usage tabs; title + lines; error state
- [ ] **Tab switching** (Overview ↔ Models): click or keyboard
- [ ] Wire `ProjectsWatcher` events to trigger incremental snapshot update + re-render
- [ ] Close on click-outside and Escape; save active profile to state on close

---

## Phase 7 — System tray integration

**Crate: `aura`**

- [ ] Add `tray-icon` dependency
- [ ] Create tray icon (SVG/PNG asset at `assets/icon.png`; 16×16, 32×32, 64×64)
- [ ] Set tray tooltip: `Aura · <active profile> · <total tokens today>`
- [ ] On tray click: open GPUI modal anchored near click position
- [ ] Update tray tooltip after every `ProjectsWatcher` event (total tokens for today, without opening modal)
- [ ] On modal close: remove focus from modal, return to tray-idle state
- [ ] Test: tray appears in system tray, click opens modal, tooltip updates live

---

## Phase 8 — Codex adapter (stub → real)

**Crate: `aura-core`**

- [ ] Stub: implement `AgentReader` for `CodexReader` returning `UsageSnapshot::empty()` with a "not yet supported" note
- [ ] Research Codex data location (local files vs. OpenAI API usage endpoint)
- [ ] Implement real `CodexReader` once data source is confirmed
- [ ] Register `CodexReader` in the adapter factory; wire into config `kind = "codex"`

---

## Phase 9 — Packaging & install

- [ ] `Makefile` (or `just`) targets: `build`, `install` (copies binary to `~/.local/bin/`), `install-plugin-rtk`
- [ ] Systemd user service unit: `aura.service` (autostart on login, `ExecStart=aura`)
- [ ] `install.sh`: copies binaries, installs systemd unit, enables it
- [ ] Update `README.md` Installation section with actual steps
- [ ] Verify install on clean user session: tray appears, modal opens, stats match `claude /usage`

---

## Deferred / roadmap

- Light theme
- Historical usage charts in the modal (beyond ASCII)
- Plugin authoring guide + example repo
- Plugin registry (`aura plugin install <name>`)
- macOS support (kqueue watcher, macOS tray via `tray-icon`)
- Custom command agents (BYOA)
- Cost alerts / budget warnings
