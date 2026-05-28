//! Read, refresh, and write back Codex (ChatGPT) OAuth credentials.
//!
//! Schema of `~/.codex/auth.json` (the fields we care about):
//! ```json
//! {
//!   "OPENAI_API_KEY": null,
//!   "tokens": {
//!     "id_token":      "<JWT — chatgpt_account_id, chatgpt_plan_type claims>",
//!     "access_token":  "<JWT — exp claim>",
//!     "refresh_token": "rt_…",
//!     "account_id":    "<uuid>"
//!   },
//!   "last_refresh": "2026-05-25T20:55:27.651480222Z"
//! }
//! ```
//!
//! Mirrors `quota/oauth.rs` (Claude Code) but talks to OpenAI's auth service.
//! Refresh endpoint and client_id are the same values the upstream Codex CLI
//! uses for its own token rotation.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};

const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Refresh proactively when the token has < this much life left.
const REFRESH_LEEWAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct CodexTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: Option<String>,
    pub chatgpt_plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

/// Path to `~/.codex/auth.json` (or equivalent under a custom config dir).
pub fn auth_path(codex_config_dir: &Path) -> PathBuf {
    codex_config_dir.join("auth.json")
}

/// Read the auth file and extract the ChatGPT OAuth tokens.
///
/// `account_id` and `chatgpt_plan_type` come from the `id_token` JWT claims
/// when present, falling back to `tokens.account_id` for the account id.
pub fn read(codex_config_dir: &Path) -> Result<CodexTokens> {
    let path = auth_path(codex_config_dir);
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading auth file at {}", path.display()))?;
    let json: Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing auth file at {}", path.display()))?;

    let tokens = json
        .get("tokens")
        .ok_or_else(|| anyhow!("auth.json missing `tokens` block"))?;

    let id_token = string_field(tokens, "id_token")
        .ok_or_else(|| anyhow!("auth.json missing `tokens.id_token`"))?;
    let access_token = string_field(tokens, "access_token")
        .ok_or_else(|| anyhow!("auth.json missing `tokens.access_token`"))?;
    let refresh_token = string_field(tokens, "refresh_token")
        .ok_or_else(|| anyhow!("auth.json missing `tokens.refresh_token`"))?;

    let file_account_id = string_field(tokens, "account_id");
    let claims = decode_jwt_claims(&id_token).ok();
    let claim_account_id = claims
        .as_ref()
        .and_then(|c| chatgpt_auth_claim(c, "chatgpt_account_id"));
    let claim_plan_type = claims
        .as_ref()
        .and_then(|c| chatgpt_auth_claim(c, "chatgpt_plan_type"));

    Ok(CodexTokens {
        id_token,
        access_token,
        refresh_token,
        account_id: claim_account_id.or(file_account_id),
        chatgpt_plan_type: claim_plan_type,
    })
}

/// Returns true when the access token is missing an `exp` claim, fails to
/// parse, or expires within the leeway. Fail-closed so a malformed token
/// triggers a refresh attempt rather than a stale API call.
pub fn is_expired(tokens: &CodexTokens) -> bool {
    let now = now_secs();
    let leeway = REFRESH_LEEWAY.as_secs() as i64;
    match access_token_exp(&tokens.access_token) {
        Some(exp) => exp <= now + leeway,
        None => true,
    }
}

/// Hit `auth.openai.com/oauth/token` with `grant_type=refresh_token` and
/// return a new `CodexTokens`. Preserves `account_id` and `chatgpt_plan_type`
/// from the input. Falls back to existing token values if a particular field
/// is absent from the response (the upstream sometimes omits one of the
/// three token fields when nothing changed).
pub fn refresh(tokens: &CodexTokens) -> Result<CodexTokens> {
    let body = serde_json::json!({
        "client_id":     CLIENT_ID,
        "grant_type":    "refresh_token",
        "refresh_token": &tokens.refresh_token,
    });

    let response = ureq::post(REFRESH_URL)
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

    let id_token = parsed.id_token.unwrap_or_else(|| tokens.id_token.clone());
    let access_token = parsed
        .access_token
        .unwrap_or_else(|| tokens.access_token.clone());
    let refresh_token = parsed
        .refresh_token
        .unwrap_or_else(|| tokens.refresh_token.clone());

    // Re-read account_id / plan_type from the fresh id_token where possible.
    let claims = decode_jwt_claims(&id_token).ok();
    let account_id = claims
        .as_ref()
        .and_then(|c| chatgpt_auth_claim(c, "chatgpt_account_id"))
        .or_else(|| tokens.account_id.clone());
    let chatgpt_plan_type = claims
        .as_ref()
        .and_then(|c| chatgpt_auth_claim(c, "chatgpt_plan_type"))
        .or_else(|| tokens.chatgpt_plan_type.clone());

    Ok(CodexTokens {
        id_token,
        access_token,
        refresh_token,
        account_id,
        chatgpt_plan_type,
    })
}

/// Atomically write the refreshed tokens back into `auth.json`, preserving
/// any extra top-level keys we don't model (e.g. `OPENAI_API_KEY`).
pub fn save(codex_config_dir: &Path, fresh: &CodexTokens) -> Result<()> {
    let path = auth_path(codex_config_dir);
    let existing = fs::read_to_string(&path)
        .with_context(|| format!("reading auth file at {}", path.display()))?;
    let mut json: Value = serde_json::from_str(&existing)
        .with_context(|| format!("parsing auth file at {}", path.display()))?;

    let root = json
        .as_object_mut()
        .ok_or_else(|| anyhow!("auth.json is not a JSON object"))?;

    let tokens_entry = root
        .entry("tokens".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let tokens_obj = tokens_entry
        .as_object_mut()
        .ok_or_else(|| anyhow!("auth.json `tokens` is not a JSON object"))?;
    tokens_obj.insert("id_token".into(), Value::String(fresh.id_token.clone()));
    tokens_obj.insert(
        "access_token".into(),
        Value::String(fresh.access_token.clone()),
    );
    tokens_obj.insert(
        "refresh_token".into(),
        Value::String(fresh.refresh_token.clone()),
    );
    if let Some(account_id) = &fresh.account_id {
        tokens_obj.insert("account_id".into(), Value::String(account_id.clone()));
    }

    root.insert("last_refresh".into(), Value::String(format_rfc3339_now()));

    let serialized = serde_json::to_string_pretty(&json)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f =
            fs::File::create(&tmp).with_context(|| format!("creating tmp at {}", tmp.display()))?;
        f.write_all(serialized.as_bytes())?;
        f.sync_all().ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&tmp) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(&tmp, perms);
        }
    }
    fs::rename(&tmp, &path).context("atomic rename of auth.json")?;
    Ok(())
}

/// Convenience: read tokens, refresh if expired, persist the refreshed copy,
/// return the freshest tokens.
pub fn ensure_fresh(codex_config_dir: &Path) -> Result<CodexTokens> {
    let tokens = read(codex_config_dir)?;
    if !is_expired(&tokens) {
        return Ok(tokens);
    }
    let new = refresh(&tokens)?;
    if let Err(e) = save(codex_config_dir, &new) {
        eprintln!("warning: failed to persist refreshed Codex tokens: {e}");
    }
    Ok(new)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Decode the middle segment of a JWT (`header.payload.sig`) into JSON.
/// Returns an error if the input is malformed; callers fail-closed.
fn decode_jwt_claims(jwt: &str) -> Result<Value> {
    let mut parts = jwt.split('.');
    let _header = parts.next().ok_or_else(|| anyhow!("JWT missing header"))?;
    let payload = parts.next().ok_or_else(|| anyhow!("JWT missing payload"))?;
    let decoded = base64url_decode(payload).context("JWT payload base64 decode")?;
    let claims: Value = serde_json::from_slice(&decoded).context("JWT payload JSON parse")?;
    Ok(claims)
}

/// Pull `exp` (seconds since epoch) from the access_token's JWT payload.
fn access_token_exp(jwt: &str) -> Option<i64> {
    let claims = decode_jwt_claims(jwt).ok()?;
    claims.get("exp").and_then(Value::as_i64)
}

/// ChatGPT-specific claims live under the namespaced object
/// `https://api.openai.com/auth` in both id_tokens and access_tokens.
fn chatgpt_auth_claim(claims: &Value, key: &str) -> Option<String> {
    let obj = claims.get("https://api.openai.com/auth")?;
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Minimal base64url decoder (RFC 4648 §5, no padding required). Avoids
/// pulling in the `base64` crate as a direct dependency just for this.
fn base64url_decode(input: &str) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4 + 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '-' => 62,
            '_' => 63,
            '=' => break,
            _ => return Err(anyhow!("invalid base64url character: {c:?}")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Ok(buf)
}

fn format_rfc3339_now() -> String {
    use chrono::Utc;
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_jwt(payload: &Value) -> String {
        let payload_b64 = base64url_encode(serde_json::to_vec(payload).unwrap().as_slice());
        format!("eyJhbGciOiJub25lIn0.{payload_b64}.sig")
    }

    fn base64url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
        let mut acc: u32 = 0;
        let mut bits: u32 = 0;
        for &b in bytes {
            acc = (acc << 8) | (b as u32);
            bits += 8;
            while bits >= 6 {
                bits -= 6;
                out.push(ALPHABET[((acc >> bits) & 0x3F) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(ALPHABET[((acc << (6 - bits)) & 0x3F) as usize] as char);
        }
        out
    }

    #[test]
    fn is_expired_returns_false_for_future_exp() {
        let exp = now_secs() + 3600;
        let access_token = make_jwt(&serde_json::json!({ "exp": exp }));
        let tokens = CodexTokens {
            id_token: "x.y.z".to_string(),
            access_token,
            refresh_token: "rt".to_string(),
            account_id: None,
            chatgpt_plan_type: None,
        };
        assert!(!is_expired(&tokens));
    }

    #[test]
    fn is_expired_returns_true_for_past_exp() {
        let exp = now_secs() - 3600;
        let access_token = make_jwt(&serde_json::json!({ "exp": exp }));
        let tokens = CodexTokens {
            id_token: "x.y.z".to_string(),
            access_token,
            refresh_token: "rt".to_string(),
            account_id: None,
            chatgpt_plan_type: None,
        };
        assert!(is_expired(&tokens));
    }

    #[test]
    fn is_expired_fail_closed_on_garbage() {
        let tokens = CodexTokens {
            id_token: "x.y.z".to_string(),
            access_token: "not-a-jwt".to_string(),
            refresh_token: "rt".to_string(),
            account_id: None,
            chatgpt_plan_type: None,
        };
        assert!(is_expired(&tokens));
    }

    #[test]
    fn read_extracts_account_and_plan_from_id_token_claims() {
        let dir = tempdir().unwrap();
        let id_token = make_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-from-claim",
                "chatgpt_plan_type":  "pro",
            },
            "exp": now_secs() + 3600,
        }));
        let access_token = make_jwt(&serde_json::json!({ "exp": now_secs() + 3600 }));
        let auth = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token":      id_token,
                "access_token":  access_token,
                "refresh_token": "rt_abc",
                "account_id":    "acct-from-file",
            },
            "last_refresh": "2026-05-25T20:55:27.651480222Z"
        });
        fs::write(
            dir.path().join("auth.json"),
            serde_json::to_string_pretty(&auth).unwrap(),
        )
        .unwrap();

        let tokens = read(dir.path()).unwrap();
        // Claim wins over the file field when present.
        assert_eq!(tokens.account_id.as_deref(), Some("acct-from-claim"));
        assert_eq!(tokens.chatgpt_plan_type.as_deref(), Some("pro"));
        assert_eq!(tokens.refresh_token, "rt_abc");
    }

    #[test]
    fn read_falls_back_to_file_account_id_when_id_token_lacks_claim() {
        let dir = tempdir().unwrap();
        let id_token = make_jwt(&serde_json::json!({ "exp": now_secs() + 3600 }));
        let access_token = make_jwt(&serde_json::json!({ "exp": now_secs() + 3600 }));
        let auth = serde_json::json!({
            "tokens": {
                "id_token":      id_token,
                "access_token":  access_token,
                "refresh_token": "rt_abc",
                "account_id":    "acct-from-file",
            }
        });
        fs::write(
            dir.path().join("auth.json"),
            serde_json::to_string_pretty(&auth).unwrap(),
        )
        .unwrap();

        let tokens = read(dir.path()).unwrap();
        assert_eq!(tokens.account_id.as_deref(), Some("acct-from-file"));
        assert!(tokens.chatgpt_plan_type.is_none());
    }

    #[test]
    fn save_preserves_unknown_top_level_keys() {
        let dir = tempdir().unwrap();
        let original = serde_json::json!({
            "OPENAI_API_KEY": "sk-keep-me",
            "tokens": {
                "id_token":      "old.id.token",
                "access_token":  "old.access.token",
                "refresh_token": "old-rt",
                "account_id":    "old-acct",
            },
            "last_refresh": "2026-05-25T20:55:27.651480222Z",
            "weird_extra_key": { "nested": true }
        });
        fs::write(
            dir.path().join("auth.json"),
            serde_json::to_string_pretty(&original).unwrap(),
        )
        .unwrap();

        let fresh = CodexTokens {
            id_token: "new.id.token".to_string(),
            access_token: "new.access.token".to_string(),
            refresh_token: "new-rt".to_string(),
            account_id: Some("new-acct".to_string()),
            chatgpt_plan_type: Some("plus".to_string()),
        };
        save(dir.path(), &fresh).unwrap();

        let after: Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(after["OPENAI_API_KEY"], Value::String("sk-keep-me".into()));
        assert_eq!(after["weird_extra_key"]["nested"], Value::Bool(true));
        assert_eq!(after["tokens"]["id_token"], "new.id.token");
        assert_eq!(after["tokens"]["access_token"], "new.access.token");
        assert_eq!(after["tokens"]["refresh_token"], "new-rt");
        assert_eq!(after["tokens"]["account_id"], "new-acct");
        // last_refresh got rewritten — just confirm it's a non-empty string.
        assert!(after["last_refresh"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn base64url_roundtrip() {
        for bytes in [
            &b""[..],
            &b"f"[..],
            &b"fo"[..],
            &b"foo"[..],
            &b"foob"[..],
            &b"fooba"[..],
            &b"foobar"[..],
        ] {
            let encoded = base64url_encode(bytes);
            assert_eq!(base64url_decode(&encoded).unwrap(), bytes.to_vec());
        }
    }
}
