//! Read, refresh, and write back Claude Code's OAuth credentials.
//!
//! Schema of `~/.claude/.credentials.json` (the fields we care about):
//! ```json
//! {
//!   "claudeAiOauth": {
//!     "accessToken":   "sk-ant-oat01-...",
//!     "refreshToken":  "sk-ant-ort01-...",
//!     "expiresAt":     1779062922187,            // ms since epoch
//!     "scopes":        ["user:profile", ...],
//!     "subscriptionType": "pro",
//!     "rateLimitTier":    "default_claude_ai"
//!   }
//! }
//! ```

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const REFRESH_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// Refresh proactively when the token has < this much life left.
const REFRESH_LEEWAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeOauth {
    pub access_token: String,
    pub refresh_token: String,
    /// ms since Unix epoch
    pub expires_at: i64,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub subscription_type: Option<String>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: ClaudeOauth,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64, // seconds
}

/// Path to `~/.claude/.credentials.json` (or equivalent under a custom config dir).
pub fn credentials_path(claude_config_dir: &Path) -> PathBuf {
    claude_config_dir.join(".credentials.json")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Read the credentials file and return the OAuth block.
pub fn read(claude_config_dir: &Path) -> Result<ClaudeOauth> {
    let path = credentials_path(claude_config_dir);
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading credentials at {}", path.display()))?;
    let file: CredentialsFile = serde_json::from_str(&content)
        .with_context(|| format!("parsing credentials at {}", path.display()))?;
    Ok(file.claude_ai_oauth)
}

/// Returns true when the access token is missing or expires within the leeway.
pub fn is_expired(creds: &ClaudeOauth) -> bool {
    let leeway_ms = REFRESH_LEEWAY.as_millis() as i64;
    creds.expires_at <= now_ms() + leeway_ms
}

/// Hit `platform.claude.com/v1/oauth/token` with `grant_type=refresh_token`
/// and return a new `ClaudeOauth`. Preserves `scopes`, `subscription_type`,
/// `rate_limit_tier` from the input.
pub fn refresh(creds: &ClaudeOauth) -> Result<ClaudeOauth> {
    let body = serde_json::json!({
        "grant_type":    "refresh_token",
        "refresh_token": &creds.refresh_token,
        "client_id":     CLIENT_ID,
    });

    let response = ureq::post(REFRESH_URL)
        .header("anthropic-beta", OAUTH_BETA)
        .header("content-type", "application/json")
        .send_json(&body);

    let mut response = match response {
        Ok(r) => r,
        Err(e) => return Err(anyhow!("refresh request failed: {e}")),
    };

    if response.status() != 200 {
        let status = response.status();
        let body = response
            .body_mut()
            .read_to_string()
            .unwrap_or_else(|_| "<unreadable>".to_string());
        return Err(anyhow!("refresh returned HTTP {status}: {body}"));
    }

    let parsed: RefreshResponse = response
        .body_mut()
        .read_json()
        .context("parsing refresh response")?;

    Ok(ClaudeOauth {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at: now_ms() + parsed.expires_in.saturating_mul(1000),
        scopes: creds.scopes.clone(),
        subscription_type: creds.subscription_type.clone(),
        rate_limit_tier: creds.rate_limit_tier.clone(),
    })
}

/// Atomically write the refreshed OAuth block back into the credentials file,
/// preserving any extra top-level keys we didn't model (e.g. `mcpOAuth`).
pub fn save(claude_config_dir: &Path, fresh: &ClaudeOauth) -> Result<()> {
    let path = credentials_path(claude_config_dir);
    let existing = fs::read_to_string(&path)
        .with_context(|| format!("reading credentials at {}", path.display()))?;
    let mut file: CredentialsFile = serde_json::from_str(&existing)
        .with_context(|| format!("parsing credentials at {}", path.display()))?;
    file.claude_ai_oauth = fresh.clone();

    let serialized = serde_json::to_string_pretty(&file)?;
    let tmp = path.with_extension("credentials.json.tmp");
    {
        let mut f =
            fs::File::create(&tmp).with_context(|| format!("creating tmp at {}", tmp.display()))?;
        f.write_all(serialized.as_bytes())?;
        f.sync_all().ok();
    }
    // Match the original 0600 perms when on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o600);
        let _ = fs::set_permissions(&tmp, perms);
    }
    fs::rename(&tmp, &path).context("atomic rename of credentials")?;
    Ok(())
}

/// Convenience: read creds, refresh if needed, persist the refreshed copy,
/// return the freshest credentials.
pub fn ensure_fresh(claude_config_dir: &Path) -> Result<ClaudeOauth> {
    let creds = read(claude_config_dir)?;
    if !is_expired(&creds) {
        return Ok(creds);
    }
    let new = refresh(&creds)?;
    if let Err(e) = save(claude_config_dir, &new) {
        // Non-fatal: we can still use the freshly refreshed in-memory creds.
        eprintln!("warning: failed to persist refreshed credentials: {e}");
    }
    Ok(new)
}
