use std::path::PathBuf;

use anyhow::Result;

use super::{AgentReader, Period, UsageSnapshot};

/// Stub Codex adapter. Returns an empty snapshot until a real data
/// source (local files or OpenAI usage endpoint) is wired up.
pub struct CodexReader {
    #[allow(dead_code)] // honored once a real data source is wired up
    pub config_path: PathBuf,
}

impl CodexReader {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl AgentReader for CodexReader {
    fn snapshot(&self, _period: Period) -> Result<UsageSnapshot> {
        Ok(UsageSnapshot::default())
    }
}
