//! The one-time "consider sponsoring" nudge.
//!
//! Aura asks exactly once, a week after it first ran, and never again once the
//! user has closed the card with its ×. Both timestamps live in
//! [`AppState`] (`state.json`), not `config.toml`: they are facts Aura records
//! about itself, not settings. The only user-facing knob is the opt-out,
//! `[sponsor] nudge` in config.
//!
//! Everything here takes `now` as an argument so the gating is testable
//! without a clock.

use chrono::{DateTime, TimeDelta, Utc};

use crate::state::AppState;

/// Where the nudge's primary "Sponsor on GitHub" button goes.
pub const SPONSOR_URL: &str = "https://github.com/sponsors/Rfluid";

/// Where the nudge's secondary "Pix" button goes — a one-off tip in BRL for
/// users without a GitHub Sponsors-friendly card.
pub const PIX_URL: &str = "https://livepix.gg/rfluid";

/// How long after the first run the nudge waits before appearing.
pub const NUDGE_DELAY: TimeDelta = TimeDelta::days(7);

/// Stamp `state.first_run` with `now` if it has never been set. Returns whether
/// the state changed, so the caller only writes the file when it has to.
///
/// A state file from a build that predates the field has no `first_run`, so an
/// existing user's first launch after upgrading counts as their first run: they
/// get the full week too, rather than a nudge on the very first open.
pub fn record_first_run(state: &mut AppState, now: DateTime<Utc>) -> bool {
    if state.first_run.is_some() {
        return false;
    }
    state.first_run = Some(now);
    true
}

/// Whether the nudge card should render right now.
///
/// False when the user opted out (`enabled` is `[sponsor] nudge`), when the card
/// was already answered (any of its buttons), and when there is no recorded first run — a
/// missing timestamp means the clock hasn't started, not that it ran out. A
/// first run in the future (the system clock moved backwards) also reads as
/// "not yet".
pub fn nudge_due(state: &AppState, enabled: bool, now: DateTime<Utc>) -> bool {
    if !enabled || state.sponsor_nudge_done {
        return false;
    }
    let Some(first_run) = state.first_run else {
        return false;
    };
    now.signed_duration_since(first_run) >= NUDGE_DELAY
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap()
    }

    fn started_at(first_run: DateTime<Utc>) -> AppState {
        AppState {
            first_run: Some(first_run),
            ..AppState::default()
        }
    }

    #[test]
    fn hidden_before_seven_days() {
        let state = started_at(t0());
        assert!(!nudge_due(&state, true, t0()));
        assert!(!nudge_due(&state, true, t0() + TimeDelta::days(6)));
        let just_short = t0() + NUDGE_DELAY - TimeDelta::seconds(1);
        assert!(!nudge_due(&state, true, just_short));
    }

    #[test]
    fn shown_at_and_after_seven_days() {
        let state = started_at(t0());
        assert!(nudge_due(&state, true, t0() + NUDGE_DELAY));
        assert!(nudge_due(&state, true, t0() + TimeDelta::days(90)));
    }

    #[test]
    fn never_shown_once_done() {
        let state = AppState {
            sponsor_nudge_done: true,
            ..started_at(t0())
        };
        assert!(!nudge_due(&state, true, t0() + TimeDelta::days(30)));
    }

    #[test]
    fn opt_out_is_respected() {
        let state = started_at(t0());
        assert!(!nudge_due(&state, false, t0() + TimeDelta::days(30)));
    }

    #[test]
    fn clock_moving_backwards_reads_as_not_yet() {
        let state = started_at(t0());
        assert!(!nudge_due(&state, true, t0() - TimeDelta::days(30)));
    }

    #[test]
    fn missing_first_run_initializes_rather_than_shows() {
        // An upgraded install: state.json exists but predates the field.
        let mut state = AppState::default();
        let now = t0() + TimeDelta::days(365);
        assert!(!nudge_due(&state, true, now));

        assert!(record_first_run(&mut state, now));
        assert_eq!(state.first_run, Some(now));
        // Still not due: the week starts now.
        assert!(!nudge_due(&state, true, now));
        assert!(nudge_due(&state, true, now + NUDGE_DELAY));
    }

    #[test]
    fn record_first_run_never_overwrites() {
        let mut state = started_at(t0());
        assert!(!record_first_run(&mut state, t0() + TimeDelta::days(3)));
        assert_eq!(state.first_run, Some(t0()));
    }
}
