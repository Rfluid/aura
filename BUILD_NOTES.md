# Fleet (F1) — Build & Verify Notes

Implementation of the **Fleet** feature (cross-machine Claude usage comparison
over an E2E-encrypted ntfy.sh channel). Code was written but **not compiled or
run** in this worktree (machine under load). This file lists everything a later
build/verify pass needs.

## Crates added (`crates/aura-core/Cargo.toml`)

All chosen to reuse transitive versions already pinned in `Cargo.lock` (no
duplicate-version bloat). All pure-Rust (no C deps) → cross-compiles to Linux.

| Crate | Version | Why |
|---|---|---|
| `chacha20poly1305` | `0.10` | XChaCha20-Poly1305 AEAD (`seal`/`open`). **NEW to the lock tree** — pulls `chacha20`, `poly1305`, `aead 0.5` (all RustCrypto, pure Rust). |
| `hkdf` | `0.12` | HKDF-SHA256 → AEAD key. Already in lock (`hkdf 0.12.4`). |
| `hmac` | `0.12` | HMAC-SHA256 → ntfy topic. Already in lock (`0.12.1`). |
| `sha2` | `0.10` | Hash backing HKDF/HMAC. Already in lock (`0.10.9`). |
| `base64` | `0.22` | Wire encoding of the sealed blob. Already in lock (`0.22.1`). |
| `rand` | `0.8` | CSPRNG for secret + nonce. Already in lock (`0.8.6`). |
| `zeroize` | `1` | Wipe the pairing secret on drop. Already in lock (`1.8.2`). |
| `uuid` | `1` (`v4`) | Per-install machine id. Already in lock (`1.23.1`). |
| `keyring` (Linux only) | `3` features `["sync-secret-service","crypto-rust"]` | Secret Service backend. `keyring 3.6.3` already in lock for the Windows path; the Linux features add `sync-secret-service` deps (`crypto-rust` keeps it OpenSSL-free). |

`security-framework` (macOS) and `keyring` (Windows) were **already**
dependencies for the OAuth path; Fleet reuses them with a dedicated service
name and adds no new macOS/Windows crates.

> **Build-time risk:** `chacha20poly1305 0.10` is the only genuinely new
> dependency subtree. It depends on `aead 0.5` / `crypto-common 0.1` / `cipher
> 0.4`, all of which match versions already in the lock — so no duplicate
> majors are expected, but `cargo build -p aura-core` will resolve and download
> the chacha/poly1305 crates for the first time. Verify the lock update adds
> only the RustCrypto AEAD subtree.
>
> **Linux secret-service risk:** the `sync-secret-service` feature pulls a
> blocking DBus stack (`zbus`/`dbus` family). If the build target is musl or a
> minimal container without the needed system bits, prefer building with the
> file fallback only — but `crypto-rust` was chosen specifically to avoid the
> OpenSSL C dependency. Confirm it resolves on the Linux CI image.

## Files

**New (`crates/aura-core/src/net/`):**
- `mod.rs` — module docs, re-exports, `FleetSync` (the single background
  thread: publish cadence + one-shot poll + outbound queue, backoff+jitter).
- `pairing.rs` — `PairingSecret` (gen, base32 code encode/decode, topic + key
  derivations). Pure, unit-tested.
- `crypto.rs` — `seal`/`open` (XChaCha20-Poly1305). Pure, unit-tested + vector.
- `fleet.rs` — `Heartbeat`, `PeerSnapshot`, `FleetRow`, `FleetState` (ingest,
  replay/dup rejection, stale prune, token-share math, ordering). Pure tests.
- `transport.rs` — `FleetTransport` trait, `NtfyTransport` (ureq), `MockTransport`
  (in-memory) + the `#[ignore]` live ntfy round-trip test.
- `secret_store.rs` — keychain get/set/delete (service `aura-fleet-secret`) +
  per-install `machine_id`, with the Linux file fallback.

**Modified:**
- `crates/aura-core/src/lib.rs` — `pub mod net;`.
- `crates/aura-core/src/config.rs` — `FleetConfig` + `AppConfig.fleet` (additive;
  `default_config` + test literals updated).
- `crates/aura-core/src/config_schema.rs` — `[fleet]` field descriptors,
  get/set/toml_rhs, `render_commented` table, anti-drift test loop + literal.
- `crates/aura-core/Cargo.toml` — deps above.
- `crates/aura/src/app.rs` — `AgentSection::Fleet`, `fleet_sync`/`fleet_code`/
  `fleet_status` fields, `ensure_fleet_started`/pairing actions/heartbeat push,
  `render_fleet` + `render_fleet_pairing` + row/bar helpers, tab gating.
- `docs/fleet.md` — feature + security docs.

## ntfy.sh API coded against (verify these)

Docs: <https://docs.ntfy.sh/publish/>, <https://docs.ntfy.sh/subscribe/api/>.

- **Publish:** `POST {broker_url}/{topic}`, body = base64 of the sealed blob
  (ASCII, ≪ ntfy's 4 KiB UTF-8 message cap). Headers: `Priority: min`,
  `X-Cache: no`, `Content-Type: text/plain`. ureq treats 4xx/5xx as `Err`.
- **Subscribe:** `GET {broker_url}/{topic}/json?poll=1&since={cursor}`. `poll=1`
  returns cached messages and closes immediately (no held-open stream — keeps
  the single sync thread free to publish on cadence). First poll uses
  `since=30s`; subsequent polls pass the last message `id` as `since`. Body is
  newline-delimited JSON; we parse `{ id, event, message }` and forward only
  `event == "message"`. `open`/`keepalive`/`poll_request` are ignored.

## Keychain service name

`"aura-fleet-secret"` (account = `$USER`/`$USERNAME`). **Distinct** from Claude
Code's `"Claude Code-credentials"` — the OAuth entry is never read or written.

## Crypto parameters

- Topic = `"aura-" + base32_lower(HMAC-SHA256(secret, "ntfy-topic"))[..16]` (80-bit).
- AEAD key = `HKDF-SHA256(ikm=secret, salt=∅, info="aura-fleet-v1")` → 32 bytes.
- Wire = `base64( XNonce(24B, random per msg) || XChaCha20-Poly1305(key, nonce, json) )`.
- Replay: heartbeats with `ts` older than `2 × heartbeat_secs` are dropped.

## Things unverifiable without compiling / network

- `ureq 3.x` exact method spelling: `Agent::config_builder().timeout_global(...)
  .build().into()`, `.post(url).header(...).send(&str)`,
  `resp.into_parts(); body.into_reader()`. Cross-checked against
  `crates/aura/src/updater.rs` (same ureq version) — should match.
- `chacha20poly1305 0.10` exact re-exports (`Key`, `XNonce`, `aead::{Aead,
  KeyInit}`) — cross-checked against RustCrypto docs.
- `keyring 3` Linux feature set actually building on the target image.
- GPUI `rounded_full()`, `border_t_1()`, `pt_3()`, `flex_wrap()` — all confirmed
  present in `vendor/gpui` examples / styled.rs.
- Live ntfy round-trip (the `#[ignore]` test) — needs the network.

## Known design limitation (by intent)

`FleetSync` is owned by the `AuraView` (the modal). Aura fetches Claude usage
only when the modal opens (no persistent background quota poll exists), and that
fetch is the heartbeat source. So **Fleet syncs while the modal is open** and
pauses (thread joined on `AuraView` drop) when it's closed; the next open
resumes. Making sync 24/7 would require threading a process-level singleton
through `main.rs`/`runtime.rs` — a larger shared-file change deliberately
avoided to keep the shared-file edits additive and merge-clean with the sibling
features. Documented in `docs/fleet.md` and worth revisiting upstream.

## Verify checklist (run later, in dependency order)

```sh
# 1. Core library + all pure unit tests (pairing, crypto, fleet, transport mock).
cargo build -p aura-core
cargo test  -p aura-core

# 2. The app crate (UI integration).
cargo build -p aura

# 3. Lint / format if part of the normal gate.
cargo clippy --all-targets
cargo fmt --all -- --check

# 4. The single LIVE ntfy round-trip (network; never in CI):
cargo test -p aura-core -- --ignored net::transport::live_ntfy_round_trip

# 5. Manual two-machine pairing smoke test:
#    - Machine A: set [fleet].enabled=true, open Aura → Fleet → "Pair a machine"
#      → "Copy code".
#    - Machine B: same config, copy A's code to clipboard, Aura → Fleet →
#      "Join from clipboard". Confirm each shows the other's row with 5h/weekly
#      share bars and a live freshness dot. "Leave fleet" clears the keychain.
```
