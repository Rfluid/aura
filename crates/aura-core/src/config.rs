use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub kind: AgentKind,
    pub config_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub default_period: String,
    pub anchor: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_period: "all".to_string(),
            anchor: "auto".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(rename = "agents", default)]
    pub agents: Vec<AgentConfig>,
    #[serde(rename = "plugins", default)]
    pub plugins: Vec<PluginConfig>,
    #[serde(rename = "display", default)]
    pub display: DisplayConfig,
}
