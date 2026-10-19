//! Password and token handling (ADR-0007 §§2–4, ADR-0008 §§1–3).
//!
//! The account credential is a *verifier*: the client derives it from the one
//! password (`Argon2id(password, salt_auth)`) and sends it; the server stores
//! only `salt_auth` and `SHA-256(verifier)` and runs no KDF on a request
//! (ADR-0008 §1). Device tokens are high-entropy random strings stored as a
//! single SHA-256; they authenticate HTTP requests and are unrelated to the
//! vault key.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac as _};
use rand::RngCore as _;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// Shortest accepted password (bytes). Enforced by the clients and the admin
/// CLI — the server never receives the password itself (ADR-0008 §1).
pub const PASSWORD_MIN_LEN: usize = 8;
/// Longest accepted password (bytes; KDF cost guard).
pub const PASSWORD_MAX_LEN: usize = 1024;
/// Length of an account's auth salt (`salt_auth`).
pub const SALT_LEN: usize = 16;
/// Length of a verifier (the Argon2id output the client sends).
pub const VERIFIER_LEN: usize = 32;
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

/// Decode an account auth salt (16 bytes, standard base64).
pub fn parse_salt(raw: &str) -> Result<[u8; SALT_LEN], ApiError> {
    decode_fixed(raw.trim(), "salt")
}

/// Decode a verifier (32 bytes, standard base64).
pub fn parse_verifier(raw: &str) -> Result<[u8; VERIFIER_LEN], ApiError> {
    decode_fixed(raw.trim(), "verifier")
}

fn decode_fixed<const N: usize>(raw: &str, what: &str) -> Result<[u8; N], ApiError> {
    STANDARD
        .decode(raw)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| ApiError::BadRequest(format!("{what} must be {N} base64-encoded bytes")))
}

/// What the server stores for a verifier: its SHA-256 (see `verifier_matches`).
pub fn hash_verifier(verifier: &[u8; VERIFIER_LEN]) -> [u8; VERIFIER_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(verifier);
    hasher.finalize().into()
}

/// Constant-time comparison of a presented verifier with the stored hash.
pub fn verifier_matches(verifier: &[u8; VERIFIER_LEN], stored: &[u8; VERIFIER_LEN]) -> bool {
    ct_eq(&hash_verifier(verifier), stored)
}

/// The salt handed to an unknown email: `HMAC-SHA256(secret, email)` cut to
/// [`SALT_LEN`], so the answer is stable per email and tells an attacker
/// nothing about which accounts exist (ADR-0008 §3).
pub fn dummy_salt(secret: &[u8; 32], email: &str) -> [u8; SALT_LEN] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(email.as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&tag[..SALT_LEN]);
    salt
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
    fn salt_and_verifier_parsing_is_strict() {
        let salt = parse_salt(&STANDARD.encode([7u8; SALT_LEN])).expect("base64 salt");
        assert_eq!(salt, [7u8; SALT_LEN]);
        let verifier =
            parse_verifier(&STANDARD.encode([9u8; VERIFIER_LEN])).expect("base64 verifier");
        assert_eq!(verifier, [9u8; VERIFIER_LEN]);
        // Whitespace around the field is tolerated; wrong lengths and other
        // alphabets are not.
        assert!(parse_salt(&format!(" {} ", STANDARD.encode([7u8; SALT_LEN]))).is_ok());
        // The field is standard base64: unpadded (url-safe) alphabets are not it.
        assert!(parse_salt(&URL_SAFE_NO_PAD.encode([7u8; SALT_LEN])).is_err());
        for bad in [
            "",
            "not base64",
            &STANDARD.encode([7u8; 15]),
            &STANDARD.encode([7u8; 17]),
        ] {
            assert!(parse_salt(bad).is_err(), "{bad:?} must be refused");
        }
        for bad in ["", &STANDARD.encode([9u8; 31]), &STANDARD.encode([9u8; 33])] {
            assert!(parse_verifier(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn verifier_matching_accepts_the_hash_of_the_same_verifier() {
        let verifier = [9u8; VERIFIER_LEN];
        let stored = hash_verifier(&verifier);
        assert!(verifier_matches(&verifier, &stored));
        assert!(!verifier_matches(&[8u8; VERIFIER_LEN], &stored));
        assert_ne!(
            stored, verifier,
            "the stored form is a hash, not the verifier"
        );
    }

    #[test]
    fn dummy_salt_is_stable_per_email_and_secret() {
        let secret = [3u8; 32];
        let salt = dummy_salt(&secret, "ada@example.com");
        assert_eq!(salt, dummy_salt(&secret, "ada@example.com"), "stable");
        assert_ne!(salt, dummy_salt(&secret, "bob@example.com"));
        assert_ne!(
            salt,
            dummy_salt(&[4u8; 32], "ada@example.com"),
            "secret matters"
        );
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
