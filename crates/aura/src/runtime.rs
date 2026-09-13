//! Cross-cutting runtime state shared by `main.rs` (tray poll loop) and
//! `app.rs` (the view's refresh task).
//!
//! The poll loop in `main()` and the in-modal "Refresh" task each reload
//! `AppConfig` from disk independently — without a shared bus they'd drift
//! and the user would see e.g. the modal honour a new
//! `dismiss_on_focus_loss` value while the tray loop kept using the
//! startup snapshot. Funnelling both reload paths through
//! [`set_from_config`] keeps every consumer in sync without threading
//! `Arc<...>` channels through every callback.
//!
//! Add a new atomic / accessor pair here when another `[display]` knob
//! needs to be visible to both the modal view and the background loop.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use aura_core::config::AppConfig;

/// Mirrors `AppConfig.display.dismiss_on_focus_loss`. The poll loop
/// reads it every 150 ms; the modal's refresh task updates it whenever
/// the user clicks the refresh icon.
static DISMISS_ON_FOCUS_LOSS: AtomicBool = AtomicBool::new(true);

/// Mirrors `AppConfig.display.show_in_app_switcher`. Used by main.rs
/// when opening the modal (picks `WindowKind`) and as the source of
/// truth for the macOS process-wide NSApp activation policy applied at
/// startup and on every refresh.
static SHOW_IN_APP_SWITCHER: AtomicBool = AtomicBool::new(false);

/// Set when the user presses Escape with the modal open and nothing else
/// claimed the key. Drained by the poll loop, which owns the window handle.
///
/// A flag rather than a direct close because the keystroke observer runs with
/// an `&mut App` and no access to the loop's `current` handle — the same
/// reason tray clicks take the long way round through a channel.
static DISMISS_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True while a plugin button action is executing. Plugin actions may
/// open dialogs (e.g. native file pickers) that take focus away from the
/// modal; the poll loop's focus-loss check must not dismiss the modal
/// mid-action or the user returns from the picker to a closed window.
static PLUGIN_ACTION_INFLIGHT: AtomicBool = AtomicBool::new(false);

/// Height the modal's content measured at the end of the last open, in whole
/// logical pixels. Zero means "not measured yet in this process".
///
/// The modal opens at `placement::MODAL_H` and the auto-fit pass shrinks it to
/// fit one frame later, which on X11 shows up as the window visibly jumping
/// from a full-height rect to its final one. Reopening at the height the
/// content actually settled at removes that jump for every open after the
/// first: the window is already the right size, so the auto-fit pass has
/// nothing to correct.
///
/// Deliberately process-local rather than persisted to `AppState` — it is a
/// paint-smoothing hint, not state worth surviving a restart, and it must not
/// outlive a config or theme change that alters the content height.
static LAST_MODAL_HEIGHT: AtomicU32 = AtomicU32::new(0);

/// Height to open the modal at, or `None` before the first measurement.
pub fn last_modal_height() -> Option<f32> {
    match LAST_MODAL_HEIGHT.load(Ordering::Relaxed) {
        0 => None,
        h => Some(h as f32),
    }
}

/// Record the content height the auto-fit pass settled on. See
/// [`LAST_MODAL_HEIGHT`].
pub fn set_last_modal_height(height: f32) {
    if height.is_finite() && height >= 1.0 {
        LAST_MODAL_HEIGHT.store(height.round() as u32, Ordering::Relaxed);
    }
}

/// Returns the latest snapshot of `display.dismiss_on_focus_loss`.
pub fn dismiss_on_focus_loss() -> bool {
    DISMISS_ON_FOCUS_LOSS.load(Ordering::Relaxed)
}

/// Ask the poll loop to close the modal (Escape was pressed).
pub fn request_dismiss() {
    DISMISS_REQUESTED.store(true, Ordering::Relaxed);
}

/// Consume a pending dismiss request, if any.
pub fn take_dismiss_request() -> bool {
    DISMISS_REQUESTED.swap(false, Ordering::Relaxed)
}

/// See [`PLUGIN_ACTION_INFLIGHT`].
pub fn plugin_action_inflight() -> bool {
    PLUGIN_ACTION_INFLIGHT.load(Ordering::Relaxed)
}

/// Set by `AuraView::run_plugin_action` for the duration of the action.
pub fn set_plugin_action_inflight(inflight: bool) {
    PLUGIN_ACTION_INFLIGHT.store(inflight, Ordering::Relaxed);
}

/// Returns the latest snapshot of `display.show_in_app_switcher`.
pub fn show_in_app_switcher() -> bool {
    SHOW_IN_APP_SWITCHER.load(Ordering::Relaxed)
}

/// Push every shared-config field out of `config` into its atomic, then
/// reapply any platform-level state that depends on those fields (today:
/// the macOS NSApp activation policy). Call this whenever a fresh
/// `AppConfig` lands — at startup, on each tray click (before opening
/// the modal), and at the end of every refresh.
pub fn set_from_config(config: &AppConfig) {
    DISMISS_ON_FOCUS_LOSS.store(config.display.dismiss_on_focus_loss, Ordering::Relaxed);
    SHOW_IN_APP_SWITCHER.store(config.display.show_in_app_switcher, Ordering::Relaxed);
    crate::platform::apply_app_switcher_policy(config.display.show_in_app_switcher);
}
