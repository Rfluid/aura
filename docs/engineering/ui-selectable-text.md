---
title: Selectable read-only text
status: active
version: 0.1.0
last_updated: 2026-08-23
source_refs:
    - crates/aura/Cargo.toml
    - crates/aura/src/app.rs
    - crates/aura/src/main.rs
    - scripts/check-selectable-text.sh
owner: "@rfluid"
tags: [ui, selection, clipboard, gpui]
---

# Selectable read-only text

The Aura modal is mostly labels — quota numbers, model names, plugin cells —
and users want to copy those values. A plain `div().child("…")` renders its
string through GPUI's internal `StyledText` but hides the `TextLayout`, so the
text can't be drag-selected or copied.

Aura uses the external
[`gpui-selectable-text`](https://github.com/Rfluid/gpui-selectable-text) crate.
Its `SelectableText` is an unfocusable leaf element that adds mouse and keyboard
selection, a painted highlight, clipboard support, selectable links, and named
selection scopes. It drops into any `div` exactly where a bare string child sat.

The crate supersedes Aura's former in-tree custom `Element` implementation.
Keeping the behavior in the shared crate avoids maintaining GPUI layout,
hit-testing, gesture, and clipboard code in the application.

## The rule

**This is the only sanctioned way to render copyable read-only text.** Do not
construct `gpui::StyledText` / `InteractiveText` anywhere in `crates/aura/src`
and do not repurpose a text input for display.

`scripts/check-selectable-text.sh` grep-enforces this and runs in
`scripts/pre-pr.sh`.

## Usage

```rust
use gpui_selectable_text::SelectableText;

// Before:
div().child(SharedString::from(value))
// After (id must be unique among the parent's children):
div().child(SelectableText::new(
    sid(format!("model-name-{model}")),
    value,
))
```

`ElementId` converts from `SharedString`/`&'static str` but **not** `String`,
so keyed ids route through the `sid(String) -> SharedString` helper in
`app.rs`. That module also keeps a small `sel(id, text)` constructor helper to
make the many label call sites compact; it contains no selection behavior. In
lists, key the id by stable content or index so no two runs collide.

Inline styles and links use the crate's `.highlights(..)` and `.links(..)`
builders.

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
with `set_selection_theme(cx, SelectionStyle::from_background(..))`. The crate
resolves its app-global theme during paint, so runtime theme changes retint live
selections.

## Keyboard behavior

`main.rs` installs `register_keyboard_bridge(cx)` once at startup. The bridge
observes only keystrokes that no focused control claimed, preserving normal
text-input shortcuts while enabling copy, select-all, escape-to-clear, and
shift+arrow extension for unfocused selectable labels.
