//! Password and token handling (ADR-0007 §§2–4).
//!
//! Account passwords are hashed with the same Argon2id parameters as the
//! E2EE KEK ([`writan_crypto::derive_kek`]), so the workspace keeps one KDF.
//! Device tokens are high-entropy random strings stored as a single SHA-256;
//! they authenticate HTTP requests and are unrelated to the vault key.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore as _;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// Shortest accepted account password (bytes).
pub const PASSWORD_MIN_LEN: usize = 8;
/// Longest accepted account password (bytes; KDF cost guard).
pub const PASSWORD_MAX_LEN: usize = 1024;
/// Entropy of a device token.
pub const TOKEN_BYTES: usize = 32;
/// `last_seen_at` is written at most this often (ADR-0007 §3).
pub const LAST_SEEN_STALE_SECS: i64 = 300;

/// Failed-login lockout (ADR-0007 §2): after `after` consecutive failures the
/// account is locked for `base_secs`, doubling with every further failure up
/// to `cap_secs`. Plain numbers, so tests can shrink the policy.
#[derive(Debug, Clone, Copy)]
pub struct LockoutPolicy {
    /// Failures that trigger the first lock.
    pub after: i64,
    /// First lock length, in seconds.
    pub base_secs: i64,
    /// Longest lock, in seconds.
    pub cap_secs: i64,
}

impl Default for LockoutPolicy {
    fn default() -> Self {
        Self {
            after: 5,
            base_secs: 60,
            cap_secs: 900,
        }
    }
}

impl LockoutPolicy {
    /// The `locked_until` timestamp for `failures` consecutive failures at
    /// `now`; 0 while no lock applies.
    pub fn until(&self, failures: i64, now: i64) -> i64 {
        if failures < self.after {
            return 0;
        }
        let doublings = (failures - self.after).clamp(0, 16) as u32;
        let secs = self
            .base_secs
            .saturating_mul(1i64 << doublings)
            .min(self.cap_secs);
        now.saturating_add(secs)
    }
}

/// The login id: trimmed, lowercased, exactly one `@`, sane length.
pub fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_lowercase();
    let valid = (3..=254).contains(&email.len())
        && email.matches('@').count() == 1
        && !email.starts_with('@')
        && !email.ends_with('@')
        && !email
            .chars()
            .any(|character| character.is_whitespace() || character.is_control());
    if !valid {
        return Err(ApiError::BadRequest(
            "enter a valid email address".to_string(),
        ));
    }
    Ok(email)
}

/// The account-password length policy.
pub fn validate_password(password: &str) -> Result<(), ApiError> {
    let len = password.len();
    if len < PASSWORD_MIN_LEN {
        return Err(ApiError::BadRequest(format!(
            "password must be at least {PASSWORD_MIN_LEN} characters"
        )));
    }
    if len > PASSWORD_MAX_LEN {
        return Err(ApiError::BadRequest(format!(
            "password must be at most {PASSWORD_MAX_LEN} bytes"
        )));
    }
    Ok(())
}

/// Derive a fresh salt and the stored password hash.
pub fn hash_password(password: &str) -> Result<([u8; 16], [u8; 32]), ApiError> {
    let salt = writan_crypto::random_salt();
    let hash = writan_crypto::derive_kek(password, &salt)
        .map_err(|error| ApiError::Internal(format!("kdf: {error}")))?;
    Ok((salt, hash))
}

/// Constant-time comparison of the derived hash with the stored one.
pub fn verify_password(password: &str, salt: &[u8; 16], hash: &[u8; 32]) -> bool {
    match writan_crypto::derive_kek(password, salt) {
        Ok(derived) => ct_eq(&derived, hash),
        Err(_) => false,
    }
}

/// Pay one KDF derivation so a login for an unknown email costs a real one
/// (no account probing, ADR-0007 §2).
pub fn verify_dummy(password: &str) {
    const SALT: [u8; 16] = [0; 16];
    let _ = writan_crypto::derive_kek(password, &SALT);
}

/// A fresh device token: 32 random bytes, base64url without padding.
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The stored form of a device token.
pub fn hash_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Byte equality without a data-dependent early exit.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_normalization_accepts_and_refuses() {
        assert_eq!(
            normalize_email("  Ada@Example.COM ").expect("normalizes"),
            "ada@example.com"
        );
        let long = "a".repeat(300);
        for bad in [
            "",
            "no-at-sign",
            "a@b@c",
            "@example.com",
            "ada@",
            "a b@example.com",
            "a\tb@example.com",
            long.as_str(),
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn password_length_policy() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("longenough").is_ok());
        assert!(validate_password(&"x".repeat(PASSWORD_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn hash_verify_roundtrip_and_rejection() {
        let (salt, hash) = hash_password("correct horse battery").expect("hash");
        assert!(verify_password("correct horse battery", &salt, &hash));
        assert!(!verify_password("wrong password here", &salt, &hash));
        let (other_salt, _) = hash_password("correct horse battery").expect("hash");
        assert_ne!(salt, other_salt, "salts are random");
        verify_dummy("anything"); // must not panic, and must not be verifiable
    }

    #[test]
    fn tokens_are_random_and_hashed() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43, "32 bytes base64url is 43 characters");
        assert!(
            a.chars().all(|character| character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'),
            "{a} is base64url"
        );
        assert_eq!(hash_token(&a), hash_token(&a));
        assert_ne!(hash_token(&a), hash_token(&b));
    }

    #[test]
    fn lockout_grows_to_the_cap() {
        let policy = LockoutPolicy::default();
        assert_eq!(policy.until(4, 1_000), 0);
        assert_eq!(policy.until(5, 1_000), 1_060);
        assert_eq!(policy.until(6, 1_000), 1_120);
        assert_eq!(policy.until(7, 1_000), 1_240);
        assert_eq!(policy.until(100, 1_000), 1_900);
    }
}
