---
title: Fleet — cross-machine usage compare (F1)
status: design
version: 0.1.0
last_updated: 2026-06-05
owner: "@pedro (fork) → upstream rfluid/aura"
tags: [fleet, networking, e2e, sync, design]
---

# F1 — Fleet tab

## Problem

A single Claude subscription's rate limits (5h session + weekly) are **shared across every
machine the user runs Claude Code on**. Aura today only sees the local machine. The user runs CC
on two machines and wants to know **which machine is eating more of the shared limit**, with each
machine's **share of the day (5h window) and of the week**.

The Claude OAuth token is **opaque** (`sk-ant-oat01-…`, not a JWT) and **differs per machine**
even for the same account, so there is no reliable zero-config "same account" fingerprint to
auto-discover peers. Pairing must establish a shared secret explicitly, once.

## Scope

A new modal tab named **Fleet**, opt-in, off by default. When ≥1 peer is paired and reachable:

- Per-machine rows: label, **% of the 5h session window** and **% of the weekly window** this
  machine is responsible for, plus a "you / them" highlight and a "who's using more" sort.
- Aggregate line: combined 5h% and weekly% (sanity vs the account's real numbers).
- Freshness: each peer row shows "updated 12s ago"; stale (> 2 min) peers dim.

Out of scope: more than the pairing handshake of identity; controlling/limiting other machines;
historical peer charts; any non-Claude agent.

## Transport — ntfy.sh, end-to-end encrypted

**Why ntfy:** free, no account, no server to run, HTTP pub/sub, tiny footprint (one long-poll
GET + periodic POST). Works across networks (not LAN-bound). Public-server rate limit
(60 burst, then 1 req / 10 s) is far above a 30–60 s heartbeat. Self-host is a one-line config
override for the privacy-conscious.

**Topic:** never a guessable name. `topic = "aura-" + base32( HMAC-SHA256(pairing_secret,
"ntfy-topic") )[:16]`. The topic name itself is a high-entropy capability derived from the
shared secret — only paired devices know it.

**Encryption (defense in depth — the broker is untrusted):** every published payload is sealed
with **XChaCha20-Poly1305** (`chacha20poly1305` crate, or `crypto_secretbox` via `dryoc`/
libsodium — pick the lighter pure-Rust option to avoid a C dep in the workspace). Symmetric key
`k = HKDF-SHA256(pairing_secret, info="aura-fleet-v1")`. Random 24-byte nonce per message,
prepended to the ciphertext, whole blob base64'd as the ntfy message body. A peer that can't
decrypt (wrong/rotated secret) is ignored. So even though the topic is on a public broker, the
broker and any eavesdropper see only ciphertext.

## Pairing flow (one-time, then automatic forever)

1. Machine A: Fleet tab → "Pair a machine" → Aura generates a 256-bit `pairing_secret`, shows it
   as a short human-typable code (e.g. `bip39` 6–8 words, or base32 grouped) + a copy button.
2. Machine B: "Join fleet" → paste the code → derive the same `pairing_secret`.
3. Both store `pairing_secret` in the **OS keychain** (see cross-platform note), never in
   `config.toml`. Both derive the same topic + key and immediately start publishing/subscribing.
4. Each machine has a stable `machine_label` (default = hostname, editable in config) and a random
   per-install `machine_id` (uuid) so two machines with the same hostname still disambiguate.

Re-pairing / rotation: "Leave fleet" deletes the keychain secret; "Rotate" generates a new secret
and re-shows the code (all peers must re-join). A pairing code is single-secret, not per-peer —
N machines share one fleet secret (fine for a personal 2–3 machine setup; documented).

## Heartbeat message

Published every `heartbeat_secs` (default 45) and once immediately on any local usage change
(hook into the existing snapshot refresh, not a new timer where avoidable):

```jsonc
// plaintext, before sealing
{
  "v": 1,
  "machine_id": "8f3c…",          // random per install
  "label": "Pedros-MacBook-Air",
  "session_pct": 22.4,            // local 5h QuotaWindow.used_percentage
  "weekly_pct":  41.0,            // local 7d  QuotaWindow.used_percentage
  "session_tokens": 38295380,     // optional, for relative "who used more"
  "weekly_tokens":  895321877,
  "ts": "2026-06-05T18:03:11Z"
}
```

Note: the account's % values are **shared/global** — every machine's API call returns the *same*
account-wide 5h% and weekly%. So "% per machine" is computed from **tokens**, not from these
percentages: `machine_share = machine_tokens / Σ peer_tokens`, then multiply by the account-wide
window % to attribute it. The %s are still sent for the aggregate sanity line and freshness.

## Architecture

- **`aura-core/src/net/mod.rs`** (new module tree):
  - `pairing.rs` — secret generation, code encode/decode (`bip39` or base32), HKDF/HMAC
    derivations (`hkdf` + `hmac` + `sha2` crates). Pure, fully unit-testable.
  - `crypto.rs` — `seal(secret, plaintext) -> Vec<u8>` / `open(secret, blob) -> Option<Vec<u8>>`
    (XChaCha20-Poly1305). Pure, unit-tested with known vectors.
  - `transport.rs` — `trait FleetTransport { publish(&[u8]); subscribe() -> Receiver<Vec<u8>> }`
    with an `NtfyTransport` impl (the broker URL is configurable). One background async task
    (reuse the runtime aura already has; do **not** spin a new full tokio runtime if avoidable —
    check `crates/aura/src/runtime.rs`) owns the long-poll subscribe + a small outbound queue.
  - `fleet.rs` — `FleetState { peers: HashMap<machine_id, PeerSnapshot>, last_publish }`,
    prunes peers idle > `stale_secs`, computes the per-machine shares. `PeerSnapshot` is the
    decoded heartbeat + `received_at`.
  - `secret_store.rs` — get/set/delete the fleet secret in the keychain, reusing the
    cross-platform pattern from `quota/oauth.rs`.
- **`aura-core/src/quota/`** — read the local window %s + tokens to build the outbound heartbeat
  (already computed for the Quota tab; just reused).
- **`aura/src/app.rs`** — `Fleet` tab in the tab enum + `render_tab_row`; `render_fleet(theme,
  &FleetState)` (rows, bars, freshness dots); pairing UI (generate/show/paste) as a small
  sub-panel reusing the existing settings-panel/more-modal primitives (`render_settings_panel`
  at line ~2276 is the structural template). Tab hidden unless `[fleet].enabled`.
- **`aura-core/src/config.rs`** — `[fleet]` section: `enabled` (false), `broker_url`
  (`https://ntfy.sh`), `machine_label` (default hostname), `heartbeat_secs` (45),
  `stale_secs` (120).

### Cross-platform secret storage (mac + linux)

`oauth.rs` already does macOS Keychain via `security-framework` with a file fallback. For F1 use
the same approach but a **dedicated service name** `"aura-fleet-secret"` (do NOT touch the
`Claude Code-credentials` entry). On Linux, use the `keyring` crate (Secret Service / libsecret);
if no secret service is present (headless), fall back to `~/.local/share/aura/fleet-secret`
written `0600`, mirroring how `oauth.rs` falls back to the on-disk credential file. Document the
fallback's weaker guarantees.

## Security threat model

| Threat | Mitigation |
|---|---|
| Broker reads usage data | E2E XChaCha20-Poly1305; broker sees only ciphertext + opaque topic |
| Topic guessed / enumerated | Topic = 80-bit HMAC of a 256-bit secret; not derivable without the secret |
| Eavesdropper on the wire | HTTPS to ntfy **and** app-layer AEAD |
| Replay of old heartbeats | `ts` checked; messages older than `2 × heartbeat_secs` dropped; nonce per msg |
| Secret at rest | OS keychain; file fallback `0600`; never in `config.toml` or logs |
| Compromised broker injects msgs | Forged msgs fail AEAD auth (`open` returns `None`) → ignored |
| Accidental leak in screenshots | Pairing code shown only during pairing, never persisted to the visible UI afterward |

No Claude tokens or message content ever leave the machine — only window %s and aggregate token
counts.

## Resource budget (RAM/CPU)

- One async task: a long-poll `GET /<topic>/json` (or SSE) + a bounded outbound channel. No
  per-peer threads. Peer map is a few small structs (2–3 machines). Target: well under ~2 MB
  additional RSS and ~0 CPU between heartbeats.
- When `[fleet].enabled = false` (default) the task is never spawned — zero cost for users who
  don't opt in.
- Backoff + jitter on broker errors; cap reconnect attempts; never busy-loop.

## UI sketch (terminal mock)

```
┌ Fleet ─────────────────────────────────────────────┐
│ Account weekly: 41%   ·   5h session: 22%           │
│                                                     │
│  MACHINE            5h share   weekly share         │
│  ● MacBook-Air (you) ███████ 64%   ████████ 71%     │
│  ● Linux-desktop     ████    36%   ███      29%     │
│                                                     │
│  ● updated 8s ago   ○ Linux-desktop 12s ago         │
│  [ Pair a machine ]   [ Leave fleet ]               │
└─────────────────────────────────────────────────────┘
```

## Testing

- **`pairing.rs`**: code round-trips (secret → code → secret); two machines from the same code
  derive identical topic + key; different secrets → different topics.
- **`crypto.rs`**: `open(seal(x)) == x`; tampered ciphertext → `None`; wrong key → `None`; known
  test vectors.
- **`fleet.rs`**: feed synthetic decoded heartbeats → assert share math, stale pruning, "who's
  using more" ordering, self-vs-peer tagging.
- **`transport.rs`**: trait-mock transport (in-memory channel) for `fleet.rs` tests; a single
  **ignored-by-default** integration test against real ntfy.sh (`#[ignore]`, run manually) that
  publishes + subscribes a random topic and asserts the round-trip. Never required in CI.

## Risks / open items

- Public ntfy rate limits could throttle if many machines or short heartbeats — defaults stay
  well under; document self-host for heavier setups.
- ntfy public-server availability is best-effort; show "broker unreachable" state, never crash.
- Crate choice: prefer pure-Rust `chacha20poly1305` + `hkdf` + `hmac` + `sha2` + `bip39` to keep
  the build C-dependency-free and cross-compilable for the Linux target. Confirm against the
  existing `Cargo.lock` to avoid duplicate-version bloat.
