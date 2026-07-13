//! Read-only **selectable** text — the GPUI analog of selecting a paragraph in
//! Zed's markdown preview or its agent message history.
//!
//! ## Why this exists
//! A plain `div().child("…")` renders its string through GPUI's internal
//! [`StyledText`], but hides the [`TextLayout`], so the user can't drag-select
//! or copy it. Aura's modal is nothing *but* such labels (quota numbers, model
//! names, plugin cells), and users routinely want to copy those values. Wrapping
//! them in a focusable text input would be wrong: it turns a label into a
//! caret-bearing, IME-driven input and forces every call site to restructure.
//!
//! Zed solves this in `crates/markdown/src/markdown.rs` by painting selection
//! quads + handling mouse events on top of a rendered text layout, never an
//! editor. We do the same, but the *minimum* version: [`SelectableText`] wraps
//! [`StyledText`] (so wrapping, auto-height and glyph painting come for free)
//! and adds exactly three things —
//!   1. a drag-select gesture (mouse down → anchor, move → extend),
//!   2. a painted selection highlight, and
//!   3. copy-on-release to the system clipboard (+ the X11/Wayland primary
//!      selection on Linux).
//!
//! Selection state lives in per-element state (keyed by the [`ElementId`] the
//! caller passes), held as `Rc<…>` cells so the paint-phase mouse listeners and
//! the next frame's highlight paint share it — the same pattern GPUI's own
//! `InteractiveText` uses. No focus handle, no entity, no text input: it drops
//! straight into any existing `div` as a leaf child.
//!
//! This port originates from `warren/crates/warren-ui/src/app/selectable_text.rs`
//! (both projects run gpui 0.2.2). The only Aura-specific change is the
//! selection tint: rather than a hardcoded constant, it is derived from the
//! themed accent and published through the [`SelectionTint`] global (see
//! [`crate::app`], which refreshes it each render).
//!
//! ## How to use it
//! Build it through the [`sel`] helper (or the styled [`sel_styled`] /
//! [`sel_linked`] wrappers) and hand it to `.child(..)` like any string. The
//! `id` must be unique within the parent (use `("kind", index)` in lists).
//! Text color / size are inherited from the surrounding `div`, exactly like a
//! bare string child.
//!
//! **This is the only sanctioned way to render copyable read-only text.** Do
//! not construct `gpui::StyledText` / `InteractiveText` elsewhere, and do not
//! repurpose a text input for display. The rule is grep-enforced by
//! `scripts/check-selectable-text.sh` (run from `scripts/pre-pr.sh`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};

use gpui::{
    fill, point, rgba, App, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId,
    Global, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, SharedString, StyledText, TextLayout, Window,
};

/// Fallback selection-highlight tint (accent `0x8b5cf6` at ~20% alpha), used
/// until [`crate::app`] publishes the themed value via [`SelectionTint`].
const DEFAULT_SELECTION_BG: u32 = 0x8b5cf633;

/// App-global selection tint (`0xRRGGBBAA`). [`crate::app`] refreshes this from
/// the active theme's accent each render so runtime theme changes retint live
/// selections. See [`set_selection_tint`].
#[derive(Clone, Copy)]
pub struct SelectionTint(pub u32);

impl Default for SelectionTint {
    fn default() -> Self {
        SelectionTint(DEFAULT_SELECTION_BG)
    }
}

impl Global for SelectionTint {}

/// Publish the selection tint (`0xRRGGBBAA`) for [`SelectableText`] to read.
/// Cheap and idempotent — call once per render from the theme's accent.
pub fn set_selection_tint(cx: &mut App, rgba: u32) {
    cx.set_global(SelectionTint(rgba));
}

/// A namespace that a [`SelectableText`] selection belongs to. Starting a new
/// selection clears every *other* live selection that shares at least one scope
/// with the one being started — so two runs in the same scope can't stay
/// highlighted at once, while runs in disjoint scopes are independent.
///
/// [`Global`](SelectionScope::Global) is the default and behaves as one shared
/// namespace across the whole app: unless a caller opts into a narrower scope,
/// every selection mutually excludes every other. `Named` scopes are the escape
/// hatch for future features — a run can subscribe to a private scope (and only
/// that scope) to opt out of the global "single selection" behavior.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SelectionScope {
    /// The default app-wide scope. All global selections exclude each other.
    Global,
    /// A named scope; only selections sharing this name exclude each other.
    /// Reserved for future scoped-selection features (e.g. per plugin section).
    #[allow(dead_code)]
    Named(SharedString),
}

/// App-global registry mapping each active scope to the selection currently
/// holding it. Stored as a [`Weak`] so a dropped element's entry becomes inert
/// on its own (upgrade fails) without needing explicit cleanup.
#[derive(Default)]
struct SelectionRegistry {
    active: HashMap<SelectionScope, Weak<RefCell<Range<usize>>>>,
}

impl Global for SelectionRegistry {}

/// Begin a selection owned by `my_range` across `scopes`: clear any other live
/// selection registered in a shared scope, then claim those scopes for this one.
/// The `ptr_eq` guard keeps a run from clearing *itself* when it re-anchors.
fn activate_selection(
    cx: &mut App,
    scopes: &[SelectionScope],
    my_range: &Rc<RefCell<Range<usize>>>,
) {
    let registry = cx.default_global::<SelectionRegistry>();
    for scope in scopes {
        if let Some(prev) = registry.active.get(scope).and_then(Weak::upgrade) {
            if !Rc::ptr_eq(&prev, my_range) {
                *prev.borrow_mut() = 0..0;
            }
        }
        registry
            .active
            .insert(scope.clone(), Rc::downgrade(my_range));
    }
}

/// Per-element selection state, persisted across frames via
/// [`Window::with_element_state`]. The `Rc` cells are cloned into the paint-time
/// mouse listeners so a drag started this frame is visible the next.
#[derive(Clone)]
struct SelectionState {
    /// Byte offset where the current drag began.
    anchor: Rc<Cell<usize>>,
    /// Ordered selected byte range (`start == end` ⇒ nothing selected).
    range: Rc<RefCell<Range<usize>>>,
    /// True while the mouse button is held after a press inside this element.
    dragging: Rc<Cell<bool>>,
}

impl Default for SelectionState {
    fn default() -> Self {
        SelectionState {
            anchor: Rc::new(Cell::new(0)),
            range: Rc::new(RefCell::new(0..0)),
            dragging: Rc::new(Cell::new(false)),
        }
    }
}

/// A read-only, drag-selectable, copy-on-release run of text. Wraps
/// [`StyledText`]; see the module docs for the rationale.
pub struct SelectableText {
    id: ElementId,
    /// The raw source string (sliced on copy; byte offsets from the layout
    /// index into this).
    text: SharedString,
    inner: StyledText,
    links: Vec<(Range<usize>, SharedString)>,
    /// Scopes this selection participates in (see [`SelectionScope`]). Defaults
    /// to `[Global]`; override with [`in_scopes`](Self::in_scopes).
    scopes: Vec<SelectionScope>,
}

impl SelectableText {
    /// Construct a selectable run. `id` must be unique among the parent's
    /// children — in lists use a keyed id like `("model-name", ix)`.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        SelectableText {
            id: id.into(),
            text: text.clone(),
            inner: StyledText::new(text),
            links: Vec::new(),
            scopes: vec![SelectionScope::Global],
        }
    }

    /// Replace the scopes this run participates in (default `[Global]`). Pass a
    /// single narrow scope to opt a run out of the app-wide "single selection"
    /// behavior, or several to have it excluded by any of them.
    #[allow(dead_code)]
    pub fn in_scopes(mut self, scopes: impl IntoIterator<Item = SelectionScope>) -> Self {
        self.scopes = scopes.into_iter().collect();
        self
    }

    /// Like [`new`](Self::new) but paints inline style runs (bold, italic,
    /// code, links…) over the text. Each `(range, style)` indexes byte offsets
    /// into `text`; gaps inherit the parent text style. `with_highlights`
    /// resolves the runs against that inherited style at layout time, so callers
    /// leave `color: None` to keep the default color.
    pub fn with_runs(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        let text = text.into();
        SelectableText {
            id: id.into(),
            text: text.clone(),
            inner: StyledText::new(text).with_highlights(highlights),
            links: Vec::new(),
            scopes: vec![SelectionScope::Global],
        }
    }

    /// Like [`with_runs`](Self::with_runs), with clickable/copyable link ranges.
    pub fn with_runs_and_links(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        links: Vec<(Range<usize>, SharedString)>,
    ) -> Self {
        let text = text.into();
        SelectableText {
            id: id.into(),
            text: text.clone(),
            inner: StyledText::new(text).with_highlights(highlights),
            links,
            scopes: vec![SelectionScope::Global],
        }
    }

    /// Map a mouse position to the nearest byte offset in the source text.
    fn offset_at(layout: &TextLayout, position: Point<Pixels>) -> usize {
        layout.index_for_position(position).unwrap_or_else(|e| e)
    }

    fn link_at(
        links: &[(Range<usize>, SharedString)],
        layout: &TextLayout,
        position: Point<Pixels>,
    ) -> Option<SharedString> {
        let ix = layout.index_for_position(position).ok()?;
        links
            .iter()
            .find(|(range, _)| range.contains(&ix))
            .map(|(_, url)| url.clone())
    }

    /// One highlight quad per visual row covering `range`. Coordinates are
    /// absolute (from [`TextLayout::position_for_index`], which already adds the
    /// element's bounds origin).
    fn selection_quads(layout: &TextLayout, range: &Range<usize>, color: u32) -> Vec<PaintQuad> {
        if range.start >= range.end {
            return Vec::new();
        }
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return Vec::new();
        };
        let line_height = layout.line_height();
        let bounds = layout.bounds();
        let color = rgba(color);
        let rect = |x0: Pixels, y: Pixels, x1: Pixels| {
            fill(
                Bounds::from_corners(point(x0, y), point(x1, y + line_height)),
                color,
            )
        };

        // Number of visual rows the selection straddles (0 ⇒ single row).
        let rows = ((end.y - start.y) / line_height).round() as i32;
        if rows <= 0 {
            return vec![rect(start.x, start.y, end.x)];
        }
        let mut quads = Vec::with_capacity(rows as usize + 1);
        quads.push(rect(start.x, start.y, bounds.right()));
        for r in 1..rows {
            quads.push(rect(
                bounds.left(),
                start.y + line_height * r as f32,
                bounds.right(),
            ));
        }
        quads.push(rect(bounds.left(), end.y, end.x));
        quads
    }
}

impl IntoElement for SelectableText {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // StyledText owns the measured layout (wrap + auto-height).
        self.inner.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        // Touch the element state so it exists for the paint phase.
        window.with_optional_element_state::<SelectionState, _>(global_id, |s, _| {
            ((), Some(s.flatten().unwrap_or_default()))
        });
        self.inner
            .prepaint(None, inspector_id, bounds, state, window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.inner.layout().clone();
        let text = self.text.clone();
        let links = self.links.clone();
        let scopes = self.scopes.clone();
        let tint = cx.default_global::<SelectionTint>().0;

        window.with_element_state::<SelectionState, _>(global_id.unwrap(), |state, window| {
            let state = state.unwrap_or_default();

            // 1. Highlight behind the glyphs (painted first so text sits on top).
            for quad in Self::selection_quads(&layout, &state.range.borrow(), tint) {
                window.paint_quad(quad);
            }

            let hovering_link = Self::link_at(&links, &layout, window.mouse_position()).is_some();
            window.set_cursor_style(
                if hovering_link {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                },
                hitbox,
            );

            // 2a. Press inside ⇒ start a drag, anchoring at the clicked offset.
            window.on_mouse_event({
                let (anchor, range, dragging, layout, hitbox, links, scopes) = (
                    state.anchor.clone(),
                    state.range.clone(),
                    state.dragging.clone(),
                    layout.clone(),
                    hitbox.clone(),
                    links.clone(),
                    scopes.clone(),
                );
                move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if !hitbox.is_hovered(window) {
                        // Press landed outside this run (blank space or another
                        // widget): drop our selection so a click anywhere clears
                        // it, not only a click on another selectable run.
                        if event.button == MouseButton::Left && !range.borrow().is_empty() {
                            *range.borrow_mut() = 0..0;
                            dragging.set(false);
                            window.refresh();
                        }
                        return;
                    }
                    {
                        if event.button == MouseButton::Right {
                            if let Some(url) =
                                SelectableText::link_at(&links, &layout, event.position)
                            {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
                                cx.stop_propagation();
                                window.prevent_default();
                            }
                            return;
                        }
                        if event.button != MouseButton::Left {
                            return;
                        }
                        // Clear any other selection sharing a scope, then claim
                        // ours — so only one run per scope stays highlighted.
                        activate_selection(cx, &scopes, &range);
                        let ix = SelectableText::offset_at(&layout, event.position);
                        anchor.set(ix);
                        *range.borrow_mut() = ix..ix;
                        dragging.set(true);
                        window.refresh();
                    }
                }
            });

            // 2b. Move while dragging ⇒ extend (even past the element bounds, so
            //     the drag keeps tracking once the cursor leaves the text).
            window.on_mouse_event({
                let (anchor, range, dragging, layout) = (
                    state.anchor.clone(),
                    state.range.clone(),
                    state.dragging.clone(),
                    layout.clone(),
                );
                move |event: &MouseMoveEvent, phase, window: &mut Window, _| {
                    if phase == DispatchPhase::Bubble && dragging.get() {
                        let head = SelectableText::offset_at(&layout, event.position);
                        let a = anchor.get();
                        *range.borrow_mut() = a.min(head)..a.max(head);
                        window.refresh();
                    }
                }
            });

            // 2c. Release ⇒ end the drag and, if anything is selected, copy it.
            //     Copying on release (rather than via a focus-bound Ctrl-C) is
            //     what lets read-only text be "selected to copy" with no focus.
            window.on_mouse_event({
                let (range, dragging, layout, links) = (
                    state.range.clone(),
                    state.dragging.clone(),
                    layout.clone(),
                    links.clone(),
                );
                move |event: &MouseUpEvent, phase, _window: &mut Window, cx: &mut App| {
                    if phase != DispatchPhase::Bubble || !dragging.get() {
                        return;
                    }
                    dragging.set(false);
                    let sel = range.borrow().clone();
                    if sel.start < sel.end && sel.end <= text.len() {
                        let copied = text[sel].to_string();
                        // X11/Wayland primary selection (middle-click paste).
                        #[cfg(target_os = "linux")]
                        cx.write_to_primary(ClipboardItem::new_string(copied.clone()));
                        cx.write_to_clipboard(ClipboardItem::new_string(copied));
                    } else if event.button == MouseButton::Left {
                        if let Some(url) = SelectableText::link_at(&links, &layout, event.position)
                        {
                            cx.open_url(url.as_ref());
                        }
                    }
                }
            });

            // 3. The glyphs themselves.
            self.inner
                .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

            ((), state)
        });
    }
}

// ── call-site helpers ───────────────────────────────────────────────────────
// Swapping a bare string child for a selectable one is a one-word change; the
// surrounding `div` structure is untouched. Color/size are inherited from the
// parent `div`.

/// A selectable run inheriting the parent's text style. The canonical entry
/// point — reach for this wherever a copyable string would otherwise be a bare
/// `.child("…")`.
pub fn sel(id: impl Into<ElementId>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id, text)
}

/// A selectable run carrying inline style highlights.
/// See [`SelectableText::with_runs`].
#[allow(dead_code)]
pub fn sel_styled(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> SelectableText {
    SelectableText::with_runs(id, text, highlights)
}

/// A selectable run with clickable/copyable link ranges.
/// See [`SelectableText::with_runs_and_links`].
#[allow(dead_code)]
pub fn sel_linked(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, SharedString)>,
) -> SelectableText {
    SelectableText::with_runs_and_links(id, text, highlights, links)
}
