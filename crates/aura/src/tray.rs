//! System-tray indicator.
//!
//! Linux uses [`ksni`] — a direct StatusNotifierItem implementation —
//! because it surfaces `Activate()` (primary-click) as a callback, which
//! gives us single-click open/close. tray-icon's libayatana-appindicator
//! backend hides primary-click events behind the context menu, costing a
//! second click to actually open the modal.
//!
//! macOS / Windows still use [`tray_icon`]: those backends already get
//! single-click activation natively via AppKit / Win32.
//!
//! Both backends feed a unified [`TrayEvent`] stream that `main.rs` drains
//! from the GPUI side via [`try_recv_event`].

use std::sync::OnceLock;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

use crate::assets::AURA_LOGO_SVG;

/// Render size in physical pixels. 64×64 looks crisp on HiDPI and
/// downscales cleanly on standard DPI bars.
const ICON_SIZE: u32 = 64;

/// Aura purple — must stay in sync with `app.rs::COLOR_ACCENT` so the tray
/// icon matches the in-app brand color.
const ICON_COLOR: &str = "#8b5cf6";

/// User-driven actions that originate from the tray icon and end up
/// toggling the modal on the GPUI side. We keep the enum tiny — a single
/// "Show" semantic — because that matches the wifi/volume tray UX (one
/// indicator, one interaction).
#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    /// Primary-click on the icon, or "Show Aura" picked from the menu.
    /// `hint` carries the tray icon's screen coordinates when the host
    /// sent them with the activate request — `None` when the trigger
    /// was a menu item (which doesn't surface a click position).
    ///
    /// Plumbed end-to-end but unused on the consumer side for now: a
    /// first attempt at anchoring the modal next to the click on
    /// Wayland produced a malformed (very narrow) window for reasons
    /// we haven't root-caused yet, so the consumer falls back to a
    /// centered open. Keeping the field so the fix is a one-line
    /// re-wire when we figure it out.
    Show {
        #[allow(dead_code)]
        hint: Option<(i32, i32)>,
    },
}

/// Opaque handle returned by [`install`]. Must be kept alive for the
/// lifetime of the app — dropping it removes the icon.
pub struct TrayHandle {
    #[cfg(target_os = "linux")]
    _ksni: ksni::blocking::Handle<linux::AuraTray>,
    #[cfg(not(target_os = "linux"))]
    _icon: tray_icon::TrayIcon,
}

/// Non-blocking poll. Returns the next pending [`TrayEvent`] or `None`.
/// Called from GPUI's async task on a short timer.
pub fn try_recv_event() -> Option<TrayEvent> {
    #[cfg(target_os = "linux")]
    {
        linux::try_recv()
    }
    #[cfg(not(target_os = "linux"))]
    {
        non_linux::try_recv()
    }
}

// ── Shared icon rasteriser ───────────────────────────────────────────────────
//
// Both backends paint the same brand SVG; only the destination buffer
// format differs (ksni wants ARGB32, tray-icon wants straight RGBA).
//
// Returns RGBA8 with *straight* (non-premultiplied) alpha — both call
// sites massage it from here.

fn render_logo_rgba() -> Result<(u32, u32, Vec<u8>)> {
    let svg_text = std::str::from_utf8(AURA_LOGO_SVG).context("aura.svg is not UTF-8")?;
    let svg_text = svg_text.replace("currentColor", ICON_COLOR);

    let tree =
        usvg::Tree::from_str(&svg_text, &usvg::Options::default()).context("parsing aura.svg")?;

    let mut pixmap =
        tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE).context("allocating tray pixmap")?;

    let scale = ICON_SIZE as f32 / tree.size().width().max(tree.size().height());
    let transform = tiny_skia::Transform::from_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // resvg produces premultiplied RGBA — demultiply to straight.
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

    Ok((ICON_SIZE, ICON_SIZE, rgba))
}

// ── Linux: ksni ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::Mutex;

    /// Channel used by both `activate()` (primary-click) and the
    /// fallback "Show Aura" menu item to signal the GPUI side. We stash
    /// the receiver in a process-global so `try_recv_event` can drain it
    /// without threading it through `TrayHandle`.
    static EVENT_RX: OnceLock<Mutex<Receiver<TrayEvent>>> = OnceLock::new();

    pub(super) struct AuraTray {
        tx: Sender<TrayEvent>,
        icon: Icon,
    }

    impl ksni::Tray for AuraTray {
        fn id(&self) -> String {
            // Becomes part of the DBus object path; keep stable.
            "aura".into()
        }

        fn title(&self) -> String {
            "Aura — Agent Usage Reporter".into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            vec![self.icon.clone()]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Aura".into(),
                description: "Click to open Agent Usage Reporter".into(),
                icon_name: String::new(),
                icon_pixmap: Vec::new(),
            }
        }

        /// Primary-click. Plasma + GNOME + most KSNI hosts route the
        /// user's left-click here, which is exactly the wifi-style UX
        /// we want. `x` / `y` are the icon's position in screen coords
        /// — we forward them so the modal can anchor near the icon.
        fn activate(&mut self, x: i32, y: i32) {
            let _ = self.tx.send(TrayEvent::Show { hint: Some((x, y)) });
        }

        /// Right-click → minimal context menu. We keep a single "Show
        /// Aura" item so hosts that route right-click only to the menu
        /// still surface the same action; there's deliberately no Quit
        /// so a stray click can't make the icon disappear.
        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![StandardItem {
                label: "Show Aura".into(),
                activate: Box::new(|tray: &mut AuraTray| {
                    // Menu doesn't surface a click position — let the
                    // modal fall back to centered placement.
                    let _ = tray.tx.send(TrayEvent::Show { hint: None });
                }),
                ..Default::default()
            }
            .into()]
        }
    }

    pub(super) fn install() -> Result<TrayHandle> {
        let (width, height, rgba) = render_logo_rgba()?;

        // RGBA8 → ARGB32 (network byte order, big-endian).
        // ksni::Icon::data layout in memory is `[A, R, G, B, A, R, G, B, …]`.
        let mut argb = rgba;
        for px in argb.chunks_exact_mut(4) {
            px.rotate_right(1);
        }

        let icon = Icon {
            width: width as i32,
            height: height as i32,
            data: argb,
        };

        let (tx, rx) = mpsc::channel::<TrayEvent>();

        // Stash the receiver where `try_recv_event` can find it.
        // Re-install would overwrite — but install() is called once.
        let _ = EVENT_RX.set(Mutex::new(rx));

        let tray = AuraTray { tx, icon };
        let handle = tray.spawn().context("ksni spawn (register on D-Bus)")?;

        Ok(TrayHandle { _ksni: handle })
    }

    pub(super) fn try_recv() -> Option<TrayEvent> {
        let mtx = EVENT_RX.get()?;
        mtx.lock().ok()?.try_recv().ok()
    }
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<TrayHandle> {
    linux::install()
}

// ── macOS / Windows: tray-icon ───────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod non_linux {
    use super::*;
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuId, MenuItem},
        Icon, MouseButton, TrayIconBuilder, TrayIconEvent,
    };

    pub(super) const MENU_ID_SHOW: &str = "aura.show";

    pub(super) fn install() -> Result<TrayHandle> {
        let (width, height, rgba) = render_logo_rgba()?;
        let icon = Icon::from_rgba(rgba, width, height).context("Icon::from_rgba")?;

        let menu = Menu::new();
        let show = MenuItem::with_id(MenuId::new(MENU_ID_SHOW), "Show Aura", true, None);
        menu.append(&show).context("menu append Show")?;

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Aura — Agent Usage Reporter")
            .with_menu(Box::new(menu))
            // Primary-click activates directly; menu is right-click only.
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .build()
            .context("building tray icon")?;

        Ok(TrayHandle { _icon: tray })
    }

    pub(super) fn try_recv() -> Option<TrayEvent> {
        // Menu first (right-click → "Show Aura").
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id().0.as_str() == MENU_ID_SHOW {
                return Some(TrayEvent::Show { hint: None });
            }
        }
        // Primary-click on the icon itself. `position` is the cursor
        // location at click time — close enough to the icon to anchor
        // the modal near it.
        if let Ok(TrayIconEvent::Click {
            button, position, ..
        }) = TrayIconEvent::receiver().try_recv()
        {
            if button == MouseButton::Left {
                return Some(TrayEvent::Show {
                    hint: Some((position.x as i32, position.y as i32)),
                });
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install() -> Result<TrayHandle> {
    non_linux::install()
}
