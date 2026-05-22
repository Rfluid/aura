mod app;
mod assets;
mod format;
#[cfg(target_os = "macos")]
mod macos;
mod tray;

use anyhow::Result;
use aura_core::{
    config::{AgentStatus, AppConfig},
    state::AppState,
};
use gpui::{prelude::*, px, size, Application, Bounds, WindowBounds, WindowOptions};

use crate::{app::AuraView, assets::EmbeddedAssets};

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
    let _tray = match tray::install() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("warning: could not install tray icon: {e}");
            None
        }
    };

    // ── Launch GPUI app ───────────────────────────────────────────────────────
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(move |cx| {
            let bounds = Bounds::centered(None, size(px(520.), px(640.)), cx);
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                ..Default::default()
            };

            let config = config.clone();
            let config_path = config_path.clone();
            let state = state.clone();
            cx.open_window(opts, |_window, cx| {
                cx.new(|cx| AuraView::new(config, config_path, state, cx))
            })
            .expect("failed to open window");

            cx.activate(true);

            // GPUI sets NSApplicationActivationPolicyRegular at startup,
            // which gives Aura a Dock icon. Override it on macOS so we
            // behave like a menu-bar accessory app instead.
            #[cfg(target_os = "macos")]
            macos::set_accessory_activation_policy();
        });

    Ok(())
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
