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
//!
//! ## Which monitor
//!
//! Both entry points work against *a* display, not "the primary display".
//! [`modal_display`] picks the one the tray icon actually lives on (from the
//! [`TrayAnchor`] the backend reported) and falls back to the primary when
//! there's no anchor or no match. The chosen display's id rides along on
//! `AuraView` so the auto-fit callback keeps measuring against the same
//! screen it opened on — otherwise a modal opened on a secondary monitor
//! would be height-capped by the *primary* monitor's taskbar.

use std::rc::Rc;

use gpui::{point, px, size, App, Bounds, DisplayId, Pixels, PlatformDisplay, Point, Size};

use crate::tray::TrayAnchor;

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

// ── Display selection ────────────────────────────────────────────────────────

/// The display the modal should open on: the one containing the tray icon,
/// else the primary.
///
/// Returns the display's id alongside its bounds so the caller can hand the id
/// to `AuraView` and have the auto-fit callback re-resolve the same screen
/// later (displays can be unplugged while the modal is open, hence an id and
/// not a captured `Rc`).
pub fn modal_display(cx: &App, anchor: Option<TrayAnchor>) -> Option<(DisplayId, Bounds<Pixels>)> {
    if let Some(anchor) = anchor {
        let (x, y) = anchor.locator();
        let locator = point(px(x), px(y));
        if let Some(display) = cx
            .displays()
            .into_iter()
            .find(|d| crate::platform::display_bounds(d).contains(&locator))
        {
            return Some((display.id(), crate::platform::display_bounds(&display)));
        }
    }
    let primary = cx.primary_display()?;
    Some((primary.id(), crate::platform::display_bounds(&primary)))
}

/// Bounds of the display `id`, or the primary display's when that id is gone
/// (monitor unplugged since the modal opened).
pub fn display_bounds_or_primary(cx: &App, id: Option<DisplayId>) -> Option<Bounds<Pixels>> {
    let display: Rc<dyn PlatformDisplay> = id
        .and_then(|id| cx.find_display(id))
        .or_else(|| cx.primary_display())?;
    Some(crate::platform::display_bounds(&display))
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// Desired top-left of the modal, in the same logical-pixel space
/// `App::displays` uses (origin at the virtual desktop's top-left, Y
/// increasing downward), for a window whose content is `content_h` pixels
/// tall under `anchor`.
///
/// Horizontal placement: macOS centres the modal on the status item (the tray
/// lives in the menu bar, and a menu-bar popover hangs directly beneath its
/// item), so `tray` supplies the centre. Windows and Linux put the tray in a
/// screen corner and their native flyouts — the volume and network panels —
/// right-align to the screen edge rather than tracking the icon, so those
/// platforms ignore `tray` horizontally.
///
/// Vertical placement follows `anchor` (see [`Anchor`]). `Anchor::None` uses
/// the platform-natural corner — top (below the menu bar) on macOS, bottom on
/// Windows/Linux — and is never repositioned afterwards.
pub fn modal_origin(
    display: Bounds<Pixels>,
    tray: Option<TrayAnchor>,
    content_h: f32,
    anchor: Anchor,
) -> Point<Pixels> {
    let screen_left = f32::from(display.origin.x);
    let screen_top = f32::from(display.origin.y);
    let screen_right = f32::from(display.origin.x + display.size.width);
    let screen_bottom_full = f32::from(display.origin.y + display.size.height);

    #[cfg(target_os = "macos")]
    let x = {
        let icon_x = tray
            .map(|t| t.center_x())
            .unwrap_or(screen_right - MODAL_W / 2.0);
        (icon_x - MODAL_W / 2.0).clamp(screen_left, (screen_right - MODAL_W).max(screen_left))
    };
    #[cfg(not(target_os = "macos"))]
    let x = {
        let _ = tray;
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

/// Re-express an absolute screen origin in the coordinate space
/// `WindowOptions::window_bounds` uses on this platform.
///
/// The two GPUI backends disagree, and the disagreement is invisible on a
/// single-monitor machine — which is exactly why it is worth spelling out:
///
/// * **macOS** treats the origin as **display-relative**. `open_window` adds
///   the target `NSScreen`'s frame origin back on
///   (`vendor/gpui/src/platform/mac/window.rs`), so handing it an absolute
///   coordinate would double the offset and throw the modal onto the wrong
///   screen — or off the desktop entirely.
/// * **Windows and X11** take the origin **absolute**, in the same virtual
///   desktop space `display.bounds()` reports.
///
/// Note this conversion applies only to window *creation*. The post-resize
/// move in `app.rs` goes through `platform::set_window_origin`, which drives
/// `setFrameTopLeftPoint` / `ConfigureWindow` / `SetWindowPos` — all of which
/// are absolute on every platform — so that path keeps using
/// [`modal_origin`]'s output unchanged.
fn to_window_origin(absolute: Point<Pixels>, display: Bounds<Pixels>) -> Point<Pixels> {
    #[cfg(target_os = "macos")]
    {
        point(absolute.x - display.origin.x, absolute.y - display.origin.y)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = display;
        absolute
    }
}

/// The modal's full bounds at open: [`modal_size`] anchored at
/// [`modal_origin`] for the full [`MODAL_H`], plus the id of the display it
/// landed on. Falls back to screen-centred when there is no display at all.
///
/// The returned id must be passed to `WindowOptions::display_id`, not just
/// stored: without it GPUI validates and places the window against the
/// *primary* monitor. On Windows that is load-bearing — `open_window` checks
/// the requested bounds against that monitor and silently substitutes its
/// default (centred) bounds when they don't fit, which is precisely what a
/// secondary-monitor origin looks like.
pub fn modal_bounds(
    cx: &mut App,
    tray: Option<TrayAnchor>,
    anchor: Anchor,
) -> (Bounds<Pixels>, Option<DisplayId>) {
    let size = modal_size();
    let Some((id, display)) = modal_display(cx, tray) else {
        return (Bounds::centered(None, size, cx), None);
    };
    let absolute = modal_origin(display, tray, MODAL_H, anchor);
    (
        Bounds::new(to_window_origin(absolute, display), size),
        Some(id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn display(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn window_origin_is_absolute_off_macos() {
        // Windows and X11 place windows in virtual-desktop coordinates, so a
        // secondary monitor at x=1920 keeps its absolute origin.
        let secondary = display(1920.0, 0.0, 1920.0, 1080.0);
        let absolute = point(px(2400.0), px(900.0));
        let converted = to_window_origin(absolute, secondary);
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(converted, absolute);
        }
        // macOS re-adds the screen frame origin inside `open_window`, so what
        // we hand it must be relative to that screen.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(converted, point(px(480.0), px(900.0)));
        }
    }

    #[test]
    fn window_origin_round_trips_on_the_primary_display() {
        // The primary display starts at the virtual origin, so both
        // conventions agree there — which is why the difference stays
        // invisible until a second monitor is plugged in.
        let primary = display(0.0, 0.0, 2560.0, 1440.0);
        let absolute = point(px(2032.0), px(1200.0));
        assert_eq!(to_window_origin(absolute, primary), absolute);
    }

    #[test]
    fn anchor_parsing_falls_back_to_the_os_default() {
        assert_eq!(Anchor::from_config("bottom"), Anchor::Bottom);
        assert_eq!(Anchor::from_config("  TOP "), Anchor::Top);
        assert_eq!(Anchor::from_config("none"), Anchor::None);
        // Legacy value from older configs, plus outright nonsense.
        assert_eq!(Anchor::from_config("auto"), Anchor::os_default());
        assert_eq!(Anchor::from_config(""), Anchor::os_default());
    }

    #[test]
    fn only_the_bottom_anchor_repositions_after_a_resize() {
        assert!(Anchor::Bottom.needs_reposition());
        assert!(!Anchor::Top.needs_reposition());
        assert!(!Anchor::None.needs_reposition());
    }
}
