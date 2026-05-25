// Same suppression as platform.rs — objc 0.2 macros emit an internal
// `cargo-clippy` cfg that newer rustc/clippy flags as unexpected_cfgs.
#![allow(unexpected_cfgs)]

//! Available work-area detection — display rect minus reserved panels.
//!
//! Used in two places:
//!
//! 1. `app.rs` resize callback: caps the auto-grown modal height so it
//!    can't extend past the top of a bottom taskbar.
//! 2. `main.rs` `toggle_window`: anchors the modal at the bottom-right
//!    corner of the work area so it appears where the tray icon lives.
//!
//! Per-platform sources, in order of preference:
//!
//! - **macOS**: `NSScreen.visibleFrame` returns the screen rect minus
//!   the menu bar and any docked Dock — the canonical answer.
//! - **Windows**: `SystemParametersInfoW(SPI_GETWORKAREA)` returns the
//!   work-area rect in physical pixels; we cache the bottom-fraction so
//!   the DPI scale cancels out.
//! - **Linux KDE Plasma**: parse `~/.config/plasmashellrc` +
//!   `plasma-org.kde.plasma.desktop-appletsrc` for bottom-panel
//!   thickness. KDE's `StrutManager.availableScreenRect` D-Bus API
//!   would be cleaner but returns the full display rect on the versions
//!   we tested.
//! - **Linux X11 / XWayland**: read the root window's `_NET_WORKAREA`
//!   property via `xprop`. Covers GNOME, XFCE, Cinnamon, MATE, i3, and
//!   any other reasonably standards-compliant X11 DE.
//! - **Pure Wayland without XWayland**: no portable source today;
//!   `xprop` will fail and the caller falls back to a blind margin.
//!
//! All lookups are cached process-wide. Users who resize their panel
//! without restarting Aura get stale numbers — a fine tradeoff vs.
//! re-querying on every resize frame.

use gpui::{Bounds, Pixels};

/// Returns the bottom Y (in the same coordinate space
/// `App::primary_display` uses — logical pixels, origin at the
/// display's top-left) of the available work area for `display`.
///
/// `None` means "couldn't determine a panel reservation; fall back to
/// whatever blind margin the caller has in mind." See the module-level
/// docstring for the per-platform sources tried.
pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
    #[cfg(target_os = "linux")]
    {
        linux::available_bottom(display)
    }
    #[cfg(target_os = "macos")]
    {
        macos::available_bottom(display)
    }
    #[cfg(target_os = "windows")]
    {
        windows_impl::available_bottom(display)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = display;
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::OnceLock;

    use gpui::{Bounds, Pixels};

    /// Cached `Some(bottom_reservation_px)` — the height taken by a
    /// bottom-anchored Dock. `None` means we tried and couldn't ask
    /// NSScreen (no main screen, unusual platform, etc.).
    static CACHED_BOTTOM_RESERVATION: OnceLock<Option<f32>> = OnceLock::new();

    pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
        let dock = (*CACHED_BOTTOM_RESERVATION.get_or_init(query_bottom_reservation))?;
        let display_bottom = f32::from(display.origin.y + display.size.height);
        Some(display_bottom - dock)
    }

    /// Query NSScreen.visibleFrame on the main screen. macOS uses a
    /// bottom-left origin: `visibleFrame.origin.y` is the height of the
    /// bottom-docked Dock (0 when the Dock is auto-hidden or anchored
    /// to a side).
    fn query_bottom_reservation() -> Option<f32> {
        use cocoa::base::nil;
        use cocoa::foundation::NSRect;
        use objc::{class, msg_send, sel, sel_impl};

        unsafe {
            let screen: cocoa::base::id = msg_send![class!(NSScreen), mainScreen];
            if screen == nil {
                return None;
            }
            let visible: NSRect = msg_send![screen, visibleFrame];
            let bottom_dock = visible.origin.y as f32;
            if bottom_dock.is_nan() || bottom_dock < 0.0 {
                None
            } else {
                Some(bottom_dock)
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::sync::OnceLock;

    use gpui::{Bounds, Pixels};

    // Cached ratio: physical_work_bottom / physical_screen_height.
    // Dividing by this ratio cancels the DPI scale factor, so the same
    // value works in logical-pixel coordinates.
    static RATIO: OnceLock<Option<f32>> = OnceLock::new();

    pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
        let ratio = (*RATIO.get_or_init(query_ratio))?;
        let logical_h = f32::from(display.size.height);
        let logical_y = f32::from(display.origin.y);
        Some(logical_y + logical_h * ratio)
    }

    fn query_ratio() -> Option<f32> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SystemParametersInfoW, SM_CYSCREEN, SPI_GETWORKAREA,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        };

        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;
        if screen_h <= 0.0 {
            return None;
        }
        let mut rect = RECT::default();
        unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rect as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .ok()?;
        }
        // work_bottom / screen_h is scale-factor-agnostic: the same fraction
        // describes the work area in both physical and logical coordinates.
        Some(rect.bottom as f32 / screen_h)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use gpui::{Bounds, Pixels};

    /// Cached `Some(thickness_px)` for the bottom panel (or the
    /// thickest of several stacked panels). `None` means we tried KDE
    /// config + `xprop` and neither produced an answer.
    static CACHED_BOTTOM_THICKNESS: OnceLock<Option<f32>> = OnceLock::new();

    pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
        let thickness = (*CACHED_BOTTOM_THICKNESS.get_or_init(|| {
            query_bottom_panel_thickness().or_else(|| query_xprop_bottom_strut(display))
        }))?;
        let display_bottom = f32::from(display.origin.y + display.size.height);
        Some(display_bottom - thickness)
    }

    fn query_bottom_panel_thickness() -> Option<f32> {
        let config_dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;

        let bottom_ids =
            parse_bottom_panel_ids(&config_dir.join("plasma-org.kde.plasma.desktop-appletsrc"))
                .unwrap_or_default();
        if bottom_ids.is_empty() {
            return None;
        }

        let thicknesses =
            parse_panel_thicknesses(&config_dir.join("plasmashellrc")).unwrap_or_default();

        // Multiple bottom-anchored panels can't visibly overlap on
        // Plasma — the thickest one is the visible reservation.
        bottom_ids
            .iter()
            .filter_map(|id| thicknesses.get(id))
            .copied()
            .reduce(f32::max)
    }

    /// Read the root window's `_NET_WORKAREA` property via `xprop` and
    /// derive the bottom reservation. Works on any X11 session — and on
    /// XWayland-backed Wayland sessions, which is most of them. Returns
    /// `None` on pure Wayland (no xprop available, or xprop has no X
    /// display to talk to).
    ///
    /// `_NET_WORKAREA(CARDINAL)` is a list of 4-tuples (x, y, w, h), one
    /// per virtual desktop. We use the first tuple — Aura's window
    /// rules pin the modal to the current desktop anyway.
    fn query_xprop_bottom_strut(display: Bounds<Pixels>) -> Option<f32> {
        let output = std::process::Command::new("xprop")
            .args(["-root", "_NET_WORKAREA"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = std::str::from_utf8(&output.stdout).ok()?;
        parse_xprop_bottom_strut(stdout, f32::from(display.size.height))
    }

    /// Pure parser for the xprop response. Returns the height of the
    /// bottom reservation (panel/taskbar height), or `None` if the
    /// response is unparsable or describes a multi-monitor virtual root
    /// we can't safely interpret.
    fn parse_xprop_bottom_strut(stdout: &str, display_h: f32) -> Option<f32> {
        // Expected line: `_NET_WORKAREA(CARDINAL) = 0, 24, 1920, 1056`.
        let after_eq = stdout.split_once('=')?.1.trim();
        let parts: Vec<f32> = after_eq
            .split(',')
            .filter_map(|p| p.trim().parse::<f32>().ok())
            .collect();
        if parts.len() < 4 {
            return None;
        }
        let work_y = parts[1];
        let work_h = parts[3];
        if display_h <= 0.0 {
            return None;
        }
        // Most X11/XWayland sessions run at scale 1.0 and the rect
        // matches our logical display. If the work-area rect's bottom
        // edge exceeds display_h, we're looking at a multi-monitor
        // virtual root — return None rather than guess which monitor.
        if work_y + work_h > display_h + 1.0 {
            return None;
        }
        let reservation = display_h - (work_y + work_h);
        if reservation < 0.0 {
            None
        } else {
            Some(reservation)
        }
    }

    /// IDs of `[Containments][N]` sections whose `location=4` (Plasma's
    /// bottom-anchor constant).
    ///
    /// Plasma's INI dialect lets sections nest like
    /// `[Containments][2][General]` — we only treat the exact two-part
    /// `[Containments][N]` headers as "panel root"; nested headers reset
    /// the current scope.
    fn parse_bottom_panel_ids(path: &Path) -> std::io::Result<Vec<u32>> {
        let content = std::fs::read_to_string(path)?;
        let mut result = Vec::new();
        let mut current: Option<u32> = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('[') {
                current = parse_containment_root_id(rest);
            } else if let Some(id) = current {
                if trimmed == "location=4" && !result.contains(&id) {
                    result.push(id);
                }
            }
        }

        Ok(result)
    }

    /// Returns `Some(N)` for the exact `[Containments][N]` header;
    /// `None` for nested forms like `[Containments][N][General]`.
    fn parse_containment_root_id(rest: &str) -> Option<u32> {
        let after_first = rest.strip_prefix("Containments][")?;
        let (id_str, tail) = after_first.split_once(']')?;
        if !tail.is_empty() {
            return None;
        }
        id_str.parse().ok()
    }

    /// Map of `Panel N` ID → thickness in logical px.
    ///
    /// `plasmashellrc` puts the default thickness under
    /// `[PlasmaViews][Panel N][Defaults]` and per-screen overrides under
    /// `[PlasmaViews][Panel N][Screens][<name>]`. We take the max per ID
    /// so explicit overrides beat the default.
    fn parse_panel_thicknesses(path: &Path) -> std::io::Result<HashMap<u32, f32>> {
        let content = std::fs::read_to_string(path)?;
        let mut result: HashMap<u32, f32> = HashMap::new();
        let mut current: Option<u32> = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("[PlasmaViews][Panel ") {
                let id_str = rest.split(']').next().unwrap_or("");
                current = id_str.parse().ok();
            } else if trimmed.starts_with('[') {
                current = None;
            } else if let Some(id) = current {
                if let Some(value) = trimmed.strip_prefix("thickness=") {
                    if let Ok(t) = value.parse::<f32>() {
                        result.entry(id).and_modify(|v| *v = v.max(t)).or_insert(t);
                    }
                }
            }
        }

        Ok(result)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_bottom_containment_ids_only() {
            let path = std::env::temp_dir().join("aura-work-area-appletsrc");
            std::fs::write(
                &path,
                "\
[Containments][1]
location=0
[Containments][2]
formfactor=2
location=4
[Containments][2][General]
location=3
[Containments][30]
location=4
",
            )
            .unwrap();
            let ids = parse_bottom_panel_ids(&path).unwrap();
            assert_eq!(ids, vec![2, 30]);
        }

        #[test]
        fn parses_panel_thicknesses() {
            let path = std::env::temp_dir().join("aura-work-area-plasmashellrc");
            std::fs::write(
                &path,
                "\
[PlasmaViews][Panel 2]
floating=0
[PlasmaViews][Panel 2][Defaults]
thickness=44
[PlasmaViews][Panel 30]
[PlasmaViews][Panel 30][Defaults]
thickness=60
",
            )
            .unwrap();
            let map = parse_panel_thicknesses(&path).unwrap();
            assert_eq!(map.get(&2), Some(&44.0));
            assert_eq!(map.get(&30), Some(&60.0));
        }

        #[test]
        fn ignores_nested_containment_headers() {
            assert_eq!(parse_containment_root_id("Containments][2]"), Some(2));
            assert_eq!(parse_containment_root_id("Containments][2][General]"), None);
            assert_eq!(parse_containment_root_id("PlasmaViews][Panel 2]"), None);
        }

        #[test]
        fn parses_xprop_workarea_bottom_panel() {
            // GNOME / XFCE with a top bar of 27px on a 1080px screen
            // → workarea y=27 h=1053 → bottom reservation = 0.
            let stdout = "_NET_WORKAREA(CARDINAL) = 0, 27, 1920, 1053\n";
            assert_eq!(parse_xprop_bottom_strut(stdout, 1080.0), Some(0.0));

            // Bottom-panel case: y=0, h=1024 on 1080px → 56px reserved
            // at the bottom.
            let stdout = "_NET_WORKAREA(CARDINAL) = 0, 0, 1920, 1024\n";
            assert_eq!(parse_xprop_bottom_strut(stdout, 1080.0), Some(56.0));
        }

        #[test]
        fn parses_xprop_workarea_rejects_virtual_root() {
            // Two monitors stacked vertically → workarea y=0, h=2160 on
            // a 1080 logical display. We can't safely interpret this.
            let stdout = "_NET_WORKAREA(CARDINAL) = 0, 0, 1920, 2160\n";
            assert_eq!(parse_xprop_bottom_strut(stdout, 1080.0), None);
        }

        #[test]
        fn parses_xprop_workarea_rejects_garbage() {
            assert_eq!(parse_xprop_bottom_strut("not a property", 1080.0), None);
            assert_eq!(parse_xprop_bottom_strut("= a, b, c, d", 1080.0), None);
            assert_eq!(parse_xprop_bottom_strut("= 0, 0, 1920", 1080.0), None);
        }
    }
}
