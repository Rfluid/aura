mod app;
mod assets;
mod format;
mod tray;

use anyhow::Result;
use aura_core::{config::AppConfig, state::AppState};
use gpui::{prelude::*, px, size, Application, Bounds, WindowBounds, WindowOptions};

use crate::{app::AuraView, assets::EmbeddedAssets};

fn main() -> Result<()> {
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
        });

    Ok(())
}
