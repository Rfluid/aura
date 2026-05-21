# Aura — Implementation Plan

> Phased checklist. Work top-to-bottom within each phase; check items off as they land.
> Stack reference: `.agent/context/stack.md` · Architecture: `docs/architecture.md`

---

## Phase 1 — Cargo workspace & CI ✓

- [x] Create `Cargo.toml` workspace at repo root with members: `crates/aura`, `crates/aura-core`, `plugins/rtk-gains`
- [x] `crates/aura-core` — library crate (data layer, plugin runner, config, state); no UI deps
- [x] `crates/aura` — binary crate (GPUI app, tray icon, modal); depends on `aura-core`
- [x] `plugins/rtk-gains` — standalone binary crate (the RTK Gains plugin)
- [x] Add `.rustfmt.toml` (defaults) and `clippy.toml`
- [x] Add `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`
- [x] Add `.gitignore` (standard Rust + `target/`)
- [x] Smoke-test: `cargo build --workspace` passes

---

## Phase 2 — Configuration & state ✓

**Crate: `aura-core`**

- [x] Define `AgentConfig` and `AgentKind` (enum: `ClaudeCode`, `Codex`) in `config.rs`
- [x] Define `PluginConfig` (name + command path) in `config.rs`
- [x] Define `AppConfig` (agents list, plugins list, display defaults) — deserialize from TOML via `serde` + `toml`
- [x] Implement `AppConfig::load(path)` — reads `~/.config/aura/config.toml`; creates default file on first run
- [x] Write default config template (two Claude Code profiles + RTK plugin entry)
- [x] Define `AppState` (active profile name) in `state.rs`
- [x] Implement `AppState::load()` / `AppState::save()` — reads/writes `~/.local/share/aura/state.json` via `serde_json`
- [x] Unit tests: round-trip serialize/deserialize for both config and state
- [x] Smoke-test: binary loads config and prints active profile

---

## Phase 3 — JSONL data engine (Claude Code adapter) ✓

**Crate: `aura-core`** — mirrors the `ML8` / `AT5` / `Wp6` logic from `claude /usage`

- [x] Define `UsageSnapshot` struct: `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `sessions`, `active_days`, `longest_session_secs`, `current_streak`, `longest_streak`, `peak_hour`, `favorite_model`, `per_model: Vec<ModelUsage>`, `daily_tokens: Vec<DailyModelTokens>` — all fields needed to render Overview + Models panels
- [x] Define `Period` enum: `AllTime`, `Last7Days`, `Last30Days`
- [x] Implement `list_session_files(config_path)` — `readdir` on `projects/` subdirectories, collect all `*.jsonl` paths including `subagents/` subdirs
- [x] Implement `scan_jsonl_files(files, from_date, to_date)` → raw aggregated data:
  - Skip file by `mtime < from_date` (cheap OS check before opening)
  - Parse each `assistant` entry: `timestamp`, `message.model`, `message.usage.*`
  - Skip synthetic models (model name `<synthetic>`)
  - Accumulate per-model: `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`
  - `dailyModelTokens[date][model]` += `input + output` only (matches `/usage`)
  - Track session stats: start/end timestamps per file → duration, message count
  - Track `hourCounts` from session start hours
- [x] Implement streak computation from daily activity dates (matches `WM4` logic)
- [x] Implement `ClaudeCodeReader::snapshot(config, period)`:
  - `Last7Days` / `Last30Days`: call `scan_jsonl_files` with date range; return `UsageSnapshot` from results
  - `AllTime`: load `stats-cache.json` as baseline, find JSONL files newer than `lastComputedDate`, merge delta, append today's JSONL on top
- [x] Define `AgentReader` trait with `fn snapshot(&self, period: Period) -> Result<UsageSnapshot>`
- [x] Implement `AgentReader` for `ClaudeCodeReader`
- [x] Unit tests:
  - Fixture JSONL files covering multi-model, multi-day, cache tokens, synthetic model filtering
  - Assert `snapshot(Last7Days)` totals match hand-computed values
  - Assert `AllTime` merges cache + delta correctly
  - Assert streak computation for gaps, current streak reaching today
- [x] Integration smoke-test: run against real `~/.claude/projects/` — outputs 119,845 tokens / 5 sessions / 2 active days / favorite model claude-opus-4-7

---

## Phase 4 — File watcher (live updates) ✓

**Crate: `aura-core`**

- [x] Add `notify-debouncer-mini = "0.7"` dependency (pairs with notify 8.2; stable; built-in debouncing)
- [x] Implement `ProjectsWatcher` wrapping `notify::RecommendedWatcher` on `{config_path}/projects/`
- [x] Expose channel via `try_recv()` / `recv_timeout()` returning `Vec<PathBuf>` of changed `*.jsonl` files (non-JSONL events filtered out)
- [x] Implement `read_jsonl_since(path, offset)`: returns parsed new entries + new byte offset; respects partial trailing lines mid-write
- [ ] **Deferred to Phase 6** — `UsageSnapshot` merge logic lives with the UI consumer; for now the UI calls `snapshot()` after watcher events (mtime fast-skip already keeps this cheap)
- [x] Debounce: 500ms quiet-window coalescing handled by `notify-debouncer-mini`
- [x] Unit tests: file create fires event, non-JSONL ignored, rapid writes coalesced, `read_jsonl_since` byte-offset correctness incl. partial trailing line

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
