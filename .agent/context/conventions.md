---
title: Conventions
status: current
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [context]
---

# Conventions

Cross-cutting conventions for agents working in this repo.

## Documentation

Every doc has YAML frontmatter (title, status, version, last_updated, last_verified, source_refs, owner, tags). When you touch code referenced in a doc's `source_refs`, refresh `last_verified`.

## Memory

After every non-trivial task, distill learnings into `.agent/memory/`. One file per entry under `facts/`, `lessons/`, or `patterns/`. Backlink to the originating task via `source_task`.

## Code

- Language: Rust (stable toolchain)
- Formatting: `rustfmt` (default config); enforced in CI
- Lints: `clippy` with default lints; no `#[allow]` without a comment explaining why
- No `unwrap()` in library code; use `?` or explicit error handling
- Tests live in `tests/` (integration) or inline `#[cfg(test)]` modules (unit)

## Commit style

`type(scope): short description` — e.g., `feat(plugins): add RTK gains plugin`, `fix(ui): modal closes on Escape key`

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`

## Plugins

Plugin authoring conventions live in `docs/plugin-system.md`. Never embed plugin logic in the core crate; plugins are dynamically loaded or configured as external binaries.
