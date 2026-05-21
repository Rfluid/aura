---
title: UI design
status: draft
version: 0.1.0
last_updated: 2026-05-21
last_verified: 2026-05-21
source_refs: []
owner: "@rfluid"
tags: [ui, design, docs]
---

# UI design

## Principles

- **Zero friction** — one click to see usage; one click to switch agents
- **Minimal footprint** — no persistent window; modal appears on demand, disappears on blur
- **Dense but legible** — pack information tightly without overwhelming; use clear labels and hierarchy
- **Dark-first** — most terminal/taskbar users prefer dark themes; light theme is a stretch goal

## Modal anatomy

The usage panel mirrors the exact output of `claude /usage`. Two sub-tabs (Overview / Models) match the original, with the period selector above both.

```
┌──────────────────────────────────────────────────────┐
│  ◉ Aura                     Claude (Personal)  ▾     │  ← profile picker
├──────────────────────────────────────────────────────┤
│  All time · Last 7 days · Last 30 days               │  ← period selector
├──────────────────────────────────────────────────────┤
│  [ Overview ]  [ Models ]                            │  ← sub-tabs
│                                                      │
│  Favorite model:  Claude Opus 4.7                    │
│  Total tokens:    2,847,391                          │
│                                                      │
│  Sessions:        94        Longest: 2h 15m          │
│  Active days:     42/90     Peak hour: 15:00–16:00   │
│  Current streak:  3 days    Longest: 12 days         │
├──────────────────────────────────────────────────────┤
│  ⚡ RTK Gains                                        │  ← plugin panel
│  Saved today      1,247,832 tokens                   │
│  Savings rate              61%                       │
└──────────────────────────────────────────────────────┘
```

**Models sub-tab:**

```
│  [ Overview ]  [ Models ]                            │
│                                                      │
│  Tokens per Day                                      │
│  ██▁▂▃▅▆██▁▂▃▅▆█  (ASCII chart)                     │
│                                                      │
│  ● Claude Opus 4.7  (78%)   ● Sonnet 4.6  (22%)     │
│    In: 141k · Out: 384k       In: 50k · Out: 83k     │
└──────────────────────────────────────────────────────┘
```

Width: ~380px. Height: dynamic. Anchored near the tray icon.

Note: Cache token breakdown (`cache_read`, `cache_creation`) is tracked internally but not shown in the main panels — `/usage` itself omits it. It may appear as a collapsed "Details" row in a future version.

## Interaction model

| Interaction | Behavior |
|---|---|
| Click taskbar widget | Open modal; load active profile's usage |
| Click outside modal / press Escape | Close modal; write active profile to state |
| Click profile name in header | Open profile dropdown |
| Click a profile in dropdown | Switch active profile; re-fetch usage; re-render |
| Click period pill (Today / Month / All) | Re-fetch usage for the new period; re-render |
| Click plugin panel header | Expand/collapse plugin panel |
| Click anywhere outside the dropdown | Close dropdown without switching |

## Profile switcher dropdown

```
  Claude (Personal)   ✓
  Claude (Enterprise)
  Codex
```

The active profile has a checkmark. Clicking a different profile triggers a usage re-fetch with a brief loading indicator (spinner on the stats area, not the whole modal).

## Loading states

- **Usage loading**: replace the numbers with a short animated dots or spinner; never blank the labels
- **Plugin error**: show the plugin panel title with a muted "unavailable" message instead of hiding the panel entirely
- **Plugin timeout (>500ms)**: same as error; log a warning

## Color / theming

Visual style decisions are pending the UI framework choice. Key constraints:

- Must not clash with common dark taskbar setups (solarized, gruvbox, nord)
- Accent color for highlights: pending
- Font: system monospace for numbers; system UI font for labels

## Taskbar widget

The taskbar widget (e.g., eww module) shows a minimal summary: the active agent name and a cost figure. Clicking it opens the modal.

```
◉ Claude  $8.54
```

The widget binary output format depends on the taskbar tool (eww JSON, waybar JSON, plain text for polybar). Exact format TBD.
