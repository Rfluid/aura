//! macOS-specific runtime tweaks.
//!
//! GPUI hard-codes `setActivationPolicy(Regular)` at startup, so setting
//! `LSUIElement=true` in `Info.plist` alone is not enough to keep Aura
//! out of the Dock. We re-set the policy from the main thread after the
//! GPUI app has activated.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;

/// Switch the running NSApplication to "accessory" — menu-bar only, no
/// Dock icon, no app switcher entry. Must be called from the main thread
/// (inside GPUI's `Application::run` closure).
pub fn set_accessory_activation_policy() {
    // GPUI runs its closure on the main thread, so this assertion always
    // succeeds at the call site — but if a future caller breaks that, we
    // want a clear panic rather than UB.
    let mtm = MainThreadMarker::new()
        .expect("set_accessory_activation_policy must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
