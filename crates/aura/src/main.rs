mod app;
mod assets;
mod format;
#[cfg(target_os = "macos")]
mod macos;
mod tray;

use std::time::Duration;

use anyhow::Result;
use aura_core::{
    config::{AgentStatus, AppConfig},
    state::AppState,
};
use gpui::{
    div, prelude::*, px, size, Application, Bounds, IntoElement, Render, WindowBounds,
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
                        }
                    }
                }
            })
            .detach();

            // macOS: don't claim a Dock slot — we're a menu-bar accessory.
            #[cfg(target_os = "macos")]
            macos::set_accessory_activation_policy();
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

/// Open the always-present hidden window. See the call site for why this
/// is necessary on Linux Wayland. Failures are non-fatal but logged: if
/// the keepalive can't open, aura will still work — just with the old
/// "process exits on last window close" behaviour.
fn open_keepalive_window(cx: &mut gpui::App) -> Option<WindowHandle<KeepAliveView>> {
    let opts = WindowOptions {
        // 1×1 in case some compositor refuses zero-size surfaces.
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::Normal,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };

    match cx.open_window(opts, |_window, cx| cx.new(|_| KeepAliveView)) {
        Ok(handle) => Some(handle),
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
    state: AppState,
) -> Option<WindowHandle<AuraView>> {
    cx.update(move |cx| toggle_window(cx, existing, config, config_path, state))
        .ok()
        .flatten()
}

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

    let bounds = Bounds::centered(None, size(px(520.), px(640.)), cx);
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
