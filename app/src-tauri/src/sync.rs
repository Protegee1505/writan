//! Sync session glue: the vault key in the OS keychain plus the small
//! bookkeeping the app keeps around [`writan_sync`] (docs/adr/0005).
//!
//! The keychain is an optional convenience: a stored key lets the app unlock
//! sync on start without the password. Losing it only means typing the
//! password (or the recovery code) again — the vault key itself is wrapped in
//! the keyring file on the sync target (ADR-0005 §10).

use uuid::Uuid;
use writan_sync::{SyncReport, SyncState};

/// Keychain service name for remembered vault keys; the account is the vault
/// id.
const KEYCHAIN_SERVICE: &str = "com.writan.desktop.sync";
/// Keychain service for the S3 secret key (ADR-0006); the account is
/// `endpoint/bucket`.
const S3_SECRET_SERVICE: &str = "com.writan.desktop.s3";

/// An unlocked sync configuration: the vault key plus the engine's local state.
pub struct SyncSession {
    pub vault_id: Uuid,
    pub device: writan_sync::DeviceId,
    pub vault_key: zeroize::Zeroizing<[u8; 32]>,
    pub state: SyncState,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReport>,
}

fn entry(service: &str, account: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(service, account).ok()
}

/// Best-effort: remember the vault key so a restart can unlock without the
/// password. Failure is fine — the password still works.
pub fn remember_vault_key(vault_id: Uuid, key: &[u8; 32]) {
    use base64::Engine as _;
    if let Some(entry) = entry(KEYCHAIN_SERVICE, &vault_id.to_string()) {
        let text = base64::engine::general_purpose::STANDARD.encode(key);
        let _ = entry.set_password(&text);
    }
}

/// Drop the remembered key (server-side lock; the password still unlocks).
pub fn forget_vault_key(vault_id: Uuid) {
    if let Some(entry) = entry(KEYCHAIN_SERVICE, &vault_id.to_string()) {
        let _ = entry.delete_credential();
    }
}

/// The vault key remembered for `vault_id`, if any.
pub fn recall_vault_key(vault_id: Uuid) -> Option<[u8; 32]> {
    use base64::Engine as _;
    let text = entry(KEYCHAIN_SERVICE, &vault_id.to_string())?
        .get_password()
        .ok()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    bytes.try_into().ok()
}

/// Remember the secret key of an S3 target (ADR-0006). Best-effort, like the
/// vault key above: losing it only means typing the secret again.
pub fn remember_s3_secret(endpoint: &str, bucket: &str, secret: &str) {
    let account = s3_account(endpoint, bucket);
    if let Some(entry) = entry(S3_SECRET_SERVICE, &account) {
        let _ = entry.set_password(secret);
    }
}

/// Drop the stored S3 secret (turning sync off; the config keeps the rest).
pub fn forget_s3_secret(endpoint: &str, bucket: &str) {
    let account = s3_account(endpoint, bucket);
    if let Some(entry) = entry(S3_SECRET_SERVICE, &account) {
        let _ = entry.delete_credential();
    }
}

/// The stored S3 secret for this endpoint/bucket, if any.
pub fn recall_s3_secret(endpoint: &str, bucket: &str) -> Option<String> {
    let account = s3_account(endpoint, bucket);
    entry(S3_SECRET_SERVICE, &account)?.get_password().ok()
}

fn s3_account(endpoint: &str, bucket: &str) -> String {
    format!("{endpoint}/{bucket}")
}
