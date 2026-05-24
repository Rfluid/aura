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

/// Returns the latest snapshot of `display.dismiss_on_focus_loss`.
pub fn dismiss_on_focus_loss() -> bool {
    DISMISS_ON_FOCUS_LOSS.load(Ordering::Relaxed)
}

/// Push every shared-config field out of `config` into its atomic. Call
/// this whenever a fresh `AppConfig` lands — at startup, on each tray
/// click (before opening the modal), and at the end of every refresh.
pub fn set_from_config(config: &AppConfig) {
    DISMISS_ON_FOCUS_LOSS.store(config.display.dismiss_on_focus_loss, Ordering::Relaxed);
}
