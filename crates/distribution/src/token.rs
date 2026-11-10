//! Token issuing/verification (P5-03 §6). Plaintext tokens never touch storage:
//! only `sha256(plain || pepper)` hashes are persisted; verification is constant-time.
use sha2::{Digest, Sha256};

/// Pepper from config/env; rotating it invalidates all tokens (documented rotation path).
pub fn hash_token(plain: &str, pepper: &str) -> String {
    let mut h = Sha256::new();
    h.update(plain.as_bytes());
    h.update(b"||");
    h.update(pepper.as_bytes());
    hex::encode(h.finalize())
}

/// Constant-time comparison over the hashed bytes.
pub fn verify(plain: &str, pepper: &str, hash: &str) -> bool {
    let computed = hash_token(plain, pepper);
    if computed.len() != hash.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in computed.bytes().zip(hash.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Issue a new token: 32 random bytes hex-encoded (192-bit entropy).
pub fn issue(pepper: &str) -> (String, String) {
    let mut bytes = [0u8; 32];
    let mut seed_accum: u64 = 0;
    // Use the OS entropy via `rand`'s thread rng (rand 0.8).
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let _ = &mut seed_accum;
    let plain = hex::encode(bytes);
    (plain.clone(), hash_token(&plain, pepper))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_peppered() {
        assert_eq!(hash_token("abc", "p1"), hash_token("abc", "p1"));
        assert_ne!(hash_token("abc", "p1"), hash_token("abc", "p2"));
        assert_ne!(hash_token("abc", "p1"), hash_token("abd", "p1"));
    }

    #[test]
    fn verify_constant_time_and_rejects() {
        let (plain, hash) = issue("pepper");
        assert!(verify(&plain, "pepper", &hash));
        assert!(!verify(&plain, "wrong", &hash));
        assert!(!verify("tampered", "pepper", &hash));
        assert!(!verify(&plain, "pepper", &hash[..hash.len() - 2]));
    }

    #[test]
    fn issue_unique() {
        let (a, ha) = issue("p");
        let (b, hb) = issue("p");
        assert_ne!(a, b);
        assert_ne!(ha, hb);
        assert_eq!(a.len(), 64);
    }
}
