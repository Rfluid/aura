# Aura Design System

Aura is a Rust + GPUI desktop widget that mirrors `claude /usage` and surfaces
AI agent usage statistics through a plugin system. This directory is the
source-of-truth for visual design decisions.

## Design philosophy

**Zed-ish dark, monospace numbers, terminal-style data density.**

- **Dark first.** Aura lives next to your taskbar. A near-black surface
  (`#0e0e10`) keeps it visually quiet against any wallpaper and matches the
  editors most users run all day (Zed, neovim themes, modern terminals).
- **Monospace everywhere.** Numbers must align. Token counts, percentages,
  reset timestamps, model names — all of them are scanned, not read. A single
  monospace family makes the cards behave like a terminal report.
- **Data density.** The widget is a glance, not a dashboard. Tight padding,
  small text (`text_xs` / `text_sm`), no decorative chrome. If a row does not
  carry data, it should not exist.
- **One accent.** A single violet accent (`#8b5cf6`) carries all "active /
  selected / progress" meaning. Per-agent brand colors are reserved for the
  agent's own icon (see `agents.md`); they never compete with the accent.
- **No motion for decoration.** The only animation is the spinner that signals
  in-flight work (see `loading.md`).

## Index

| File              | What's in it                                                                    |
| ----------------- | ------------------------------------------------------------------------------- |
| `README.md`       | This file. Philosophy + index.                                                  |
| `tokens.md`       | Canonical color, type, spacing, radius, shadow tokens.                          |
| `agents.md`       | Per-agent brand colors and the luminance fallback rule.                         |
| `components.md`   | Visual primitives (stat-card, pill, tab, progress bar, plugin panel, modal).    |
| `customization.md`| Schema for user-overridable theme (`~/.config/aura/theme.toml`). Not built yet. |
| `loading.md`      | Spinner spec for every fetch-triggering action.                                 |

## How to use this doc

Every token table cell ends in a "used in" column with a `file:line` reference
into the live codebase. When you change a token, update the reference. When you
add a new visual primitive, add it to `components.md` and link the renderer.

This is intentionally code-first and opinionated. If a rule feels wrong while
you're implementing, fix the rule here in the same PR — do not drift.
