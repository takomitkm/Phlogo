//! Node payload encryption at rest (plan §7 / §16.8; user ruling A2, 2026-11-13).
//!
//! The raw vendor payload is a Clash-shaped JSON object that carries the node's
//! **credentials** (password / uuid / psk / private key). Storing it in plaintext
//! violates §16.8, so it is sealed with **XChaCha20-Poly1305** and the node id is
//! bound as AAD: a ciphertext cannot be moved to another node without failing to open.
//!
//! Key material comes from `PHLOGO_PAYLOAD_KEY` (32 bytes, base64 or 64-char hex).
//! When the variable is absent a **process-local ephemeral key** is generated, so
//! development and tests still run — but payloads written under it become unreadable
//! after a restart. That failure mode is deliberate: it is impossible to "forget" to
//! configure the key in production silently, because subscriptions stop resolving.
use std::sync::OnceLock;

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

/// AES-256-grade symmetric key length.
pub const KEY_LEN: usize = 32;
/// XChaCha20 nonce length (random per message).
pub const NONCE_LEN: usize = 24;
/// Version stamped next to each ciphertext so a future rotation can migrate rows.
pub const KEY_VERSION: i32 = 1;

/// A sealed payload as stored in `node_payloads`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_version: i32,
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("payload key must be 32 bytes (base64 or 64-char hex), got {0}")]
    BadKey(String),
    #[error("payload encryption failed")]
    Seal,
    #[error("payload decryption failed (wrong key, tampered ciphertext, or wrong node id)")]
    Open,
    #[error("bad nonce length: expected {expected}, got {got}")]
    BadNonce { expected: usize, got: usize },
}

#[derive(Clone)]
pub struct PayloadKey {
    key: [u8; KEY_LEN],
}

/// Never print key material: test helpers and logs may format a key by accident.
impl std::fmt::Debug for PayloadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PayloadKey(<redacted>)")
    }
}

impl PayloadKey {
    pub fn from_bytes(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }

    /// Accept a 64-char hex string, or base64/base64url with or without padding.
    pub fn from_encoded(encoded: &str) -> Result<Self, CryptoError> {
        let s = encoded.trim();
        if s.len() == KEY_LEN * 2 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut key = [0u8; KEY_LEN];
            for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
                let text = std::str::from_utf8(chunk).map_err(|_| CryptoError::BadKey(s.into()))?;
                key[i] = u8::from_str_radix(text, 16).map_err(|_| CryptoError::BadKey(s.into()))?;
            }
            return Ok(Self { key });
        }
        for engine in [
            base64::engine::general_purpose::STANDARD,
            base64::engine::general_purpose::STANDARD_NO_PAD,
            base64::engine::general_purpose::URL_SAFE,
            base64::engine::general_purpose::URL_SAFE_NO_PAD,
        ] {
            if let Ok(raw) = engine.decode(s) {
                if raw.len() == KEY_LEN {
                    let mut key = [0u8; KEY_LEN];
                    key.copy_from_slice(&raw);
                    return Ok(Self { key });
                }
            }
        }
        Err(CryptoError::BadKey(format!("length {}", s.len())))
    }

    /// `None` when the variable is unset or empty.
    pub fn from_env() -> Result<Option<Self>, CryptoError> {
        match std::env::var("PHLOGO_PAYLOAD_KEY") {
            Ok(v) if !v.trim().is_empty() => Self::from_encoded(&v).map(Some),
            _ => Ok(None),
        }
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.key))
    }

    /// Seal `plaintext` bound to `aad` (use [`aad_for`]).
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
        use rand::RngCore;
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Seal)?;
        Ok(Sealed {
            key_version: KEY_VERSION,
            nonce,
            ciphertext,
        })
    }

    /// Open a sealed payload; fails on wrong key, tampered bytes or wrong AAD.
    pub fn open(
        &self,
        aad: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if nonce.len() != NONCE_LEN {
            return Err(CryptoError::BadNonce {
                expected: NONCE_LEN,
                got: nonce.len(),
            });
        }
        self.cipher()
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Open)
    }
}

/// AAD for a node payload: the node's hyphenated uuid string.
pub fn aad_for(node_id: &str) -> Vec<u8> {
    node_id.as_bytes().to_vec()
}

static GLOBAL: OnceLock<PayloadKey> = OnceLock::new();

/// Initialize the process key from `PHLOGO_PAYLOAD_KEY`.
///
/// Returns `true` when a configured key was installed. `false` means an ephemeral key
/// is in use (payloads will not survive a restart).
pub fn init_from_env() -> Result<bool, CryptoError> {
    match PayloadKey::from_env()? {
        Some(k) => {
            let _ = GLOBAL.set(k);
            Ok(true)
        }
        None => {
            let _ = GLOBAL.set(ephemeral());
            Ok(false)
        }
    }
}

/// The process-wide key (configured, or the ephemeral fallback created on first use).
pub fn global() -> &'static PayloadKey {
    GLOBAL.get_or_init(ephemeral)
}

fn ephemeral() -> PayloadKey {
    use rand::RngCore;
    let mut key = [0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    PayloadKey::from_bytes(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn key() -> PayloadKey {
        PayloadKey::from_encoded(HEX).expect("hex key")
    }

    #[test]
    fn roundtrip_binds_the_node_id() {
        let k = key();
        let aad = aad_for("6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f");
        let sealed = k.seal(&aad, b"{\"password\":\"pw\"}").unwrap();
        assert_eq!(sealed.key_version, KEY_VERSION);
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        let out = k.open(&aad, &sealed.nonce, &sealed.ciphertext).unwrap();
        assert_eq!(out, b"{\"password\":\"pw\"}");
    }

    #[test]
    fn ciphertext_cannot_move_to_another_node() {
        let k = key();
        let a = aad_for("node-a");
        let sealed = k.seal(&a, b"secret").unwrap();
        let err = k
            .open(&aad_for("node-b"), &sealed.nonce, &sealed.ciphertext)
            .unwrap_err();
        assert!(matches!(err, CryptoError::Open));
    }

    #[test]
    fn tampering_and_wrong_key_fail() {
        let k = key();
        let aad = aad_for("n");
        let mut sealed = k.seal(&aad, b"secret").unwrap();
        sealed.ciphertext[0] ^= 0x01;
        assert!(matches!(
            k.open(&aad, &sealed.nonce, &sealed.ciphertext).unwrap_err(),
            CryptoError::Open
        ));

        let other = PayloadKey::from_bytes([9u8; KEY_LEN]);
        let fresh = k.seal(&aad, b"secret").unwrap();
        assert!(matches!(
            other
                .open(&aad, &fresh.nonce, &fresh.ciphertext)
                .unwrap_err(),
            CryptoError::Open
        ));
    }

    #[test]
    fn nonce_length_is_checked() {
        let err = key().open(b"aad", &[0u8; 3], b"x").unwrap_err();
        assert!(matches!(err, CryptoError::BadNonce { .. }));
    }

    #[test]
    fn encoded_key_accepts_hex_and_base64() {
        assert!(PayloadKey::from_encoded(HEX).is_ok());
        let raw: [u8; KEY_LEN] = [7; KEY_LEN];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let from_b64 = PayloadKey::from_encoded(&b64).unwrap();
        let from_raw = PayloadKey::from_bytes(raw);
        let aad = aad_for("n");
        let sealed = from_raw.seal(&aad, b"x").unwrap();
        assert!(from_b64
            .open(&aad, &sealed.nonce, &sealed.ciphertext)
            .is_ok());
        assert!(matches!(
            PayloadKey::from_encoded("too-short").unwrap_err(),
            CryptoError::BadKey(_)
        ));
    }

    #[test]
    fn global_key_is_stable() {
        let a = global();
        let b = global();
        let aad = aad_for("n");
        let sealed = a.seal(&aad, b"x").unwrap();
        assert!(b.open(&aad, &sealed.nonce, &sealed.ciphertext).is_ok());
    }
}
