use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AppState {
    /// Name of the last active agent profile. `None` means "use first in config".
    pub active_profile: Option<String>,
}

impl AppState {
    /// Default on-disk location: `$XDG_DATA_HOME/aura/state.json`.
    pub fn state_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("aura")
            .join("state.json")
    }

    /// Load from an explicit path. Returns `Default` when the file is absent.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("read state file {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parse state file {}", path.display()))
    }

    /// Save to an explicit path, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("serialize state")?;
        fs::write(path, json).with_context(|| format!("write state file {}", path.display()))
    }

    /// Convenience: load from the default platform path.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::state_path())
    }

    /// Convenience: save to the default platform path.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::state_path())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trips_through_json() {
        let state = AppState {
            active_profile: Some("Claude Code (Enterprise)".to_string()),
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: AppState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn load_from_returns_default_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let state = AppState::load_from(&path).unwrap();
        assert_eq!(state, AppState::default());
        assert!(state.active_profile.is_none());
    }

    #[test]
    fn save_to_and_load_from_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");

        let original = AppState {
            active_profile: Some("My Profile".to_string()),
        };
        original.save_to(&path).unwrap();

        // File should now exist.
        assert!(path.exists());

        let loaded = AppState::load_from(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn save_to_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c").join("state.json");

        AppState::default().save_to(&deep).unwrap();
        assert!(deep.exists());
    }
}
