use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};

// ── Per-model entry in the cache ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CacheModelUsage {
    #[serde(rename = "inputTokens", default)]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens", default)]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadInputTokens", default)]
    pub cache_read_input_tokens: u64,
    #[serde(rename = "cacheCreationInputTokens", default)]
    pub cache_creation_input_tokens: u64,
}

// ── Daily entries ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CacheDailyActivity {
    pub date: String,
    #[serde(rename = "messageCount", default)]
    pub message_count: u64,
    #[serde(rename = "sessionCount", default)]
    pub session_count: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CacheDailyModelTokens {
    pub date: String,
    #[serde(rename = "tokensByModel", default)]
    pub tokens_by_model: HashMap<String, u64>,
}

// ── Longest session entry ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CacheLongestSession {
    /// Duration in **milliseconds**.
    #[serde(default)]
    pub duration: u64,
    pub timestamp: String,
}

// ── Root cache struct ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct StatsCache {
    #[serde(default)]
    pub version: u32,
    /// Last calendar day whose data was fully rolled into this cache.
    #[serde(rename = "lastComputedDate", default)]
    pub last_computed_date: String,
    #[serde(rename = "dailyActivity", default)]
    pub daily_activity: Vec<CacheDailyActivity>,
    #[serde(rename = "dailyModelTokens", default)]
    pub daily_model_tokens: Vec<CacheDailyModelTokens>,
    #[serde(rename = "modelUsage", default)]
    pub model_usage: HashMap<String, CacheModelUsage>,
    #[serde(rename = "totalSessions", default)]
    pub total_sessions: u64,
    #[serde(rename = "totalMessages", default)]
    pub total_messages: u64,
    #[serde(rename = "longestSession")]
    pub longest_session: Option<CacheLongestSession>,
    /// ISO 8601 timestamp of the very first session.
    #[serde(rename = "firstSessionDate")]
    pub first_session_date: Option<String>,
    /// Map of local-hour string ("0"–"23") → session-start count.
    #[serde(rename = "hourCounts", default)]
    pub hour_counts: HashMap<String, u64>,
}

impl StatsCache {
    /// Load `stats-cache.json` from `config_path`. Returns `None` if the file
    /// does not exist; propagates parse errors.
    pub fn load(config_path: &Path) -> anyhow::Result<Option<Self>> {
        let path = config_path.join("stats-cache.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let cache: StatsCache = serde_json::from_str(&content)?;
        Ok(Some(cache))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn load_returns_none_when_missing() {
        let dir = tempdir().unwrap();
        let result = StatsCache::load(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_parses_real_schema() {
        let dir = tempdir().unwrap();
        let json = serde_json::json!({
            "version": 2,
            "lastComputedDate": "2026-02-16",
            "dailyActivity": [
                { "date": "2026-01-15", "messageCount": 246, "sessionCount": 1 }
            ],
            "dailyModelTokens": [
                { "date": "2026-01-15", "tokensByModel": { "claude-opus-4-6": 33630 } }
            ],
            "modelUsage": {
                "claude-opus-4-6": {
                    "inputTokens": 141539,
                    "outputTokens": 383584,
                    "cacheReadInputTokens": 93765314,
                    "cacheCreationInputTokens": 6441101
                }
            },
            "totalSessions": 94,
            "totalMessages": 18175,
            "longestSession": {
                "duration": 116457388,
                "timestamp": "2026-01-15T20:22:24.514Z"
            },
            "firstSessionDate": "2026-01-15T20:22:24.514Z",
            "hourCounts": { "15": 13, "17": 17 }
        });

        let path = dir.path().join("stats-cache.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{}", json).unwrap();

        let cache = StatsCache::load(dir.path()).unwrap().unwrap();
        assert_eq!(cache.last_computed_date, "2026-02-16");
        assert_eq!(cache.total_sessions, 94);
        assert_eq!(cache.daily_activity.len(), 1);
        assert_eq!(cache.daily_activity[0].message_count, 246);
        let ou = cache.model_usage.get("claude-opus-4-6").unwrap();
        assert_eq!(ou.input_tokens, 141539);
        assert_eq!(*cache.hour_counts.get("17").unwrap(), 17);
        assert_eq!(cache.longest_session.as_ref().unwrap().duration, 116457388);
    }
}
