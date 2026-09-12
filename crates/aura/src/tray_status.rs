//! Turning usage data into the one line the tray icon shows.
//!
//! A tray indicator that only launches a window is a button, not an
//! indicator — the wifi and battery icons it sits next to all convey their
//! state without being clicked. This module is the bridge: it renders a
//! [`QuotaSnapshot`] into a [`TrayStatus`] (tooltip text + attention flag) and
//! provides the off-thread refresh used when the modal is closed.
//!
//! Two producers push into the same sink (`tray::set_status`):
//!
//! 1. `app.rs`'s refresh worker — free, since it has just loaded a snapshot
//!    for the modal anyway. Keeps the tooltip exact while the user is looking.
//! 2. [`spawn_poll`] — a slow background tick for the much longer stretches
//!    when the modal is closed. Same code path, own thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aura_core::{
    config::{AgentKind, AppConfig},
    quota::{CodexQuota, GeminiQuota, QuotaApi, QuotaSnapshot, QuotaSource},
    state::AppState,
};

use crate::tray::{set_status, TrayStatus};

/// Percentage at which a quota window flips the icon into its attention
/// state. Deliberately high: the whole value of `NeedsAttention` is that it
/// stays rare, and Plasma un-hides the icon from the overflow group when it
/// fires.
const ATTENTION_PERCENT: f64 = 90.0;

/// How many quota windows fit in the tooltip before it stops being glanceable.
const MAX_WINDOWS: usize = 2;

/// Build the indicator line for `profile` from `quota`.
///
/// Examples of the rendered summary:
///
/// - `"Claude · 5h 72% · week 31%"` — the normal case.
/// - `"Claude · quota unavailable"` — the agent has no quota source, or the
///   API call failed. Still names the profile, which is the other half of
///   what the tooltip is for.
pub fn summarize(profile: &str, quota: Option<&QuotaSnapshot>) -> TrayStatus {
    let Some(quota) = quota else {
        return TrayStatus {
            summary: format!("{profile} · no data yet"),
            attention: false,
        };
    };

    if quota.source == QuotaSource::Unavailable {
        return TrayStatus {
            summary: format!("{profile} · quota unavailable"),
            attention: false,
        };
    }

    let mut parts: Vec<String> = Vec::with_capacity(MAX_WINDOWS);
    let mut peak: f64 = 0.0;
    for window in quota.windows.iter() {
        let Some(pct) = window.used_percentage else {
            continue;
        };
        peak = peak.max(pct);
        if parts.len() < MAX_WINDOWS {
            parts.push(format!("{} {:.0}%", window.label, pct));
        }
    }

    if parts.is_empty() {
        return TrayStatus {
            summary: format!("{profile} · quota unavailable"),
            attention: false,
        };
    }

    TrayStatus {
        summary: format!("{profile} · {}", parts.join(" · ")),
        attention: peak >= ATTENTION_PERCENT,
    }
}

/// Load config + state from disk and fetch the active agent's quota. Blocking
/// and network-touching — call from a background thread only.
fn poll_once(config_path: &Path) -> Option<TrayStatus> {
    let config = AppConfig::load_with_discovery(config_path).ok()?;
    let active = AppState::load().ok().and_then(|s| s.active_profile);
    let agent = match active {
        Some(name) => config
            .agents
            .iter()
            .find(|a| a.name == name)
            .or_else(|| config.agents.first()),
        None => config.agents.first(),
    }?;

    let agent_path = agent.resolved_config_path();
    let quota = match agent.kind {
        AgentKind::ClaudeCode => QuotaApi::new(agent_path).snapshot(),
        AgentKind::Codex => CodexQuota::new(agent_path).snapshot(),
        AgentKind::Gemini => GeminiQuota::new(agent_path).snapshot(),
    };

    Some(summarize(&agent.name, Some(&quota)))
}

/// Start the background refresh thread.
///
/// The interval is deliberately long. Each tick can reach the agent's quota
/// endpoint, and an indicator is not worth generating steady background
/// traffic for — the modal's own refresh already covers every period the user
/// is actually looking. A detached thread (rather than a GPUI task) keeps the
/// blocking HTTP call off the foreground executor entirely.
///
/// Does nothing when `interval` is zero, which is how `display.tray_status =
/// false` turns the feature off.
pub fn spawn_poll(config_path: PathBuf, interval: Duration) {
    if interval.is_zero() {
        return;
    }
    std::thread::spawn(move || loop {
        if let Some(status) = poll_once(&config_path) {
            set_status(status);
        }
        std::thread::sleep(interval);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::quota::QuotaWindow;

    fn window(label: &str, pct: Option<f64>) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            used_percentage: pct,
            used_tokens: None,
            resets_at: None,
            length_minutes: None,
        }
    }

    fn snapshot(windows: Vec<QuotaWindow>) -> QuotaSnapshot {
        QuotaSnapshot {
            subscription_type: None,
            windows,
            source: QuotaSource::Api,
            note: None,
        }
    }

    #[test]
    fn renders_percentages_for_each_window() {
        let snap = snapshot(vec![window("5h", Some(72.4)), window("week", Some(31.0))]);
        let status = summarize("Claude", Some(&snap));
        assert_eq!(status.summary, "Claude · 5h 72% · week 31%");
        assert!(!status.attention);
    }

    #[test]
    fn caps_the_tooltip_at_two_windows() {
        let snap = snapshot(vec![
            window("5h", Some(10.0)),
            window("week", Some(20.0)),
            window("month", Some(30.0)),
        ]);
        assert_eq!(
            summarize("Codex", Some(&snap)).summary,
            "Codex · 5h 10% · week 20%"
        );
    }

    #[test]
    fn attention_tracks_the_peak_window_even_when_it_is_not_shown() {
        // The third window is past the threshold but doesn't fit in the
        // tooltip; the icon must still go red — that's the whole point of a
        // glanceable indicator.
        let snap = snapshot(vec![
            window("5h", Some(10.0)),
            window("week", Some(20.0)),
            window("month", Some(95.0)),
        ]);
        assert!(summarize("Claude", Some(&snap)).attention);
    }

    #[test]
    fn windows_without_a_percentage_are_skipped() {
        let snap = snapshot(vec![window("5h", None), window("week", Some(44.0))]);
        assert_eq!(
            summarize("Claude", Some(&snap)).summary,
            "Claude · week 44%"
        );
    }

    #[test]
    fn unavailable_quota_still_names_the_profile() {
        let snap = QuotaSnapshot::unavailable("no oauth token");
        let status = summarize("Gemini", Some(&snap));
        assert_eq!(status.summary, "Gemini · quota unavailable");
        assert!(!status.attention);
    }

    #[test]
    fn a_snapshot_with_no_usable_windows_reads_as_unavailable() {
        let snap = snapshot(vec![window("5h", None)]);
        assert_eq!(
            summarize("Claude", Some(&snap)).summary,
            "Claude · quota unavailable"
        );
    }

    #[test]
    fn missing_snapshot_is_distinct_from_an_unavailable_one() {
        assert_eq!(summarize("Claude", None).summary, "Claude · no data yet");
    }
}
