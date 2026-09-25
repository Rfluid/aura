---
title: state.json has several writers — update it read-modify-write
status: current
version: 0.1.0
last_updated: 2026-09-24
last_verified: 2026-09-24
source_refs:
  - crates/aura-core/src/state.rs
  - crates/aura-core/src/sponsor.rs
  - crates/aura/src/main.rs
  - crates/aura/src/app.rs
owner: "@rfluid"
tags: [memory, fact, state]
source_task: one-time sponsor nudge (docs/configuration.md#sponsor)
---

# state.json has several writers

- `AppState` (`~/.local/share/aura/state.json`) is written by the tray loop
  (`modal_height`, while the modal is open), the tray startup (`first_run`),
  the modal view (`active_profile`, `sponsor_nudge_done`), and the `aura state`
  CLI. The view's copy is loaded once per open, so saving it wholesale can undo
  a write made since. New writers should `AppState::load()`, set their one
  field, and `save()` — see `main.rs` (modal height, first run) and
  `AuraView::finish_sponsor_nudge`.
- App-recorded facts (timestamps, "already answered" flags) belong in
  state.json; `config.toml` holds only user settings. The update chip's
  `update.dismissed_version` predates this split.
- Every new `AppState` field needs `#[serde(default)]`: a state file from an
  older build must still load (`a_state_file_without_a_modal_height_still_loads`).
