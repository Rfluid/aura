//! Fleet state: the decoded peer heartbeats and the share math that turns
//! per-machine token counts into "share of the 5h / weekly window".
//!
//! # Why share is computed from tokens, not percentages
//!
//! The Claude account's window percentages are **global** — every machine's
//! `/usage` call returns the *same* account-wide 5h% and weekly%. So a peer's
//! percentage tells you nothing about *that machine's* contribution. The
//! per-machine split must come from token counts:
//!
//! ```text
//! machine_share = machine_tokens / Σ(peer_tokens)
//! attributed_pct = machine_share × account_window_pct
//! ```
//!
//! The percentages are still carried in the heartbeat for the aggregate sanity
//! line and freshness, but the bars are driven by token shares.
//!
//! This module is pure: it has no network and takes the current time as an
//! argument so the stale-pruning and freshness logic is deterministic in tests.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current heartbeat schema version. Bumped on a breaking envelope change so
/// receivers can drop messages they can't interpret.
pub const HEARTBEAT_VERSION: u32 = 1;

/// The plaintext heartbeat, before sealing. Serialized to JSON, sealed with
/// XChaCha20-Poly1305, base64'd, and published to the ntfy topic. No Claude
/// tokens or message content ever appear here — only usage aggregates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {
    /// Schema version. See [`HEARTBEAT_VERSION`].
    pub v: u32,
    /// Random per-install machine id (uuid). Disambiguates two machines that
    /// happen to share a hostname.
    pub machine_id: String,
    /// Human-friendly label (hostname or the configured override).
    pub label: String,
    /// Account-wide 5h "session" window usage percentage (0–100). Same on
    /// every machine; used for the aggregate sanity line, not the per-machine
    /// share.
    pub session_pct: f64,
    /// Account-wide 7d "weekly" window usage percentage (0–100).
    pub weekly_pct: f64,
    /// This machine's tokens in the 5h window. Drives the 5h share. `None`
    /// when the local backend couldn't supply a token count.
    #[serde(default)]
    pub session_tokens: Option<u64>,
    /// This machine's tokens in the 7d window. Drives the weekly share.
    #[serde(default)]
    pub weekly_tokens: Option<u64>,
    /// When this heartbeat was produced (sender's clock). Used for replay
    /// rejection and freshness.
    pub ts: DateTime<Utc>,
}

impl Heartbeat {
    /// Build a heartbeat for the local machine from the two account windows.
    pub fn new(
        machine_id: impl Into<String>,
        label: impl Into<String>,
        session_pct: f64,
        weekly_pct: f64,
        session_tokens: Option<u64>,
        weekly_tokens: Option<u64>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            v: HEARTBEAT_VERSION,
            machine_id: machine_id.into(),
            label: label.into(),
            session_pct,
            weekly_pct,
            session_tokens,
            weekly_tokens,
            ts: now,
        }
    }
}

/// A peer's most recent heartbeat plus the local clock time we received it.
/// `received_at` (not the sender's `ts`) drives stale-pruning so a peer with a
/// skewed clock still dims correctly.
#[derive(Debug, Clone)]
pub struct PeerSnapshot {
    pub heartbeat: Heartbeat,
    pub received_at: DateTime<Utc>,
}

impl PeerSnapshot {
    /// Seconds since we last heard from this peer, relative to `now`.
    pub fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        (now - self.received_at).num_seconds().max(0)
    }

    /// Whether this peer is stale (silent for longer than `stale_secs`).
    pub fn is_stale(&self, now: DateTime<Utc>, stale_secs: u64) -> bool {
        self.age_secs(now) > stale_secs as i64
    }
}

/// One row in the rendered Fleet table: a machine and its computed shares.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetRow {
    pub machine_id: String,
    pub label: String,
    /// True for the local machine's own row ("you").
    pub is_self: bool,
    /// Fraction (0.0–1.0) of the fleet's 5h tokens attributable to this
    /// machine. `None` when no machine reported 5h tokens.
    pub session_share: Option<f64>,
    /// Fraction (0.0–1.0) of the fleet's weekly tokens for this machine.
    pub weekly_share: Option<f64>,
    /// Seconds since this machine's last heartbeat (0 for the local row).
    pub age_secs: i64,
    /// Whether this peer is dimmed for staleness.
    pub is_stale: bool,
}

/// The live Fleet model: every known peer keyed by `machine_id`, plus the
/// local machine's identity and its latest self-heartbeat.
#[derive(Debug, Clone, Default)]
pub struct FleetState {
    /// Remote peers, keyed by their `machine_id`. Never includes self.
    peers: HashMap<String, PeerSnapshot>,
    /// This machine's id (so an echo of our own publish is ignored).
    self_id: String,
    /// This machine's latest self-heartbeat, mirrored into the rows so the
    /// local machine appears in the table even before any peer replies.
    self_heartbeat: Option<Heartbeat>,
    /// Last time we published, for the "updated Ns ago" self dot.
    pub last_publish: Option<DateTime<Utc>>,
    /// Whether the broker is currently reachable. UI shows a banner when false.
    pub broker_reachable: bool,
}

impl FleetState {
    /// Create an empty state for the given local machine id.
    pub fn new(self_id: impl Into<String>) -> Self {
        Self {
            peers: HashMap::new(),
            self_id: self_id.into(),
            self_heartbeat: None,
            last_publish: None,
            broker_reachable: true,
        }
    }

    /// Record the local machine's latest heartbeat (the one we're about to
    /// publish), so it shows up as the "you" row.
    pub fn set_self_heartbeat(&mut self, hb: Heartbeat, now: DateTime<Utc>) {
        self.self_heartbeat = Some(hb);
        self.last_publish = Some(now);
    }

    /// Ingest a decoded heartbeat from the broker. Drops:
    /// - our own echoed publish (same `machine_id` as `self_id`),
    /// - replays / stale messages whose `ts` is older than `max_age_secs`
    ///   (the caller passes `2 × heartbeat_secs`), and
    /// - out-of-order duplicates older than the one we already hold.
    ///
    /// Returns `true` if the peer map changed (so the UI can re-render).
    pub fn ingest(&mut self, hb: Heartbeat, now: DateTime<Utc>, max_age_secs: u64) -> bool {
        if hb.machine_id == self.self_id {
            return false; // our own echo
        }
        // Replay protection: reject heartbeats whose timestamp is too far in
        // the past relative to our clock. A small future skew is tolerated.
        let age = (now - hb.ts).num_seconds();
        if age > max_age_secs as i64 {
            return false;
        }
        // Drop strictly-older duplicates so a late-delivered stale message
        // can't overwrite a fresher one we already have.
        if let Some(existing) = self.peers.get(&hb.machine_id) {
            if hb.ts < existing.heartbeat.ts {
                return false;
            }
        }
        self.peers.insert(
            hb.machine_id.clone(),
            PeerSnapshot {
                heartbeat: hb,
                received_at: now,
            },
        );
        true
    }

    /// Remove peers silent for longer than `stale_secs`. Returns the number
    /// pruned. (Stale peers are also excluded from share math even before they
    /// are pruned — see [`Self::rows`]; pruning frees the map entry once the
    /// peer is clearly gone, at `4 × stale_secs`.)
    pub fn prune(&mut self, now: DateTime<Utc>, stale_secs: u64) -> usize {
        let hard_cutoff = stale_secs.saturating_mul(4) as i64;
        let before = self.peers.len();
        self.peers
            .retain(|_, p| p.age_secs(now) <= hard_cutoff);
        before - self.peers.len()
    }

    /// Number of currently-known peers (excludes self).
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Clone the latest self-heartbeat, if one has been set. Used by the sync
    /// loop to re-publish the current local state on the heartbeat cadence.
    pub fn self_heartbeat_clone(&self) -> Option<Heartbeat> {
        self.self_heartbeat.clone()
    }

    /// Account-wide window percentages for the aggregate sanity line, taken
    /// from the freshest available heartbeat (self preferred, else any peer).
    /// Returns `(session_pct, weekly_pct)`.
    pub fn account_pcts(&self) -> Option<(f64, f64)> {
        self.self_heartbeat
            .as_ref()
            .or_else(|| {
                self.peers
                    .values()
                    .max_by_key(|p| p.received_at)
                    .map(|p| &p.heartbeat)
            })
            .map(|hb| (hb.session_pct, hb.weekly_pct))
    }

    /// Build the rendered rows, sorted "who's using more first" by 5h share
    /// (then weekly share, then label) so the heaviest machine is on top.
    ///
    /// Shares are computed only over **non-stale** machines that reported
    /// tokens; a stale machine still appears (dimmed) but its tokens are
    /// excluded from the denominator so the live split stays meaningful.
    pub fn rows(&self, now: DateTime<Utc>, stale_secs: u64) -> Vec<FleetRow> {
        // Collect (machine_id, label, is_self, session_tokens, weekly_tokens,
        // age_secs, is_stale) for self + every peer.
        struct Entry {
            machine_id: String,
            label: String,
            is_self: bool,
            session_tokens: Option<u64>,
            weekly_tokens: Option<u64>,
            age_secs: i64,
            is_stale: bool,
        }

        let mut entries: Vec<Entry> = Vec::new();

        if let Some(hb) = &self.self_heartbeat {
            entries.push(Entry {
                machine_id: hb.machine_id.clone(),
                label: hb.label.clone(),
                is_self: true,
                session_tokens: hb.session_tokens,
                weekly_tokens: hb.weekly_tokens,
                age_secs: self
                    .last_publish
                    .map(|t| (now - t).num_seconds().max(0))
                    .unwrap_or(0),
                is_stale: false, // our own row is never stale
            });
        }

        for peer in self.peers.values() {
            let hb = &peer.heartbeat;
            entries.push(Entry {
                machine_id: hb.machine_id.clone(),
                label: hb.label.clone(),
                is_self: false,
                session_tokens: hb.session_tokens,
                weekly_tokens: hb.weekly_tokens,
                age_secs: peer.age_secs(now),
                is_stale: peer.is_stale(now, stale_secs),
            });
        }

        // Denominators: sum tokens of fresh machines only.
        let session_total: u64 = entries
            .iter()
            .filter(|e| !e.is_stale)
            .filter_map(|e| e.session_tokens)
            .sum();
        let weekly_total: u64 = entries
            .iter()
            .filter(|e| !e.is_stale)
            .filter_map(|e| e.weekly_tokens)
            .sum();

        let mut rows: Vec<FleetRow> = entries
            .into_iter()
            .map(|e| {
                let session_share = match (e.session_tokens, session_total) {
                    (Some(t), total) if total > 0 && !e.is_stale => Some(t as f64 / total as f64),
                    _ => None,
                };
                let weekly_share = match (e.weekly_tokens, weekly_total) {
                    (Some(t), total) if total > 0 && !e.is_stale => Some(t as f64 / total as f64),
                    _ => None,
                };
                FleetRow {
                    machine_id: e.machine_id,
                    label: e.label,
                    is_self: e.is_self,
                    session_share,
                    weekly_share,
                    age_secs: e.age_secs,
                    is_stale: e.is_stale,
                }
            })
            .collect();

        // "Who's using more" ordering: highest 5h share first. Stale rows (no
        // share) sink to the bottom; ties broken by weekly share then label.
        rows.sort_by(|a, b| {
            let sa = a.session_share.unwrap_or(-1.0);
            let sb = b.session_share.unwrap_or(-1.0);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let wa = a.weekly_share.unwrap_or(-1.0);
                    let wb = b.weekly_share.unwrap_or(-1.0);
                    wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.label.cmp(&b.label))
        });

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn hb(id: &str, label: &str, st: Option<u64>, wt: Option<u64>, ts: DateTime<Utc>) -> Heartbeat {
        Heartbeat::new(id, label, 22.0, 41.0, st, wt, ts)
    }

    #[test]
    fn ignores_own_echo() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        let changed = state.ingest(hb("me", "self", Some(10), Some(10), now), now, 120);
        assert!(!changed);
        assert_eq!(state.peer_count(), 0);
    }

    #[test]
    fn rejects_replayed_old_heartbeat() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        let old = hb("peer", "p", Some(10), Some(10), now - Duration::seconds(200));
        // max_age = 120 (2× a 60s heartbeat) → 200s old is rejected.
        assert!(!state.ingest(old, now, 120));
        assert_eq!(state.peer_count(), 0);
    }

    #[test]
    fn accepts_fresh_heartbeat_and_updates() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        assert!(state.ingest(hb("peer", "p", Some(10), Some(10), now), now, 120));
        assert_eq!(state.peer_count(), 1);
        // A newer heartbeat replaces it.
        let later = now + Duration::seconds(5);
        assert!(state.ingest(hb("peer", "p", Some(20), Some(20), later), later, 120));
        assert_eq!(state.peer_count(), 1);
    }

    #[test]
    fn drops_out_of_order_older_duplicate() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        let newer = now;
        let older = now - Duration::seconds(10);
        assert!(state.ingest(hb("peer", "p", Some(20), Some(20), newer), now, 120));
        // An older heartbeat for the same peer arrives late → ignored.
        assert!(!state.ingest(hb("peer", "p", Some(5), Some(5), older), now, 120));
    }

    #[test]
    fn share_math_splits_by_tokens() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        state.set_self_heartbeat(hb("me", "MacBook", Some(64), Some(71), now), now);
        state.ingest(hb("peer", "Linux", Some(36), Some(29), now), now, 120);

        let rows = state.rows(now, 120);
        assert_eq!(rows.len(), 2);
        // Heaviest 5h share first.
        assert_eq!(rows[0].label, "MacBook");
        assert!(rows[0].is_self);
        let s = rows[0].session_share.unwrap();
        assert!((s - 0.64).abs() < 1e-9, "got {s}");
        let w = rows[0].session_share.unwrap();
        assert!(w > rows[1].session_share.unwrap());
        // Peer share is the complement.
        assert!((rows[1].session_share.unwrap() - 0.36).abs() < 1e-9);
        assert!(!rows[1].is_self);
    }

    #[test]
    fn stale_peer_excluded_from_denominator_but_still_listed() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        state.set_self_heartbeat(hb("me", "Mac", Some(50), Some(50), now), now);
        // Peer last heard 300s ago, stale_secs = 120 → stale.
        let stale_ts = now - Duration::seconds(300);
        state.ingest(hb("peer", "Old", Some(50), Some(50), stale_ts), stale_ts, 1_000);

        let rows = state.rows(now, 120);
        assert_eq!(rows.len(), 2);
        let me = rows.iter().find(|r| r.is_self).unwrap();
        // Self should own 100% of the live split (stale peer excluded).
        assert!((me.session_share.unwrap() - 1.0).abs() < 1e-9);
        let old = rows.iter().find(|r| !r.is_self).unwrap();
        assert!(old.is_stale);
        assert_eq!(old.session_share, None);
    }

    #[test]
    fn prune_removes_long_silent_peers() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        let ancient = now - Duration::seconds(600);
        state.ingest(hb("peer", "Gone", Some(1), Some(1), ancient), ancient, 100_000);
        assert_eq!(state.peer_count(), 1);
        // stale_secs = 120 → hard cutoff 480s; 600s old peer is pruned.
        let pruned = state.prune(now, 120);
        assert_eq!(pruned, 1);
        assert_eq!(state.peer_count(), 0);
    }

    #[test]
    fn account_pcts_prefers_self() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        state.ingest(hb("peer", "p", Some(1), Some(1), now), now, 120);
        state.set_self_heartbeat(
            Heartbeat::new("me", "self", 33.0, 77.0, Some(1), Some(1), now),
            now,
        );
        assert_eq!(state.account_pcts(), Some((33.0, 77.0)));
    }

    #[test]
    fn who_used_more_ordering_is_by_session_share() {
        let now = Utc::now();
        let mut state = FleetState::new("me");
        state.set_self_heartbeat(hb("me", "Light", Some(10), Some(10), now), now);
        state.ingest(hb("a", "Heavy", Some(80), Some(5), now), now, 120);
        state.ingest(hb("b", "Medium", Some(40), Some(85), now), now, 120);

        let rows = state.rows(now, 120);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Heavy", "Medium", "Light"]);
    }
}
