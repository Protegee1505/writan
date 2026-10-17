//! Keyring generations, recovery codes and unlock (ADR-0005 §7–10).
//!
//! The keyring holds the vault key sealed under both the password KEK and the
//! recovery-code KEK. Generations are append-only (`keys/<n>.json`, never
//! rewritten in place): a reader uses the newest generation its credential
//! opens, so a torn newest upload can only ever fall back, and a password
//! change or recovery regeneration rewrites only the newest generation.
//!
//! Recovery codes are 32 random bytes shown once as Crockford base32 grouped
//! in fours; they go through the exact same Argon2id path as the password.
//! Creating the first keyring requires the caller to pass the (already
//! exported) code — sync cannot be enabled without one.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::target::{SyncTarget, TargetError};
use writan_crypto::{
    CryptoError, KEY_LEN, SALT_LEN, derive_kek, open, random_key, random_salt, seal,
};

/// Format banner for keyring files (ADR-0005 §9).
pub const FORMAT_KEYRING: &str = "writan-keyring/1";
/// Argon2id parameters fixed by ADR-0005 §7 — stored in the file, enforced on
/// read (a generation with other parameters is treated as unreadable, so the
/// reader falls back to an older one).
pub const KDF_ALGO: &str = "argon2id";
pub const KDF_M_KIB: u32 = 19 * 1024;
pub const KDF_T: u32 = 2;
pub const KDF_P: u32 = 1;

/// Crockford base32 alphabet (no I, L, O, U).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("keyring target error: {0}")]
    Target(#[from] TargetError),
    #[error("keyring json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("unexpected keyring format {found}, want {want}")]
    Format { found: String, want: &'static str },
    #[error("unsupported keyring kdf parameters: {0}")]
    Kdf(String),
    #[error("keyring has no wrap for {0:?}")]
    NoWrap(KeyringPurpose),
    #[error("keyring field is not valid base64")]
    Base64,
    #[error("keyring salt has the wrong length")]
    BadSalt,
    #[error("wrapped vault key has unexpected length {found}")]
    WrongKeyLength { found: usize },
    #[error("recovery code is malformed")]
    BadRecoveryCode,
    #[error("keyring belongs to vault {found}, not {want}")]
    ForeignVault { found: Uuid, want: Uuid },
    #[error("no keyring generation opens with this credential")]
    Locked,
}

/// What a wrap is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyringPurpose {
    Password,
    Recovery,
}

impl KeyringPurpose {
    /// The ASCII label bound into the wrap's AAD.
    pub fn label(self) -> &'static str {
        match self {
            KeyringPurpose::Password => "password",
            KeyringPurpose::Recovery => "recovery",
        }
    }
}

/// The KDF parameters recorded in a keyring generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub algo: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    pub salt_password: String,
    pub salt_recovery: String,
}

/// One sealed vault-key copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringWrap {
    pub purpose: KeyringPurpose,
    pub blob: String,
}

/// A keyring generation, as stored at `keys/<generation>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringFile {
    pub format: String,
    pub vault_id: Uuid,
    pub generation: u32,
    pub kdf: KdfParams,
    pub wraps: Vec<KeyringWrap>,
}

/// The result of unlocking the keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlocked {
    pub vault_id: Uuid,
    pub generation: u32,
    pub vault_key: [u8; KEY_LEN],
    pub file: KeyringFile,
}

/// Path of one keyring generation inside the sync target.
pub fn keyring_path(generation: u32) -> String {
    format!("keys/{generation}.json")
}

/// Parse `keys/<n>.json` (n ≥ 1); anything else is not a keyring.
pub fn parse_keyring_path(path: &str) -> Option<u32> {
    let stem = path.strip_prefix("keys/")?.strip_suffix(".json")?;
    if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    stem.parse::<u32>()
        .ok()
        .filter(|generation| *generation >= 1)
}

/// AAD for a wrap: `"writan-keyring/1" ‖ 0x00 ‖ purpose ‖ generation-be-u32`.
pub fn keyring_aad(purpose: KeyringPurpose, generation: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(FORMAT_KEYRING.len() + 1 + 8 + 4);
    aad.extend_from_slice(FORMAT_KEYRING.as_bytes());
    aad.push(0);
    aad.extend_from_slice(purpose.label().as_bytes());
    aad.extend_from_slice(&generation.to_be_bytes());
    aad
}

/// Build one keyring generation: the vault key sealed under both KEKs.
pub fn create_keyring(
    vault_id: Uuid,
    vault_key: &[u8; KEY_LEN],
    password: &str,
    recovery_code: &str,
    generation: u32,
) -> Result<KeyringFile, KeyringError> {
    let salt_password = random_salt();
    let salt_recovery = random_salt();
    let kek_password = derive_kek(password, &salt_password)?;
    let kek_recovery = derive_kek(recovery_code, &salt_recovery)?;
    let wrap_password = seal(
        &kek_password,
        &keyring_aad(KeyringPurpose::Password, generation),
        vault_key,
    )?;
    let wrap_recovery = seal(
        &kek_recovery,
        &keyring_aad(KeyringPurpose::Recovery, generation),
        vault_key,
    )?;
    Ok(KeyringFile {
        format: FORMAT_KEYRING.to_string(),
        vault_id,
        generation,
        kdf: KdfParams {
            algo: KDF_ALGO.to_string(),
            m_kib: KDF_M_KIB,
            t: KDF_T,
            p: KDF_P,
            salt_password: encode_b64(&salt_password),
            salt_recovery: encode_b64(&salt_recovery),
        },
        wraps: vec![
            KeyringWrap {
                purpose: KeyringPurpose::Password,
                blob: encode_b64(&wrap_password),
            },
            KeyringWrap {
                purpose: KeyringPurpose::Recovery,
                blob: encode_b64(&wrap_recovery),
            },
        ],
    })
}

/// Parse and validate a keyring file (format banner + pinned KDF parameters).
pub fn parse_keyring(bytes: &[u8]) -> Result<KeyringFile, KeyringError> {
    let file: KeyringFile = serde_json::from_slice(bytes)?;
    if file.format != FORMAT_KEYRING {
        return Err(KeyringError::Format {
            found: file.format,
            want: FORMAT_KEYRING,
        });
    }
    let kdf = &file.kdf;
    if kdf.algo != KDF_ALGO || kdf.m_kib != KDF_M_KIB || kdf.t != KDF_T || kdf.p != KDF_P {
        return Err(KeyringError::Kdf(format!(
            "algo={} m_kib={} t={} p={}",
            kdf.algo, kdf.m_kib, kdf.t, kdf.p
        )));
    }
    Ok(file)
}

/// Recover the vault key from one generation with one credential.
pub fn unwrap_vault_key(
    file: &KeyringFile,
    purpose: KeyringPurpose,
    credential: &str,
) -> Result<[u8; KEY_LEN], KeyringError> {
    let salt_b64 = match purpose {
        KeyringPurpose::Password => &file.kdf.salt_password,
        KeyringPurpose::Recovery => &file.kdf.salt_recovery,
    };
    let salt: [u8; SALT_LEN] = decode_b64(salt_b64)?
        .try_into()
        .map_err(|_| KeyringError::BadSalt)?;
    let kek = derive_kek(credential, &salt)?;
    let wrap = file
        .wraps
        .iter()
        .find(|wrap| wrap.purpose == purpose)
        .ok_or(KeyringError::NoWrap(purpose))?;
    let blob = decode_b64(&wrap.blob)?;
    let plain = open(&kek, &keyring_aad(purpose, file.generation), &blob)?;
    let key: [u8; KEY_LEN] = plain
        .as_slice()
        .try_into()
        .map_err(|_| KeyringError::WrongKeyLength { found: plain.len() })?;
    Ok(key)
}

/// Collect every keyring generation present on a target.
pub fn keyring_generations(target: &impl SyncTarget) -> Result<Vec<(u32, Vec<u8>)>, KeyringError> {
    let mut generations = Vec::new();
    for path in target.list()? {
        if let Some(generation) = parse_keyring_path(&path) {
            generations.push((generation, target.read(&path)?));
        }
    }
    Ok(generations)
}

/// Unlock newest-first: the highest generation that parses (name and content
/// generation agree) and whose wrap this credential opens wins. Unparseable
/// generations are skipped (a torn newest file falls back); a generation for a
/// different `vault_id` is refused outright — targets are never merged.
pub fn unlock_keyring(
    entries: &[(u32, Vec<u8>)],
    purpose: KeyringPurpose,
    credential: &str,
    expected_vault_id: Option<Uuid>,
) -> Result<Unlocked, KeyringError> {
    let mut ordered: Vec<&(u32, Vec<u8>)> = entries.iter().collect();
    ordered.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
    for (name_generation, bytes) in ordered {
        let Ok(file) = parse_keyring(bytes) else {
            continue;
        };
        if file.generation != *name_generation {
            continue;
        }
        if let Some(want) = expected_vault_id {
            if file.vault_id != want {
                return Err(KeyringError::ForeignVault {
                    found: file.vault_id,
                    want,
                });
            }
        }
        if let Ok(vault_key) = unwrap_vault_key(&file, purpose, credential) {
            return Ok(Unlocked {
                vault_id: file.vault_id,
                generation: file.generation,
                vault_key,
                file,
            });
        }
    }
    Err(KeyringError::Locked)
}

/// Create generation 1, write it, and return the file plus the fresh vault key.
///
/// The recovery code is a parameter on purpose: sync cannot be enabled until a
/// real (decodable) code exists, so the caller must have generated it, shown it
/// and got it confirmed before calling this (ADR-0005 §8).
pub fn bootstrap_keyring(
    target: &impl SyncTarget,
    vault_id: Uuid,
    password: &str,
    recovery_code: &str,
) -> Result<(KeyringFile, [u8; KEY_LEN]), KeyringError> {
    decode_recovery_code(recovery_code)?;
    let vault_key = random_key();
    let file = create_keyring(vault_id, &vault_key, password, recovery_code, 1)?;
    target.write(&keyring_path(1), &serde_json::to_vec_pretty(&file)?)?;
    Ok((file, vault_key))
}

/// A fresh recovery code for the user to export (shown once).
pub fn generate_recovery_code() -> String {
    encode_recovery_code(&random_key())
}

/// Encode 32 bytes as 52 Crockford base32 chars, grouped in fours with `-`.
///
/// 256 bits do not fill 52 symbols: the last symbol carries the final data bit
/// and zero padding, exactly as [`decode_recovery_code`] expects.
pub fn encode_recovery_code(bytes: &[u8; KEY_LEN]) -> String {
    let mut chars = String::with_capacity(52);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in bytes {
        acc = (acc << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(CROCKFORD[((acc >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        chars.push(CROCKFORD[((acc << (5 - bits)) & 0x1f) as usize] as char);
    }
    let mut grouped = String::with_capacity(chars.len() + chars.len() / 4);
    for (index, ch) in chars.chars().enumerate() {
        if index > 0 && index % 4 == 0 {
            grouped.push('-');
        }
        grouped.push(ch);
    }
    grouped
}

/// Decode a recovery code: case-insensitive Crockford, `-`/space ignored,
/// `I`/`L` read as 1 and `O` as 0. Exactly 52 symbols and zero padding bits.
pub fn decode_recovery_code(code: &str) -> Result<[u8; KEY_LEN], KeyringError> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut bytes = Vec::with_capacity(KEY_LEN);
    let mut symbols = 0usize;
    for ch in code.chars() {
        if ch == '-' || ch == ' ' {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        let value: u32 = if upper.is_ascii_digit() {
            u32::from(upper) - u32::from('0')
        } else {
            match upper {
                'O' => 0,
                'I' | 'L' => 1,
                other => CROCKFORD
                    .iter()
                    .position(|symbol| *symbol as char == other)
                    .ok_or(KeyringError::BadRecoveryCode)? as u32,
            }
        };
        symbols += 1;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if symbols != 52 || bytes.len() != KEY_LEN || acc != 0 {
        return Err(KeyringError::BadRecoveryCode);
    }
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn encode_b64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

fn decode_b64(text: &str) -> Result<Vec<u8>, KeyringError> {
    STANDARD.decode(text).map_err(|_| KeyringError::Base64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAULT: Uuid = Uuid::from_u128(0x0a17);

    fn temp_target() -> (tempfile::TempDir, crate::target::FolderTarget) {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = crate::target::FolderTarget::open(dir.path()).expect("target");
        (dir, target)
    }

    #[test]
    fn zero_key_encodes_to_plain_crockford() {
        let code = encode_recovery_code(&[0u8; KEY_LEN]);
        assert_eq!(
            code,
            "0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000"
        );
        assert_eq!(decode_recovery_code(&code).expect("decode"), [0u8; KEY_LEN]);
    }

    #[test]
    fn recovery_codes_roundtrip_and_tolerate_formatting() {
        let bytes = [0xa5u8; KEY_LEN];
        let code = encode_recovery_code(&bytes);
        assert_eq!(code.len(), 52 + 12, "52 symbols in 13 groups");
        assert_eq!(decode_recovery_code(&code).expect("exact"), bytes);
        let loose = code.to_lowercase().replace('-', " ");
        assert_eq!(decode_recovery_code(&loose).expect("loose"), bytes);
        assert!(decode_recovery_code("0000-0000").is_err(), "too short");
        assert!(
            decode_recovery_code(&format!("{}U", "0".repeat(51))).is_err(),
            "U is not in the alphabet"
        );
        assert!(
            decode_recovery_code(&format!("{}!", "0".repeat(51))).is_err(),
            "punctuation is not tolerated"
        );
        assert!(
            decode_recovery_code(&format!("{}1", "0".repeat(51))).is_err(),
            "non-zero padding bits are rejected"
        );
        assert!(
            decode_recovery_code(&format!("{}G", "0".repeat(51))).is_ok(),
            "the final data bit sits in the top bit of the last symbol"
        );
    }

    #[test]
    fn keyring_roundtrip_unlocks_with_password_and_recovery() {
        let code = encode_recovery_code(&[3u8; KEY_LEN]);
        let file =
            create_keyring(VAULT, &[9u8; KEY_LEN], "correct horse", &code, 1).expect("create");
        assert_eq!(
            unwrap_vault_key(&file, KeyringPurpose::Password, "correct horse").expect("password"),
            [9u8; KEY_LEN]
        );
        assert_eq!(
            unwrap_vault_key(&file, KeyringPurpose::Recovery, &code).expect("recovery"),
            [9u8; KEY_LEN]
        );
    }

    #[test]
    fn wrong_credentials_do_not_unlock() {
        let code = encode_recovery_code(&[4u8; KEY_LEN]);
        let file = create_keyring(VAULT, &[9u8; KEY_LEN], "right", &code, 1).expect("create");
        assert!(unwrap_vault_key(&file, KeyringPurpose::Password, "wrong").is_err());
        assert!(unwrap_vault_key(&file, KeyringPurpose::Recovery, "wrong").is_err());
        let other = encode_recovery_code(&[5u8; KEY_LEN]);
        assert!(unwrap_vault_key(&file, KeyringPurpose::Recovery, &other).is_err());
    }

    #[test]
    fn unlock_uses_the_newest_generation_that_opens() {
        let old_code = encode_recovery_code(&[6u8; KEY_LEN]);
        let new_code = encode_recovery_code(&[7u8; KEY_LEN]);
        let old = create_keyring(VAULT, &[1u8; KEY_LEN], "old pass", &old_code, 1).expect("gen 1");
        let new = create_keyring(VAULT, &[2u8; KEY_LEN], "new pass", &new_code, 2).expect("gen 2");
        let entries = vec![
            (1u32, serde_json::to_vec(&old).expect("json")),
            (2u32, serde_json::to_vec(&new).expect("json")),
        ];
        let unlocked = unlock_keyring(&entries, KeyringPurpose::Password, "new pass", Some(VAULT))
            .expect("newest");
        assert_eq!(unlocked.generation, 2);
        assert_eq!(unlocked.vault_key, [2u8; KEY_LEN]);
        let unlocked = unlock_keyring(&entries, KeyringPurpose::Password, "old pass", Some(VAULT))
            .expect("fallback");
        assert_eq!(unlocked.generation, 1);
        assert_eq!(unlocked.vault_key, [1u8; KEY_LEN]);
        let torn = vec![
            (1u32, serde_json::to_vec(&old).expect("json")),
            (2u32, b"torn".to_vec()),
        ];
        let unlocked = unlock_keyring(&torn, KeyringPurpose::Password, "old pass", Some(VAULT))
            .expect("a torn newest falls back");
        assert_eq!(unlocked.generation, 1);
    }

    #[test]
    fn foreign_vault_id_is_refused() {
        let code = encode_recovery_code(&[8u8; KEY_LEN]);
        let file = create_keyring(VAULT, &[9u8; KEY_LEN], "pass", &code, 1).expect("create");
        let entries = vec![(1u32, serde_json::to_vec(&file).expect("json"))];
        let other = Uuid::from_u128(0xbeef);
        let err = unlock_keyring(&entries, KeyringPurpose::Password, "pass", Some(other))
            .expect_err("foreign");
        assert!(matches!(
            err,
            KeyringError::ForeignVault { found, want } if found == VAULT && want == other
        ));
    }

    #[test]
    fn bootstrap_validates_the_code_and_writes_generation_one() {
        let (_dir, target) = temp_target();
        assert!(bootstrap_keyring(&target, VAULT, "pass", "not a code").is_err());
        assert!(
            target.list().expect("list").is_empty(),
            "nothing may be written before the code validates"
        );
        let code = generate_recovery_code();
        let (file, key) = bootstrap_keyring(&target, VAULT, "pass", &code).expect("bootstrap");
        assert_eq!(file.generation, 1);
        assert_eq!(
            target.list().expect("list"),
            vec!["keys/1.json".to_string()]
        );
        let entries = keyring_generations(&target).expect("entries");
        let unlocked =
            unlock_keyring(&entries, KeyringPurpose::Recovery, &code, Some(VAULT)).expect("unlock");
        assert_eq!(unlocked.vault_key, key);
    }

    #[test]
    fn keyring_paths_parse_strictly() {
        assert_eq!(parse_keyring_path("keys/1.json"), Some(1));
        assert_eq!(parse_keyring_path("keys/42.json"), Some(42));
        assert_eq!(parse_keyring_path("keys/0.json"), None);
        assert_eq!(parse_keyring_path("keys/.json"), None);
        assert_eq!(parse_keyring_path("keys/x.json"), None);
        assert_eq!(parse_keyring_path("keys/1"), None);
        assert_eq!(parse_keyring_path("keys/1.json/extra"), None);
        assert_eq!(parse_keyring_path("items/1.json"), None);
    }

    #[test]
    fn keyring_file_shape_is_stable() {
        // Hand-built (no KDF cost): the on-disk shape is pinned by ADR-0005 §9.
        let file = KeyringFile {
            format: FORMAT_KEYRING.to_string(),
            vault_id: VAULT,
            generation: 1,
            kdf: KdfParams {
                algo: "argon2id".to_string(),
                m_kib: 19456,
                t: 2,
                p: 1,
                salt_password: "AA==".to_string(),
                salt_recovery: "BB==".to_string(),
            },
            wraps: vec![KeyringWrap {
                purpose: KeyringPurpose::Recovery,
                blob: "CC==".to_string(),
            }],
        };
        let value: serde_json::Value = serde_json::to_value(&file).expect("json");
        assert_eq!(value["format"], "writan-keyring/1");
        assert_eq!(value["vault_id"], VAULT.to_string());
        assert_eq!(value["kdf"]["algo"], "argon2id");
        assert_eq!(value["kdf"]["m_kib"], 19456);
        assert_eq!(value["wraps"][0]["purpose"], "recovery");
        let back = parse_keyring(&serde_json::to_vec(&file).expect("bytes")).expect("parse");
        assert_eq!(back, file);
    }

    #[test]
    fn parse_rejects_foreign_formats_and_kdf_params() {
        let base = serde_json::json!({
            "format": "writan-keyring/1",
            "vault_id": VAULT,
            "generation": 1,
            "kdf": {
                "algo": "argon2id", "m_kib": 19456, "t": 2, "p": 1,
                "salt_password": "AA==", "salt_recovery": "BB==",
            },
            "wraps": [],
        });
        assert!(parse_keyring(&serde_json::to_vec(&base).expect("bytes")).is_ok());
        let mut other_format = base.clone();
        other_format["format"] = "writan-keyring/2".into();
        assert!(matches!(
            parse_keyring(&serde_json::to_vec(&other_format).expect("bytes")),
            Err(KeyringError::Format { .. })
        ));
        let mut other_kdf = base.clone();
        other_kdf["kdf"]["m_kib"] = 8.into();
        assert!(matches!(
            parse_keyring(&serde_json::to_vec(&other_kdf).expect("bytes")),
            Err(KeyringError::Kdf(_))
        ));
    }
}
