//! Pairing primitives for the Fleet feature: the 256-bit shared secret, its
//! human-typable code encoding, and the deterministic derivations every paired
//! machine performs from that secret (ntfy topic + symmetric AEAD key).
//!
//! Everything here is **pure** (no I/O, no clock, no randomness except the
//! single [`PairingSecret::generate`] entry point) so it is fully unit-testable
//! and gives identical output on every machine that shares the same secret.
//!
//! # Derivations
//!
//! Given the raw secret `s` (32 bytes):
//!
//! - **Topic** = `"aura-" + base32_lower( HMAC-SHA256(s, "ntfy-topic") )[..16]`.
//!   The topic name is itself a high-entropy capability — only holders of `s`
//!   can compute it, so it doubles as the channel's access control on the
//!   public broker. We expose 80 bits (16 base32 chars) of the HMAC.
//! - **AEAD key** = `HKDF-SHA256(salt=∅, ikm=s, info="aura-fleet-v1")` → 32 bytes.
//!   Consumed by [`crate::net::crypto`] as the XChaCha20-Poly1305 key.
//!
//! Using HMAC for the topic and HKDF for the key keeps the two outputs
//! domain-separated: the broker only ever sees the topic, never anything that
//! could leak the encryption key.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use zeroize::Zeroize;

type HmacSha256 = Hmac<Sha256>;

/// HKDF `info` string binding the derived key to this protocol version. Bump
/// the suffix to force a key rotation across a protocol change.
const HKDF_INFO: &[u8] = b"aura-fleet-v1";

/// HMAC message that derives the ntfy topic from the secret.
const TOPIC_INFO: &[u8] = b"ntfy-topic";

/// Topic prefix so every Fleet channel is recognizable (and namespaced away
/// from unrelated ntfy traffic) while the secret-derived suffix stays opaque.
const TOPIC_PREFIX: &str = "aura-";

/// Number of base32 characters of the topic HMAC we expose. 16 chars × 5 bits
/// = 80 bits of entropy — astronomically unguessable, comfortably within
/// ntfy's topic-name length limits.
const TOPIC_CHARS: usize = 16;

/// A 256-bit pairing secret shared by every machine in a fleet. Held only in
/// the OS keychain at rest (see [`crate::net::secret_store`]); this type is the
/// in-memory representation while deriving the topic / key. Zeroized on drop so
/// it doesn't linger in freed memory.
#[derive(Clone)]
pub struct PairingSecret([u8; 32]);

impl Drop for PairingSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for PairingSecret {
    /// Never print the secret bytes — even in debug output a leak into a log
    /// would defeat the whole threat model.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingSecret(<redacted>)")
    }
}

impl PairingSecret {
    /// Generate a fresh 256-bit secret from the OS CSPRNG. The only source of
    /// randomness in this module.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Construct from raw bytes (e.g. read back out of the keychain).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the raw secret bytes — for persisting to the keychain only.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the secret as a human-typable pairing code: the 32 secret bytes
    /// in lowercase RFC-4648 base32 (no padding), grouped into 4-char blocks
    /// separated by `-` for readability. 32 bytes → 52 base32 chars → 13
    /// groups. Round-trips losslessly through [`Self::from_code`].
    pub fn to_code(&self) -> String {
        let raw = base32_encode(&self.0);
        raw.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).expect("base32 alphabet is ASCII"))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Parse a pairing code produced by [`Self::to_code`]. Tolerant of the
    /// human factors of typing a code by hand: case-insensitive, ignores
    /// grouping dashes and surrounding whitespace. Returns [`PairingError`]
    /// when the decoded length is not exactly 32 bytes or a character is
    /// outside the base32 alphabet.
    pub fn from_code(code: &str) -> Result<Self, PairingError> {
        let cleaned: String = code
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        if cleaned.is_empty() {
            return Err(PairingError::Empty);
        }
        let bytes = base32_decode(&cleaned)?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| PairingError::WrongLength(bytes.len()))?;
        Ok(Self(arr))
    }

    /// Deterministic ntfy topic for this secret. Same on every paired machine,
    /// not derivable without the secret. See the module docs for the formula.
    pub fn topic(&self) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.0)
            .expect("HMAC accepts keys of any length");
        mac.update(TOPIC_INFO);
        let tag = mac.finalize().into_bytes();
        let encoded = base32_encode(&tag);
        let suffix: String = encoded.chars().take(TOPIC_CHARS).collect();
        format!("{TOPIC_PREFIX}{suffix}")
    }

    /// Derive the 32-byte XChaCha20-Poly1305 key for sealing heartbeats.
    /// HKDF-SHA256 with an empty salt and the versioned `info` string.
    pub fn aead_key(&self) -> [u8; 32] {
        let hk = Hkdf::<Sha256>::new(None, &self.0);
        let mut okm = [0u8; 32];
        hk.expand(HKDF_INFO, &mut okm)
            .expect("32 is a valid HKDF-SHA256 output length");
        okm
    }
}

/// Errors decoding a pairing code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingError {
    /// The code was empty after stripping whitespace / dashes.
    Empty,
    /// A character was not in the base32 alphabet.
    BadChar(char),
    /// Decoded to a byte length other than the expected 32.
    WrongLength(usize),
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingError::Empty => write!(f, "pairing code is empty"),
            PairingError::BadChar(c) => write!(f, "invalid character `{c}` in pairing code"),
            PairingError::WrongLength(n) => {
                write!(f, "pairing code decoded to {n} bytes, expected 32")
            }
        }
    }
}

impl std::error::Error for PairingError {}

// ── base32 (RFC 4648, lowercase, no padding) ─────────────────────────────────

/// Lowercase RFC-4648 base32 alphabet (no padding). Lowercase because it's
/// easier to type and ntfy topic names are case-sensitive — we always emit the
/// same case so the topic is stable.
const B32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Encode bytes as lowercase base32 without padding.
fn base32_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in input {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((buffer >> bits) & 0x1f) as usize;
            out.push(B32_ALPHABET[idx] as char);
        }
    }
    if bits > 0 {
        let idx = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(B32_ALPHABET[idx] as char);
    }
    out
}

/// Decode a lowercase (or mixed-case) base32 string without padding back to
/// bytes. Rejects any character outside the alphabet.
fn base32_decode(input: &str) -> Result<Vec<u8>, PairingError> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        let val = B32_ALPHABET
            .iter()
            .position(|&a| a == lower as u8)
            .ok_or(PairingError::BadChar(ch))? as u32;
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips_arbitrary_bytes() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(37)).collect();
            let encoded = base32_encode(&bytes);
            let decoded = base32_decode(&encoded).unwrap();
            assert_eq!(decoded, bytes, "round-trip failed at len {len}");
        }
    }

    #[test]
    fn code_round_trips_secret() {
        let secret = PairingSecret::generate();
        let code = secret.to_code();
        let back = PairingSecret::from_code(&code).unwrap();
        assert_eq!(secret.as_bytes(), back.as_bytes());
    }

    #[test]
    fn from_code_is_case_and_dash_insensitive() {
        let secret = PairingSecret::from_bytes([7u8; 32]);
        let code = secret.to_code();
        // Mangle the human-readable formatting: uppercase, extra spaces, no dashes.
        let mangled = code.to_uppercase().replace('-', " ");
        let back = PairingSecret::from_code(&mangled).unwrap();
        assert_eq!(secret.as_bytes(), back.as_bytes());
    }

    #[test]
    fn same_code_yields_same_topic_and_key() {
        // Two machines deriving from the same code must agree on both.
        let a = PairingSecret::from_bytes([0x42; 32]);
        let code = a.to_code();
        let b = PairingSecret::from_code(&code).unwrap();
        assert_eq!(a.topic(), b.topic());
        assert_eq!(a.aead_key(), b.aead_key());
    }

    #[test]
    fn topic_has_prefix_and_expected_length() {
        let topic = PairingSecret::from_bytes([1u8; 32]).topic();
        assert!(topic.starts_with("aura-"));
        assert_eq!(topic.len(), "aura-".len() + 16);
        // Suffix is within the base32 alphabet.
        assert!(topic["aura-".len()..]
            .chars()
            .all(|c| B32_ALPHABET.contains(&(c as u8))));
    }

    #[test]
    fn different_secrets_yield_different_topics_and_keys() {
        let a = PairingSecret::from_bytes([1u8; 32]);
        let b = PairingSecret::from_bytes([2u8; 32]);
        assert_ne!(a.topic(), b.topic());
        assert_ne!(a.aead_key(), b.aead_key());
    }

    #[test]
    fn topic_and_key_are_domain_separated() {
        // The topic HMAC and the AEAD key must not coincide for any secret —
        // the broker (which sees the topic) must learn nothing about the key.
        let s = PairingSecret::from_bytes([0x9e; 32]);
        let topic_tag = {
            let mut mac = HmacSha256::new_from_slice(s.as_bytes()).unwrap();
            mac.update(TOPIC_INFO);
            mac.finalize().into_bytes()
        };
        assert_ne!(&topic_tag[..32], &s.aead_key()[..]);
    }

    #[test]
    fn from_code_rejects_wrong_length() {
        // 8 base32 chars → 5 bytes, not 32.
        let err = PairingSecret::from_code("abcdefgh").unwrap_err();
        assert!(matches!(err, PairingError::WrongLength(5)));
    }

    #[test]
    fn from_code_rejects_bad_char() {
        // '1' and '0' and '8' '9' are not in the alphabet.
        let err = PairingSecret::from_code("aaaa-aaaa-1111").unwrap_err();
        assert!(matches!(err, PairingError::BadChar('1')));
    }

    #[test]
    fn from_code_rejects_empty() {
        assert_eq!(PairingSecret::from_code("   -- - ").unwrap_err(), PairingError::Empty);
    }
}
