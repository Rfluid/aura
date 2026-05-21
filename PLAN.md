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

## Phase 5 — Plugin system ✓

**Crate: `aura-core`**

- [x] Define `PluginPanel` struct: `title: String`, `lines: Vec<PluginLine>`, `error: Option<String>`
- [x] Define `PluginLine`: `label: String`, `value: String`, `highlight: bool`
- [x] Implement `PluginRunner::run(config: &PluginConfig) -> PluginPanel`:
  - Spawn subprocess via `std::process::Command`
  - 500ms timeout via `std::thread` + `mpsc::recv_timeout`
  - Read stdout, parse JSON into `PluginPanel`
  - On non-zero exit, timeout, missing binary, or bad JSON: return `PluginPanel` with `error` set (never panics, never returns Err)
- [x] Unit tests: valid JSON, missing binary, timeout, bad JSON, non-zero exit (5 cases)

**Plugin: `plugins/rtk-gains`**

- [x] Uses `rtk gain -a --format json` (covers summary + daily + monthly in one call)
- [x] Outputs `PluginPanel` JSON with: tokens saved today / this month / all-time, savings rate, commands intercepted
- [x] Graceful fallback if `rtk` is missing or fails (emits error panel)
- [x] Unit tests: `format_thousands` formatting, real `rtk gain` JSON parsing

---

## Phase 6 — GPUI modal (UI) ✓ (needs interactive visual verification)

**Crate: `aura`**

- [x] Add `gpui = "0.2"` (crates.io release); builds on Linux X11/Wayland
- [x] `AuraView` holding: `config`, `state`, `active_profile`, `active_period`, `active_tab`, `snapshot`, `plugin_panels`, `error`
- [x] Borderless 520×640 window, dark background (Zed-ish palette), monospace font
- [x] **Profile picker** — pills in the header row; click switches profile, persists to state, refreshes snapshot
- [x] **Period selector** — three pills (All time / Last 7 days / Last 30 days); active pill in accent color
- [x] **Overview panel** — 2-col grid of stat cards: Favorite model, Total tokens, Sessions, Longest session, Active days, Peak hour, Current/Longest streak
- [x] **Models panel** — daily token bar chart + per-model rows with horizontal % bar
- [x] **Loading / error states** — "Loading…" placeholder; red error message replaces body when snapshot fails
- [x] **Plugin panels** — RTK gains rendered as title + key/value lines with highlight + error variant
- [x] **Tab switching** (Overview ↔ Models) via accent-underlined header
- [x] Click `Aura ⟳` title to manually refresh
- [x] Active profile saved to state on selection change
- [ ] **Deferred** — wire `ProjectsWatcher` to GPUI background executor for auto-refresh. Manual refresh via title click works today.
- [ ] **Manual test required** — UI rendering can't be verified from headless CLI. Launch with `cargo run -p aura` to confirm layout

---

## Phase 7 — System tray integration ✓ (basic; tooltip+click integration deferred)

**Crate: `aura`**

- [x] Add `tray-icon = "0.24"` with `default-features = false, features = ["gtk"]` (drops `libxdo` system dep)
- [x] Programmatic 32×32 RGBA placeholder icon (purple/violet square); real SVG asset is a follow-up
- [x] Tray installed at startup with tooltip "Aura — Agent Usage Reporter"; handle held by main fn so it persists for app lifetime
- [x] Best-effort install: warns and continues if no tray host is available
- [ ] **Deferred** — dynamic tooltip ("Aura · profile · tokens today") and click-to-toggle-window. Requires bridging tray-icon's event channel into the GPUI main loop; left as a future iteration since main flow is "window opens on launch"

---

## Phase 8 — Codex adapter (stub → real) ✓ (stub only)

**Crate: `aura-core`**

- [x] Stub `CodexReader` returning `UsageSnapshot::default()`
- [x] `reader::make_reader(agent)` factory dispatches on `AgentKind` (ClaudeCode → real, Codex → stub)
- [ ] **Future** — research Codex data source (local files vs. OpenAI usage endpoint) and replace stub

---

## Phase 9 — Packaging & install ✓

- [x] `justfile` with targets: `build`, `run`, `test`, `lint`, `install`, `install-plugin-rtk`, `uninstall`, `start/stop/status/logs`
- [x] Systemd user unit `packaging/aura.service` (`graphical-session.target`, restart-on-failure, inherits DISPLAY/WAYLAND_DISPLAY)
- [x] `install.sh` — checks prereqs, builds release, installs binaries to `~/.local/bin/`, drops the systemd unit
- [x] Updated `README.md` Installation section with system deps, one-shot install, manual install, and common commands
- [ ] **Manual test required** — verify on a clean session: `./install.sh`, `systemctl --user enable --now aura`, tray icon appears, app launches

---

## Deferred / roadmap

- Light theme
- Historical usage charts in the modal (beyond ASCII)
- Plugin authoring guide + example repo
- Plugin registry (`aura plugin install <name>`)
- macOS support (kqueue watcher, macOS tray via `tray-icon`)
- Custom command agents (BYOA)
- Cost alerts / budget warnings
