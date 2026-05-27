//! Background "is a newer release available?" check.
//!
//! Aura ships from GitHub releases. There is no in-app downloader (see
//! `docs/plans/update-button.md` for the rationale): we just compare the
//! local `CARGO_PKG_VERSION` against `releases/latest` from the GitHub
//! REST API and, when a newer tag is out, surface a header button that
//! opens the README's `### Updating` anchor.
//!
//! The fetch is synchronous and lives in a single function so the caller
//! can spawn it on GPUI's background executor without dragging in
//! tokio. Errors are non-UI — a degraded check (timeout, 5xx, malformed
//! body) yields `Err` and the caller drops the button silently.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use semver::Version;
use serde::Deserialize;

/// GitHub's anonymous release endpoint. Returns the metadata for the most
/// recent non-draft release. Anonymous requests are rate-limited to 60/hr
/// per IP; a once-per-launch fetch is comfortably below that.
const RELEASES_API_URL: &str = "https://api.github.com/repos/Rfluid/aura/releases/latest";

/// What the user clicks on the update button: the README's `### Updating`
/// anchor on `main`. GitHub's slug rules turn `### Updating` into
/// `#updating`, which is stable as long as the heading text doesn't
/// change.
pub const UPDATE_INSTRUCTIONS_URL: &str =
    "https://github.com/Rfluid/aura/blob/main/README.md#updating";

/// Outcome of a successful release check.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// The remote tag with the leading 'v' stripped — e.g. "0.1.18".
    pub latest: Version,
}

/// `env!("CARGO_PKG_VERSION")` parsed into a `semver::Version`. Panics at
/// compile-time-determined-string-parse only if Aura's own version
/// somehow fails to parse, which would be a build-script bug.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("aura version is valid semver")
}

/// Synchronous network call. Spawned on the background executor by
/// `AuraView::new`; never called from the render thread.
///
/// Returns `Ok(Some(info))` when a newer release exists, `Ok(None)` when
/// the local build is up-to-date or newer, and `Err(_)` on any failure
/// (timeout, non-200 status, missing field, unparseable semver).
pub fn fetch_latest() -> Result<Option<UpdateInfo>> {
    // 5s end-to-end. GitHub usually answers in well under a second, but a
    // captive-portal hijack can dribble bytes forever otherwise.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();

    // GitHub requires a User-Agent on every API request.
    let ua = format!("aura/{}", env!("CARGO_PKG_VERSION"));
    let mut response = agent
        .get(RELEASES_API_URL)
        .header("User-Agent", ua.as_str())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow!("GitHub releases call failed: {e}"))?;

    if response.status() != 200 {
        return Err(anyhow!(
            "GitHub releases returned HTTP {}",
            response.status()
        ));
    }

    let body = response
        .body_mut()
        .read_to_string()
        .context("reading releases response body")?;

    let info = parse_release_response(&body)?;
    let current = current_version();
    if info.latest > current {
        Ok(Some(info))
    } else {
        Ok(None)
    }
}

/// Stripped-down view of the GitHub `releases/latest` payload — only the
/// `tag_name` field. Everything else (release notes, asset URLs, draft
/// flags) is irrelevant here; the button just opens the README anchor.
#[derive(Debug, Deserialize)]
struct ReleaseEnvelope {
    tag_name: String,
}

/// Parse a GitHub `releases/latest` JSON body into an `UpdateInfo`. Split
/// out so the unit tests can cover the parsing without making a network
/// call.
pub fn parse_release_response(body: &str) -> Result<UpdateInfo> {
    let envelope: ReleaseEnvelope = serde_json::from_str(body).context("parsing releases JSON")?;
    let trimmed = envelope.tag_name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("releases response had empty tag_name"));
    }
    // Tags ship as `v0.1.18`; `Version::parse` rejects the leading 'v'.
    let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let latest =
        Version::parse(stripped).with_context(|| format!("parsing release tag `{trimmed}`"))?;
    Ok(UpdateInfo { latest })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_response_strips_leading_v() {
        let body = r#"{"tag_name": "v0.1.99"}"#;
        let info = parse_release_response(body).unwrap();
        assert_eq!(info.latest, Version::parse("0.1.99").unwrap());
    }

    #[test]
    fn parse_release_response_accepts_bare_semver() {
        let body = r#"{"tag_name": "1.2.3"}"#;
        let info = parse_release_response(body).unwrap();
        assert_eq!(info.latest, Version::parse("1.2.3").unwrap());
    }

    #[test]
    fn parse_release_response_rejects_missing_tag_name() {
        let body = r#"{"name": "v0.1.18"}"#;
        let err = parse_release_response(body).unwrap_err();
        assert!(err.to_string().contains("parsing releases JSON"));
    }

    #[test]
    fn parse_release_response_rejects_bad_semver() {
        let body = r#"{"tag_name": "v-not-a-version"}"#;
        let err = parse_release_response(body).unwrap_err();
        assert!(
            err.to_string().contains("parsing release tag"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn current_version_parses() {
        // Just ensure the env value round-trips through semver at runtime.
        let _ = current_version();
    }
}
