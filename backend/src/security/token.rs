//! Opaque session tokens: 256 bits from the OS CSPRNG. The client holds the
//! token; the server stores only its SHA-256.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub struct NewToken {
    /// Goes in the cookie. Never stored or logged.
    pub token: String,
    /// Goes in the database.
    pub hash: Vec<u8>,
}

pub fn generate() -> NewToken {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator unavailable");
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash(&token);
    NewToken { token, hash }
}

pub fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Cheap shape check before touching the database.
pub fn looks_valid(token: &str) -> bool {
    token.len() == 43 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// SHA-256 of arbitrary bytes (used for idempotency request fingerprints).
pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_hash_matches() {
        let a = generate();
        let b = generate();
        assert_ne!(a.token, b.token);
        assert!(looks_valid(&a.token));
        assert_eq!(a.hash, hash(&a.token));
        assert_eq!(a.hash.len(), 32);
        assert!(!looks_valid("short"));
        assert!(!looks_valid(&"!".repeat(43)));
    }
}
