use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

// Each SVG is embedded at compile time so launching from any cwd works.
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
}

pub struct EmbeddedAssets;

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ALL
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(ALL.iter().map(|(name, _)| (*name).into()).collect())
    }
}

/// Raw bytes of the Aura logo SVG. Used by the tray-icon rasteriser.
pub const AURA_LOGO_SVG: &[u8] = AURA;
