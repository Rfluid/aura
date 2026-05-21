pub mod runner;

pub use runner::PluginRunner;

use serde::{Deserialize, Serialize};

/// A single key/value line rendered in a plugin panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginLine {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub highlight: bool,
}

/// The full payload a plugin emits on stdout. The UI renders one
/// per plugin under the usage tabs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPanel {
    pub title: String,
    #[serde(default)]
    pub lines: Vec<PluginLine>,
    /// If `Some`, the UI shows the error in place of the lines.
    #[serde(default)]
    pub error: Option<String>,
}

impl PluginPanel {
    /// Construct an error panel — used by the runner when spawn/timeout/parse fails.
    pub fn from_error(title: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            error: Some(msg.into()),
        }
    }
}
