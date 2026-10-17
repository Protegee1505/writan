//! Sync session glue: the vault key in the OS keychain plus the small
//! bookkeeping the app keeps around [`writan_sync`] (docs/adr/0005).
//!
//! The keychain is an optional convenience: a stored key lets the app unlock
//! sync on start without the password. Losing it only means typing the
//! password (or the recovery code) again — the vault key itself is wrapped in
//! the keyring file on the sync target (ADR-0005 §10).

use uuid::Uuid;
use writan_sync::{SyncReport, SyncState};

/// Keychain service name; the account is the vault id.
const KEYCHAIN_SERVICE: &str = "com.writan.desktop.sync";

/// An unlocked sync configuration: the vault key plus the engine's local state.
pub struct SyncSession {
    pub vault_id: Uuid,
    pub device: writan_sync::DeviceId,
    pub vault_key: zeroize::Zeroizing<[u8; 32]>,
    pub state: SyncState,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReport>,
}

fn keychain_entry(vault_id: Uuid) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &vault_id.to_string()).map_err(|e| e.to_string())
}

/// Best-effort: remember the vault key so a restart can unlock without the
/// password. Failure is fine — the password still works.
pub fn remember_vault_key(vault_id: Uuid, key: &[u8; 32]) {
    use base64::Engine as _;
    if let Ok(entry) = keychain_entry(vault_id) {
        let text = base64::engine::general_purpose::STANDARD.encode(key);
        let _ = entry.set_password(&text);
    }
}

/// Drop the remembered key (server-side lock; the password still unlocks).
pub fn forget_vault_key(vault_id: Uuid) {
    if let Ok(entry) = keychain_entry(vault_id) {
        let _ = entry.delete_credential();
    }
}

/// The vault key remembered for `vault_id`, if any.
pub fn recall_vault_key(vault_id: Uuid) -> Option<[u8; 32]> {
    use base64::Engine as _;
    let entry = keychain_entry(vault_id).ok()?;
    let text = entry.get_password().ok()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    bytes.try_into().ok()
}
