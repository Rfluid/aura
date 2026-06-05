//! Cross-platform at-rest storage for the Fleet pairing secret.
//!
//! Mirrors the pattern in [`crate::quota::oauth`] but with a **dedicated**
//! service name `"aura-fleet-secret"` — it never touches Claude Code's
//! `"Claude Code-credentials"` entry. The secret (32 raw bytes) is stored
//! base64-encoded so every backend handles it as a plain string.
//!
//! Backends:
//! - **macOS** — login Keychain via `security-framework` (same crate
//!   `quota/oauth.rs` already uses).
//! - **Windows** — Credential Manager via the `keyring` crate's
//!   `windows-native` backend (already a dependency for OAuth).
//! - **Linux** — Secret Service (libsecret / GNOME Keyring / KWallet) via the
//!   `keyring` crate's `sync-secret-service` backend. When no secret service
//!   is reachable (headless servers, no DBus session) we **fall back to a 0600
//!   file** under the data dir, exactly as `oauth.rs` falls back to the
//!   on-disk credential file.
//!
//! # Fallback security note
//!
//! The file fallback (`$XDG_DATA_HOME/aura/fleet-secret`) is protected only by
//! Unix file permissions (`0600`, owner-only). That is strictly weaker than a
//! hardware-backed or session-encrypted keychain: anyone who can read the
//! user's home dir (root, a backup, a misconfigured sync tool) can read the
//! secret. It exists so Fleet works at all on headless boxes; the keychain is
//! always preferred and only used as a last resort. Documented in
//! `docs/fleet.md`.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use uuid::Uuid;

use super::pairing::PairingSecret;

/// Service / target name for the Fleet secret. **Distinct** from Claude Code's
/// `"Claude Code-credentials"` so we never read or clobber the OAuth blob.
const SERVICE: &str = "aura-fleet-secret";

/// Account name under the service entry. Resolved to the OS user so multi-user
/// machines don't collide.
fn account() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "aura".to_string())
}

/// Directory backing the file fallback + the machine-id file.
fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("aura")
}

/// Path of the 0600 file fallback for the secret (Linux headless only).
fn fallback_path() -> PathBuf {
    data_dir().join("fleet-secret")
}

/// Path of the persisted per-install machine id.
fn machine_id_path() -> PathBuf {
    data_dir().join("fleet-machine-id")
}

// ── Machine id ───────────────────────────────────────────────────────────────

/// Stable random per-install machine id (uuid v4). Generated and persisted on
/// first read so two machines with the same hostname still disambiguate. Not a
/// secret — it only identifies a row in the peer table.
pub fn machine_id() -> Result<String> {
    let path = machine_id_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create data dir {}", parent.display()))?;
    }
    std::fs::write(&path, &id)
        .with_context(|| format!("write machine id to {}", path.display()))?;
    Ok(id)
}

// ── Secret get / set / delete ────────────────────────────────────────────────

/// Read the pairing secret, or `Ok(None)` when no fleet has been paired yet.
pub fn get() -> Result<Option<PairingSecret>> {
    let encoded = read_raw()?;
    match encoded {
        None => Ok(None),
        Some(s) => {
            let bytes = B64
                .decode(s.trim())
                .context("decoding stored fleet secret")?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("stored fleet secret is not 32 bytes"))?;
            Ok(Some(PairingSecret::from_bytes(arr)))
        }
    }
}

/// Store (or overwrite) the pairing secret.
pub fn set(secret: &PairingSecret) -> Result<()> {
    let encoded = B64.encode(secret.as_bytes());
    write_raw(&encoded)
}

/// Delete the pairing secret ("Leave fleet"). Idempotent: a missing entry is
/// not an error.
pub fn delete() -> Result<()> {
    delete_raw()
}

// ── Platform backends ────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn read_raw() -> Result<Option<String>> {
    use security_framework::passwords::get_generic_password;
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    match get_generic_password(SERVICE, &account()) {
        Ok(bytes) => Ok(Some(
            String::from_utf8(bytes).context("fleet secret blob is not UTF-8")?,
        )),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        Err(e) => Err(anyhow!("Keychain read failed (OSStatus {})", e.code())),
    }
}

#[cfg(target_os = "macos")]
fn write_raw(value: &str) -> Result<()> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(SERVICE, &account(), value.as_bytes())
        .map_err(|e| anyhow!("Keychain write failed (OSStatus {})", e.code()))
}

#[cfg(target_os = "macos")]
fn delete_raw() -> Result<()> {
    use security_framework::passwords::delete_generic_password;
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    match delete_generic_password(SERVICE, &account()) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(e) => Err(anyhow!("Keychain delete failed (OSStatus {})", e.code())),
    }
}

#[cfg(target_os = "windows")]
fn read_raw() -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE, &account()).context("constructing keyring entry")?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("Credential Manager read failed: {e}")),
    }
}

#[cfg(target_os = "windows")]
fn write_raw(value: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account()).context("constructing keyring entry")?;
    entry
        .set_password(value)
        .map_err(|e| anyhow!("Credential Manager write failed: {e}"))
}

#[cfg(target_os = "windows")]
fn delete_raw() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account()).context("constructing keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("Credential Manager delete failed: {e}")),
    }
}

// On Linux we try the Secret Service first and fall back to a 0600 file when
// no secret service is reachable (headless / no DBus), mirroring oauth.rs's
// file fallback.
#[cfg(target_os = "linux")]
fn read_raw() -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE, &account()) {
        Ok(entry) => match entry.get_password() {
            Ok(v) => return Ok(Some(v)),
            Err(keyring::Error::NoEntry) => {
                // No keychain entry — still check the file fallback in case a
                // previous headless run wrote one.
            }
            Err(e) => {
                eprintln!("aura: Secret Service read failed ({e}); trying file fallback");
            }
        },
        Err(e) => {
            eprintln!("aura: Secret Service unavailable ({e}); using file fallback");
        }
    }
    read_file_fallback()
}

#[cfg(target_os = "linux")]
fn write_raw(value: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE, &account()) {
        Ok(entry) => match entry.set_password(value) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("aura: Secret Service write failed ({e}); using file fallback");
            }
        },
        Err(e) => {
            eprintln!("aura: Secret Service unavailable ({e}); using file fallback");
        }
    }
    write_file_fallback(value)
}

#[cfg(target_os = "linux")]
fn delete_raw() -> Result<()> {
    if let Ok(entry) = keyring::Entry::new(SERVICE, &account()) {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => eprintln!("aura: Secret Service delete failed ({e})"),
        }
    }
    // Always also clear any file fallback so "Leave fleet" is thorough.
    let path = fallback_path();
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing fleet secret file {}", path.display()))?;
    }
    Ok(())
}

// ── File fallback (Linux headless) ───────────────────────────────────────────

#[cfg(target_os = "linux")]
fn read_file_fallback() -> Result<Option<String>> {
    let path = fallback_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("reading fleet secret file {}: {e}", path.display())),
    }
}

#[cfg(target_os = "linux")]
fn write_file_fallback(value: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = fallback_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create data dir {}", parent.display()))?;
    }
    // Create with 0600 from the start so the secret is never briefly
    // world-readable between create and chmod.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("opening fleet secret file {}", path.display()))?;
    file.write_all(value.as_bytes())
        .with_context(|| format!("writing fleet secret file {}", path.display()))?;
    Ok(())
}

// Catch-all for platforms we don't target (BSDs etc.): file fallback only.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn read_raw() -> Result<Option<String>> {
    let path = fallback_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("reading fleet secret file {}: {e}", path.display())),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn write_raw(value: &str) -> Result<()> {
    let path = fallback_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, value)
        .with_context(|| format!("writing fleet secret file {}", path.display()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn delete_raw() -> Result<()> {
    let path = fallback_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
