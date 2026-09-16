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
    config::{AgentConfig, AgentKind, AppConfig, TrayConfig},
    quota::{CodexQuota, GeminiQuota, QuotaApi, QuotaSnapshot, QuotaSource, QuotaWindow},
    state::AppState,
};

use crate::tray::{set_status, TrayStatus, TrayVisuals};

/// How many quota windows fit in the tooltip before it stops being glanceable.
const MAX_WINDOWS: usize = 2;

/// Window the ring reads when the agent names no `tray_progress_source`: the
/// first one, which is the session window on every backend Aura speaks to.
const DEFAULT_PROGRESS_SOURCE: u32 = 0;

/// Window the color ramp reads when the agent names no `tray_color_source`:
/// the second one, the week. Splitting the two halves by default is the point
/// of the indicator — one number for the burst you're in, one for the budget
/// you're spending.
const DEFAULT_COLOR_SOURCE: u32 = 1;

/// Read the three drawn-visual toggles off the `[tray]` config.
pub fn visuals(tray: &TrayConfig) -> TrayVisuals {
    TrayVisuals {
        progress: tray.progress,
        color: tray.color,
        pulse: tray.pulse,
    }
}

/// Build the indicator line for `agent` from `quota`.
///
/// Examples of the rendered summary:
///
/// - `"Claude · 5h 72% · week 31%"` — the normal case.
/// - `"Claude · quota unavailable"` — the agent has no quota source, or the
///   API call failed. Still names the profile, which is the other half of
///   what the tooltip is for.
///
/// By default the ring reads the first window and the color the second — the
/// session and the week on every backend Aura speaks to. The agent's own
/// `tray_progress_source` / `tray_color_source` point either half somewhere
/// else — see [`reading`].
pub fn summarize(
    agent: &AgentConfig,
    quota: Option<&QuotaSnapshot>,
    tray: &TrayConfig,
) -> TrayStatus {
    let profile = &agent.name;
    let visuals = visuals(tray);
    let unreadable = |summary: String| TrayStatus {
        summary,
        gauge_percent: None,
        color_percent: None,
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
        gauge_percent: Some(reading(
            &quota.windows,
            agent
                .tray_progress_source
                .unwrap_or(DEFAULT_PROGRESS_SOURCE),
            peak,
        )),
        color_percent: Some(reading(
            &quota.windows,
            agent.tray_color_source.unwrap_or(DEFAULT_COLOR_SOURCE),
            peak,
        )),
        visuals,
    }
}

/// Read window `source` of `windows`, falling back to `peak`.
///
/// `source` is a position in the agent's window list — the order the modal and
/// the tooltip already show, so "the first number in the tooltip" is a
/// selector the user can count off the screen.
///
/// Positions are not stable across agents or over time: backends push only the
/// windows they actually have, so an idle Claude session drops the 5h window
/// and shifts the rest up. A selector that lands past the end — or on a window
/// reporting tokens but no percentage — therefore falls back to `peak`. That
/// covers the single-window agents too, where the color's default position 1
/// doesn't exist and the ramp should track the one window there is. Falling
/// back leaves the indicator reading a real number rather than going blank,
/// which would look identical to "quota unavailable".
fn reading(windows: &[QuotaWindow], source: u32, peak: f64) -> u8 {
    let pct = windows
        .get(source as usize)
        .and_then(|w| w.used_percentage)
        .unwrap_or(peak);
    pct.round().clamp(0.0, 100.0) as u8
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

    Some(summarize(agent, Some(&quota), &config.tray))
}

/// Start the background refresh thread.
///
/// The interval is deliberately long. Each tick can reach the agent's quota
/// endpoint, and an indicator is not worth generating steady background
/// traffic for — the modal's own refresh already covers every period the user
/// is actually looking. A detached thread (rather than a GPUI task) keeps the
/// blocking HTTP call off the foreground executor entirely.
///
/// Does nothing when `interval` is zero, which is how `tray.indicator =
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
    fn all_on() -> TrayConfig {
        TrayConfig {
            progress: true,
            color: true,
            pulse: true,
            ..TrayConfig::default()
        }
    }

    /// An agent on the default selectors: ring off window 0, color off 1.
    fn agent(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            kind: AgentKind::ClaudeCode,
            config_path: None,
            color: None,
            tray_progress_source: None,
            tray_color_source: None,
        }
    }

    fn line(profile: &str, quota: Option<&QuotaSnapshot>) -> TrayStatus {
        summarize(&agent(profile), quota, &all_on())
    }

    #[test]
    fn renders_percentages_for_each_window() {
        let snap = snapshot(vec![window("5h", Some(72.4)), window("week", Some(31.0))]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.summary, "Claude · 5h 72% · week 31%");
        // Out of the box the ring is the session and the color is the week.
        assert_eq!(status.gauge_percent, Some(72));
        assert_eq!(status.color_percent, Some(31));
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
    fn the_defaults_read_by_position_not_by_peak() {
        // The third window is the one running out, but the defaults name
        // positions 0 and 1 — the tooltip is where a window past those speaks
        // up. Point a source at it to put it on the icon.
        let snap = snapshot(vec![
            window("5h", Some(10.0)),
            window("week", Some(20.0)),
            window("month", Some(95.0)),
        ]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.gauge_percent, Some(10));
        assert_eq!(status.color_percent, Some(20));
        assert!(!status.attention());
    }

    #[test]
    fn one_window_means_both_halves_read_it() {
        // The color's default position 1 doesn't exist here; the fallback puts
        // the ramp on the only window there is rather than leaving it purple.
        let snap = snapshot(vec![window("5h", Some(95.0))]);
        let status = line("Claude", Some(&snap));
        assert_eq!(status.gauge_percent, Some(95));
        assert_eq!(status.color_percent, Some(95));
        assert!(status.attention());
    }

    #[test]
    fn the_sources_override_the_default_positions() {
        // Swapped against the defaults, so this fails if the config is ignored.
        let agent = AgentConfig {
            tray_progress_source: Some(1),
            tray_color_source: Some(0),
            ..agent("Claude")
        };
        let snap = snapshot(vec![window("5h", Some(91.0)), window("week", Some(12.0))]);
        let status = summarize(&agent, Some(&snap), &all_on());
        assert_eq!(status.gauge_percent, Some(12));
        assert_eq!(status.color_percent, Some(91));
        // Pulse follows the color half — it is the loud end of that ramp.
        assert!(status.attention());
    }

    #[test]
    fn a_source_past_the_end_falls_back_to_the_peak() {
        // Backends push only the windows they have, so an index can go stale.
        // Reading the safe number beats blanking the icon.
        let agent = AgentConfig {
            tray_progress_source: Some(7),
            ..agent("Claude")
        };
        let snap = snapshot(vec![window("5h", Some(12.0)), window("week", Some(91.0))]);
        assert_eq!(
            summarize(&agent, Some(&snap), &all_on()).gauge_percent,
            Some(91)
        );
    }

    #[test]
    fn a_source_naming_a_percentless_window_falls_back_to_the_peak() {
        // Token-only windows (the local-estimate paths) have no percentage to
        // fill a ring with.
        let agent = AgentConfig {
            tray_color_source: Some(0),
            ..agent("Claude")
        };
        let snap = snapshot(vec![window("5h", None), window("week", Some(44.0))]);
        assert_eq!(
            summarize(&agent, Some(&snap), &all_on()).color_percent,
            Some(44)
        );
    }

    #[test]
    fn pulse_is_opt_in() {
        // Past the threshold, but `tray.pulse` is off: the gauge
        // still reads 95% and turns red, the desktop is not asked to shout.
        let snap = snapshot(vec![window("5h", Some(95.0))]);
        let status = summarize(&agent("Claude"), Some(&snap), &TrayConfig::default());
        assert_eq!(status.gauge_percent, Some(95));
        assert!(!status.attention());
    }

    #[test]
    fn the_visual_toggles_ride_along_on_the_status() {
        // The backends render from the status alone, so whatever config said
        // has to survive the trip.
        let tray = TrayConfig {
            progress: false,
            color: false,
            pulse: false,
            ..TrayConfig::default()
        };
        let off = TrayVisuals {
            progress: false,
            color: false,
            pulse: false,
        };
        let snap = snapshot(vec![window("5h", Some(95.0))]);
        let agent = agent("Claude");
        assert_eq!(summarize(&agent, Some(&snap), &tray).visuals, off);
        // Turning the visuals off doesn't cost the tooltip its numbers.
        assert_eq!(
            summarize(&agent, Some(&snap), &tray).summary,
            "Claude · 5h 95%"
        );
    }

    #[test]
    fn visuals_come_straight_from_the_tray_config() {
        let tray = TrayConfig {
            progress: false,
            color: true,
            pulse: true,
            ..TrayConfig::default()
        };
        assert_eq!(
            visuals(&tray),
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
        assert_eq!(status.gauge_percent, Some(44));
    }

    #[test]
    fn unavailable_quota_still_names_the_profile() {
        let snap = QuotaSnapshot::unavailable("no oauth token");
        let status = line("Gemini", Some(&snap));
        assert_eq!(status.summary, "Gemini · quota unavailable");
        assert_eq!(status.gauge_percent, None);
        assert_eq!(status.color_percent, None);
        assert!(!status.attention());
    }

    #[test]
    fn a_snapshot_with_no_usable_windows_reads_as_unavailable() {
        let status = line("Claude", Some(&snapshot(vec![window("5h", None)])));
        assert_eq!(status.summary, "Claude · quota unavailable");
        // No reading means the plain logo, not an empty gauge reading 0%.
        assert_eq!(status.gauge_percent, None);
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
        assert_eq!(line("Claude", Some(&snap)).gauge_percent, Some(73));
        let over = snapshot(vec![window("5h", Some(140.0))]);
        assert_eq!(line("Claude", Some(&over)).gauge_percent, Some(100));
    }
}
