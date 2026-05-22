//! Available work-area detection — display rect minus reserved panels.
//!
//! Used in two places:
//!
//! 1. `app.rs` resize callback: caps the auto-grown modal height so it
//!    can't extend past the top of a bottom taskbar.
//! 2. `main.rs` `toggle_window`: anchors the modal at the bottom-right
//!    corner of the work area so it appears where the tray icon lives.
//!
//! On KDE Plasma we parse the user's panel config files directly; on
//! every other platform / parse failure we return `None` and let the
//! caller fall back to a conservative blind margin.
//!
//! ## Why config-file parsing instead of D-Bus
//!
//! KDE Plasma 6's `org.kde.PlasmaShell.StrutManager.availableScreenRect`
//! returns the **full** display rect on the versions we tested — the
//! strut manager is a setter registry that panels register themselves
//! with, but the read side doesn't subtract those struts. The panel
//! thickness is, however, reliably persisted to `~/.config/plasmashellrc`,
//! so we read it from there and correlate with
//! `~/.config/plasma-org.kde.plasma.desktop-appletsrc` to find which
//! panels live at the bottom.

use gpui::{Bounds, Pixels};

/// Returns the bottom Y (in the same coordinate space
/// `App::primary_display` uses — logical pixels, origin at the
/// display's top-left) of the available work area for `display`.
///
/// `None` means "couldn't determine a panel reservation; fall back to
/// whatever blind margin the caller has in mind". This happens on
/// non-Linux platforms, on Linux without KDE Plasma config files
/// present, or on any I/O / parse error.
///
/// The lookup is cached process-wide after the first call. Users who
/// resize their panel without restarting Aura get stale numbers — a
/// fine tradeoff vs. re-reading the files on every resize frame.
pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
    #[cfg(target_os = "linux")]
    {
        linux::available_bottom(display)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = display;
        None
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use gpui::{Bounds, Pixels};

    /// Cached `Some(thickness_px)` for the bottom panel (or the
    /// thickest of several stacked panels). `None` means we tried and
    /// couldn't determine a reservation.
    static CACHED_BOTTOM_THICKNESS: OnceLock<Option<f32>> = OnceLock::new();

    pub fn available_bottom(display: Bounds<Pixels>) -> Option<f32> {
        let thickness = (*CACHED_BOTTOM_THICKNESS.get_or_init(query_bottom_panel_thickness))?;
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
    }
}
