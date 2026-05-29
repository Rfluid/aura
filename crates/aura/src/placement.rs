//! Modal placement — the single source of truth for where the tray popup
//! sits and how large it is.
//!
//! Placement is computed as an **absolute function of `(display work area,
//! content height, anchor)`**. It deliberately never reads the window's
//! *current* position: that is what makes the post-resize reposition
//! idempotent. Reading the live origin back and feeding it into the next
//! reposition is exactly the feedback loop that made the Windows modal "walk"
//! across the screen on every click (issue #27) — keeping the math here,
//! sourced only from the work area, removes any opportunity for that drift.
//!
//! The [`Anchor`] (from `display.anchor` in the config) selects how the modal
//! behaves as it auto-fits its content height. Two callers share the module:
//!
//! 1. [`modal_bounds`] — `main.rs::toggle_window` uses it for the initial
//!    window bounds at open (size + origin for the full [`MODAL_H`]).
//! 2. [`modal_origin`] — `app.rs`'s auto-fit callback uses it to recompute
//!    where the (now shorter) window should sit after it shrinks to the
//!    measured content height. Only [`Anchor::Bottom`] actually repositions
//!    (see [`Anchor::needs_reposition`] and `platform::reposition_after_resize`
//!    / `platform::set_window_origin`).

use gpui::{point, px, size, Bounds, Pixels, Point, Size};

/// Fixed modal width. The window grows vertically to fit content (see
/// `app.rs::on_children_prepainted`), so only the height is dynamic.
pub const MODAL_W: f32 = 520.0;

/// Initial modal height at open. The auto-fit callback shrinks the window to
/// the measured content height on the next frame, so this is just a sensible
/// starting size that lets the first paint render without thrashing.
pub const MODAL_H: f32 = 640.0;

/// Gap between the modal and the nearest screen edge / taskbar.
pub const SCREEN_GAP: f32 = 8.0;

/// Defensive blind reserve for the bottom edge when
/// [`crate::work_area::available_bottom`] returns `None` (non-KDE,
/// non-Linux, or parse failure). Comfortably clears KDE Plasma's "Huge"
/// panel preset (~120px) so bottom anchoring degrades to "bottom-right
/// placement minus a safe margin" instead of dumping the modal into a
/// taskbar.
pub const BLIND_BOTTOM_RESERVE: f32 = 120.0;

/// Approximate height of the macOS menu bar, cleared by [`Anchor::Top`].
#[cfg(target_os = "macos")]
const MENU_BAR_H: f32 = 25.0;

/// How the modal anchors as it auto-fits its content height. Parsed from the
/// `display.anchor` config string (see [`Anchor::from_config`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Open at the platform's natural tray corner and grow downward from
    /// there (GPUI's default resize behaviour); never reposition after a
    /// resize. Safe on Wayland, where the compositor owns placement.
    None,
    /// Pin the bottom edge above a bottom taskbar so the modal grows *upward*.
    /// GPUI's `resize()` keeps the top fixed and grows downward, so this is
    /// the only anchor that needs an active post-resize move.
    Bottom,
    /// Pin the top edge just below a top panel / menu bar and grow downward.
    /// No active move needed — GPUI already grows down from a fixed top.
    Top,
}

impl Anchor {
    /// Per-OS default, baked in at compile time for this target. Mirrors
    /// `aura_core::config::default_anchor` (kept in sync by value, since the
    /// two live in different crates).
    pub fn os_default() -> Self {
        #[cfg(target_os = "windows")]
        {
            Anchor::Bottom
        }
        #[cfg(not(target_os = "windows"))]
        {
            Anchor::None
        }
    }

    /// Parse the `display.anchor` config string. Unrecognised values
    /// (including the legacy `"auto"`) fall back to the per-OS default so old
    /// configs keep working.
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Anchor::None,
            "bottom" => Anchor::Bottom,
            "top" => Anchor::Top,
            _ => Self::os_default(),
        }
    }

    /// Whether this anchor needs an active reposition after the auto-fit
    /// resize. Only [`Anchor::Bottom`] does: GPUI keeps the top fixed and
    /// grows downward, so a bottom-pinned window must be moved back up.
    pub fn needs_reposition(self) -> bool {
        matches!(self, Anchor::Bottom)
    }
}

/// The modal's size at open ([`MODAL_W`] × [`MODAL_H`]).
pub fn modal_size() -> Size<Pixels> {
    size(px(MODAL_W), px(MODAL_H))
}

/// Desired top-left of the modal, in the same logical-pixel space
/// `App::primary_display` uses (origin at the display's top-left, Y
/// increasing downward), for a window whose content is `content_h` pixels
/// tall under `anchor`.
///
/// Horizontal placement: macOS centres on the tray-icon click (the tray lives
/// in the menu bar), so `hint` is the click X; every other platform's tray
/// sits in a screen corner, so the modal right-aligns and `hint` is ignored.
///
/// Vertical placement follows `anchor` (see [`Anchor`]). `Anchor::None` uses
/// the platform-natural corner — top (below the menu bar) on macOS, bottom on
/// Windows/Linux — and is never repositioned afterwards.
pub fn modal_origin(
    display: Bounds<Pixels>,
    hint: Option<(i32, i32)>,
    content_h: f32,
    anchor: Anchor,
) -> Point<Pixels> {
    let screen_left = f32::from(display.origin.x);
    let screen_top = f32::from(display.origin.y);
    let screen_right = f32::from(display.origin.x + display.size.width);
    let screen_bottom_full = f32::from(display.origin.y + display.size.height);

    #[cfg(target_os = "macos")]
    let x = {
        let icon_x = hint
            .map(|(x, _)| x as f32)
            .unwrap_or(screen_right - MODAL_W / 2.0);
        (icon_x - MODAL_W / 2.0).clamp(screen_left, screen_right - MODAL_W)
    };
    #[cfg(not(target_os = "macos"))]
    let x = {
        let _ = hint;
        (screen_right - MODAL_W - SCREEN_GAP).max(screen_left)
    };

    let bottom_y = || {
        let work_bottom = crate::work_area::available_bottom(display)
            .unwrap_or(screen_bottom_full - BLIND_BOTTOM_RESERVE);
        (work_bottom - content_h - SCREEN_GAP).max(screen_top)
    };
    let top_y = || {
        // macOS clears the menu bar; elsewhere we sit at the top of the
        // display. We only detect *bottom* panel reservations today, so on a
        // Linux setup with a top panel this can land under it — documented in
        // docs/configuration.md.
        #[cfg(target_os = "macos")]
        {
            screen_top + MENU_BAR_H + SCREEN_GAP
        }
        #[cfg(not(target_os = "macos"))]
        {
            screen_top + SCREEN_GAP
        }
    };

    let y = match anchor {
        Anchor::Bottom => bottom_y(),
        Anchor::Top => top_y(),
        Anchor::None => {
            #[cfg(target_os = "macos")]
            {
                top_y()
            }
            #[cfg(not(target_os = "macos"))]
            {
                bottom_y()
            }
        }
    };

    point(px(x), px(y))
}

/// The modal's full bounds at open: [`modal_size`] anchored at
/// [`modal_origin`] for the full [`MODAL_H`]. Falls back to screen-centred
/// when there is no primary display.
pub fn modal_bounds(
    cx: &mut gpui::App,
    hint: Option<(i32, i32)>,
    anchor: Anchor,
) -> Bounds<Pixels> {
    let size = modal_size();
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, size, cx);
    };
    Bounds::new(modal_origin(display.bounds(), hint, MODAL_H, anchor), size)
}
