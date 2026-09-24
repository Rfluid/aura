---
title: GPUI key bindings need a focused element and insertion order breaks context ties
status: current
version: 0.1.0
last_updated: 2026-09-23
last_verified: 2026-09-23
source_refs:
  - crates/aura/src/keys.rs
  - crates/aura/src/app.rs
  - vendor/gpui/src/keymap.rs
  - vendor/gpui/src/window.rs
owner: "@rfluid"
tags: [memory, fact, gpui, keybindings]
source_task: keybindings feature (docs/keybindings.md)
---

# GPUI keymap dispatch facts

- With nothing focused, GPUI dispatches from the dispatch tree's root node, which is
  *not* the view's root `div`. Contexts and `on_action` listeners on that div are then
  off the dispatch path, so bindings never match. The modal root holds a
  `FocusHandle` (`track_focus`) focused at open; a mouse-down anywhere on a
  focus-tracked element re-focuses it.
- `Keymap::bindings_for_input` ranks by context depth, then by insertion order (later
  wins). `Aura` and `overlay` live on the same node, so they tie on depth: overlay
  bindings must be `bind_keys`'d after global ones.
- `NoAction` bindings without `meta` count as user unbinds and mask lower-precedence
  matches — that is how `"x" = "none"` in `[overlay]` blocks a `[global]` binding.
- Keystroke observers (`observe_keystrokes`, incl. gpui-selectable-text's bridge) see
  `event.action.is_some()` once a binding fires and step aside, so binding
  ctrl-c / ctrl-a / shift+arrows steals them from text selection.
- `KeyBinding::load` parses `G` as `shift-g`; typed shift-g matches both spellings.
  `?` matches via `key_char`.
