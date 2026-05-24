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

use std::sync::atomic::{AtomicBool, Ordering};

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

/// Returns the latest snapshot of `display.dismiss_on_focus_loss`.
pub fn dismiss_on_focus_loss() -> bool {
    DISMISS_ON_FOCUS_LOSS.load(Ordering::Relaxed)
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
