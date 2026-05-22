//! macOS lifecycle driver.
//!
//! Aura ships as a tray-only app on macOS (see the gpui-exclusion comment
//! in `crates/aura/Cargo.toml`). This module replaces the gpui event loop
//! with a minimal AppKit one:
//!
//! 1. Set the activation policy to `Accessory` so the app lives in the
//!    menu bar and never claims a Dock slot.
//! 2. Pump the main run loop in short 150 ms bursts so AppKit can deliver
//!    `NSStatusItem` clicks and menu actions to `tray-icon`.
//! 3. Drain `tray::try_recv_event()` between bursts. `Show` is a no-op
//!    for now (no native modal yet); `Quit` exits the loop and the
//!    process returns cleanly.
//!
//! We deliberately don't call `NSApplication::run()` — it would block
//! forever and there's no convenient way to interleave non-Objective-C
//! work without scheduling a block-based `NSTimer`, which would pull in
//! the `block2` dep just to fire a closure every 150 ms. The pump-and-
//! drain pattern keeps the surface area small.

use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode, NSRunLoop};

use crate::tray::{self, TrayEvent};

/// How long each AppKit pump runs before we drain the tray event channel.
/// Matches the GPUI-side `MENU_POLL_INTERVAL` so the click → response
/// latency is identical across platforms.
const POLL_INTERVAL_SECS: f64 = 0.150;

pub fn run_event_loop() {
    // `MainThreadMarker::new()` succeeds only on the AppKit main thread.
    // `fn main()` runs there by definition, so a missing marker means the
    // caller wired the lifecycle up wrong — fail loud.
    let mtm = MainThreadMarker::new()
        .expect("aura::macos::run_event_loop must be called from the main thread");

    let app = NSApplication::sharedApplication(mtm);
    // Returns `false` only on macOS < 10.9 when downgrading away from
    // Regular — we never do that, and we're far past 10.9, so the result
    // is uninteresting.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let run_loop = NSRunLoop::currentRunLoop();
    // SAFETY: `NSDefaultRunLoopMode` is a static Foundation string
    // constant. The `unsafe` is purely for accessing an extern static.
    let mode = unsafe { NSDefaultRunLoopMode };

    loop {
        // Drain anything the tray pushed since the last pump. Quit short-
        // circuits the loop so we don't wait another 150 ms before exit.
        let mut should_quit = false;
        while let Some(event) = tray::try_recv_event() {
            match event {
                // No modal UI on macOS yet — gpui 0.2.2 panics on macOS 26
                // (see issue #4). The tray icon stays visible and exposes
                // Quit; rich UI returns once we ship a native modal.
                TrayEvent::Show { .. } => {}
                TrayEvent::Quit => {
                    should_quit = true;
                    break;
                }
            }
        }
        if should_quit {
            return;
        }

        // Service AppKit input sources (status-item clicks, menu actions,
        // etc.) for up to POLL_INTERVAL_SECS. `runMode:beforeDate:` blocks
        // until the deadline OR an event fires, whichever is sooner — so
        // an idle app sleeps the full interval, and a clicky one wakes
        // promptly.
        let until = NSDate::dateWithTimeIntervalSinceNow(POLL_INTERVAL_SECS);
        run_loop.runMode_beforeDate(mode, &until);
    }
}
