//! Argon2id password hashing (OWASP-recommended parameters, crate defaults:
//! m=19 MiB, t=2, p=1). Hashing runs on the blocking pool so it never stalls
//! the async runtime.

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

pub const MIN_LEN: usize = 12;
pub const MAX_LEN: usize = 128;

/// NIST SP 800-63B style policy: length matters, composition rules don't.
pub fn check_policy(password: &str, email: &str) -> Result<(), &'static str> {
    let chars = password.chars().count();
    if chars < MIN_LEN {
        return Err("password must be at least 12 characters");
    }
    if chars > MAX_LEN {
        return Err("password must be at most 128 characters");
    }
    let first = password.chars().next().unwrap_or_default();
    if password.chars().all(|c| c == first) {
        return Err("password must not be a single repeated character");
    }
    let local = email.split('@').next().unwrap_or_default().to_lowercase();
    if local.len() >= 4 && password.to_lowercase().contains(&local) {
        return Err("password must not contain your email address");
    }
    const COMMON: &[&str] = &["password1234", "123456789012", "qwertyuiopas", "letmein12345", "passwordpassword"];
    if COMMON.contains(&password.to_lowercase().as_str()) {
        return Err("password is too common");
    }
    Ok(())
}

pub async fn hash(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))
    })
    .await?
}

/// Constant-work verification. Pass `None` for an unknown user: a dummy hash
/// is verified instead so response time doesn't reveal whether the account exists.
pub async fn verify(password: String, stored: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        let (hash, real) = match stored {
            Some(h) => (h, true),
            None => (dummy_hash().to_string(), false),
        };
        let Ok(parsed) = PasswordHash::new(&hash) else { return false };
        let ok = Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok();
        ok && real
    })
    .await
    .unwrap_or(false)
}

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        Argon2::default().hash_password(b"dummy password for timing equalization").expect("hash").to_string()
    })
}

/// Compute the dummy hash at startup instead of on the first login.
pub fn warm_up() {
    let _ = dummy_hash();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy() {
        assert!(check_policy("short", "a@b.c").is_err());
        assert!(check_policy(&"a".repeat(20), "a@b.c").is_err());
        assert!(check_policy(&"ab".repeat(70), "a@b.c").is_err());
        assert!(check_policy("Password1234", "x@y.z").is_err());
        assert!(check_policy("alice-is-great-2026", "alice@example.com").is_err());
        assert!(check_policy("correct horse battery staple", "alice@example.com").is_ok());
    }

    #[tokio::test]
    async fn hash_and_verify() {
        let h = hash("correct horse battery staple".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse battery staple".into(), Some(h.clone())).await);
        assert!(!verify("wrong horse battery staple".into(), Some(h)).await);
        // Unknown user: never succeeds, even with the dummy's own password.
        assert!(!verify("dummy password for timing equalization".into(), None).await);
        assert!(!verify("x".into(), Some("not a hash".into())).await);
    }
}
