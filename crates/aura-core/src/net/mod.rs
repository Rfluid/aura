//! Fleet — cross-machine Claude usage comparison over an end-to-end-encrypted
//! ntfy.sh pub/sub channel.
//!
//! Two or more machines on the **same Claude account** pair once (a one-time
//! code), then automatically sync small encrypted "heartbeats" describing how
//! much of the shared 5h / weekly rate-limit windows each machine is using. A
//! Fleet tab shows each machine's share.
//!
//! # Security model (summary — full table in `docs/fleet.md`)
//!
//! The ntfy broker is **untrusted**. Defense in depth:
//! - The topic is a high-entropy HMAC of the secret, not a guessable name
//!   ([`pairing`]).
//! - Every payload is sealed with XChaCha20-Poly1305 under an HKDF-derived key
//!   ([`crypto`]); the broker sees only ciphertext.
//! - Forged / tampered messages fail AEAD authentication and are ignored.
//! - Replays are dropped by timestamp ([`fleet::FleetState::ingest`]).
//! - The 256-bit secret lives only in the OS keychain ([`secret_store`]), never
//!   in `config.toml` or logs.
//!
//! No Claude tokens or message content ever leave the machine — only window
//! percentages and aggregate token counts.
//!
//! # Module map
//!
//! - [`pairing`] — secret, pairing-code encode/decode, topic + key derivations.
//! - [`crypto`] — `seal` / `open` (XChaCha20-Poly1305).
//! - [`fleet`] — [`fleet::FleetState`], heartbeat type, share math, pruning.
//! - [`transport`] — [`transport::FleetTransport`] trait + ntfy impl + mock.
//! - [`secret_store`] — keychain get/set/delete + per-install machine id.
//! - This file — [`FleetSync`], the single background loop that owns the
//!   long-poll subscribe and the outbound publish queue.

pub mod crypto;
pub mod fleet;
pub mod pairing;
pub mod secret_store;
pub mod transport;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;

use fleet::{FleetState, Heartbeat};
use pairing::PairingSecret;
use transport::FleetTransport;

/// Lower bound on the heartbeat interval, enforced regardless of config, to
/// stay polite to the public broker's rate limit.
const MIN_HEARTBEAT_SECS: u64 = 10;

/// Initial reconnect backoff after a broker error.
const BACKOFF_START: Duration = Duration::from_secs(2);
/// Cap on the reconnect backoff.
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Bound on the outbound publish queue. Heartbeats are tiny and superseded by
/// the next one, so a small queue is plenty; a full queue drops the oldest
/// rather than blocking the UI.
const OUTBOUND_CAPACITY: usize = 4;

/// Tuning knobs handed to the sync loop (a subset of `FleetConfig`, decoupled
/// so `aura-core::net` doesn't depend on the config module's layout).
#[derive(Debug, Clone)]
pub struct FleetParams {
    pub broker_url: String,
    pub heartbeat_secs: u64,
    pub stale_secs: u64,
}

impl FleetParams {
    /// Effective heartbeat interval, clamped to [`MIN_HEARTBEAT_SECS`].
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_secs.max(MIN_HEARTBEAT_SECS))
    }

    /// Replay-rejection window: messages older than `2 × heartbeat` are dropped.
    pub fn replay_window_secs(&self) -> u64 {
        self.heartbeat_secs.max(MIN_HEARTBEAT_SECS) * 2
    }
}

/// Handle to the running Fleet background loop. Dropping it (or calling
/// [`Self::shutdown`]) signals the thread to stop. The UI reads the shared
/// [`FleetState`] via [`Self::state`] and pushes fresh local heartbeats via
/// [`Self::publish_local`].
pub struct FleetSync {
    state: Arc<Mutex<FleetState>>,
    outbound: SyncSender<Heartbeat>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FleetSync {
    /// Spawn the background loop. Only call this when `[fleet].enabled` is true
    /// — when disabled, never construct a `FleetSync` so there is zero cost.
    ///
    /// `transport` is boxed so callers can inject a mock in tests; production
    /// passes a [`transport::NtfyTransport`].
    pub fn spawn(
        secret: PairingSecret,
        self_machine_id: String,
        params: FleetParams,
        transport: Box<dyn FleetTransport>,
    ) -> Self {
        let state = Arc::new(Mutex::new(FleetState::new(self_machine_id)));
        let (tx, rx) = mpsc::sync_channel::<Heartbeat>(OUTBOUND_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));

        let loop_state = Arc::clone(&state);
        let loop_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("aura-fleet".to_string())
            .spawn(move || {
                run_loop(secret, params, transport, loop_state, loop_stop, rx);
            })
            .expect("spawn fleet thread");

        Self {
            state,
            outbound: tx,
            stop,
            handle: Some(handle),
        }
    }

    /// Shared view of the current fleet state for rendering. The UI locks,
    /// reads, and unlocks — never holds the lock across a render.
    pub fn state(&self) -> Arc<Mutex<FleetState>> {
        Arc::clone(&self.state)
    }

    /// Enqueue a fresh local heartbeat to publish on the next loop tick (also
    /// called immediately on a local usage change). Non-blocking: if the queue
    /// is full the oldest pending heartbeat is dropped in favour of this newer
    /// one, since only the latest matters.
    pub fn publish_local(&self, hb: Heartbeat) {
        match self.outbound.try_send(hb.clone()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                // Best-effort: drop on a full/closed queue. The next tick's
                // heartbeat carries the same information.
            }
        }
        // Mirror into the shared state immediately so the "you" row updates
        // without waiting for the publish to round-trip.
        if let Ok(mut st) = self.state.lock() {
            st.set_self_heartbeat(hb, Utc::now());
        }
    }

    /// Signal the loop to stop and join the thread. Idempotent.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for FleetSync {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The background loop: publish on a cadence, drain the outbound queue, and
/// long-poll the broker for peers — all on one thread, with backoff+jitter on
/// errors and never a busy-loop.
fn run_loop(
    secret: PairingSecret,
    params: FleetParams,
    transport: Box<dyn FleetTransport>,
    state: Arc<Mutex<FleetState>>,
    stop: Arc<AtomicBool>,
    outbound: Receiver<Heartbeat>,
) {
    let topic = secret.topic();
    let key = secret.aead_key();
    let interval = params.heartbeat_interval();
    let replay_window = params.replay_window_secs();
    // Poll at the heartbeat cadence, but never less than every 5 s, so a peer's
    // freshness stays current without hammering the broker.
    let poll_interval = interval.max(Duration::from_secs(5));

    let mut backoff = BACKOFF_START;
    let mut cursor: Option<String> = None;
    let mut last_publish = std::time::Instant::now()
        .checked_sub(interval)
        .unwrap_or_else(std::time::Instant::now);

    while !stop.load(Ordering::Relaxed) {
        // 1. Publish if it's time, or if a fresh local heartbeat was queued.
        let queued = drain_latest(&outbound);
        let due = last_publish.elapsed() >= interval;
        if let Some(hb) = queued.or_else(|| {
            if due {
                state.lock().ok().and_then(|s| s.self_heartbeat_clone())
            } else {
                None
            }
        }) {
            let plaintext = serde_json::to_vec(&hb).unwrap_or_default();
            if !plaintext.is_empty() {
                let sealed = crypto::seal(&key, &plaintext);
                let body = B64.encode(&sealed);
                match transport.publish(&topic, &body) {
                    Ok(()) => {
                        last_publish = std::time::Instant::now();
                        set_reachable(&state, true);
                    }
                    Err(e) => {
                        eprintln!("aura: fleet publish failed: {e}");
                        set_reachable(&state, false);
                    }
                }
            }
        }

        // 2. Long-poll for peers. The broker holds the connection open and
        //    streams messages; `poll` returns on close/timeout.
        match transport.poll(&topic, cursor.as_deref()) {
            Ok(messages) => {
                backoff = BACKOFF_START;
                set_reachable(&state, true);
                let now = Utc::now();
                for msg in messages {
                    cursor = Some(msg.id.clone());
                    let Ok(blob) = B64.decode(msg.body.as_bytes()) else {
                        continue; // not our base64 → ignore
                    };
                    let Some(plain) = crypto::open(&key, &blob) else {
                        continue; // forged / wrong-key / tampered → ignore
                    };
                    let Ok(hb) = serde_json::from_slice::<Heartbeat>(&plain) else {
                        continue;
                    };
                    if let Ok(mut st) = state.lock() {
                        st.ingest(hb, now, replay_window);
                    }
                }
                // Prune stale peers each cycle regardless of new traffic.
                if let Ok(mut st) = state.lock() {
                    st.prune(now, params.stale_secs);
                }
            }
            Err(e) => {
                eprintln!("aura: fleet poll failed: {e}; backing off {backoff:?}");
                set_reachable(&state, false);
                sleep_with_stop(&stop, jittered(backoff));
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }

        // Nap between one-shot polls so we stay well under the public broker's
        // rate limit (it tolerates ~1 req / 10 s after an initial burst). A
        // locally-queued heartbeat doesn't need to interrupt this nap: the
        // user's own row is mirrored into shared state synchronously by
        // `publish_local`, and the next loop iteration picks the queued message
        // up via `drain_latest`. `poll_interval` is the heartbeat interval,
        // floored at 5 s, so peers refresh at the publish cadence.
        sleep_with_stop(&stop, poll_interval);
    }
}

/// Drain the outbound queue, keeping only the most recent heartbeat (older
/// queued ones are stale).
fn drain_latest(rx: &Receiver<Heartbeat>) -> Option<Heartbeat> {
    let mut latest = None;
    while let Ok(hb) = rx.try_recv() {
        latest = Some(hb);
    }
    latest
}

fn set_reachable(state: &Arc<Mutex<FleetState>>, reachable: bool) {
    if let Ok(mut st) = state.lock() {
        st.broker_reachable = reachable;
    }
}

/// Add ±25% jitter to a backoff so reconnecting peers don't synchronize.
fn jittered(base: Duration) -> Duration {
    use rand::Rng;
    let millis = base.as_millis() as f64;
    let factor = rand::thread_rng().gen_range(0.75..1.25);
    Duration::from_millis((millis * factor) as u64)
}

/// Sleep in short slices so a stop signal is honoured promptly.
fn sleep_with_stop(stop: &Arc<AtomicBool>, total: Duration) {
    let slice = Duration::from_millis(100);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let nap = remaining.min(slice);
        std::thread::sleep(nap);
        remaining = remaining.saturating_sub(nap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::MockTransport;

    fn params() -> FleetParams {
        FleetParams {
            broker_url: "mock".to_string(),
            heartbeat_secs: 10,
            stale_secs: 120,
        }
    }

    #[test]
    fn params_clamp_and_replay_window() {
        let p = FleetParams {
            broker_url: "x".into(),
            heartbeat_secs: 1, // below the floor
            stale_secs: 120,
        };
        assert_eq!(p.heartbeat_interval(), Duration::from_secs(MIN_HEARTBEAT_SECS));
        assert_eq!(p.replay_window_secs(), MIN_HEARTBEAT_SECS * 2);
    }

    /// End-to-end through the mock transport: a published local heartbeat is
    /// sealed, lands on the topic, and a *second* sync (a stand-in for the
    /// peer machine, with a different self_id and shared secret) ingests it.
    #[test]
    fn two_syncs_over_mock_transport_see_each_other() {
        let secret = PairingSecret::generate();
        let shared = Arc::new(MockTransport::new());

        // Machine A.
        let a = FleetSync::spawn(
            secret.clone(),
            "machine-a".to_string(),
            params(),
            Box::new((*shared).clone()),
        );
        // Machine B (same secret → same topic + key).
        let b = FleetSync::spawn(
            secret.clone(),
            "machine-b".to_string(),
            params(),
            Box::new((*shared).clone()),
        );

        let now = Utc::now();
        a.publish_local(Heartbeat::new(
            "machine-a", "A", 20.0, 40.0, Some(100), Some(200), now,
        ));
        b.publish_local(Heartbeat::new(
            "machine-b", "B", 20.0, 40.0, Some(300), Some(400), now,
        ));

        // Give the loops time to publish + poll. The poll cadence is the
        // (clamped) 10 s heartbeat interval, so the deadline must clear at
        // least two polls; we poll the assertion to finish as soon as both
        // sides converge rather than always waiting the full timeout.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut a_sees_b = false;
        let mut b_sees_a = false;
        while std::time::Instant::now() < deadline {
            a_sees_b = a.state().lock().unwrap().peer_count() >= 1;
            b_sees_a = b.state().lock().unwrap().peer_count() >= 1;
            if a_sees_b && b_sees_a {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(a_sees_b, "machine A never saw machine B");
        assert!(b_sees_a, "machine B never saw machine A");
    }

    #[test]
    fn publish_local_updates_self_row_immediately() {
        let secret = PairingSecret::generate();
        let transport = Box::new(MockTransport::new());
        let sync = FleetSync::spawn(secret, "me".to_string(), params(), transport);

        sync.publish_local(Heartbeat::new(
            "me", "Self", 22.0, 41.0, Some(10), Some(10), Utc::now(),
        ));
        // Self heartbeat is mirrored synchronously, before any network.
        let st = sync.state();
        let guard = st.lock().unwrap();
        assert!(guard.self_heartbeat_clone().is_some());
    }
}
