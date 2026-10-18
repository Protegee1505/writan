//! Shell-agnostic sync session glue (ADR-0005 §§6–10).
//!
//! The Tauri app (`app/src-tauri`) and the GTK shell (`app-linux`) both talk
//! to sync through this module: [`SyncConfig`] is the shape of the `sync`
//! object in the app-data `config.json`, [`SyncSession`] is an unlocked
//! session (vault key + engine bookkeeping), and the helper functions cover
//! the lifecycle — connect (create or join a keyring), unlock after a
//! restart, resume from the OS keychain, one engine pass, and the status
//! snapshot the UI renders. Keeping them here means the on-disk shapes and
//! the user-facing error wording cannot drift between shells.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::keychain;
use crate::keys::KeyringError;
use crate::{
    FolderTarget, KeyringPurpose, S3Config, S3HttpTransport, S3Target, SyncReport, SyncState,
    SyncTarget, TargetError, bootstrap_keyring, generate_recovery_code, keyring_generations,
    unlock_keyring,
};

/// File name of the local sync state inside the app-data directory. It is
/// shared by both shells on purpose: a device has one identity whatever
/// shell opened the vault.
pub const SYNC_STATE_FILE: &str = "sync-state.json";

/// Which kind of target notes sync through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncKind {
    #[default]
    Folder,
    S3,
}

impl SyncKind {
    /// The lowercase name used in `config.json` and the status DTO.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::S3 => "s3",
        }
    }
}

/// S3 target details kept in `config.json` (ADR-0006). The secret key never
/// lives here — only in the OS keychain ([`keychain::recall_s3_secret`]).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct S3ConfigFile {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
}

/// How and where this device syncs; the vault key and bookkeeping live in the
/// OS keychain and `sync-state.json` (ADR-0005 §§6–10).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub kind: SyncKind,
    /// The shared folder notes sync through (folder targets).
    pub target: Option<PathBuf>,
    /// The bucket details (S3 targets); the secret key is in the keychain.
    #[serde(default)]
    pub s3: Option<S3ConfigFile>,
    /// The vault this sync was set up for; opening another vault keeps sync
    /// locked rather than mixing notes between vaults.
    pub vault_path: Option<PathBuf>,
    #[serde(default)]
    pub enabled: bool,
}

/// An unlocked sync configuration: the vault key plus the engine's local state.
pub struct SyncSession {
    pub vault_id: Uuid,
    pub device: crate::DeviceId,
    pub vault_key: zeroize::Zeroizing<[u8; 32]>,
    pub state: SyncState,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReport>,
}

/// Everything a shell needs to know about sync to paint its UI.
#[derive(Debug, Clone, Default)]
pub struct SyncStatus {
    pub enabled: bool,
    pub unlocked: bool,
    pub kind: SyncKind,
    /// Display string: the folder path, or `s3://<bucket>`.
    pub target: Option<String>,
    pub vault_id: Option<String>,
    pub device_id: Option<String>,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReport>,
}

/// A refused or failed session operation; `Display` carries the exact text
/// the UI shows.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("sync is not configured yet")]
    NotConfigured,
    #[error("choose a sync folder first")]
    ChooseFolderFirst,
    #[error("set a sync password first")]
    PasswordFirst,
    #[error("set the S3 endpoint first")]
    EndpointFirst,
    #[error("set the bucket first")]
    BucketFirst,
    #[error("set the access key and secret first")]
    AccessKeyFirst,
    #[error("the S3 secret key is not on this device — connect sync again")]
    MissingS3Secret,
    #[error("failed to create the keyring: {0}")]
    CreateKeyring(KeyringError),
    #[error("could not unlock the existing keyring: {0}")]
    JoinKeyring(KeyringError),
    #[error("could not reach the bucket: {0}")]
    ReachBucket(KeyringError),
    #[error("could not unlock: {0}")]
    Unlock(KeyringError),
    #[error("{0}")]
    Keyring(#[from] KeyringError),
    #[error("the sync folder has no keyring yet")]
    NoKeyring,
    #[error("enter the password or recovery code")]
    NoCredential,
    #[error("this device already syncs a different vault")]
    DifferentVault,
    #[error("this sync belongs to a different vault folder")]
    ForeignVaultFolder,
    #[error("sync is disabled")]
    Disabled,
    #[error("sync is locked; unlock it first")]
    Locked,
    #[error("open a vault before setting up sync")]
    NoVaultOpen,
    #[error("{0}")]
    Target(#[from] TargetError),
    #[error("{0}")]
    Engine(#[from] crate::EngineError),
}

/// The configured target for one engine pass: the shared folder, or the
/// S3-compatible bucket (ADR-0006).
pub enum AnyTarget {
    Folder(FolderTarget),
    S3(S3Target<S3HttpTransport>),
}

impl SyncTarget for AnyTarget {
    fn list(&self) -> Result<Vec<String>, TargetError> {
        match self {
            Self::Folder(target) => target.list(),
            Self::S3(target) => target.list(),
        }
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError> {
        match self {
            Self::Folder(target) => target.read(path),
            Self::S3(target) => target.read(path),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError> {
        match self {
            Self::Folder(target) => target.write(path, bytes),
            Self::S3(target) => target.write(path, bytes),
        }
    }
}

/// Open the configured target; S3 targets pull the secret key from the OS
/// keychain, so a config restored without one fails with [`SessionError::MissingS3Secret`].
pub fn open_target(config: &SyncConfig) -> Result<AnyTarget, SessionError> {
    match config.kind {
        SyncKind::Folder => {
            let path = config.target.as_ref().ok_or(SessionError::NotConfigured)?;
            let target = FolderTarget::open(path)?;
            Ok(AnyTarget::Folder(target))
        }
        SyncKind::S3 => {
            let s3 = config.s3.as_ref().ok_or(SessionError::NotConfigured)?;
            let secret = keychain::recall_s3_secret(&s3.endpoint, &s3.bucket)
                .ok_or(SessionError::MissingS3Secret)?;
            let cfg = S3Config {
                endpoint: s3.endpoint.clone(),
                region: s3.region.clone(),
                bucket: s3.bucket.clone(),
                access_key: s3.access_key.clone(),
                secret_key: secret,
            };
            Ok(AnyTarget::S3(S3Target::new(S3HttpTransport::new(cfg))))
        }
    }
}

/// A refused pass when the current vault is not the one this sync was set up
/// for; `None` bound vaults (older configs) pass.
pub fn check_vault(config: &SyncConfig, current: Option<&Path>) -> Result<(), SessionError> {
    if let Some(bound) = config.vault_path.as_deref()
        && current != Some(bound)
    {
        return Err(SessionError::ForeignVaultFolder);
    }
    Ok(())
}

/// What a successful connect produced: the config and session to install,
/// plus the recovery code shown exactly once when a keyring was created
/// (`None` when joining an existing one).
pub struct Connected {
    pub config: SyncConfig,
    pub session: SyncSession,
    pub recovery_code: Option<String>,
}

/// Connect this vault to a shared folder: create a fresh keyring there (the
/// recovery code is returned exactly once) or unlock an existing one with the
/// password. Remembers the vault key in the OS keychain.
///
/// `vault_path` is the vault to bind the sync to; `None` (no vault open) is
/// refused after the keyring work, so a missing vault never leaves a
/// half-made keyring behind without a clear error.
pub fn connect_folder(
    target: &Path,
    password: &str,
    vault_path: Option<&Path>,
    state_path: &Path,
) -> Result<Connected, SessionError> {
    if password.is_empty() {
        return Err(SessionError::PasswordFirst);
    }
    let folder = FolderTarget::open(target)?;
    let generations = keyring_generations(&folder)?;
    let (vault_id, vault_key, recovery_code) = create_or_join(&generations, &folder, password)?;
    keychain::remember_vault_key(vault_id, &vault_key);
    let vault_path = vault_path.ok_or(SessionError::NoVaultOpen)?.to_path_buf();
    let config = SyncConfig {
        kind: SyncKind::Folder,
        target: Some(target.to_path_buf()),
        s3: None,
        vault_path: Some(vault_path),
        enabled: true,
    };
    let session = bind_session(state_path, vault_id, vault_key)?;
    Ok(Connected {
        config,
        session,
        recovery_code,
    })
}

/// The raw S3 connection details as typed by the user; trimmed and validated
/// here.
#[derive(Debug, Clone, Default)]
pub struct S3Connect {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

/// Connect this vault to an S3-compatible bucket (ADR-0006). Same keyring
/// flow as the shared folder; the secret key goes to the OS keychain and the
/// rest of the connection details to `config.json`.
pub fn connect_s3(
    details: &S3Connect,
    password: &str,
    vault_path: Option<&Path>,
    state_path: &Path,
) -> Result<Connected, SessionError> {
    let endpoint = details.endpoint.trim().to_string();
    let bucket = details.bucket.trim().to_string();
    let region = match details.region.trim() {
        "" => "us-east-1".to_string(),
        region => region.to_string(),
    };
    let access_key = details.access_key.trim().to_string();
    let secret_key = details.secret_key.trim().to_string();
    if endpoint.is_empty() {
        return Err(SessionError::EndpointFirst);
    }
    if bucket.is_empty() {
        return Err(SessionError::BucketFirst);
    }
    if access_key.is_empty() || secret_key.is_empty() {
        return Err(SessionError::AccessKeyFirst);
    }
    if password.is_empty() {
        return Err(SessionError::PasswordFirst);
    }
    let cfg = S3Config {
        endpoint: endpoint.clone(),
        region: region.clone(),
        bucket: bucket.clone(),
        access_key: access_key.clone(),
        secret_key: secret_key.clone(),
    };
    let target = S3Target::new(S3HttpTransport::new(cfg));
    let generations = keyring_generations(&target).map_err(SessionError::ReachBucket)?;
    let (vault_id, vault_key, recovery_code) = create_or_join(&generations, &target, password)?;
    keychain::remember_vault_key(vault_id, &vault_key);
    keychain::remember_s3_secret(&endpoint, &bucket, &secret_key);
    let vault_path = vault_path.ok_or(SessionError::NoVaultOpen)?.to_path_buf();
    let config = SyncConfig {
        kind: SyncKind::S3,
        target: None,
        s3: Some(S3ConfigFile {
            endpoint,
            region,
            bucket,
            access_key,
        }),
        vault_path: Some(vault_path),
        enabled: true,
    };
    let session = bind_session(state_path, vault_id, vault_key)?;
    Ok(Connected {
        config,
        session,
        recovery_code,
    })
}

/// Unlock an existing keyring with the password or the recovery code; returns
/// the config with `enabled` set and the session to install.
pub fn unlock(
    mut config: SyncConfig,
    state_path: &Path,
    purpose: KeyringPurpose,
    credential: &str,
) -> Result<(SyncConfig, SyncSession), SessionError> {
    if credential.is_empty() {
        return Err(SessionError::NoCredential);
    }
    let target = open_target(&config)?;
    let generations = keyring_generations(&target)?;
    if generations.is_empty() {
        return Err(SessionError::NoKeyring);
    }
    let expected = SyncState::load(state_path)
        .ok()
        .and_then(|stored| stored.vault_id);
    let unlocked = unlock_keyring(&generations, purpose, credential, expected)
        .map_err(SessionError::Unlock)?;
    keychain::remember_vault_key(unlocked.vault_id, &unlocked.vault_key);
    let session = bind_session(state_path, unlocked.vault_id, unlocked.vault_key)?;
    config.enabled = true;
    Ok((config, session))
}

/// Restore an enabled session from the OS keychain, if possible. Failing just
/// leaves sync locked; the UI asks for the password.
pub fn resume(state_path: &Path, config: &SyncConfig) -> Option<SyncSession> {
    if !config.enabled {
        return None;
    }
    let state = SyncState::load(state_path).ok()?;
    let vault_id = state.vault_id?;
    let device = state.device?;
    let key = keychain::recall_vault_key(vault_id)?;
    Some(SyncSession {
        vault_id,
        device,
        vault_key: zeroize::Zeroizing::new(key),
        state,
        last_sync: None,
        last_report: None,
    })
}

/// One engine pass over `vault`, persisting the state and stamping the
/// session's last-sync time and report.
pub fn run_pass(
    state_path: &Path,
    session: &mut SyncSession,
    vault: &impl crate::VaultIo,
    target: &impl SyncTarget,
) -> Result<SyncReport, SessionError> {
    let report = crate::sync(
        &session.vault_key,
        session.device,
        session.vault_id,
        &mut session.state,
        vault,
        target,
    )?;
    session.state.save(state_path)?;
    session.last_sync = Some(writan_core::now_rfc3339());
    session.last_report = Some(report);
    Ok(report)
}

/// The status snapshot the UI renders: from the live session when unlocked,
/// otherwise from the stored state.
pub fn status(config: &SyncConfig, session: Option<&SyncSession>, state_path: &Path) -> SyncStatus {
    let target = match config.kind {
        SyncKind::Folder => config
            .target
            .as_ref()
            .map(|path| path.display().to_string()),
        SyncKind::S3 => config.s3.as_ref().map(|s3| format!("s3://{}", s3.bucket)),
    };
    let mut status = SyncStatus {
        enabled: config.enabled,
        unlocked: session.is_some(),
        kind: config.kind,
        target,
        ..SyncStatus::default()
    };
    if let Some(session) = session {
        status.vault_id = Some(session.vault_id.to_string());
        status.device_id = Some(session.device.to_string());
        status.last_sync = session.last_sync.clone();
        status.last_report = session.last_report;
    } else if let Ok(stored) = SyncState::load(state_path) {
        status.vault_id = stored.vault_id.map(|id| id.to_string());
        status.device_id = stored.device.map(|id| id.to_string());
    }
    status
}

/// Create the keyring when the target has none (returning the recovery code
/// that must be shown once), otherwise unlock it with the password.
fn create_or_join(
    generations: &[(u32, Vec<u8>)],
    target: &impl SyncTarget,
    password: &str,
) -> Result<(Uuid, [u8; 32], Option<String>), SessionError> {
    if generations.is_empty() {
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) = bootstrap_keyring(target, vault_id, password, &code)
            .map_err(SessionError::CreateKeyring)?;
        Ok((vault_id, key, Some(code)))
    } else {
        let unlocked = unlock_keyring(generations, KeyringPurpose::Password, password, None)
            .map_err(SessionError::JoinKeyring)?;
        Ok((unlocked.vault_id, unlocked.vault_key, None))
    }
}

/// Load (or start) the local state for `vault_id`, refusing a device that
/// already syncs another vault, and wrap it in a fresh session.
fn bind_session(
    state_path: &Path,
    vault_id: Uuid,
    vault_key: [u8; 32],
) -> Result<SyncSession, SessionError> {
    let mut state = SyncState::load(state_path).unwrap_or_default();
    if let Some(existing) = state.vault_id
        && existing != vault_id
    {
        return Err(SessionError::DifferentVault);
    }
    state.vault_id = Some(vault_id);
    let device = state.device.unwrap_or_else(Uuid::new_v4);
    state.device = Some(device);
    Ok(SyncSession {
        vault_id,
        device,
        vault_key: zeroize::Zeroizing::new(vault_key),
        state,
        last_sync: None,
        last_report: None,
    })
}

impl SyncReport {
    /// The one-line summary the shells show after a pass, e.g.
    /// `3 published · 1 merged · 12 unchanged`.
    pub fn summary(&self) -> String {
        let mut line = format!(
            "{} published · {} merged · {} adopted · {} conflict copies · {} deleted · {} unchanged",
            self.published,
            self.merged,
            self.adopted,
            self.conflict_copies,
            self.deleted,
            self.unchanged
        );
        if self.unreadable > 0 {
            line.push_str(&format!(" · {} unreadable", self.unreadable));
        }
        line
    }
}
