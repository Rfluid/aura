---
title: Roadmap
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [roadmap, docs]
---

# Roadmap

## v0.1 — Foundation

_Goal: working widget for Claude Code usage with RTK Gains plugin_

- [ ] Project scaffold (Cargo workspace, CI)
- [ ] Config loader (`~/.config/aura/config.toml`)
- [ ] State persistence (`~/.local/share/aura/state.json`)
- [ ] Claude Code usage reader (parse `~/.claude/` data)
- [ ] Modal UI — usage panel (tokens, cost, period selector)
- [ ] Modal UI — profile switcher
- [ ] Plugin runner (subprocess + JSON IPC)
- [ ] RTK Gains plugin (`aura-plugin-rtk` binary)
- [ ] Taskbar widget output (eww / waybar JSON)

## v0.2 — Codex + polish

- [ ] Codex usage reader
- [ ] Animated loading states
- [ ] Light theme
- [ ] Plugin panel expand/collapse
- [ ] macOS system tray support (stretch)

## v0.3 — Plugin ecosystem

- [ ] Plugin authoring guide
- [ ] Example plugin (template repo)
- [ ] Plugin discovery (local scan of `~/.config/aura/plugins/`)

## Backlog / under consideration

- Custom command agents (BYOA — Bring Your Own Agent via a shell command)
- Plugin registry (`aura plugin install <name>`)
- Cost alerts / budget warnings
- Per-project usage breakdown (if agents expose project-scoped data)
- Historical usage charts (daily/weekly trend in the modal)
