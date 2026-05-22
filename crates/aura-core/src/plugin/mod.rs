pub mod runner;

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
