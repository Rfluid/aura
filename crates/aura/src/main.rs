mod app;
mod assets;
mod cli;
mod format;
mod tray;
mod work_area;

use std::time::Duration;

use anyhow::Result;
use aura_core::{config::AppConfig, state::AppState};
use clap::Parser;
use gpui::{
    div, point, prelude::*, px, size, Application, Bounds, IntoElement, Render, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};

use crate::tray::TrayEvent;
use crate::{app::AuraView, assets::EmbeddedAssets};

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

    // ── Load config ───────────────────────────────────────────────────────────
    //
    // `AppState` is *not* loaded here on purpose — `toggle_window` reloads it
    // from disk each time the modal opens, so a profile change made in one
    // session is visible the next time the user clicks the tray icon. Loading
    // it once at startup would cache a stale snapshot in the closure below.
    let config_path = AppConfig::default_path();
    let config = AppConfig::load_with_discovery(&config_path)?;

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
            // Hold the handle in the move-closure so it isn't dropped.
            let _keepalive = open_keepalive_window(cx);

            let config = config.clone();
            let config_path = config_path.clone();

            cx.spawn(async move |cx| {
                // The currently-open window, if any. We toggle on each
                // "Show Aura" click: open if closed, close if open.
                let mut current: Option<WindowHandle<AuraView>> = None;

                loop {
                    // Poll: ksni / tray-icon both expose blocking
                    // crossbeam channels under the hood, so we drain
                    // them between short sleeps.
                    cx.background_executor().timer(MENU_POLL_INTERVAL).await;

                    // macOS: auto-close when the user clicks outside the modal.
                    // cx.active_window() returns None once another app takes
                    // focus; we treat that as "dismiss".
                    #[cfg(target_os = "macos")]
                    if current.is_some() {
                        let lost_focus = cx
                            .update(|cx| cx.active_window().is_none())
                            .unwrap_or(false);
                        if lost_focus {
                            if let Some(handle) = current.take() {
                                let _ = cx.update(|cx| {
                                    let _ = handle
                                        .update(cx, |_view, window, _cx| window.remove_window());
                                });
                            }
                        }
                    }

                    while let Some(event) = tray::try_recv_event() {
                        match event {
                            TrayEvent::Show { hint } => {
                                current = toggle(
                                    cx,
                                    current.take(),
                                    config.clone(),
                                    config_path.clone(),
                                    hint,
                                )
                                .await;
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
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        // Set a stable Wayland app_id / X11 WM_CLASS so KWin window rules
        // (see README "Modal placement on Wayland") can match this surface.
        // Without this, KDE shows "Window class not available" when the
        // user tries to Detect Window Properties on the modal.
        app_id: Some("aura".into()),
        ..Default::default()
    };

    match cx.open_window(opts, |_window, cx| {
        cx.new(|cx| AuraView::new(config, config_path, state, cx))
    }) {
        Ok(handle) => {
            cx.activate(true);
            Some(handle)
        }
        Err(e) => {
            eprintln!("aura: failed to open window: {e}");
            None
        }
    }
}
