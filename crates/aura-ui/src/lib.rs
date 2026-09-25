//! GPUI tray app and modal for Aura.
//!
//! The `aura` binary calls [`run`] when invoked with no subcommand; every
//! headless subcommand lives in `aura-cli` and never touches this crate's
//! event loop.

mod app;
mod assets;
mod format;
mod keys;
mod placement;
mod platform;
mod runtime;
mod tray;
mod tray_status;
mod updater;
mod work_area;

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use aura_core::{config::AppConfig, state::AppState};
use gpui::{
    prelude::*, Application, TitlebarOptions, WindowBounds, WindowDecorations, WindowHandle,
    WindowKind, WindowOptions,
};
// Only the keepalive window needs these, and Linux doesn't build it.
#[cfg(not(target_os = "linux"))]
use gpui::{div, px, size, Bounds, IntoElement, Render};

use crate::tray::TrayEvent;
use crate::{app::AuraView, assets::EmbeddedAssets};

/// DWM-cloak or -uncloak a window on Windows. Cloaking makes the window
/// invisible to the user (DWM hides it during composition) while it still
/// receives WM_PAINT and renders normally — used to hide the first-frame
/// resize flash (the window opens at its remembered height — MODAL_H on the
/// first open of the process — and the auto-fit pass corrects it on the next
/// frame; without cloaking the user sees a one-frame flicker).
#[cfg(target_os = "windows")]
pub(crate) fn win32_set_cloak(window: &gpui::Window, cloak: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};

    // Use fully-qualified syntax: Window has an inherent window_handle() that
    // returns AnyWindowHandle; we want the raw_window_handle trait method.
    let wh = match <gpui::Window as HasWindowHandle>::window_handle(window) {
        Ok(wh) => wh,
        Err(_) => return,
    };
    let RawWindowHandle::Win32(h) = wh.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as usize as *mut _);
    // pvAttribute is a pointer to a BOOL (i32, 4 bytes): 1 = cloak, 0 = uncloak.
    let val: i32 = cloak as i32;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::addr_of!(val).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    };
}

/// How often the GPUI main thread checks for pending tray menu events.
/// 150 ms is well under the human "instant" threshold (~200 ms) for the
/// click → modal latency while costing essentially nothing CPU-wise.
const MENU_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Version reported by the modal and compared against GitHub releases.
/// Set once by [`run`] from the `aura` binary's own `CARGO_PKG_VERSION`, so
/// it tracks the release tag rather than this library crate's version.
static APP_VERSION: OnceLock<&'static str> = OnceLock::new();

/// The running Aura release version. Falls back to this crate's version when
/// [`run`] hasn't set it (unit tests).
pub(crate) fn app_version() -> &'static str {
    APP_VERSION
        .get()
        .copied()
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Tray entry point. `version` is the release version of the `aura` binary,
/// shown in the modal and used by the update check.
pub fn run(version: &'static str) -> Result<()> {
    let _ = APP_VERSION.set(version);

    // Single-instance guard: if another Aura is already running, ping it
    // (see `platform::try_recv_activation` below) and exit. The lock is
    // held (intentionally leaked) for the lifetime of the process; the OS
    // releases it on exit. See `platform::acquire_single_instance`.
    if !platform::acquire_single_instance() {
        return Ok(());
    }

    // ── Load config ───────────────────────────────────────────────────────────
    //
    // `AppState` is *not* loaded here on purpose — `toggle_window` reloads it
    // from disk each time the modal opens, so a profile change made in one
    // session is visible the next time the user clicks the tray icon.
    //
    // `AppConfig` is also reloaded on every tray click (see the `Show` arm
    // below) and on every Refresh-button click (see `app::do_refresh`).
    // The shared `runtime` module mirrors a handful of `[display]` fields
    // into atomics so both reload paths keep `run`'s tray loop in sync
    // with the modal view.
    let config_path = AppConfig::default_path();
    let config = AppConfig::load_with_discovery(&config_path)?;
    runtime::set_from_config(&config);

    // Seed the height the modal opens at from the last session, so the first
    // open is placed as well as every later one — see
    // `runtime::seed_modal_height`. A failure here is not worth reporting:
    // `toggle_window` falls back to `placement::MODAL_H` and the auto-fit
    // takes over from there.
    let mut persisted_modal_height = AppState::load().ok().and_then(|s| s.modal_height);
    runtime::seed_modal_height(persisted_modal_height);

    // Start the sponsor nudge's one-week clock the first time the tray runs
    // (including the first launch after upgrading from a build that never
    // recorded it — see `sponsor::record_first_run`). Only written when the
    // stamp is missing, and read-modify-write so nothing else is clobbered.
    // A state file that fails to parse is left alone rather than overwritten
    // with defaults; the nudge simply waits until it reads again.
    if let Ok(mut state) = AppState::load() {
        if aura_core::sponsor::record_first_run(&mut state, chrono::Utc::now()) {
            if let Err(e) = state.save() {
                eprintln!("aura: could not record the first run: {e}");
            }
        }
    }

    // ── Install tray icon ─────────────────────────────────────────────────────
    //
    // Failure is not fatal, but it *is* serious: the tray icon is Aura's only
    // entry point, so a process that keeps running without one is invisible —
    // no icon, no window, and nothing to click to get either. We therefore
    // both shout on stderr (which lands in the journal / launchd log) and set
    // a flag that makes the run loop open the modal once, so the user gets a
    // window instead of silence.
    //
    // On Linux this path is now much rarer than it was: `tray::install` asks
    // ksni to treat a missing StatusNotifierWatcher as a soft error and keep
    // retrying, which covers both "the panel hasn't claimed the bus name yet"
    // at login and "SNI support was enabled after the fact".
    let tray = match tray::install() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!(
                "aura: could not install the tray icon: {e}\n\
                 aura: opening the window directly — this session has no icon to click. \
                 Re-run `aura` (or use the app-menu entry) to bring the window back."
            );
            None
        }
    };
    let tray_missing = tray.is_none();

    // Background indicator refresh. Runs on its own thread (the quota lookup
    // is blocking I/O), pushes into `tray::set_status`, and is drained on the
    // main thread by the poll loop below. Disabled by `tray.indicator`.
    if let Some(interval) = config.tray.refresh_interval() {
        tray_status::spawn_poll(config_path.clone(), interval);
    }

    // ── Launch GPUI app ───────────────────────────────────────────────────────
    //
    // No user-visible window is opened at startup, and the tray has to
    // outlive every modal open/close cycle. The two platform families get
    // there differently:
    //
    // * Linux: GPUI's Wayland and X11 clients stop the event loop the
    //   moment `state.windows.is_empty()`. We opt out of that with
    //   `set_quit_on_last_window_closed(false)` (our vendored patch) and
    //   open no window at all, so the compositor never sees a stray
    //   surface from us.
    // * macOS / Windows: the platform keeps the process alive by itself,
    //   but GPUI still needs a window to hang the run loop off, so we
    //   open the hidden keepalive described on `open_keepalive_window`.
    //
    // On Linux the backend GPUI picks is not incidental: Wayland forbids a
    // client from positioning its own toplevel, which silently disables
    // `window.anchor` and every other placement decision Aura makes. The
    // guard below applies `window.linux_backend` for exactly the duration of
    // `Application::new()` (see `platform::select_display_backend`) and then
    // puts the environment back, so child processes are unaffected.
    let app = {
        let _backend = platform::select_display_backend(&config.window.linux_backend);
        Application::new().with_assets(EmbeddedAssets)
    };
    app.run(move |cx| {
        // Selectable labels deliberately have no focus handle, so install
        // the crate's observer-based bridge for copy/select-all and
        // shift+arrow extension when no focused control claimed the key.
        //
        // Escape is handled here instead of by the bridge
        // (`clear_on_escape: false`) because the two meanings have to be
        // ordered: Escape clears a live text selection, and only closes
        // the popup when there is nothing to clear. Leaving both to fire
        // as independent keystroke observers would make the outcome depend
        // on subscriber iteration order — one Escape could clear *and*
        // close.
        gpui_selectable_text::register_keyboard_bridge_with(
            cx,
            gpui_selectable_text::KeyboardBridge {
                clear_on_escape: false,
                ..Default::default()
            },
        )
        .detach();
        //
        // With the keymap installed (`keybindings.enabled`), Escape is an
        // ordinary binding (`dismiss` / `close_overlay`) that does the same
        // ordering itself, and that the user may remap or unbind — so this
        // observer only covers the keymap-off case.
        cx.observe_keystrokes(|event, window, cx| {
            // Something with focus already claimed this keystroke.
            if event.action.is_some() || runtime::keybindings_active() {
                return;
            }
            let keystroke = &event.keystroke;
            if keystroke.key != "escape" || keystroke.modifiers.modified() {
                return;
            }
            if gpui_selectable_text::registry::clear_active_selection(window, cx) {
                return;
            }
            // Closing a tray popup with Escape is the convention on every
            // desktop; the poll loop does the actual teardown because it
            // owns the window handle.
            runtime::request_dismiss();
        })
        .detach();

        // GPUI forces NSApplicationActivationPolicyRegular in
        // did_finish_launching; reapply the user's preference here so it
        // sticks. `runtime::set_from_config` (called at startup before
        // .run) only fires once, *before* GPUI launches — without this
        // second push, the user's Accessory choice would be overwritten
        // by the time we hit the run closure on macOS.
        platform::apply_app_switcher_policy(runtime::show_in_app_switcher());

        // Hold the handle in the move-closure so it isn't dropped.
        #[cfg(not(target_os = "linux"))]
        let _keepalive = open_keepalive_window(cx);
        // No handle to hold on Linux — nothing is opened; the loop is
        // kept alive by the opt-out instead of by a window.
        #[cfg(target_os = "linux")]
        cx.set_quit_on_last_window_closed(false);

        let config = config.clone();
        let config_path = config_path.clone();

        cx.spawn(async move |cx| {
            // Owned here so the icon lives exactly as long as the loop
            // that drives it — and so the loop can push status updates
            // into it. `TrayHandle` is `!Send` on macOS / Windows (it
            // wraps an AppKit / Win32 object); GPUI's foreground executor
            // has no `Send` bound, which is what makes this legal.
            let mut tray = tray;

            // The currently-open window, if any. We toggle on each
            // "Show Aura" click: open if closed, close if open.
            let mut current: Option<WindowHandle<AuraView>> = None;

            // macOS: NSEvent global monitor flag. Accessory apps can't
            // reliably set [NSApp mainWindow], which kills cx.active_window
            // detection; the global monitor is the working alternative.
            #[cfg(target_os = "macos")]
            let outside_clicked = Arc::new(AtomicBool::new(false));
            #[cfg(target_os = "macos")]
            let mut click_monitor: Option<platform::ClickOutsideMonitor> = None;

            // Grace-period counter: skip focus-loss checks for this many
            // poll intervals after opening the modal so the platform
            // can finish delivering focus / setting up the monitor before
            // we start watching for losses.
            let mut just_opened: u8 = 0;

            // When the tray never installed, the user has no way to ask
            // for the window — so ask on their behalf, once.
            if tray_missing {
                current = toggle(cx, None, config.clone(), config_path.clone(), None).await;
                if current.is_some() {
                    just_opened = 4;
                }
            }

            loop {
                // Poll: ksni / tray-icon both expose blocking
                // crossbeam channels under the hood, so we drain
                // them between short sleeps.
                cx.background_executor().timer(MENU_POLL_INTERVAL).await;

                // Apply any indicator state queued since the last tick.
                // Must happen on this thread: AppKit refuses NSStatusItem
                // mutation from anywhere but the main thread.
                if let Some(tray) = tray.as_mut() {
                    tray.apply_pending_status();
                }

                // Write back the height the content settled at, so the next
                // session's first open is placed correctly too (see
                // `runtime::seed_modal_height`). Change-gated, and only real
                // content is ever recorded (the auto-fit skips placeholder
                // measurements), so this is one small write per open at most —
                // not one per resize, and nothing at all while the modal sits
                // open. Deliberately not deferred to the modal closing: a
                // session that ends with the window still up would save
                // nothing. Read-modify-write, so a profile the user picked in
                // the modal isn't clobbered.
                if let Some(height) = runtime::modal_height_to_persist(persisted_modal_height) {
                    let mut state = AppState::load().unwrap_or_default();
                    state.modal_height = Some(height);
                    if let Err(e) = state.save() {
                        eprintln!("aura: could not save the modal height: {e}");
                    }
                    // Either way, stop trying: a disk that refused once will
                    // refuse every 150ms, and the value still serves this
                    // session from memory.
                    persisted_modal_height = Some(height);
                }

                // Escape, routed here from the keystroke observer in the
                // run closure. Unconditional: unlike focus loss this is an
                // explicit "close it" from the user, so neither
                // `dismiss_on_focus_loss` nor an in-flight plugin action
                // suppresses it.
                if runtime::take_dismiss_request() {
                    #[cfg(target_os = "macos")]
                    if current.is_some() {
                        outside_clicked.store(false, Ordering::Relaxed);
                        if let Some(m) = click_monitor.take() {
                            platform::remove_click_outside_monitor(m);
                        }
                        if !runtime::show_in_app_switcher() {
                            platform::apply_app_switcher_policy(false);
                        }
                    }
                    if let Some(handle) = current.take() {
                        let _ = cx.update(|cx| {
                            let _ = handle.update(cx, |_view, window, _cx| window.remove_window());
                        });
                    }
                }

                if runtime::dismiss_on_focus_loss()
                    && current.is_some()
                    && !runtime::plugin_action_inflight()
                {
                    let lost_focus = if just_opened > 0 {
                        just_opened -= 1;
                        false
                    } else {
                        #[cfg(target_os = "macos")]
                        {
                            outside_clicked.load(Ordering::Relaxed)
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            cx.update(|cx| cx.active_window().is_none())
                                .unwrap_or(false)
                        }
                    };

                    if lost_focus {
                        #[cfg(target_os = "macos")]
                        {
                            outside_clicked.store(false, Ordering::Relaxed);
                            if let Some(m) = click_monitor.take() {
                                platform::remove_click_outside_monitor(m);
                            }
                            if !runtime::show_in_app_switcher() {
                                platform::apply_app_switcher_policy(false);
                            }
                        }
                        if let Some(handle) = current.take() {
                            let _ = cx.update(|cx| {
                                let _ =
                                    handle.update(cx, |_view, window, _cx| window.remove_window());
                            });
                        }
                    }
                }

                // A second `aura` launch (e.g. from the app-search
                // launcher) lost the single-instance race and pinged us
                // instead of silently exiting into nothing. Treat it as
                // "show the window" — but don't toggle an already-open
                // one closed the way a tray click would; just focus it.
                if platform::try_recv_activation() {
                    if let Some(handle) = &current {
                        let _ = cx.update(|cx| {
                            let _ =
                                handle.update(cx, |_view, window, _cx| window.activate_window());
                        });
                    } else {
                        let fresh_config = AppConfig::load_with_discovery(&config_path)
                            .unwrap_or_else(|e| {
                                eprintln!(
                                    "aura: config reload failed ({e}); using cached snapshot"
                                );
                                config.clone()
                            });
                        runtime::set_from_config(&fresh_config);

                        current = toggle(cx, None, fresh_config, config_path.clone(), None).await;

                        if current.is_some() {
                            just_opened = 4; // ~600 ms at 150 ms/poll
                            #[cfg(target_os = "macos")]
                            {
                                outside_clicked.store(false, Ordering::Relaxed);
                                click_monitor = Some(platform::install_click_outside_monitor(
                                    Arc::clone(&outside_clicked),
                                ));
                            }
                        }
                    }
                }

                while let Some(event) = tray::try_recv_event() {
                    match event {
                        TrayEvent::Show { anchor } => {
                            // Reload AppConfig from disk so edits made
                            // since the last open (whether via the
                            // settings panel, an external editor, or
                            // `aura plugin add`) take effect on this
                            // open. Fall back to the startup snapshot
                            // if the reload fails so a transient I/O
                            // error doesn't break the toggle.
                            let fresh_config = AppConfig::load_with_discovery(&config_path)
                                .unwrap_or_else(|e| {
                                    eprintln!(
                                        "aura: config reload failed ({e}); using cached snapshot"
                                    );
                                    config.clone()
                                });
                            runtime::set_from_config(&fresh_config);

                            // If a window was open, tear down its monitor
                            // and demote the activation policy before the
                            // toggle (which closes it).
                            #[cfg(target_os = "macos")]
                            if current.is_some() {
                                if let Some(m) = click_monitor.take() {
                                    platform::remove_click_outside_monitor(m);
                                }
                                if !runtime::show_in_app_switcher() {
                                    platform::apply_app_switcher_policy(false);
                                }
                            }

                            current = toggle(
                                cx,
                                current.take(),
                                fresh_config,
                                config_path.clone(),
                                anchor,
                            )
                            .await;

                            if current.is_some() {
                                just_opened = 4; // ~600 ms at 150 ms/poll
                                                 // macOS: install the click-outside
                                                 // monitor for the new window.
                                #[cfg(target_os = "macos")]
                                {
                                    outside_clicked.store(false, Ordering::Relaxed);
                                    click_monitor = Some(platform::install_click_outside_monitor(
                                        Arc::clone(&outside_clicked),
                                    ));
                                }
                            }
                        }
                        TrayEvent::OpenConfig => {
                            open_config_file(&config_path);
                        }
                        TrayEvent::OpenConfigTutorial => {
                            platform::open_url(app::CONFIG_TUTORIAL_URL);
                        }
                        TrayEvent::Quit => {
                            // Explicit user exit from the right-click
                            // menu. cx.quit() tears down the GPUI
                            // event loop; aura exits cleanly so
                            // systemd's Restart=on-failure won't
                            // respawn us.
                            let _ = cx.update(|cx| cx.quit());
                            return;
                        }
                    }
                }
            }
        })
        .detach();
    });

    Ok(())
}

fn open_config_file(config_path: &std::path::Path) {
    if !config_path.exists() {
        if let Err(e) = AppConfig::load(config_path) {
            eprintln!("aura: could not create config before opening it: {e}");
            return;
        }
    }
    platform::open_path(config_path);
}

/// Empty root view for the hidden keepalive window. The view is never
/// rendered to a screen — its only job is to satisfy `open_window`'s
/// `V: Render` bound so the window can exist in `state.windows`.
#[cfg(not(target_os = "linux"))]
struct KeepAliveView;

#[cfg(not(target_os = "linux"))]
impl Render for KeepAliveView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        div()
    }
}

/// Open the always-present keepalive window (macOS and Windows only).
/// See the call site for why GPUI needs it. Failures are non-fatal but
/// logged: if the keepalive can't open, aura will still work — just with
/// the old "process exits on last window close" behaviour.
///
/// Linux doesn't build this at all. It used to, and the surface was a
/// steady source of trouble: GPUI's Wayland backend silently ignores
/// `show: false` (it creates an xdg_toplevel and commits the surface
/// unconditionally), so KWin treated the 1×1 keepalive as a real window
/// — decorating it, listing it, and, because `on_window_should_close`
/// refuses every close request, naming it under "The following
/// applications did not close" on the logout screen, where it blocked
/// shutdown for two minutes. Opting out of GPUI's quit-on-last-window
/// rule instead means there is no surface for the compositor to find.
///
/// What's left here keeps that history in mind, since the same window is
/// still created on the other two platforms:
///
/// * open it at `(-9999, -9999)` so even if the platform doesn't clamp
///   it back on-screen, the user can't accidentally focus or click it;
/// * `minimize_window()` it immediately (except on Windows, where
///   SW_MINIMIZE would force a hidden window visible);
/// * give it a distinct `app_id` ("aura-keepalive") so a task manager
///   doesn't group it under the main "Aura" entry;
/// * give it a human-readable title ("Aura"), because window lists and
///   session managers surface titles in places we don't control and an
///   untitled entry tells the user nothing. `WindowOptions::titlebar`
///   can't do this for us — setting it would also change the window's
///   decoration behaviour — so we call `set_window_title` after open,
///   which routes through the per-platform `set_title`;
/// * intercept every platform-level close request with
///   `on_window_should_close` returning `false`, so the tray can't be
///   killed by a stray click. Our own `toggle()` uses
///   `window.remove_window()`, which bypasses this guard (it's an
///   internal close, not a platform request).
#[cfg(not(target_os = "linux"))]
fn open_keepalive_window(cx: &mut gpui::App) -> Option<WindowHandle<KeepAliveView>> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(-9999.), px(-9999.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        // On Windows use PopUp (WS_EX_TOOLWINDOW) so the hidden keepalive
        // doesn't create a taskbar button. Normal (WS_EX_APPWINDOW) is fine
        // on other platforms where the window is never surfaced to the user.
        #[cfg(target_os = "windows")]
        kind: WindowKind::PopUp,
        #[cfg(not(target_os = "windows"))]
        kind: WindowKind::Normal,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("aura-keepalive".into()),
        ..Default::default()
    };

    match cx.open_window(opts, |_window, cx| cx.new(|_| KeepAliveView)) {
        Ok(handle) => {
            // Best-effort hide + lock. The `update` returns Err only if
            // the window vanished between open and now (shouldn't happen);
            // either way we return the handle so the caller's reference
            // keeps the keepalive alive.
            let _ = handle.update(cx, |_view, window, cx| {
                // Name the surface before anything else so compositors and
                // session managers have it from the first commit. See the
                // doc comment: an untitled keepalive shows up as a nameless
                // entry in KDE's "these applications did not close" list.
                window.set_window_title("Aura");
                window.on_window_should_close(cx, |_, _| false);
                // On Wayland, `show: false` is ignored — the compositor
                // creates a surface unconditionally. Minimize immediately so
                // KDE places it in the taskbar overflow instead of the
                // desktop. On Windows, SW_MINIMIZE on a hidden window would
                // force it visible (minimized), so skip this call there.
                #[cfg(not(target_os = "windows"))]
                window.minimize_window();
            });
            Some(handle)
        }
        Err(e) => {
            eprintln!("warning: failed to open keepalive window: {e}");
            None
        }
    }
}

/// If `existing` is alive, close it and return `None`; otherwise open a
/// fresh window and return its handle. Called from both the tray "Show"
/// menu item and a primary-click on the tray icon — each click flips
/// modal visibility.
async fn toggle(
    cx: &gpui::AsyncApp,
    existing: Option<WindowHandle<AuraView>>,
    config: AppConfig,
    config_path: std::path::PathBuf,
    tray_anchor: Option<tray::TrayAnchor>,
) -> Option<WindowHandle<AuraView>> {
    cx.update(move |cx| toggle_window(cx, existing, config, config_path, tray_anchor))
        .ok()
        .flatten()
}

// Modal geometry (size + anchor math) lives in `placement.rs` — the single
// source of truth shared by `toggle_window` (open) and `app.rs`'s auto-fit
// reposition. See that module for the per-OS anchoring rules.

fn toggle_window(
    cx: &mut gpui::App,
    existing: Option<WindowHandle<AuraView>>,
    config: AppConfig,
    config_path: std::path::PathBuf,
    tray_anchor: Option<tray::TrayAnchor>,
) -> Option<WindowHandle<AuraView>> {
    if let Some(handle) = existing {
        // `update` returns Err if the window has already been removed;
        // either way we're done with this handle.
        let _ = handle.update(cx, |_view, window, _cx| window.remove_window());
        return None;
    }

    // Reload AppState from disk so the active profile reflects what the user
    // picked in any prior modal session. The process keeps running between
    // modal open/close cycles (see the keepalive window), so a snapshot
    // loaded once at startup would go stale on the first profile change.
    let state = AppState::load().unwrap_or_else(|e| {
        eprintln!("aura: could not reload state, using defaults: {e}");
        AppState::default()
    });

    // Same reasoning for the keymap: re-read `keybindings.toml` on every open
    // so an edit applies without a restart.
    let keymap = aura_core::keymap::Keymap::load(&aura_core::keymap::Keymap::default_path());
    keys::install(cx, &keymap, config.keybindings.enabled);

    let anchor = placement::Anchor::from_config(&config.window.anchor);
    // `display_id` rides along to `AuraView` so the auto-fit callback caps the
    // modal's height against the screen it actually opened on. Reading
    // `primary_display()` there instead would measure the wrong taskbar the
    // moment the tray lives on a secondary monitor.
    // Open at the height the content settled at last time, so the auto-fit
    // pass has nothing to correct and the window doesn't visibly jump one
    // frame after it appears. `MODAL_H` on the first open of the process.
    let open_h = runtime::last_modal_height().unwrap_or(placement::MODAL_H);
    let (bounds, display_id) = placement::modal_bounds(cx, tray_anchor, anchor, open_h);
    // `window.show_in_app_switcher` controls whether the modal appears in
    // the OS's "where are my windows" surfaces — Cmd+Tab + Dock on macOS,
    // Alt+Tab + taskbar on Windows, panel + window switcher on Linux.
    //
    // Linux / Windows:
    //   - true  → WindowKind::Normal (xdg_toplevel / WS_EX_APPWINDOW).
    //   - false → WindowKind::PopUp  (no taskbar entry, WS_EX_TOOLWINDOW).
    //
    // `WindowKind::PopUp` also strips chrome on every backend: Windows applies
    // `WS_EX_TOOLWINDOW` + `WINDOW_STYLE(0x0)` (no caption, no resize frame),
    // and X11 sets `_NET_WM_WINDOW_TYPE_NOTIFICATION` which tells the WM to
    // drop decorations. So `window_chrome` has to override the kind too —
    // otherwise the titlebar/is_resizable we set below are silently ignored.
    // Side-effect: enabling chrome also puts the modal in the taskbar /
    // alt-tab list, which is consistent with it being a "real" window.
    //
    // macOS: ALWAYS Normal. GPUI maps WindowKind::PopUp to NSPanel with
    // NSWindowStyleMaskNonactivatingPanel, which deliberately prevents the
    // window from becoming key. We need the window to be key so
    // `cx.active_window()` can track focus (the focus-loss check below
    // depends on this). What keeps Aura out of Cmd+Tab on macOS is the
    // NSApplicationActivationPolicy — promoted to Regular only while the
    // modal is open, demoted back to Accessory on close (see
    // `platform::apply_app_switcher_policy` calls).
    #[cfg(target_os = "macos")]
    let kind = WindowKind::Normal;
    #[cfg(not(target_os = "macos"))]
    let kind = if config.window.show_in_app_switcher || config.window.chrome {
        WindowKind::Normal
    } else {
        WindowKind::PopUp
    };
    // `window.chrome` controls only the native title bar:
    //   false (default): chromeless tray-popup, fixed width.
    //   true: native OS chrome (title bar + min/max/close). window_decorations:
    //     Server asks Wayland compositors to draw SSD.
    // Whether the modal auto-fits its content height is a separate axis,
    // governed by `window.auto_resize` in app.rs (see on_children_prepainted) —
    // independent of chrome, so the auto-fit works in both modes.
    //
    // window_decorations must be `Some(..)` in both branches, not `None` for
    // the chromeless case: GPUI's `request_decorations` defaults a `None` to
    // `WindowDecorations::Server` (see gpui `window.rs`). Both Linux backends
    // (this branch runs on X11 and Wayland alike — no per-backend split here)
    // override `request_decorations`, so that default actively asks for
    // server-side chrome: on X11 it writes an explicit `_MOTIF_WM_HINTS`
    // "show decorations" hint, on Wayland it requests SSD via the
    // xdg-decoration protocol. Some WMs/compositors (KWin observed) honor
    // that over the `_NET_WM_WINDOW_TYPE_NOTIFICATION` borderless hint from
    // `WindowKind::PopUp`, so the titlebar reappears even though
    // `window_chrome` is false. Requesting `Client` explicitly writes the
    // "hide decorations" hint/request instead. (macOS and Windows don't
    // override `request_decorations` at all — this field is a no-op there;
    // chrome is controlled by `titlebar`/`kind` alone on those platforms.)
    let (titlebar, is_resizable, window_decorations) = if config.window.chrome {
        (
            Some(TitlebarOptions::default()),
            true,
            Some(WindowDecorations::Server),
        )
    } else {
        (None, false, Some(WindowDecorations::Client))
    };
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar,
        is_resizable,
        window_decorations,
        // Set a stable Wayland app_id / X11 WM_CLASS so KWin window rules
        // (see README "Modal placement on Wayland") can match this surface.
        // Without this, KDE shows "Window class not available" when the
        // user tries to Detect Window Properties on the modal.
        app_id: Some("aura".into()),
        // Target the display the tray icon lives on. Not optional on Windows:
        // `open_window` validates the requested bounds against this display
        // (the primary one when unset) and silently substitutes its centred
        // default when they fall outside — which is what any secondary-monitor
        // origin looks like. macOS also resolves the origin relative to this
        // screen's frame; see `placement::to_window_origin`.
        display_id,
        kind,
        // On macOS, GPUI creates the window with NSTitled|NSFullSizeContentView
        // even when titlebar:None. The native title-bar drag zone covers our
        // header; with is_movable:true the OS handles drags there, which can
        // route mouse events outside GPUI's queue. Disabling movability tells
        // AppKit to forward those clicks to the content view instead, so the
        // header buttons behave like normal content.
        is_movable: false,
        ..Default::default()
    };

    // Cloak (Windows) hides the first-frame flash that only happens when the
    // auto-fit step shrinks the window from its open-time MODAL_H to the
    // content height. So it must track `auto_resize` (the same flag that gates
    // the auto-fit callback / uncloak in app.rs) — NOT `window_chrome`. Tying
    // it to chrome would leave a chromeless + fixed-size window (auto_resize =
    // false) cloaked forever, since no uncloak step ever runs.
    #[cfg(target_os = "windows")]
    let cloak = config.window.auto_resize();

    match cx.open_window(opts, |window, cx| {
        cx.new(|cx| {
            let view = AuraView::new(
                config,
                config_path,
                state,
                keymap,
                display_id,
                tray_anchor,
                cx,
            );
            // Key bindings dispatch from the focused element, and nothing
            // else in the modal takes focus, so the root holds it for the
            // window's lifetime (a click anywhere re-focuses it).
            window.focus(&view.focus_handle);
            view
        })
    }) {
        Ok(handle) => {
            // On macOS, if we are running as NSApplicationActivationPolicyAccessory
            // (background-only mode), the OS won't grant foreground focus to the
            // window. Promote to Regular while the modal is open so activate()
            // and active_window() work normally. We demote back to Accessory
            // when the window is closed (see the focus-loss / remove_window paths).
            #[cfg(target_os = "macos")]
            if !runtime::show_in_app_switcher() {
                platform::apply_app_switcher_policy(true);
            }

            // Name the surface. Every window list we don't control reads
            // this — alt-tab, task managers, session managers — and an
            // untitled entry tells the user nothing about which app it
            // belongs to. `titlebar` can't carry the name for us: the
            // Wayland backend ignores `TitlebarOptions::title` (only
            // `set_title` reaches `xdg_toplevel`), and with
            // `window_chrome = false` there are no `TitlebarOptions` at all.
            // `set_window_title` routes through the per-platform `set_title`
            // in both configurations. With chrome on, this is also what the
            // decorated title bar now shows.
            let _ = handle.update(cx, |_, window, _| window.set_window_title("Aura"));

            // Re-assert the origin we asked `open_window` for. A window
            // manager is free to ignore the position a client requests at map
            // time unless the WM_NORMAL_HINTS carry `PPosition`, which GPUI
            // does not set — KWin applies its own placement policy to the
            // first window a process opens and centres it. The auto-fit pass
            // corrects that a frame or two later, which is exactly the visible
            // jump this call removes. Absolute coordinates: on X11
            // `placement::to_window_origin` is the identity, so `bounds` is
            // already in the space `set_window_origin` wants.
            #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
            {
                let _ = handle.update(cx, |_, window, cx| {
                    // No frame shift: the window manager has not framed the
                    // window yet, so there is nothing to measure. The auto-fit
                    // pass corrects for it once `_NET_FRAME_EXTENTS` appears.
                    platform::set_window_origin(window, cx, bounds.origin, bounds.size, 0.0)
                });
            }

            cx.activate(true);

            #[cfg(target_os = "windows")]
            {
                // Cloak immediately so the first frame (at the open height,
                // before on_children_prepainted fits it to the content) is
                // invisible. AuraView's on_children_prepainted uncloak fires
                // on the second frame after the resize, showing the window at
                // the correct size.
                //
                // Skipped when `auto_resize` is off (`cloak` is false): there
                // is no auto-shrink step then, so cloaking would leave the
                // window invisible forever.
                let _ = handle.update(cx, |_, window, _| {
                    if cloak {
                        win32_set_cloak(window, true);
                    }
                    window.activate_window();
                });
            }
            #[cfg(target_os = "macos")]
            {
                let _ = handle.update(cx, |_, window, _| {
                    // Raise above other apps' windows. GPUI's Normal kind sets
                    // NSNormalWindowLevel, so without this the modal opens
                    // behind whatever app the user was focused on.
                    platform::raise_window_to_floating(window);
                    window.activate_window();
                });
            }
            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }

            Some(handle)
        }
        Err(e) => {
            eprintln!("aura: failed to open window: {e}");
            None
        }
    }
}
