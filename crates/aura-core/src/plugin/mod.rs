pub mod discovery;
pub mod runner;

pub use discovery::{
    add_plugin, discover_plugins, plugins_dir_for_config, read_sidecar, remove_plugin,
    sidecar_path_for, user_plugins_dir, AddOptions, AddOutcome, PluginSidecar, RemoveOutcome,
};
pub use runner::PluginRunner;

use serde::{Deserialize, Serialize};

// ── Section content variants ──────────────────────────────────────────────────

/// A single key/value line rendered in a plugin section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginLine {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub highlight: bool,
    /// Optional 0.0–1.0 fill ratio. When present the renderer draws a
    /// progress bar under the value (used e.g. for the rtk-gains
    /// "Efficiency meter").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

/// One row in a `Table`-typed section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginRow {
    pub cells: Vec<String>,
    #[serde(default)]
    pub highlight: bool,
    /// Optional 0.0–1.0 fill ratio. When present the renderer draws a
    /// trailing "Impact" bar at the end of the row (used e.g. for the
    /// rtk-gains "By Command" impact column).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

/// A clickable pill inside a `Controls` section. Clicking it makes the
/// host re-invoke the plugin as `<cmd> action <id> --period <p>`; the
/// plugin performs the operation and prints a refreshed panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginButton {
    /// Action identifier handed back to the plugin verbatim.
    pub id: String,
    /// Pill text. May be empty when `icon` is set (icon-only pill).
    pub label: String,
    /// Render as the current selection (accent background).
    #[serde(default)]
    pub active: bool,
    /// Render in the error color (destructive actions).
    #[serde(default)]
    pub danger: bool,
    /// Optional SVG icon rendered before the label: an embedded asset
    /// name (`icons/close.svg`), an absolute path, or a `~/` path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Two-click confirmation. When set, the first click arms the
    /// button (it re-renders with this label in the error color) and
    /// only a second click fires the action. Clicking anything else
    /// disarms. Use for destructive actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// One labeled row of buttons in a `Controls` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginControl {
    /// Left-hand label for the row.
    pub label: String,
    /// Optional dim second line under the label (e.g. a path or status).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Nesting depth. Rows with `indent > 0` render inset under the
    /// previous shallower row with a vertical guide bar, making the
    /// parent/child relationship visible (e.g. events under a profile).
    #[serde(default)]
    pub indent: u8,
    /// Buttons rendered after the label. May be empty (status-only row).
    #[serde(default)]
    pub buttons: Vec<PluginButton>,
}

/// What kind of content a section holds. Tagged on the wire as
/// `{"type": "lines", ...}` / `{"type": "table", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginContent {
    /// Key/value lines (the original layout).
    Lines { lines: Vec<PluginLine> },
    /// Tabular data with headers + rows.
    Table {
        headers: Vec<String>,
        rows: Vec<PluginRow>,
    },
    /// Free-form text block (preformatted). Useful for ASCII charts.
    Text { text: String },
    /// Interactive rows of action buttons (see [`PluginButton`]).
    Controls { controls: Vec<PluginControl> },
}

impl Default for PluginContent {
    fn default() -> Self {
        Self::Lines { lines: Vec::new() }
    }
}

// ── Section + payload ────────────────────────────────────────────────────────

/// One tab/section inside a plugin panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginSection {
    /// Stable identifier (used for tab selection state).
    pub id: String,
    /// Human label shown on the tab.
    pub label: String,
    /// Whether this section filters data by the active period. When `false`
    /// the UI hides the period-pill row so the pills aren't misleading.
    /// Defaults to `true` for backwards compatibility.
    #[serde(default = "default_true")]
    pub uses_period: bool,
    #[serde(flatten)]
    pub content: PluginContent,
}

fn default_true() -> bool {
    true
}

/// The full payload a plugin emits on stdout.
///
/// Backwards-compatible: if a plugin still emits the old flat
/// `{title, lines, error}` shape, the runner wraps it into a single
/// "default" section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginPanel {
    pub title: String,
    #[serde(default)]
    pub sections: Vec<PluginSection>,
    /// If `Some`, the UI shows the error in place of the sections.
    #[serde(default)]
    pub error: Option<String>,
}

impl PluginPanel {
    pub fn from_error(title: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            sections: Vec::new(),
            error: Some(msg.into()),
        }
    }

    /// Find a section by id.
    pub fn section(&self, id: &str) -> Option<&PluginSection> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// Whether any section filters by the active period. Error panels and
    /// panels with no sections return `true` so the host re-runs them on
    /// period change (retry rather than cache a failure).
    pub fn uses_period(&self) -> bool {
        self.error.is_some()
            || self.sections.is_empty()
            || self.sections.iter().any(|s| s.uses_period)
    }
}

// ── Legacy wire format (single flat panel) ────────────────────────────────────

/// Older plugins emit `{title, lines: [...], error}` with no `sections` field.
/// The runner accepts that shape and wraps it into a default section.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LegacyPluginPanel {
    pub title: String,
    #[serde(default)]
    pub lines: Vec<PluginLine>,
    #[serde(default)]
    pub error: Option<String>,
}

impl From<LegacyPluginPanel> for PluginPanel {
    fn from(legacy: LegacyPluginPanel) -> Self {
        if legacy.error.is_some() || legacy.lines.is_empty() {
            return PluginPanel {
                title: legacy.title,
                sections: Vec::new(),
                error: legacy.error,
            };
        }
        PluginPanel {
            title: legacy.title,
            sections: vec![PluginSection {
                id: "default".to_string(),
                label: "Overview".to_string(),
                uses_period: true,
                content: PluginContent::Lines {
                    lines: legacy.lines,
                },
            }],
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(uses_period: bool) -> PluginSection {
        PluginSection {
            id: "s".to_string(),
            label: "S".to_string(),
            uses_period,
            content: PluginContent::default(),
        }
    }

    #[test]
    fn controls_section_roundtrips() {
        let json = r#"{
            "id": "agents", "label": "Agents", "uses_period": false,
            "type": "controls",
            "controls": [{
                "label": "Peh",
                "hint": "hooks: Stop",
                "indent": 1,
                "buttons": [
                    {"id": "agent:Peh:tags", "label": "tags", "active": true},
                    {"id": "hooks:Peh:remove", "label": "Remove", "danger": true,
                     "icon": "icons/close.svg", "confirm": "Sure?"}
                ]
            }]
        }"#;
        let section: PluginSection = serde_json::from_str(json).unwrap();
        let PluginContent::Controls { controls } = &section.content else {
            panic!("expected Controls, got {:?}", section.content);
        };
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].indent, 1);
        assert_eq!(controls[0].buttons[0].id, "agent:Peh:tags");
        assert!(controls[0].buttons[0].active);
        assert!(!controls[0].buttons[0].danger);
        assert!(controls[0].buttons[0].confirm.is_none());
        assert!(controls[0].buttons[1].danger);
        assert_eq!(
            controls[0].buttons[1].icon.as_deref(),
            Some("icons/close.svg")
        );
        assert_eq!(controls[0].buttons[1].confirm.as_deref(), Some("Sure?"));
        let back = serde_json::to_string(&section).unwrap();
        let again: PluginSection = serde_json::from_str(&back).unwrap();
        assert_eq!(section, again);
    }

    #[test]
    fn uses_period_true_for_error_panel() {
        let panel = PluginPanel::from_error("t", "boom");
        assert!(panel.uses_period());
    }

    #[test]
    fn uses_period_true_for_empty_sections() {
        let panel = PluginPanel {
            title: "t".to_string(),
            sections: Vec::new(),
            error: None,
        };
        assert!(panel.uses_period());
    }

    #[test]
    fn uses_period_false_when_no_section_uses_it() {
        let panel = PluginPanel {
            title: "t".to_string(),
            sections: vec![section(false), section(false)],
            error: None,
        };
        assert!(!panel.uses_period());
    }

    #[test]
    fn uses_period_true_when_any_section_uses_it() {
        let panel = PluginPanel {
            title: "t".to_string(),
            sections: vec![section(false), section(true)],
            error: None,
        };
        assert!(panel.uses_period());
    }
}
