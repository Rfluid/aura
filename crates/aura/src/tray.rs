use anyhow::Result;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 32;

/// Build a tiny solid-color RGBA icon at startup. Replacing it with a real
/// SVG/PNG asset is a follow-up; this just gets the icon on the bar.
fn placeholder_icon() -> Result<Icon> {
    // 32×32 RGBA: filled purple square with a darker border
    let mut bytes = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let border = x == 0 || y == 0 || x == ICON_SIZE - 1 || y == ICON_SIZE - 1;
            if border {
                bytes.extend_from_slice(&[0x3a, 0x1f, 0x5d, 0xff]);
            } else {
                bytes.extend_from_slice(&[0x8b, 0x5c, 0xf6, 0xff]);
            }
        }
    }
    Ok(Icon::from_rgba(bytes, ICON_SIZE, ICON_SIZE)?)
}

/// Install a system tray icon. Keep the returned handle alive — dropping it
/// removes the icon from the bar.
pub fn install() -> Result<TrayIcon> {
    let icon = placeholder_icon()?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Aura — Agent Usage Reporter")
        .build()?;
    Ok(tray)
}
