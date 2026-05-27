//! Forecasts where each quota window will end up if the current burn rate
//! continues — see `docs/forecast-tab.md`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{QuotaSnapshot, QuotaWindow};

/// Below this elapsed fraction we don't extrapolate — too little signal.
const INSUFFICIENT_BELOW: f64 = 0.05;

/// Status badge keyed off the projected end-of-window percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForecastStatus {
    /// Projection stays under 90% — comfortably within the window.
    Ok,
    /// Projection is between 90% and 100% — likely to land near the cap.
    Watch,
    /// Projection exceeds 100% — current rate overshoots the window.
    Overshoot,
    /// Not enough elapsed time to project — show "warming up" placeholder.
    Insufficient,
}

/// One row in the Forecast tab — mirrors a [`QuotaWindow`] but projected
/// forward to `resets_at` at the current burn rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastWindow {
    pub label: String,
    pub used_percentage_now: Option<f64>,
    pub projected_percentage: Option<f64>,
    pub projected_tokens: Option<u64>,
    /// When linear extrapolation crosses 100%. `None` unless status is
    /// `Overshoot`.
    pub overshoot_at: Option<DateTime<Utc>>,
    pub status: ForecastStatus,
    pub resets_at: Option<DateTime<Utc>>,
}

/// A full set of forecasts — one per supported quota window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastSnapshot {
    pub windows: Vec<ForecastWindow>,
    pub computed_at: DateTime<Utc>,
    /// Free-form note (e.g. "session has 8 min elapsed") for an
    /// `Insufficient` row, or other context.
    pub note: Option<String>,
}

/// Project every supported window in `quota` forward to its `resets_at`
/// assuming a uniform burn rate from the start of the window until `now`.
pub fn forecast(quota: &QuotaSnapshot, now: DateTime<Utc>) -> ForecastSnapshot {
    let windows = quota
        .windows
        .iter()
        .filter_map(|w| project_window(w, now))
        .collect();
    ForecastSnapshot {
        windows,
        computed_at: now,
        note: None,
    }
}

/// Project a single window. Returns `None` for windows that don't have a
/// known length (e.g. "Overage") or that lack the inputs we need.
fn project_window(w: &QuotaWindow, now: DateTime<Utc>) -> Option<ForecastWindow> {
    let length = window_length(w)?;
    let resets_at = w.resets_at?;
    let started_at = resets_at - length;

    let elapsed = (now - started_at).num_milliseconds() as f64;
    let total = length.num_milliseconds() as f64;
    if total <= 0.0 {
        return None;
    }
    let elapsed_fraction = (elapsed / total).clamp(0.0, 1.0);

    // No activity yet (or zero percentage) — flat projection.
    let used_pct = w.used_percentage.unwrap_or(0.0);

    if elapsed_fraction < INSUFFICIENT_BELOW || used_pct <= 0.0 {
        return Some(ForecastWindow {
            label: w.label.clone(),
            used_percentage_now: w.used_percentage,
            projected_percentage: None,
            projected_tokens: None,
            overshoot_at: None,
            status: ForecastStatus::Insufficient,
            resets_at: w.resets_at,
        });
    }

    let projected_pct = used_pct / elapsed_fraction;
    let projected_tokens = w
        .used_tokens
        .map(|t| ((t as f64) / elapsed_fraction).round() as u64);

    let status = if projected_pct > 100.0 {
        ForecastStatus::Overshoot
    } else if projected_pct >= 90.0 {
        ForecastStatus::Watch
    } else {
        ForecastStatus::Ok
    };

    // Linear extrapolation: rate = used_pct / elapsed. Time to 100% from
    // `started_at` is `100 / rate = elapsed * (100 / used_pct)`.
    let overshoot_at = if status == ForecastStatus::Overshoot {
        let ms_to_100 = (elapsed * (100.0 / used_pct)).round() as i64;
        Some(started_at + Duration::milliseconds(ms_to_100))
    } else {
        None
    };

    Some(ForecastWindow {
        label: w.label.clone(),
        used_percentage_now: w.used_percentage,
        projected_percentage: Some(projected_pct),
        projected_tokens,
        overshoot_at,
        status,
        resets_at: w.resets_at,
    })
}

/// Resolve a window's length. Prefers `QuotaWindow.length_minutes` when the
/// backend populates it (Codex, Claude Code API); falls back to a closed-set
/// label map for the legacy path. Returns `None` for windows we deliberately
/// skip (Overage) or for local-fallback labels that lack percentage data.
fn window_length(w: &QuotaWindow) -> Option<Duration> {
    if let Some(m) = w.length_minutes {
        if m > 0 {
            return Some(Duration::minutes(m as i64));
        }
    }
    match w.label.as_str() {
        "Current session" => Some(Duration::hours(5)),
        "Current week (all models)" | "Current week (Opus only)" | "Current week (Sonnet only)" => {
            Some(Duration::days(7))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{QuotaSnapshot, QuotaWindow};

    fn window(label: &str, pct: Option<f64>, resets_in_hours: i64) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            used_percentage: pct,
            used_tokens: None,
            resets_at: Some(Utc::now() + Duration::hours(resets_in_hours)),
            length_minutes: None,
        }
    }

    /// 25% elapsed, 30% used → projects to 120% (overshoot).
    #[test]
    fn overshoot_when_pace_above_uniform() {
        let now = Utc::now();
        let resets_at = now + Duration::hours(4); // session: 1h elapsed of 5h = 20%
        let w = QuotaWindow {
            label: "Current session".to_string(),
            used_percentage: Some(30.0),
            used_tokens: None,
            resets_at: Some(resets_at),
            length_minutes: None,
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        let proj = &fc.windows[0];
        assert_eq!(proj.status, ForecastStatus::Overshoot);
        let p = proj.projected_percentage.unwrap();
        assert!((p - 150.0).abs() < 0.5, "expected ~150%, got {p}");
        assert!(proj.overshoot_at.is_some());
        // 100% is reached when used_pct hits 100, at rate 30%/h → 100/30 ≈ 3.33h
        // from start, i.e. ~2.33h after `now`.
        let over = proj.overshoot_at.unwrap();
        let delta = (over - now).num_minutes();
        assert!(
            (130..=150).contains(&delta),
            "expected overshoot ~2h20m out, got {delta}m"
        );
    }

    /// 50% elapsed, 45% used → linear, lands ≈ 90% (Watch boundary).
    #[test]
    fn watch_band_around_100() {
        let now = Utc::now();
        let resets_at = now + Duration::hours(2) + Duration::minutes(30); // 2.5h elapsed of 5h
        let w = QuotaWindow {
            label: "Current session".to_string(),
            used_percentage: Some(45.0),
            used_tokens: None,
            resets_at: Some(resets_at),
            length_minutes: None,
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        let proj = &fc.windows[0];
        let p = proj.projected_percentage.unwrap();
        assert!((p - 90.0).abs() < 1.0, "expected ~90%, got {p}");
        assert_eq!(proj.status, ForecastStatus::Watch);
    }

    /// 75% elapsed, 30% used → projects ≈ 40% (Ok).
    #[test]
    fn ok_status_when_under_pace() {
        let now = Utc::now();
        let resets_at = now + Duration::hours(1) + Duration::minutes(15); // 3.75h elapsed of 5h
        let w = QuotaWindow {
            label: "Current session".to_string(),
            used_percentage: Some(30.0),
            used_tokens: None,
            resets_at: Some(resets_at),
            length_minutes: None,
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        let proj = &fc.windows[0];
        let p = proj.projected_percentage.unwrap();
        assert!((p - 40.0).abs() < 0.5, "expected ~40%, got {p}");
        assert_eq!(proj.status, ForecastStatus::Ok);
        assert!(proj.overshoot_at.is_none());
    }

    /// < 5% elapsed → Insufficient regardless of usage.
    #[test]
    fn insufficient_when_barely_started() {
        let now = Utc::now();
        // 5h session window, 5min elapsed = 1.6%
        let resets_at = now + Duration::hours(5) - Duration::minutes(5);
        let w = QuotaWindow {
            label: "Current session".to_string(),
            used_percentage: Some(10.0),
            used_tokens: None,
            resets_at: Some(resets_at),
            length_minutes: None,
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        let proj = &fc.windows[0];
        assert_eq!(proj.status, ForecastStatus::Insufficient);
        assert!(proj.projected_percentage.is_none());
    }

    /// 0% used → Insufficient ("no activity yet").
    #[test]
    fn insufficient_when_no_usage() {
        let now = Utc::now();
        let w = QuotaWindow {
            label: "Current session".to_string(),
            used_percentage: Some(0.0),
            used_tokens: None,
            resets_at: Some(now + Duration::hours(2)),
            length_minutes: None,
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        assert_eq!(fc.windows[0].status, ForecastStatus::Insufficient);
    }

    /// Empty snapshot → empty forecast (no panic).
    #[test]
    fn empty_snapshot_yields_empty_forecast() {
        let fc = forecast(&QuotaSnapshot::default(), Utc::now());
        assert!(fc.windows.is_empty());
    }

    /// Unknown labels (Overage, local-fallback) are dropped.
    #[test]
    fn unknown_labels_are_skipped() {
        let snap = QuotaSnapshot {
            windows: vec![
                window("Overage", Some(50.0), 1),
                window("Current session (local est.)", Some(50.0), 1),
                window("Current session", Some(50.0), 1),
            ],
            ..Default::default()
        };
        let fc = forecast(&snap, Utc::now());
        assert_eq!(fc.windows.len(), 1);
        assert_eq!(fc.windows[0].label, "Current session");
    }

    /// Codex labels carry a `· 5h` / `· weekly` suffix and don't match the
    /// Claude-Code label allowlist — they must project off `length_minutes`.
    #[test]
    fn codex_labels_project_off_length_minutes() {
        let now = Utc::now();
        let w = QuotaWindow {
            label: "Primary · 5h".to_string(),
            used_percentage: Some(30.0),
            used_tokens: None,
            // 1h elapsed of 5h → projects to 150% (overshoot).
            resets_at: Some(now + Duration::hours(4)),
            length_minutes: Some(5 * 60),
        };
        let snap = QuotaSnapshot {
            windows: vec![w],
            ..Default::default()
        };
        let fc = forecast(&snap, now);
        assert_eq!(fc.windows.len(), 1, "Codex window should be projectable");
        let p = fc.windows[0].projected_percentage.unwrap();
        assert!((p - 150.0).abs() < 0.5, "expected ~150%, got {p}");
        assert_eq!(fc.windows[0].status, ForecastStatus::Overshoot);
    }

    /// 25% / 50% / 75% elapsed at 20% used: projection scales linearly.
    #[test]
    fn linear_extrapolation_scales_correctly() {
        let now = Utc::now();
        // Session window, 5h length.
        let cases = [
            (Duration::minutes(75), 80.0),  // 1.25h elapsed = 25% → 80% projected
            (Duration::minutes(150), 40.0), // 2.5h elapsed = 50% → 40% projected
            (Duration::minutes(225), 80.0 / 3.0), // 3.75h elapsed = 75% → ~26.7%
        ];
        for (elapsed, expected) in cases {
            let resets_at = now + Duration::hours(5) - elapsed;
            let w = QuotaWindow {
                label: "Current session".to_string(),
                used_percentage: Some(20.0),
                used_tokens: None,
                resets_at: Some(resets_at),
                length_minutes: None,
            };
            let snap = QuotaSnapshot {
                windows: vec![w],
                ..Default::default()
            };
            let fc = forecast(&snap, now);
            let p = fc.windows[0].projected_percentage.unwrap();
            assert!(
                (p - expected).abs() < 0.5,
                "elapsed {:?}: expected ~{expected}, got {p}",
                elapsed
            );
        }
    }
}
