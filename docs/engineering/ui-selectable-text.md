---
title: Selectable read-only text
status: active
version: 0.1.0
last_updated: 2026-07-13
source_refs:
    - crates/aura/src/selectable_text.rs
    - crates/aura/src/app.rs
    - scripts/check-selectable-text.sh
owner: "@rfluid"
tags: [ui, selection, clipboard, gpui]
---

# Selectable read-only text

The Aura modal is mostly labels — quota numbers, model names, plugin cells —
and users want to copy those values. A plain `div().child("…")` renders its
string through GPUI's internal `StyledText` but hides the `TextLayout`, so the
text can't be drag-selected or copied.

`crate::selectable_text::SelectableText` is a leaf element that wraps
`StyledText` and adds three things: a drag-select gesture, a painted highlight,
and copy-on-release to the clipboard (+ Linux primary selection). It has **no
focus handle, caret, or key handling** — it drops into any `div` exactly where
a bare string child sat.

Ported from `warren/crates/warren-ui/src/app/selectable_text.rs`; both projects
run gpui 0.2.2.

## The rule

**This is the only sanctioned way to render copyable read-only text.** Do not
construct `gpui::StyledText` / `InteractiveText` anywhere in `crates/aura/src`
outside `selectable_text.rs`, and do not repurpose a text input for display.

`scripts/check-selectable-text.sh` grep-enforces this and runs in
`scripts/pre-pr.sh`.

## Usage

```rust
use crate::selectable_text::sel;

// Before:
div().child(SharedString::from(value))
// After (id must be unique among the parent's children):
div().child(sel(sid(format!("model-name-{model}")), value))
```

`ElementId` converts from `SharedString`/`&'static str` but **not** `String`,
so keyed ids route through the `sid(String) -> SharedString` helper in
`app.rs`. In lists, key the id by stable content or index so no two runs
collide.

Variants:

- `sel(id, text)` — canonical; inherits parent text style.
- `sel_styled(id, text, highlights)` — inline `HighlightStyle` runs.
- `sel_linked(id, text, highlights, links)` — clickable / right-click-copyable
  link ranges.

## Selection scopes

Starting a new selection clears every *other* live selection that shares a
scope. `SelectionScope::Global` (the default) makes the whole modal
single-selection — exactly one run highlighted at a time.

`SelectionScope::Named(id)` opts a run into an independent namespace. Plugin
sections use `Named(section.id)` so a selection in one section is decoupled
from others:

```rust
sel(sid(format!("{}-cell-{ri}-{i}", section.id)), cell.clone())
    .in_scopes([SelectionScope::Named(section.id.clone().into())])
```

Only one plugin section body renders at a time today, so scoping is currently
a no-op guard — it keeps sections decoupled if the layout ever shows more than
one simultaneously.

## Theming

The highlight tint is the themed accent at ~20% alpha, published each render
via `set_selection_tint(cx, (accent << 8) | 0x33)` (a `SelectionTint` global
read by the element). Runtime theme changes retint live selections.
