//! Transport abstraction over the ntfy.sh pub/sub broker.
//!
//! [`FleetTransport`] is the seam the rest of Fleet talks to, so the sync loop
//! and [`crate::net::fleet`] tests can run against an in-memory [`MockTransport`]
//! with no network. The real [`NtfyTransport`] speaks the documented ntfy HTTP
//! API:
//!
//! - **Publish:** `POST {broker}/{topic}` with the message as the request
//!   body. We send the base64 of the sealed blob (ASCII, well under ntfy's
//!   4 KiB UTF-8 message limit), with `Priority: min` and `X-Cache: no` so the
//!   broker neither pushes a notification nor pins our ciphertext in its cache
//!   longer than needed.
//! - **Subscribe (poll):** `GET {broker}/{topic}/json?poll=1&since={cursor}`.
//!   With `poll=1` ntfy returns the messages cached since `since` and closes
//!   the connection immediately (no held-open stream). The body is still
//!   newline-delimited JSON, read line by line; `event` is one of `open` /
//!   `message` / `keepalive` / `poll_request` and we forward only `message`
//!   payloads. One-shot polling keeps the single sync thread free to publish
//!   on its heartbeat cadence rather than blocking on a long-lived stream; the
//!   caller naps between polls and reconnects with backoff on error.
//!
//! The broker is untrusted — every byte crossing this boundary is already
//! sealed by [`crate::net::crypto`]. This layer does no crypto itself; it only
//! moves opaque base64 strings.

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// Maximum bytes we'll read from a single subscribe response before forcing a
/// reconnect — a guard against a hostile broker streaming forever. Heartbeats
/// are tiny; a few hundred KiB is generous.
const MAX_STREAM_BYTES: usize = 512 * 1024;

/// One decoded inbound message: the raw (still-base64, still-sealed) body and
/// the broker-assigned id we use as a resume cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    /// ntfy message id — opaque; passed back as `since=` to resume without
    /// re-receiving it.
    pub id: String,
    /// The message body exactly as published (base64 of the sealed blob).
    pub body: String,
}

/// The transport seam. Implementors move opaque message bodies to/from a
/// topic. All methods are blocking; the Fleet sync loop runs them on its own
/// dedicated thread.
pub trait FleetTransport: Send + Sync {
    /// Publish `body` to `topic`. Returns once the broker has accepted it.
    fn publish(&self, topic: &str, body: &str) -> Result<()>;

    /// Long-poll `topic` for new messages, resuming after `since` (a message
    /// id, or `None` to start at "now"). Blocks until the broker closes the
    /// stream (or [`MAX_STREAM_BYTES`] is hit), returning everything received.
    /// An empty vec is a normal idle timeout, not an error.
    fn poll(&self, topic: &str, since: Option<&str>) -> Result<Vec<InboundMessage>>;
}

// ── ntfy.sh implementation ───────────────────────────────────────────────────

/// Live transport against an ntfy broker (default `https://ntfy.sh`).
pub struct NtfyTransport {
    /// Broker base URL, no trailing slash (e.g. `https://ntfy.sh`).
    broker_url: String,
    agent: ureq::Agent,
}

impl NtfyTransport {
    /// Construct against `broker_url` (trailing slash stripped). A 20 s global
    /// timeout bounds each one-shot poll/publish; since we use `poll=1` the
    /// broker returns promptly, so this only fires on a genuinely stuck
    /// connection (where backoff is exactly what we want).
    pub fn new(broker_url: impl Into<String>) -> Self {
        let broker_url = broker_url.into().trim_end_matches('/').to_string();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .build()
            .into();
        Self { broker_url, agent }
    }
}

/// Subset of the ntfy JSON message envelope we care about. See
/// <https://docs.ntfy.sh/subscribe/api/>. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct NtfyEnvelope {
    #[serde(default)]
    id: String,
    event: String,
    #[serde(default)]
    message: Option<String>,
}

impl FleetTransport for NtfyTransport {
    fn publish(&self, topic: &str, body: &str) -> Result<()> {
        let url = format!("{}/{}", self.broker_url, topic);
        // `Priority: min` keeps this silent (no phone notification). Caching is
        // left ON: the subscribe side uses one-shot `poll=1`, which only returns
        // *cached* messages, so a peer that polls a few seconds after a heartbeat
        // was published still receives it. `X-Cache: no` would drop the message
        // for any peer not holding a live stream at the exact publish instant —
        // i.e. always, with this poll-based design.
        self.agent
            .post(&url)
            .header("Priority", "min")
            .header("Content-Type", "text/plain")
            .send(body)
            .map_err(|e| anyhow!("ntfy publish to {url} failed: {e}"))?;
        Ok(())
    }

    fn poll(&self, topic: &str, since: Option<&str>) -> Result<Vec<InboundMessage>> {
        // `poll=1` returns cached messages and closes immediately. `since=<id>`
        // resumes after the last-seen message; the first poll (no cursor) uses
        // `since=30s` so a peer that published a few seconds before we started
        // is still picked up, without replaying the whole topic history.
        let cursor = since.unwrap_or("30s");
        let url = format!("{}/{}/json?poll=1&since={cursor}", self.broker_url, topic);

        let response = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("ntfy subscribe to {url}"))?;

        // Owned streaming reader so we can read the (small, poll=1) body line
        // by line without imposing the default 10 MB buffered read.
        let (_, body) = response.into_parts();
        let reader = BufReader::new(body.into_reader());
        let mut out = Vec::new();
        let mut total = 0usize;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break, // stream closed / read timeout: normal
            };
            total += line.len();
            if total > MAX_STREAM_BYTES {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            let env: NtfyEnvelope = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue, // tolerate non-JSON keepalive lines
            };
            match env.event.as_str() {
                "message" => {
                    if let Some(body) = env.message {
                        out.push(InboundMessage { id: env.id, body });
                    }
                }
                // "open" / "keepalive" / "poll_request" carry no payload.
                _ => {}
            }
        }
        Ok(out)
    }
}

// ── In-memory mock (tests) ───────────────────────────────────────────────────

/// In-memory transport for unit tests: a shared topic→messages map. `publish`
/// appends; `poll` returns everything after the `since` id. No network, fully
/// deterministic.
#[derive(Clone, Default)]
pub struct MockTransport {
    inner: Arc<Mutex<MockInner>>,
}

#[derive(Default)]
struct MockInner {
    /// (topic, id, body) in publish order.
    log: Vec<(String, String, String)>,
    next_id: u64,
    /// Set to make every publish/poll error, to exercise the backoff path.
    fail: bool,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle failure mode (every call returns `Err`).
    pub fn set_failing(&self, fail: bool) {
        self.inner.lock().unwrap().fail = fail;
    }

    /// Total messages published to `topic` so far (test introspection).
    pub fn published_count(&self, topic: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|(t, _, _)| t == topic)
            .count()
    }
}

impl FleetTransport for MockTransport {
    fn publish(&self, topic: &str, body: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.fail {
            return Err(anyhow!("mock transport in failure mode"));
        }
        inner.next_id += 1;
        let id = format!("m{}", inner.next_id);
        inner.log.push((topic.to_string(), id, body.to_string()));
        Ok(())
    }

    fn poll(&self, topic: &str, since: Option<&str>) -> Result<Vec<InboundMessage>> {
        let inner = self.inner.lock().unwrap();
        if inner.fail {
            return Err(anyhow!("mock transport in failure mode"));
        }
        // Find the index after the `since` id, then return the tail.
        let start = match since {
            None => 0,
            Some(cursor) => inner
                .log
                .iter()
                .position(|(t, id, _)| t == topic && id == cursor)
                .map(|i| i + 1)
                .unwrap_or(0),
        };
        let out = inner
            .log
            .iter()
            .skip(start)
            .filter(|(t, _, _)| t == topic)
            .map(|(_, id, body)| InboundMessage {
                id: id.clone(),
                body: body.clone(),
            })
            .collect();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_round_trips_published_messages() {
        let t = MockTransport::new();
        t.publish("topic-a", "hello").unwrap();
        t.publish("topic-a", "world").unwrap();
        t.publish("topic-b", "other").unwrap();

        let msgs = t.poll("topic-a", None).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].body, "hello");
        assert_eq!(msgs[1].body, "world");

        // Resume after the first id → only the second.
        let resumed = t.poll("topic-a", Some(&msgs[0].id)).unwrap();
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].body, "world");
    }

    #[test]
    fn mock_isolates_topics() {
        let t = MockTransport::new();
        t.publish("a", "x").unwrap();
        t.publish("b", "y").unwrap();
        assert_eq!(t.poll("a", None).unwrap().len(), 1);
        assert_eq!(t.poll("b", None).unwrap().len(), 1);
    }

    #[test]
    fn mock_failure_mode_errors() {
        let t = MockTransport::new();
        t.set_failing(true);
        assert!(t.publish("a", "x").is_err());
        assert!(t.poll("a", None).is_err());
    }

    /// Live round-trip against the real public ntfy.sh broker. **Ignored** by
    /// default so it never runs in CI or during a normal `cargo test` — it
    /// touches the network. Run manually with:
    /// `cargo test -p aura-core -- --ignored net::transport::live_ntfy`.
    #[test]
    #[ignore = "hits the live ntfy.sh broker; run manually"]
    fn live_ntfy_round_trip() {
        use rand::Rng;
        // Random topic so concurrent runs don't collide.
        let suffix: u64 = rand::thread_rng().gen();
        let topic = format!("aura-itest-{suffix:016x}");
        let t = NtfyTransport::new("https://ntfy.sh");

        let payload = "aura-fleet-integration-probe";
        t.publish(&topic, payload).expect("publish");

        // Poll back. ntfy retains recent messages; `since=all` would also
        // work, but we publish-then-poll so a fresh `None` poll may miss it
        // due to timing — use `since=all` here explicitly for determinism.
        let msgs = t
            .poll(&topic, Some("all"))
            .expect("poll");
        assert!(
            msgs.iter().any(|m| m.body == payload),
            "did not observe the published payload in {msgs:?}"
        );
    }
}
