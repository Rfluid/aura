// Suppress the console window on Windows — without this the OS opens a CMD
// prompt alongside the GUI process.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod assets;
mod cli;
mod format;
mod platform;
mod runtime;
mod tray;
mod updater;
mod work_area;

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use aura_core::{config::AppConfig, state::AppState};
use clap::Parser;
use gpui::{
    div, point, prelude::*, px, size, Application, Bounds, IntoElement, Render, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowHandle, WindowKind, WindowOptions,
};

use crate::tray::TrayEvent;
use crate::{app::AuraView, assets::EmbeddedAssets};

/// DWM-cloak or -uncloak a window on Windows. Cloaking makes the window
/// invisible to the user (DWM hides it during composition) while it still
/// receives WM_PAINT and renders normally — used to hide the first-frame
/// resize flash (window opens at MODAL_H, shrinks to content height on the
/// next frame; without cloaking the user sees a one-frame flicker).
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

fn main() -> Result<()> {
    // Subcommand dispatch — handled before GPUI init so headless commands
    // don't spin up a window/event loop. `aura` with no subcommand falls
    // through to the tray entry point below.
    let cli = cli::Cli::parse();
    if let Some(command) = cli.command {
        return cli::dispatch(command);
    }

    // Single-instance guard: if another Aura is already running, exit
    // silently — the running tray icon will service the next click. The
    // lock is held (intentionally leaked) for the lifetime of the process;
    // the OS releases it on exit. See `platform::acquire_single_instance`.
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
    // into atomics so both reload paths keep `main`'s tray loop in sync
    // with the modal view.
    let config_path = AppConfig::default_path();
    let config = AppConfig::load_with_discovery(&config_path)?;
    runtime::set_from_config(&config);

    // ── Install tray icon (best-effort: warn on failure but keep going) ───────
    let _tray = match tray::install() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("warning: could not install tray icon: {e}");
            None
        }
    };

    // ── Launch GPUI app ───────────────────────────────────────────────────────
    //
    // No user-visible window is opened at startup. We open a tiny hidden
    // "keepalive" window so GPUI's Wayland event loop doesn't stop the
    // moment the user closes the modal — `wayland/client.rs` exits the
    // process when `state.windows.is_empty()`. The keepalive guarantees
    // that count is always ≥ 1, so the tray icon survives across any
    // number of open/close cycles.
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(move |cx| {
            // GPUI forces NSApplicationActivationPolicyRegular in
            // did_finish_launching; reapply the user's preference here so it
            // sticks. `runtime::set_from_config` (called at startup before
            // .run) only fires once, *before* GPUI launches — without this
            // second push, the user's Accessory choice would be overwritten
            // by the time we hit the run closure on macOS.
            platform::apply_app_switcher_policy(runtime::show_in_app_switcher());

            // Hold the handle in the move-closure so it isn't dropped.
            let _keepalive = open_keepalive_window(cx);

            let config = config.clone();
            let config_path = config_path.clone();

            cx.spawn(async move |cx| {
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

                loop {
                    // Poll: ksni / tray-icon both expose blocking
                    // crossbeam channels under the hood, so we drain
                    // them between short sleeps.
                    cx.background_executor().timer(MENU_POLL_INTERVAL).await;

                    if runtime::dismiss_on_focus_loss() && current.is_some() {
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
                                    let _ = handle.update(cx, |_view, window, _cx| {
                                        window.remove_window()
                                    });
                                });
                            }
                        }
                    }

                    while let Some(event) = tray::try_recv_event() {
                        match event {
                            TrayEvent::Show { hint } => {
                                // Reload AppConfig from disk so edits made
                                // since the last open (whether via the
                                // settings panel, an external editor, or
                                // `aura plugin add`) take effect on this
                                // open. Fall back to the startup snapshot
                                // if the reload fails so a transient I/O
                                // error doesn't break the toggle.
                                let fresh_config =
                                    AppConfig::load_with_discovery(&config_path)
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
                                    hint,
                                )
                                .await;

                                if current.is_some() {
                                    just_opened = 4; // ~600 ms at 150 ms/poll
                                    // macOS: install the click-outside
                                    // monitor for the new window.
                                    #[cfg(target_os = "macos")]
                                    {
                                        outside_clicked.store(false, Ordering::Relaxed);
                                        click_monitor = Some(
                                            platform::install_click_outside_monitor(
                                                Arc::clone(&outside_clicked),
                                            ),
                                        );
                                    }
                                }
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

/// Empty root view for the hidden keepalive window. The view is never
/// rendered to a screen — its only job is to satisfy `open_window`'s
/// `V: Render` bound so the window can exist in `state.windows`.
struct KeepAliveView;

impl Render for KeepAliveView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        div()
    }
}

/// Open the always-present keepalive window. See the call site for why
/// this is necessary on Linux Wayland. Failures are non-fatal but logged:
/// if the keepalive can't open, aura will still work — just with the old
/// "process exits on last window close" behaviour.
///
/// ## Hiding it on Wayland
///
/// GPUI 0.2's Wayland backend silently ignores `show: false` (it creates
/// an xdg_toplevel and commits the surface unconditionally), so KWin
/// renders a 1×1 surface as a small window with server-side chrome
/// (title bar + close button). To minimise damage we:
///
/// * open the surface at `(-9999, -9999)` so even if the compositor
///   doesn't clamp it back on-screen, the user can't accidentally
///   focus or click it;
/// * `minimize_window()` it immediately so KDE puts it straight into
///   the taskbar overflow instead of painting it on the desktop;
/// * give it a distinct `app_id` ("aura-keepalive") so KDE's task
///   manager doesn't group it under the main "Aura" entry;
/// * intercept every platform-level close request with
///   `on_window_should_close` returning `false` — clicking the
///   compositor's "close window" action on the keepalive becomes a
///   no-op, so the tray can't be killed by a stray click. Our own
///   `toggle()` uses `window.remove_window()` which bypasses this
///   guard (it's an internal close, not a platform request).
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
    hint: Option<(i32, i32)>,
) -> Option<WindowHandle<AuraView>> {
    cx.update(move |cx| toggle_window(cx, existing, config, config_path, hint))
        .ok()
        .flatten()
}

/// Modal dimensions and the gap we leave between its edges and the
/// screen / panel. Width/height are intentionally fixed — the
/// `app.rs::on_children_prepainted` resize callback shrinks the window
/// vertically to fit content (capped at the available work area), so the
/// initial 640 is just a sensible starting size that lets the first
/// paint render without thrashing.
const MODAL_W: f32 = 520.;
const MODAL_H: f32 = 640.;
const SCREEN_GAP: f32 = 8.;
/// Defensive blind reserve for the bottom edge when
/// [`crate::work_area::available_bottom`] returns `None` (non-KDE,
/// non-Linux, or parse failure). Matches Strategy A's value so corner
/// anchoring degrades to "Strategy A + bottom-right placement" instead
/// of dumping the modal into a taskbar.
#[cfg(not(target_os = "macos"))]
const BLIND_BOTTOM_RESERVE: f32 = 120.;

/// Compute where to place the modal window.
///
/// **macOS**: the tray icon lives in the menu bar at the top. We anchor the
/// modal just below the bar, horizontally centred on the icon's X coord
/// (from `hint`). Falls back to top-right if `hint` is absent.
///
/// **Linux / Windows**: anchors to the bottom-right of the available work
/// area (above the taskbar/panel). Wayland compositors may ignore the
/// requested origin and centre the window instead; users can override via a
/// KWin window rule.
fn compute_modal_bounds(cx: &mut gpui::App, _hint: Option<(i32, i32)>) -> Bounds<gpui::Pixels> {
    let modal_size = size(px(MODAL_W), px(MODAL_H));

    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, modal_size, cx);
    };
    let display_bounds = display.bounds();

    let screen_left = f32::from(display_bounds.origin.x);
    let screen_top = f32::from(display_bounds.origin.y);
    let screen_right = f32::from(display_bounds.origin.x + display_bounds.size.width);

    #[cfg(target_os = "macos")]
    {
        // On macOS the tray sits in the menu bar (~25 pt tall). Place the
        // modal just below it, horizontally centred on the click position.
        // GPUI uses top-left origin with Y increasing downward on macOS.
        const MENU_BAR_H: f32 = 25.0;
        let icon_x = _hint
            .map(|(x, _)| x as f32)
            .unwrap_or(screen_right - MODAL_W / 2.0);
        let x = (icon_x - MODAL_W / 2.0).clamp(screen_left, screen_right - MODAL_W);
        let y = screen_top + MENU_BAR_H + SCREEN_GAP;
        Bounds::new(point(px(x), px(y)), modal_size)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let screen_bottom_full = f32::from(display_bounds.origin.y + display_bounds.size.height);
        let work_bottom = crate::work_area::available_bottom(display_bounds)
            .unwrap_or(screen_bottom_full - BLIND_BOTTOM_RESERVE);
        let x = (screen_right - MODAL_W - SCREEN_GAP).max(screen_left);
        let y = (work_bottom - MODAL_H - SCREEN_GAP).max(screen_top);
        Bounds::new(point(px(x), px(y)), modal_size)
    }
}

fn toggle_window(
    cx: &mut gpui::App,
    existing: Option<WindowHandle<AuraView>>,
    config: AppConfig,
    config_path: std::path::PathBuf,
    hint: Option<(i32, i32)>,
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

    let bounds = compute_modal_bounds(cx, hint);
    // `display.show_in_app_switcher` controls whether the modal appears in
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
    let kind = if config.display.show_in_app_switcher || config.display.window_chrome {
        WindowKind::Normal
    } else {
        WindowKind::PopUp
    };
    // `display.window_chrome` swaps the modal between two modes:
    //   false (default): chromeless tray-popup, fixed width, height auto-fits
    //     content (see app.rs::on_children_prepainted). is_resizable is
    //     forced false because there is no visible edge to grab anyway.
    //   true: native OS chrome (title bar + min/max/close), user-resizable.
    //     window_decorations: Server asks Wayland compositors to draw SSD;
    //     the auto-fit callback in app.rs is suppressed so user-dragged
    //     sizes stick.
    let (titlebar, is_resizable, window_decorations) = if config.display.window_chrome {
        (
            Some(TitlebarOptions::default()),
            true,
            Some(WindowDecorations::Server),
        )
    } else {
        (None, false, None)
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

    #[cfg(target_os = "windows")]
    let cloak = !config.display.window_chrome;

    match cx.open_window(opts, |_window, cx| {
        cx.new(|cx| AuraView::new(config, config_path, state, cx))
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

            cx.activate(true);

            #[cfg(target_os = "windows")]
            {
                // Cloak immediately so the first frame (at MODAL_H before
                // on_children_prepainted shrinks it to content height) is
                // invisible. AuraView's on_children_prepainted uncloak fires
                // on the second frame after the resize, showing the window at
                // the correct size.
                //
                // Skipped when `window_chrome` is on: there is no auto-shrink
                // step in that mode, so cloaking would leave the window
                // invisible forever.
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
