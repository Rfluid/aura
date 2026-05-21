use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::assets::AURA_LOGO_SVG;

/// Render size in physical pixels. 64×64 looks crisp on HiDPI and
/// downscales cleanly on standard DPI bars.
const ICON_SIZE: u32 = 64;

/// Aura purple — must stay in sync with `app.rs::COLOR_ACCENT` so the tray
/// icon matches the in-app brand color.
const ICON_COLOR: &str = "#8b5cf6";

/// Rasterise the embedded `aura.svg` to an RGBA icon at `ICON_SIZE`.
/// The SVG uses `currentColor`; we substitute it with `ICON_COLOR` before
/// parsing so resvg paints with the brand purple.
fn render_logo_icon() -> Result<Icon> {
    // Substitute `currentColor` → explicit hex so usvg can resolve it.
    let svg_text = std::str::from_utf8(AURA_LOGO_SVG).context("aura.svg is not UTF-8")?;
    let svg_text = svg_text.replace("currentColor", ICON_COLOR);

    let tree =
        usvg::Tree::from_str(&svg_text, &usvg::Options::default()).context("parsing aura.svg")?;

    let mut pixmap =
        tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE).context("allocating tray pixmap")?;

    // Fit the SVG's viewBox (32×32) into our target buffer.
    let scale = ICON_SIZE as f32 / tree.size().width().max(tree.size().height());
    let transform = tiny_skia::Transform::from_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // resvg produces premultiplied RGBA. tray-icon's Icon::from_rgba expects
    // straight (non-premultiplied) RGBA — demultiply.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let inv = 255.0 / a as f32;
            px[0] = (px[0] as f32 * inv).min(255.0) as u8;
            px[1] = (px[1] as f32 * inv).min(255.0) as u8;
            px[2] = (px[2] as f32 * inv).min(255.0) as u8;
        }
    }

    Ok(Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)?)
}

/// Install a system tray icon. Keep the returned handle alive — dropping it
/// removes the icon from the bar.
pub fn install() -> Result<TrayIcon> {
    let icon = render_logo_icon().context("rendering tray icon")?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Aura — Agent Usage Reporter")
        .build()?;
    Ok(tray)
}
