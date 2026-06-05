---
title: Fleet
status: draft
version: 0.1.0
last_updated: 2026-06-05
last_verified: 2026-06-05
source_refs: ["crates/aura-core/src/net/", "crates/aura/src/app.rs"]
owner: "@pedro"
---

# Fleet — cross-machine usage comparison

A single Claude subscription's rate limits (the 5-hour session window and the
weekly window) are **shared across every machine** you run Claude Code on. Aura
normally only sees the local machine. Fleet lets two or more machines on the
**same Claude account** compare how much of the shared limit each is eating.

Fleet is **off by default**. When disabled, no network task runs, no keychain
is touched, and the Fleet tab is hidden — zero cost for non-users.

## Enabling

```toml
[fleet]
enabled = true            # master switch (default false)
broker_url = "https://ntfy.sh"  # pub/sub broker; self-host for full privacy
machine_label = ""        # blank = system hostname
heartbeat_secs = 45       # publish cadence (min 10 enforced)
stale_secs = 120          # a silent peer dims after this many seconds
```

Restart Aura (or click Refresh) after editing. The Fleet tab appears next to
Quota/Forecast/Summary/Models when the Claude agent is selected.

## Pairing (one-time)

1. **Machine A** → Fleet tab → **Pair a machine**. Aura generates a 256-bit
   secret and shows a human-typable pairing code. Click **Copy code**.
2. **Machine B** → put the code on the clipboard → Fleet tab → **Join from
   clipboard**. Both machines now derive the same channel and start syncing.
3. **Leave fleet** deletes the secret from the keychain and stops syncing.
   Re-running **Pair a machine** rotates the secret (all peers must re-join).

A pairing code is a single fleet-wide secret, not per-peer: N machines share
one secret. That's fine for a personal 2–3 machine setup.

## What's shown

- Per-machine rows: a freshness dot, the label (yours tagged "(you)"), and two
  share bars — **5h share** and **weekly share**. Rows are sorted heaviest-5h
  first ("who's using more").
- An account sanity line with the shared 5h% and weekly%.
- Stale peers (no heartbeat for `stale_secs`) dim and drop out of the share
  math.

### Why share is computed from tokens, not percentages

The account's window percentages are **global** — every machine's `/usage`
call returns the *same* account-wide 5h% and weekly%. So a peer's percentage
says nothing about *that machine's* contribution. The per-machine split comes
from token counts:

```text
machine_share  = machine_tokens / Σ(fresh peer tokens)
attributed_pct = machine_share × account_window_pct
```

The percentages are still sent for the sanity line and freshness.

## Security model

The ntfy broker is **untrusted**. Defense in depth:

| Threat | Mitigation |
|---|---|
| Broker reads usage data | E2E XChaCha20-Poly1305; broker sees only ciphertext + an opaque topic |
| Topic guessed / enumerated | Topic = 80-bit HMAC-SHA256 of a 256-bit secret; not derivable without it |
| Eavesdropper on the wire | HTTPS to the broker **and** app-layer AEAD |
| Replay of old heartbeats | `ts` checked; messages older than `2 × heartbeat_secs` dropped; fresh nonce per message |
| Secret at rest | OS keychain (service `aura-fleet-secret`); file fallback `0600`; never in `config.toml` or logs |
| Compromised broker injects msgs | Forged messages fail AEAD authentication (`open` → `None`) → ignored |
| Screenshot leak | Pairing code shown only transiently during pairing, never persisted to the visible UI |

No Claude tokens or message content ever leave the machine — only window
percentages and aggregate token counts.

### Derivations

Given the 256-bit secret `s`:

- **Topic** = `"aura-" + base32_lower( HMAC-SHA256(s, "ntfy-topic") )[..16]`
- **AEAD key** = `HKDF-SHA256(ikm = s, info = "aura-fleet-v1")` → 32 bytes
- **Wire format** = `base64( XNonce(24B) || XChaCha20-Poly1305(key, nonce, plaintext) )`

HMAC for the topic and HKDF for the key keep the two domain-separated: the
broker (which sees the topic) learns nothing about the encryption key.

### Secret storage, per platform

- **macOS** — login Keychain via `security-framework` (dedicated service
  `aura-fleet-secret`; never touches Claude Code's `Claude Code-credentials`).
- **Windows** — Credential Manager via the `keyring` crate.
- **Linux** — Secret Service (libsecret / GNOME Keyring / KWallet) via the
  `keyring` crate. **Fallback:** when no secret service is reachable (headless
  servers, no DBus session), the secret is written to
  `$XDG_DATA_HOME/aura/fleet-secret` with mode `0600`.

> **Fallback caveat.** The `0600` file is protected only by Unix file
> permissions. Anyone who can read your home directory (root, a backup, a
> misconfigured sync tool) can read the secret. The keychain is always
> preferred; the file exists so Fleet works at all on headless machines.

## Transport — ntfy.sh

ntfy is a free, account-less HTTP pub/sub broker.

- **Publish:** `POST {broker}/{topic}` with the base64 sealed blob as the body
  (`Priority: min`, `X-Cache: no` so it's a silent, ephemeral sync channel).
- **Subscribe:** long-poll `GET {broker}/{topic}/json?since={cursor}`; ntfy
  streams newline-delimited JSON; we forward only `message` events.

The public server's rate limit (60 burst, then 1 req / 10 s) is far above the
default 45 s heartbeat. For heavier setups, self-host ntfy and set `broker_url`.

## Resource budget

- One background thread per machine: a long-poll subscribe plus a bounded
  outbound queue. No per-peer threads. The peer map holds a handful of small
  structs. Backoff + jitter on broker errors; never a busy-loop.
- The thread only runs while the Aura modal is open (Aura fetches Claude usage
  on modal open, which is the heartbeat source). When the modal is closed Aura
  is just a tray icon; the next open resumes syncing. See `BUILD_NOTES.md`.

## Limitations

- Sync is active while the modal is open (see above), not 24/7.
- ntfy public-server availability is best-effort; the tab shows "broker
  unreachable" and keeps retrying rather than crashing.
