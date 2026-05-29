//! Modal placement — the single source of truth for where the tray popup
//! sits and how large it is.
//!
//! Placement is computed as an **absolute function of `(display work area,
//! content height)`**. It deliberately never reads the window's *current*
//! position: that is what makes the post-resize reposition idempotent.
//! Reading the live origin back and feeding it into the next reposition is
//! exactly the feedback loop that made the Windows modal "walk" across the
//! screen on every click (issue #27) — keeping the math here, sourced only
//! from the work area, removes any opportunity for that drift.
//!
//! Two callers share this module:
//!
//! 1. [`modal_bounds`] — `main.rs::toggle_window` uses it for the initial
//!    window bounds at open (size + origin for the full [`MODAL_H`]).
//! 2. [`modal_origin`] — `app.rs`'s auto-fit callback uses it to recompute
//!    where the (now shorter) window should sit after it shrinks to the
//!    measured content height. See `platform::reposition_after_resize`.

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
/// panel preset (~120px) so corner anchoring degrades to "bottom-right
/// placement minus a safe margin" instead of dumping the modal into a
/// taskbar.
#[cfg(not(target_os = "macos"))]
pub const BLIND_BOTTOM_RESERVE: f32 = 120.0;

/// The modal's size at open ([`MODAL_W`] × [`MODAL_H`]).
pub fn modal_size() -> Size<Pixels> {
    size(px(MODAL_W), px(MODAL_H))
}

/// Desired top-left of the modal, in the same logical-pixel space
/// `App::primary_display` uses (origin at the display's top-left, Y
/// increasing downward), for a window whose content is `content_h` pixels
/// tall.
///
/// **macOS**: the tray icon lives in the menu bar at the top, so the modal
/// anchors just below the bar (~25pt), horizontally centred on the icon's X
/// coordinate (from `hint`). Independent of `content_h` — the window grows
/// downward from a fixed top. Falls back to the top-right when `hint` is
/// absent.
///
/// **Linux / Windows**: the tray icon lives at the bottom-right, so the
/// modal anchors there — its bottom edge hugs the top of the taskbar/panel
/// regardless of height, which is why `content_h` feeds the Y. Wayland
/// compositors may ignore the requested origin and centre the window; users
/// can override via a KWin window rule (see README "Modal placement on
/// Wayland").
pub fn modal_origin(
    display: Bounds<Pixels>,
    hint: Option<(i32, i32)>,
    content_h: f32,
) -> Point<Pixels> {
    let screen_left = f32::from(display.origin.x);
    let screen_top = f32::from(display.origin.y);
    let screen_right = f32::from(display.origin.x + display.size.width);

    #[cfg(target_os = "macos")]
    {
        let _ = content_h;
        const MENU_BAR_H: f32 = 25.0;
        let icon_x = hint
            .map(|(x, _)| x as f32)
            .unwrap_or(screen_right - MODAL_W / 2.0);
        let x = (icon_x - MODAL_W / 2.0).clamp(screen_left, screen_right - MODAL_W);
        let y = screen_top + MENU_BAR_H + SCREEN_GAP;
        point(px(x), px(y))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = hint;
        let screen_bottom_full = f32::from(display.origin.y + display.size.height);
        let work_bottom = crate::work_area::available_bottom(display)
            .unwrap_or(screen_bottom_full - BLIND_BOTTOM_RESERVE);
        let x = (screen_right - MODAL_W - SCREEN_GAP).max(screen_left);
        let y = (work_bottom - content_h - SCREEN_GAP).max(screen_top);
        point(px(x), px(y))
    }
}

/// The modal's full bounds at open: [`modal_size`] anchored at
/// [`modal_origin`] for the full [`MODAL_H`]. Falls back to screen-centred
/// when there is no primary display.
pub fn modal_bounds(cx: &mut gpui::App, hint: Option<(i32, i32)>) -> Bounds<Pixels> {
    let size = modal_size();
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, size, cx);
    };
    Bounds::new(modal_origin(display.bounds(), hint, MODAL_H), size)
}
