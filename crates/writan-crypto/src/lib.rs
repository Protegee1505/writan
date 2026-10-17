//! End-to-end encryption for Writan sync.
//!
//! Two layers:
//! - An Argon2id-derived key-encryption key (KEK) from the user's password.
//!   Parameters follow the OWASP 2026 minimum (m = 19 MiB, t = 2, p = 1).
//! - Sealed blobs: 24-byte random nonce ‖ XChaCha20-Poly1305 ciphertext,
//!   with the note id/path bound in as associated data.

#![forbid(unsafe_code)]

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::Rng;

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

/// OWASP-recommended Argon2id cost for interactive use.
const ARGON2_M_COST_KIB: u32 = 19 * 1024;
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key derivation failed: {0}")]
    Argon2(#[from] argon2::Error),
    #[error("decryption failed")]
    Decrypt,
}

/// Derive a 32-byte key-encryption key from a password and salt.
pub fn derive_kek(password: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; KEY_LEN], CryptoError> {
    let params = Params::new(
        ARGON2_M_COST_KIB,
        ARGON2_T_COST,
        ARGON2_P_COST,
        Some(KEY_LEN),
    )?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2.hash_password_into(password.as_bytes(), salt, &mut key)?;
    Ok(key)
}

/// Generate a fresh random salt.
pub fn random_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill(&mut salt);
    salt
}

/// Generate a fresh 32-byte symmetric key (vault keys, recovery codes).
pub fn random_key() -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    rand::rng().fill(&mut key);
    key
}

/// Seal `plaintext` into `nonce ‖ ciphertext`, binding `aad` to the blob.
pub fn seal(key: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let key = Key::try_from(key.as_slice()).map_err(|_| CryptoError::Decrypt)?;
    let cipher = XChaCha20Poly1305::new(&key);
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce);
    let nonce = XNonce::try_from(nonce.as_slice()).map_err(|_| CryptoError::Decrypt)?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Open a blob produced by [`seal`], verifying the associated data.
pub fn open(key: &[u8; KEY_LEN], aad: &[u8], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < NONCE_LEN {
        return Err(CryptoError::Decrypt);
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let key = Key::try_from(key.as_slice()).map_err(|_| CryptoError::Decrypt)?;
    let cipher = XChaCha20Poly1305::new(&key);
    let nonce = XNonce::try_from(nonce).map_err(|_| CryptoError::Decrypt)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kek_is_deterministic_per_salt() {
        let salt = [7u8; SALT_LEN];
        let a = derive_kek("correct horse", &salt).unwrap();
        let b = derive_kek("correct horse", &salt).unwrap();
        assert_eq!(a, b);
        let c = derive_kek("correct horse", &[8u8; SALT_LEN]).unwrap();
        assert_ne!(a, c);
        let d = derive_kek("battery staple", &salt).unwrap();
        assert_ne!(a, d);
    }

    #[test]
    fn seal_open_roundtrip() {
        let key = [42u8; KEY_LEN];
        let blob = seal(&key, b"notes/abc.md", b"# hello").unwrap();
        let plain = open(&key, b"notes/abc.md", &blob).unwrap();
        assert_eq!(plain, b"# hello");
    }

    #[test]
    fn wrong_key_fails() {
        let blob = seal(&[1u8; KEY_LEN], b"aad", b"secret").unwrap();
        assert!(open(&[2u8; KEY_LEN], b"aad", &blob).is_err());
    }

    #[test]
    fn wrong_aad_fails() {
        let key = [9u8; KEY_LEN];
        let blob = seal(&key, b"note-a", b"secret").unwrap();
        assert!(open(&key, b"note-b", &blob).is_err());
    }

    #[test]
    fn tampered_blob_fails() {
        let key = [3u8; KEY_LEN];
        let mut blob = seal(&key, b"aad", b"secret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(open(&key, b"aad", &blob).is_err());
    }

    #[test]
    fn truncated_blob_fails() {
        let key = [4u8; KEY_LEN];
        let blob = seal(&key, b"aad", b"secret").unwrap();
        assert!(open(&key, b"aad", &blob[..NONCE_LEN]).is_err());
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let key = [5u8; KEY_LEN];
        let a = seal(&key, b"aad", b"same").unwrap();
        let b = seal(&key, b"aad", b"same").unwrap();
        assert_ne!(a, b, "two seals of the same plaintext must differ");
    }
}
