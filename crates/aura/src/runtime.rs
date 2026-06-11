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
use std::sync::{Arc, Mutex, OnceLock};

use aura_core::config::AppConfig;
use aura_core::net::fleet::FleetState;

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

// ── Fleet handles ─────────────────────────────────────────────────────────────
//
// Fleet runs at the *process* level (see the `FleetManager` in `main.rs`), not
// tied to the modal window — so it keeps publishing / polling with the modal
// closed. The modal view, when open, only *reads* peers for rendering and
// *signals* the manager to reconcile after pairing/leaving. These two handles
// are the whole bridge: a shared `FleetState` for reads, and a dirty flag for
// the signal.

/// Shared `FleetState` published by the process-level Fleet manager. `Some`
/// while the manager has a running [`aura_core::net::FleetSync`] (fleet enabled
/// **and** paired); `None` when fleet is disabled or unpaired. The modal's
/// `render_fleet` locks this only to read peers for one render.
static FLEET_STATE: OnceLock<Mutex<Option<Arc<Mutex<FleetState>>>>> = OnceLock::new();

/// Set when the modal's Pair/Leave actions change the keychain secret, so the
/// process-level manager re-reconciles on its next poll tick — (re)starting or
/// stopping the sync without needing the user to reopen the modal via a tray
/// `Show`.
static FLEET_DIRTY: AtomicBool = AtomicBool::new(false);

fn fleet_state_slot() -> &'static Mutex<Option<Arc<Mutex<FleetState>>>> {
    FLEET_STATE.get_or_init(|| Mutex::new(None))
}

/// Latest shared `FleetState` from the process-level manager, or `None` when no
/// sync is running. Cloning the `Arc` is cheap; callers lock the inner mutex
/// only to read for a single render.
pub fn fleet_state() -> Option<Arc<Mutex<FleetState>>> {
    fleet_state_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Publish the running sync's `FleetState` so the modal can read peers. Called
/// by the manager right after spawning a [`aura_core::net::FleetSync`].
pub fn set_fleet_state(state: Arc<Mutex<FleetState>>) {
    *fleet_state_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
}

/// Drop the shared `FleetState` (fleet disabled or unpaired). After this,
/// `fleet_state()` returns `None` and the modal shows the enable-and-pair state.
pub fn clear_fleet_state() {
    *fleet_state_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Signal the process-level manager to reconcile on its next poll tick. The
/// modal's Pair/Leave actions call this after mutating the keychain secret.
pub fn mark_fleet_dirty() {
    FLEET_DIRTY.store(true, Ordering::Relaxed);
}

/// Consume the dirty flag, returning whether a reconcile is pending. The
/// manager calls this each tick and reconciles when it returns `true`.
pub fn take_fleet_dirty() -> bool {
    FLEET_DIRTY.swap(false, Ordering::Relaxed)
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
