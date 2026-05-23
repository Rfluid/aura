/// Raw bytes of the Aura logo SVG. Used by the tray-icon rasteriser on
/// every platform — that path is GPUI-free, so it stays compiled even on
/// macOS where the modal UI is gated out.
pub const AURA_LOGO_SVG: &[u8] = include_bytes!("../../../assets/icons/aura.svg");

macro_rules! icon_assets {
    ( $( ($name:ident, $file:literal) ),* $(,)? ) => {
        $(
            const $name: &[u8] = include_bytes!(concat!("../../../assets/icons/", $file));
        )*

        const ALL: &[(&str, &[u8])] = &[
            $( (concat!("icons/", $file), $name), )*
        ];
    };
}

icon_assets! {
    (AURA,             "aura.svg"),
    (CLAUDE,           "claude.svg"),
    (OPENAI,           "openai.svg"),
    (GEMINI,           "gemini.svg"),
    (DEFAULT,          "default.svg"),
    (CLOSE,            "close.svg"),
    (ROTATE_CW,        "rotate_cw.svg"),
    (SETTINGS,         "settings.svg"),
    (ELLIPSIS,         "ellipsis.svg"),
    (SLIDERS,          "sliders.svg"),
    (DOWNLOAD,         "download.svg"),
    (SPARKLE,          "sparkle.svg"),
    (CIRCLE_HELP,      "circle_help.svg"),
    (ARROW_UP_RIGHT,   "arrow_up_right.svg"),
    (BLOCKS,           "blocks.svg"),
    (CHECK,            "check.svg"),
    (CHEVRON_DOWN,     "chevron_down.svg"),
    (CHEVRON_UP,       "chevron_up.svg"),
    (INFO,             "info.svg"),
    (RTK,              "rtk.svg"),
}

// GPUI `AssetSource` adapter that feeds SVGs into the modal view.
mod gpui_source {
    use std::borrow::Cow;

    use anyhow::Result;
    use gpui::{AssetSource, SharedString};

    use super::ALL;

    pub struct EmbeddedAssets;

    impl AssetSource for EmbeddedAssets {
        fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
            // 1. Baked-in asset (e.g. `"icons/blocks.svg"`).
            if let Some((_, bytes)) = ALL.iter().find(|(name, _)| *name == path) {
                return Ok(Some(Cow::Borrowed(*bytes)));
            }

            // 2. Filesystem fallback: lets plugins point at a custom SVG via
            //    `icon = "/abs/path.svg"` or `icon = "~/icons/foo.svg"` in
            //    config.toml. The asset system caches the loaded bytes so a
            //    repeated lookup doesn't keep hitting disk.
            if path.starts_with('/') || path.starts_with('~') {
                let resolved = expand_tilde_path(path);
                match std::fs::read(&resolved) {
                    Ok(bytes) => return Ok(Some(Cow::Owned(bytes))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(None)
        }

        fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
            Ok(ALL.iter().map(|(name, _)| (*name).into()).collect())
        }
    }

    fn expand_tilde_path(path: &str) -> std::path::PathBuf {
        if let Some(rest) = path.strip_prefix("~/") {
            return dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(rest);
        }
        if path == "~" {
            return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
        }
        std::path::PathBuf::from(path)
    }
}

pub use gpui_source::EmbeddedAssets;
