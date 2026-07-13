---
title: Selectable text with selection scopes
status: implemented
version: 0.1.0
last_updated: 2026-07-13
last_verified: 2026-07-13
source_refs:
    - crates/aura/src/app.rs
    - crates/aura/src/main.rs
    - vendor/gpui/src/elements/text.rs
    - "warren: crates/warren-ui/src/app/selectable_text.rs"
    - "warren: docs/engineering/ui-selectable-text.md"
owner: "@rfluid"
tags: [ui, selection, clipboard, gpui, design]
---

# Selectable text with selection scopes

## Problem

Every string in the Aura modal is rendered as a non-interactive
`div().child(SharedString::from(...))` (e.g. `crates/aura/src/app.rs:1048`,
`:1141`, `:2318-2330`). GPUI shapes those into glyphs with **no selection
affordance** — the user cannot drag-select or copy a quota number, a model
name, a plugin `Text`/`Table`/`Lines` cell, or any label. There is no
clipboard path in the app at all (grep of `crates/` finds zero
`InteractiveText`/`clipboard` hits).

Users routinely want to copy these values (a token count into a spreadsheet,
a model id into a config, a cost into a note). Today they can't.

Warren solved the identical problem — same framework, same GPUI 0.2.2 — with
one self-contained element, `SelectableText`, plus a `SelectionScope`
mutual-exclusion mechanism. This plan ports that pattern into Aura.

## Scope

A single new leaf element that drops into any existing `div` where a bare
string child sits today, giving that text:

1. **Drag-select** (mouse down → anchor, move → extend).
2. A **painted selection highlight**.
3. **Copy-on-release** to the system clipboard (+ X11/Wayland primary
   selection on Linux).
4. **Selection scopes** — a new drag clears every other live selection that
   shares a scope, so only one run per scope stays highlighted; disjoint
   scopes are independent.

Plus a one-word call-site helper (`sel(id, text)`) so converting a label is a
minimal, structure-preserving edit, and a grep-enforced rule funneling all
copyable read-only text through the one element.

## Non-goals

- **Editable text / IME / caret.** This is read-only. We deliberately do
  *not* reuse or introduce a focusable text input — that would steal focus,
  show a caret, and force call sites to restructure.
- **Keyboard-driven copy (Ctrl/Cmd-C).** Copy is on mouse release. A
  focus-bound Ctrl-C would require focusability, which the design avoids —
  and Aura has no `FocusHandle`/`on_key` handling today (confirmed absent in
  `crates/aura`). Copy-on-release is what lets read-only text be
  "select-to-copy" with zero focus machinery.
- **Selecting across separate elements.** Selection is per-`SelectableText`;
  scopes only govern *mutual exclusion*, not cross-element contiguous
  selection.
- **Converting every string in one pass.** v1 ports the mechanism and
  converts a first tranche of high-value text; the rest follows incrementally.

## Design

### Why this works unchanged from Warren

Aura and Warren both depend on **gpui 0.2.2** (Aura via the vendored+patched
tree, `crates/aura/Cargo.toml:19` + root `Cargo.toml:16`; Warren from
crates.io). Every API the pattern uses exists in Aura's vendored gpui:

- `StyledText` + `TextLayout::index_for_position` / `position_for_index`
  (`vendor/gpui/src/elements/text.rs:483` and neighbors) — pixel↔byte mapping
  for hit-testing and highlight geometry.
- `ClipboardItem` + `write_to_clipboard` / `write_to_primary` (cross-platform
  trait in `vendor/gpui/src/platform.rs`, mac impl at
  `vendor/gpui/src/platform/mac/platform.rs`).
- `Element` lifecycle (`request_layout`/`prepaint`/`paint`), `Global`,
  `Window::with_element_state`, `Hitbox`, `on_mouse_event`.

So the port is **near-verbatim** — this is a copy of one file plus wiring, not
a redesign.

### New file: `crates/aura/src/selectable_text.rs`

Ported verbatim from
`warren/crates/warren-ui/src/app/selectable_text.rs` (486 lines). Aura's
`app` is a flat module (`mod app;` at `main.rs:5`), so unlike Warren — where
the file lives under `src/app/` — this becomes a **sibling module** declared
in `main.rs` (add `mod selectable_text;` alongside `mod app;`).

Core pieces (all in that one file):

```rust
// Selection namespace controlling mutual exclusion.
pub enum SelectionScope {
    Global,                 // default: app-wide single-selection
    Named(SharedString),    // opt-out into an independent scope
}

// App-global registry: which selection currently owns each scope.
// Weak<> so dropped elements self-evict.
struct SelectionRegistry {
    active: HashMap<SelectionScope, Weak<RefCell<Range<usize>>>>,
}
impl Global for SelectionRegistry {}

// The core exclusion step, run on mouse-down before anchoring:
fn activate_selection(cx, scopes, my_range) {
    for scope in scopes {
        if let Some(prev) = registry.active.get(scope).and_then(Weak::upgrade) {
            if !Rc::ptr_eq(&prev, my_range) { *prev.borrow_mut() = 0..0; } // clear sibling
        }
        registry.active.insert(scope.clone(), Rc::downgrade(my_range));    // claim
    }
}

// Per-element state, persisted across frames via with_element_state,
// held in Rc cells so paint-phase mouse listeners and next frame's
// highlight share it.
struct SelectionState {
    anchor:   Rc<Cell<usize>>,           // byte offset drag began
    range:    Rc<RefCell<Range<usize>>>, // ordered selected range
    dragging: Rc<Cell<bool>>,
}

// The element itself — wraps StyledText for free wrap/auto-height/glyphs.
pub struct SelectableText {
    id: ElementId,
    text: SharedString,                       // sliced on copy
    inner: StyledText,
    links: Vec<(Range<usize>, SharedString)>, // click-to-open / right-click-copy
    scopes: Vec<SelectionScope>,              // default [Global]
}
```

Element behavior (`impl Element for SelectableText`):

- `request_layout` / `prepaint` delegate to the inner `StyledText`
  (it owns wrap + auto-height), then `prepaint` inserts a `Hitbox`.
- `paint`:
  1. paints one highlight quad per visual row **behind** the glyphs
     (`SELECTION_BG = 0x7aa2f733`);
  2. sets cursor `IBeam` (or `PointingHand` over a link);
  3. registers three `on_mouse_event` closures:
     - **down inside** → `activate_selection(...)`, anchor at clicked byte
       offset, start drag; **down outside** → clear own selection (so a click
       anywhere deselects); right-click on a link → copy URL;
     - **move while dragging** → extend `a.min(head)..a.max(head)`;
     - **up** → end drag; if non-empty, slice `text[sel]` → clipboard
       (+ primary on Linux); if it was a click on a link → open URL;
  4. paints the glyphs last.

Call-site helpers at the bottom of the file:

```rust
pub fn sel(id, text) -> SelectableText              // canonical; inherits parent style
pub fn sel_styled(id, text, highlights)             // with inline HighlightStyle runs
pub fn sel_linked(id, text, highlights, links)      // + clickable/copyable links
```

`SelectableText::in_scopes(iter)` overrides the default `[Global]`.

**Adaptation checklist for the port** (things to verify against Aura's tree,
not blind copy):

- Aura's vendored gpui may differ slightly from crates.io 0.2.2 in the
  `Element` trait signature (the vendor patch is for a macOS ivar, so the
  public element API should be untouched) — compile-check the
  `request_layout`/`prepaint`/`paint` signatures against
  `vendor/gpui/src/element.rs` and adjust if the `inspector_id` /
  `InspectorElementId` params differ.
- The selection tint (`0x7aa2f733`) is Warren's; retint from
  `aura_core::theme::Theme` (`crates/aura-core/src/theme.rs`) so it tracks
  Aura's accent instead of hardcoding. Recommend a `Theme` field
  `selection_bg` (or derive from the existing accent at some alpha).
- `#[allow(dead_code)]` on `in_scopes` / `Named` matches Warren; keep until a
  scoped call site exists.

### Selection scopes in Aura

`Global` (default) gives app-wide single-selection: exactly one run
highlighted at a time across the whole modal. That is the right default here.

`Named(...)` maps naturally onto Aura's container hierarchy when we want
independent selections — the obvious candidates:

- **Per plugin section** — `PluginSection.id`
  (`crates/aura-core/src/plugin/mod.rs`), so selecting a value in one
  section's table doesn't clear a selection the user made in another.
- **Per agent section / tab** — `AgentSection` (`app.rs:44-80`).

v1 ships everything on `Global` (simplest, matches Warren's default). Named
scopes are wired but unused until a concrete need appears — do **not**
speculatively scope in v1.

### Call-site conversion (`crates/aura/src/app.rs`)

The mechanical change is `div().child(SharedString::from(x))` →
`div().child(sel(id, x))`. The `id` must be unique among the parent's
children; in lists use a keyed id like `("model-name", ix)` — Aura already
builds keyed ids this way (e.g. `("agent-section-…")` at `app.rs:1318`).

First tranche (highest copy value), all in `app.rs`:

- Quota numbers / percentages — `render_quota` (`:1685`),
  `render_quota_window` (`:1796`).
- Model rows — `render_models` (`:2175`).
- Summary values — `render_summary` (`:2097`).
- Plugin `Lines`/`Table`/`Text` cells — `render_plugin_section`
  (`:2297-2340`).

Leave interactive controls (pills/tabs/buttons — they have `on_click`) as-is;
selection there would fight the click gesture.

### Enforcement (port Warren's discipline)

Warren funnels *all* copyable read-only text through the one element and
grep-guards it. Port both:

1. `docs/engineering/ui-selectable-text.md` — short doc: "this is the only
   sanctioned way to render copyable read-only text; do not construct
   `StyledText`/`InteractiveText` elsewhere or repurpose an input for
   display."
2. `scripts/check-selectable-text.sh` — fails if `StyledText` /
   `InteractiveText` appear in `crates/aura/src` outside
   `selectable_text.rs`. Wire it into `scripts/pre-pr.sh` (exists,
   `:6.2K`) — mirrors Warren's CI/pre-pr hook.

## Rollout

### Phase 1 — mechanism + first tranche (target: one PR)

- Add `crates/aura/src/selectable_text.rs` (ported), declare `mod
  selectable_text;` in `main.rs`.
- Retint selection to theme accent (add `Theme::selection_bg` or equivalent).
- Convert the quota/model/summary/plugin-cell tranche in `app.rs` to `sel(..)`.
- Add `docs/engineering/ui-selectable-text.md` +
  `scripts/check-selectable-text.sh`, hook into `pre-pr.sh`.
- Verify: build, open modal, drag-select a quota number → paste elsewhere;
  confirm starting a new selection clears the prior (Global scope); confirm
  Linux middle-click paste (primary selection).

### Phase 2 — remaining text + links

- Convert remaining labels (header, forecast, daily-chart labels, etc.).
- If any plugin emits URLs in `Text`, use `sel_linked` for click-to-open /
  right-click-copy.

### Phase 3 — scoped selection (only if needed)

- Introduce `Named` scopes per plugin section / tab if user feedback shows
  single-selection across sections is annoying. Not before.

## Open questions

- **Theme tint vs. Warren's constant** — recommend deriving from accent;
  confirm `Theme` is the right home and pick the alpha (Warren uses `0x…33`
  ≈ 20%).
- **Does the vendored gpui `Element` trait match crates.io 0.2.2 exactly?**
  Assumed yes (patch is macOS-ivar only); a compile pass confirms. Only risk
  to the verbatim port.
- **Plugin-declared selectability** — should the plugin JSON schema
  (`crates/aura-core/src/plugin/mod.rs`) let a plugin mark a cell
  non-selectable or assign a copy-value distinct from display? Out of scope
  for v1 (everything selectable by default); revisit if a plugin needs it.
- **Auto-close interaction** — Aura closes the modal on focus loss
  (`main.rs:162`, `runtime.rs`). Confirm a drag-select gesture and the
  clipboard write don't trip `dismiss_on_focus_loss` mid-drag. Likely fine
  (no focus change), but verify on each platform.

## References

- Warren source to port:
  `warren/crates/warren-ui/src/app/selectable_text.rs`.
- Warren design doc: `warren/docs/engineering/ui-selectable-text.md`.
- Warren enforcement: `warren/scripts/check-selectable-text.sh`.
- Prior art both repos mirror: Zed `crates/markdown/src/markdown.rs` +
  gpui `InteractiveText`.
