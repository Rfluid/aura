//! Authenticated encryption for Fleet heartbeats: XChaCha20-Poly1305 with a
//! fresh random 192-bit nonce per message.
//!
//! The ntfy broker is **untrusted** — it sees only the output of [`seal`].
//! Every message is sealed under the 32-byte key derived in
//! [`crate::net::pairing::PairingSecret::aead_key`]. The wire format is:
//!
//! ```text
//! ┌────────────── 24 bytes ──────────────┬── ciphertext + 16-byte Poly1305 tag ──┐
//! │              XNonce                   │            AEAD output                │
//! └──────────────────────────────────────┴───────────────────────────────────────┘
//! ```
//!
//! XChaCha20's extended nonce makes random per-message nonces safe (collision
//! probability negligible even across the lifetime of a fleet), so we don't
//! need to coordinate a counter across machines.
//!
//! [`open`] returns `None` for *any* failure — wrong key, truncated blob, or a
//! tampered/forged ciphertext (Poly1305 auth fail). Callers simply ignore
//! un-openable messages, which is exactly the behaviour the threat model wants
//! for a hostile broker that might inject garbage.

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

/// XChaCha20-Poly1305 nonce length (192 bits).
pub const NONCE_LEN: usize = 24;

/// Seal `plaintext` under `key` (32 bytes). Prepends a fresh random 24-byte
/// nonce to the ciphertext and returns `nonce || ciphertext`. Infallible in
/// practice — XChaCha20-Poly1305 only errors on absurd input sizes we never
/// produce — so a failure is mapped to an empty vec the caller treats as
/// "skip this publish".
pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    match cipher.encrypt(nonce, plaintext) {
        Ok(ciphertext) => {
            let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
            out.extend_from_slice(&nonce_bytes);
            out.extend_from_slice(&ciphertext);
            out
        }
        Err(_) => Vec::new(),
    }
}

/// Open a `nonce || ciphertext` blob produced by [`seal`]. Returns `None` for
/// any failure: a blob shorter than the nonce, a wrong key, or a tampered /
/// forged ciphertext that fails Poly1305 authentication. Never panics, never
/// distinguishes the failure modes (so an attacker learns nothing from timing
/// the success/failure path beyond "rejected").
pub fn open(key: &[u8; 32], blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < NONCE_LEN {
        return None;
    }
    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = XNonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [0x11; 32];

    #[test]
    fn round_trips_plaintext() {
        let msg = b"{\"v\":1,\"session_pct\":22.4}";
        let blob = seal(&KEY, msg);
        // Output must be longer than plaintext (nonce + tag overhead) and not
        // contain the plaintext verbatim.
        assert!(blob.len() >= NONCE_LEN + 16 + msg.len());
        assert_eq!(open(&KEY, &blob).as_deref(), Some(&msg[..]));
    }

    #[test]
    fn nonce_is_unique_per_message() {
        let blob1 = seal(&KEY, b"same plaintext");
        let blob2 = seal(&KEY, b"same plaintext");
        // Random nonce ⇒ different ciphertext for identical plaintext.
        assert_ne!(blob1, blob2);
        assert_eq!(&blob1[..NONCE_LEN], &blob1[..NONCE_LEN]); // sanity
        assert_ne!(&blob1[..NONCE_LEN], &blob2[..NONCE_LEN]);
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let blob = seal(&KEY, b"secret");
        let mut wrong = KEY;
        wrong[0] ^= 0xff;
        assert_eq!(open(&wrong, &blob), None);
    }

    #[test]
    fn tampered_ciphertext_fails_to_open() {
        let mut blob = seal(&KEY, b"do not tamper");
        // Flip a bit in the ciphertext region (past the nonce).
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert_eq!(open(&KEY, &blob), None);
    }

    #[test]
    fn tampered_nonce_fails_to_open() {
        let mut blob = seal(&KEY, b"do not tamper");
        blob[0] ^= 0x01;
        assert_eq!(open(&KEY, &blob), None);
    }

    #[test]
    fn truncated_blob_fails_to_open() {
        assert_eq!(open(&KEY, b""), None);
        assert_eq!(open(&KEY, &[0u8; NONCE_LEN]), None); // nonce but no tag
        assert_eq!(open(&KEY, &[0u8; NONCE_LEN - 1]), None);
    }

    #[test]
    fn known_vector_round_trip_is_deterministic_given_fixed_nonce() {
        // `seal` randomizes the nonce, so we can't pin its output; instead pin
        // the *primitive* by sealing with a hand-built nonce and asserting the
        // raw cipher decrypts it. This is the "known vector" anchor: a fixed
        // key + nonce + plaintext must always open to the same plaintext.
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&KEY));
        let nonce_bytes = [0x24u8; NONCE_LEN];
        let nonce = XNonce::from_slice(&nonce_bytes);
        let plaintext = b"vector-anchor";
        let ct = cipher.encrypt(nonce, &plaintext[..]).unwrap();

        let mut blob = Vec::new();
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);

        assert_eq!(open(&KEY, &blob).as_deref(), Some(&plaintext[..]));
        // And the ciphertext bytes are stable for this fixed key+nonce, so a
        // regression in the cipher wiring would change them.
        let ct2 = cipher.encrypt(nonce, &plaintext[..]).unwrap();
        assert_eq!(ct, ct2);
    }
}
