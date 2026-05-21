---
title: Glossary
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [context]
---

# Glossary

Domain terms specific to Aura. Add as you encounter unfamiliar terminology.

## Terms

**Agent** — an AI coding assistant whose usage Aura monitors. Currently: Claude Code, Codex. Future: custom command agents.

**Agent profile** — a named configuration entry pointing at a specific agent kind and config path. One user can have multiple profiles for the same agent kind (e.g., personal vs. enterprise).

**Plugin** — a runtime extension that adds a custom metrics panel to the Aura modal. Loaded from a shared library (`.so`/`.dylib`) or spawned as a subprocess. Does not touch core usage data.

**RTK** — Rust Token Killer. A CLI token optimizer that rewrites shell commands to reduce Claude Code token consumption. The RTK Gains plugin surfaces how many tokens RTK saved.

**Modal** — the small popup UI that appears near the taskbar widget when the user clicks the Aura widget. Shows the active agent's usage and plugin panels.

**Widget** — the taskbar entry (e.g., eww, waybar, polybar module, or system tray icon) that the user clicks to open the modal.

**Usage snapshot** — a point-in-time reading of an agent's token and cost counters, pulled from that agent's local data store (e.g., `~/.claude/` for Claude Code).

**Active profile** — the agent profile currently shown in the modal. Persisted to `~/.local/share/aura/state.json` so the last selection survives restarts.
