//! Turning usage data into the one line the tray icon shows.
//!
//! A tray indicator that only launches a window is a button, not an
//! indicator — the wifi and battery icons it sits next to all convey their
//! state without being clicked. This module is the bridge: it renders a
//! [`QuotaSnapshot`] into a [`TrayStatus`] (tooltip text + gauge state) and
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
    config::{AgentKind, AppConfig, DisplayConfig},
    quota::{CodexQuota, GeminiQuota, QuotaApi, QuotaSnapshot, QuotaSource},
    state::AppState,
};

use crate::tray::{set_status, TrayStatus, TrayVisuals};

/// How many quota windows fit in the tooltip before it stops being glanceable.
const MAX_WINDOWS: usize = 2;

/// Read the three `display.tray_*` visual toggles.
pub fn visuals(display: &DisplayConfig) -> TrayVisuals {
    TrayVisuals {
        progress: display.tray_progress,
        color: display.tray_color,
        pulse: display.tray_pulse,
    }
}

/// Build the indicator line for `profile` from `quota`.
///
/// Examples of the rendered summary:
///
/// - `"Claude · 5h 72% · week 31%"` — the normal case.
/// - `"Claude · quota unavailable"` — the agent has no quota source, or the
///   API call failed. Still names the profile, which is the other half of
///   what the tooltip is for.
///
/// The icon's usage reading is the *peak* across every window, including ones
/// too far down the list to fit in the tooltip: a gauge that ignored the
/// window actually running out would be worse than no gauge.
pub fn summarize(profile: &str, quota: Option<&QuotaSnapshot>, visuals: TrayVisuals) -> TrayStatus {
    let unreadable = |summary: String| TrayStatus {
        summary,
        usage_percent: None,
        visuals,
    };

    let Some(quota) = quota else {
        return unreadable(format!("{profile} · no data yet"));
    };

    if quota.source == QuotaSource::Unavailable {
        return unreadable(format!("{profile} · quota unavailable"));
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
        return unreadable(format!("{profile} · quota unavailable"));
    }

    TrayStatus {
        summary: format!("{profile} · {}", parts.join(" · ")),
        usage_percent: Some(peak.round().clamp(0.0, 100.0) as u8),
        visuals,
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

    Some(summarize(
        &agent.name,
        Some(&quota),
        visuals(&config.display),
    ))
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

    /// Every visual on, so the tests exercise the full indicator. `pulse` is
    /// off in the shipped default — see [`pulse_is_opt_in`].
    fn all_on() -> TrayVisuals {
        TrayVisuals {
            progress: true,
            color: true,
            pulse: true,
        }
    }

    fn line(profile: &str, quota: Option<&QuotaSnapshot>) -> TrayStatus {
        summarize(profile, quota, all_on())
    }

    #[test]
    fn renders_percentages_for_each_window() {
        let snap = snapshot(vec![window("5h", Some(72.4)), window("week", Some(31.0))]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.summary, "Claude · 5h 72% · week 31%");
        assert_eq!(status.usage_percent, Some(72));
        assert!(!status.attention());
    }

    #[test]
    fn caps_the_tooltip_at_two_windows() {
        let snap = snapshot(vec![
            window("5h", Some(10.0)),
            window("week", Some(20.0)),
            window("month", Some(30.0)),
        ]);
        assert_eq!(
            line("Codex", Some(&snap)).summary,
            "Codex · 5h 10% · week 20%"
        );
    }

    #[test]
    fn the_gauge_tracks_the_peak_window_even_when_it_is_not_shown() {
        // The third window is past the threshold but doesn't fit in the
        // tooltip; the icon must still fill and go red — that's the whole
        // point of a glanceable indicator.
        let snap = snapshot(vec![
            window("5h", Some(10.0)),
            window("week", Some(20.0)),
            window("month", Some(95.0)),
        ]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.usage_percent, Some(95));
        assert!(status.attention());
    }

    #[test]
    fn pulse_is_opt_in() {
        // Past the threshold, but `display.tray_pulse` is off: the gauge
        // still reads 95% and turns red, the desktop is not asked to shout.
        let snap = snapshot(vec![window("5h", Some(95.0))]);
        let status = summarize("Claude", Some(&snap), TrayVisuals::default());
        assert_eq!(status.usage_percent, Some(95));
        assert!(!status.attention());
    }

    #[test]
    fn the_visual_toggles_ride_along_on_the_status() {
        // The backends render from the status alone, so whatever config said
        // has to survive the trip.
        let off = TrayVisuals {
            progress: false,
            color: false,
            pulse: false,
        };
        let snap = snapshot(vec![window("5h", Some(95.0))]);
        assert_eq!(summarize("Claude", Some(&snap), off).visuals, off);
        // Turning the visuals off doesn't cost the tooltip its numbers.
        assert_eq!(
            summarize("Claude", Some(&snap), off).summary,
            "Claude · 5h 95%"
        );
    }

    #[test]
    fn visuals_come_straight_from_the_display_config() {
        let display = DisplayConfig {
            tray_progress: false,
            tray_color: true,
            tray_pulse: true,
            ..DisplayConfig::default()
        };
        assert_eq!(
            visuals(&display),
            TrayVisuals {
                progress: false,
                color: true,
                pulse: true,
            }
        );
    }

    #[test]
    fn windows_without_a_percentage_are_skipped() {
        let snap = snapshot(vec![window("5h", None), window("week", Some(44.0))]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.summary, "Claude · week 44%");
        assert_eq!(status.usage_percent, Some(44));
    }

    #[test]
    fn unavailable_quota_still_names_the_profile() {
        let snap = QuotaSnapshot::unavailable("no oauth token");
        let status = line("Gemini", Some(&snap));
        assert_eq!(status.summary, "Gemini · quota unavailable");
        assert_eq!(status.usage_percent, None);
        assert!(!status.attention());
    }

    #[test]
    fn a_snapshot_with_no_usable_windows_reads_as_unavailable() {
        let status = line("Claude", Some(&snapshot(vec![window("5h", None)])));
        assert_eq!(status.summary, "Claude · quota unavailable");
        // No reading means the plain logo, not an empty gauge reading 0%.
        assert_eq!(status.usage_percent, None);
    }

    #[test]
    fn missing_snapshot_is_distinct_from_an_unavailable_one() {
        assert_eq!(line("Claude", None).summary, "Claude · no data yet");
    }

    #[test]
    fn usage_is_rounded_and_clamped_to_a_whole_percent() {
        // Rounding keeps `TrayStatus` `Eq` so redundant pushes are skipped;
        // clamping guards the gauge against an API reporting past 100.
        let snap = snapshot(vec![window("5h", Some(72.6))]);
        assert_eq!(line("Claude", Some(&snap)).usage_percent, Some(73));
        let over = snapshot(vec![window("5h", Some(140.0))]);
        assert_eq!(line("Claude", Some(&over)).usage_percent, Some(100));
    }
}
