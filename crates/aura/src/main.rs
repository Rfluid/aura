#[cfg(not(target_os = "macos"))]
mod app;
mod assets;
#[cfg(not(target_os = "macos"))]
mod format;
#[cfg(target_os = "macos")]
mod macos;
mod tray;
#[cfg(not(target_os = "macos"))]
mod work_area;

#[cfg(not(target_os = "macos"))]
use std::time::Duration;

use anyhow::Result;
use aura_core::{
    config::{AgentStatus, AppConfig},
    state::AppState,
};
#[cfg(not(target_os = "macos"))]
use gpui::{
    div, point, prelude::*, px, size, Application, Bounds, IntoElement, Render, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};

#[cfg(not(target_os = "macos"))]
use crate::tray::TrayEvent;

#[cfg(not(target_os = "macos"))]
use crate::{app::AuraView, assets::EmbeddedAssets};

/// How often the GPUI main thread checks for pending tray menu events.
/// 150 ms is well under the human "instant" threshold (~200 ms) for the
/// click → modal latency while costing essentially nothing CPU-wise.
#[cfg(not(target_os = "macos"))]
const MENU_POLL_INTERVAL: Duration = Duration::from_millis(150);

fn main() -> Result<()> {
    // Subcommand dispatch — handled before GPUI init so headless commands
    // don't spin up a window/event loop.
    let mut args = std::env::args().skip(1);
    if let Some(cmd) = args.next() {
        match cmd.as_str() {
            "setup-config" => return run_setup_config(),
            "--help" | "-h" | "help" => {
                print_usage();
                return Ok(());
            }
            other => {
                eprintln!("aura: unknown command '{other}'");
                print_usage();
                std::process::exit(2);
            }
        }
    }

    // ── Load config + state ───────────────────────────────────────────────────
    let config_path = AppConfig::default_path();
    let config = AppConfig::load(&config_path)?;
    let state = AppState::load()?;

    // ── Install tray icon (best-effort: warn on failure but keep going) ───────
    // On Linux this spawns a dedicated GTK thread that owns the icon; on
    // macOS / Windows the returned handle owns it and must be kept alive.
    let _tray = match tray::install() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("warning: could not install tray icon: {e}");
            None
        }
    };

    // ── macOS: skip GPUI and run AppKit directly ─────────────────────────────
    //
    // GPUI 0.2.2 panics inside `Application::run` on macOS 26 (Tahoe) — see
    // issue #4 and the comment block in `crates/aura/Cargo.toml`. Until
    // crates.io has a fixed gpui, the macOS build ships as tray-only and
    // we drive the event loop from `macos::run_event_loop` instead. The
    // config/state are still loaded above so the first-run side effect
    // (creating `~/Library/Application Support/aura/config.toml`) matches
    // the Linux experience.
    #[cfg(target_os = "macos")]
    {
        let _ = (config, config_path, state);
        macos::run_event_loop();
        return Ok(());
    }

    // ── Launch GPUI app ───────────────────────────────────────────────────────
    //
    // No user-visible window is opened at startup. We open a tiny hidden
    // "keepalive" window so GPUI's Wayland event loop doesn't stop the
    // moment the user closes the modal — `wayland/client.rs` exits the
    // process when `state.windows.is_empty()`. The keepalive guarantees
    // that count is always ≥ 1, so the tray icon survives across any
    // number of open/close cycles.
    #[cfg(not(target_os = "macos"))]
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(move |cx| {
            // Hold the handle in the move-closure so it isn't dropped.
            let _keepalive = open_keepalive_window(cx);

            let config = config.clone();
            let config_path = config_path.clone();
            let state = state.clone();

            cx.spawn(async move |cx| {
                // The currently-open window, if any. We toggle on each
                // "Show Aura" click: open if closed, close if open.
                let mut current: Option<WindowHandle<AuraView>> = None;

                loop {
                    // Poll: ksni / tray-icon both expose blocking
                    // crossbeam channels under the hood, so we drain
                    // them between short sleeps.
                    cx.background_executor().timer(MENU_POLL_INTERVAL).await;

                    while let Some(event) = tray::try_recv_event() {
                        match event {
                            // We discard `hint` for now: an earlier attempt
                            // to anchor the modal near the click coords
                            // produced a malformed window on Wayland/KWin
                            // (see git history). Restoring centered open
                            // until we understand the size regression.
                            TrayEvent::Show { hint: _ } => {
                                current = toggle(
                                    cx,
                                    current.take(),
                                    config.clone(),
                                    config_path.clone(),
                                    state.clone(),
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
#[cfg(not(target_os = "macos"))]
struct KeepAliveView;

#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
async fn toggle(
    cx: &gpui::AsyncApp,
    existing: Option<WindowHandle<AuraView>>,
    config: AppConfig,
    config_path: std::path::PathBuf,
    state: AppState,
) -> Option<WindowHandle<AuraView>> {
    cx.update(move |cx| toggle_window(cx, existing, config, config_path, state))
        .ok()
        .flatten()
}

/// Modal dimensions and the gap we leave between its edges and the
/// screen / panel. Width/height are intentionally fixed — the
/// `app.rs::on_children_prepainted` resize callback shrinks the window
/// vertically to fit content (capped at the available work area), so the
/// initial 640 is just a sensible starting size that lets the first
/// paint render without thrashing.
#[cfg(not(target_os = "macos"))]
const MODAL_W: f32 = 520.;
#[cfg(not(target_os = "macos"))]
const MODAL_H: f32 = 640.;
#[cfg(not(target_os = "macos"))]
const SCREEN_GAP: f32 = 8.;
/// Defensive blind reserve for the bottom edge when
/// [`crate::work_area::available_bottom`] returns `None` (non-KDE,
/// non-Linux, or parse failure). Matches Strategy A's value so corner
/// anchoring degrades to "Strategy A + bottom-right placement" instead
/// of dumping the modal into a taskbar.
#[cfg(not(target_os = "macos"))]
const BLIND_BOTTOM_RESERVE: f32 = 120.;

/// Computes the modal bounds anchored to the bottom-right of the
/// available work area — the corner where the system tray icon lives
/// on the common KDE Plasma / Windows / macOS layouts.
///
/// ## Wayland caveat
///
/// On Wayland (KWin / Mutter / sway) the compositor — not the client
/// — decides where xdg_toplevel surfaces appear. GPUI 0.2 always
/// creates toplevels (never xdg-popup; see
/// `gpui/src/platform/linux/wayland/window.rs:288`), so KWin will
/// generally ignore our bounds origin and center the window. That's a
/// protocol-level limit, not a GPUI bug.
///
/// Users who want exact placement on Plasma can add a KWin window
/// rule (System Settings → Window Management → Window Rules → New →
/// match WM class `aura` → Apply initially `Position`).
///
/// On X11 / Windows / macOS the request is honoured natively and the
/// modal opens where the tray icon lives.
///
/// We deliberately don't consult the tray click position — an earlier
/// attempt at that produced a malformed (very narrow) window on
/// KWin/Wayland for reasons we never root-caused.
#[cfg(not(target_os = "macos"))]
fn corner_anchored_bounds(cx: &mut gpui::App) -> Bounds<gpui::Pixels> {
    let modal_size = size(px(MODAL_W), px(MODAL_H));

    let Some(display) = cx.primary_display() else {
        // No display info — fall back to centered. We can't anchor
        // anywhere meaningful without knowing where the screen is.
        return Bounds::centered(None, modal_size, cx);
    };
    let display_bounds = display.bounds();

    let screen_left = f32::from(display_bounds.origin.x);
    let screen_top = f32::from(display_bounds.origin.y);
    let screen_right = f32::from(display_bounds.origin.x + display_bounds.size.width);
    let screen_bottom_full = f32::from(display_bounds.origin.y + display_bounds.size.height);

    // Bottom of the available area: prefer the exact panel-aware value
    // from `work_area::available_bottom`, otherwise reserve a blind
    // 120 px margin so we still clear the panel on platforms where we
    // can't measure it.
    let work_bottom = crate::work_area::available_bottom(display_bounds)
        .unwrap_or(screen_bottom_full - BLIND_BOTTOM_RESERVE);

    // Place the modal flush against the right edge (where the tray
    // sits on a horizontal panel) with a small gap; clamp so we never
    // run off the left side on tiny displays.
    let x = (screen_right - MODAL_W - SCREEN_GAP).max(screen_left);
    // Same idea vertically: bottom-anchor with a gap, clamp at top.
    let y = (work_bottom - MODAL_H - SCREEN_GAP).max(screen_top);

    Bounds::new(point(px(x), px(y)), modal_size)
}

#[cfg(not(target_os = "macos"))]
fn toggle_window(
    cx: &mut gpui::App,
    existing: Option<WindowHandle<AuraView>>,
    config: AppConfig,
    config_path: std::path::PathBuf,
    state: AppState,
) -> Option<WindowHandle<AuraView>> {
    if let Some(handle) = existing {
        // `update` returns Err if the window has already been removed;
        // either way we're done with this handle.
        let _ = handle.update(cx, |_view, window, _cx| window.remove_window());
        return None;
    }

    let bounds = corner_anchored_bounds(cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
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

fn print_usage() {
    eprintln!("Usage: aura [COMMAND]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  setup-config   Detect installed agents and write/update the config file");
    eprintln!("  help           Print this message");
    eprintln!();
    eprintln!("Run without arguments to launch the tray app.");
}

/// Detect installed agents and reconcile the on-disk config. Invoked by the
/// platform installers after copying binaries into place.
fn run_setup_config() -> Result<()> {
    let path = AppConfig::default_path();
    let report = AppConfig::run_setup(&path)?;

    if report.created {
        println!("Created {}", path.display());
    } else {
        println!("Updated {}", path.display());
    }

    for (agent, status) in &report.agents {
        let resolved = agent.resolved_config_path();
        match status {
            AgentStatus::Added => {
                println!("  + {} ({})", agent.name, resolved.display());
            }
            AgentStatus::AlreadyConfigured => {
                println!(
                    "  · {} ({}) — already configured",
                    agent.name,
                    resolved.display()
                );
            }
            AgentStatus::NotInstalled => {
                println!(
                    "  - {} ({}) — not installed, skipping",
                    agent.name,
                    resolved.display()
                );
            }
        }
    }

    Ok(())
}
