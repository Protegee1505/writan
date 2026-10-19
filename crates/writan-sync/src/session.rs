//! Shell-agnostic sync session glue (ADR-0005 §§6–10, ADR-0008).
//!
//! The Tauri app (`app/src-tauri`) and the GTK shell (`app-linux`) both talk
//! to sync through this module: [`SyncConfig`] is the shape of the `sync`
//! object in the app-data `config.json`, [`SyncSession`] is an unlocked
//! session (vault key + engine bookkeeping), and the helper functions cover
//! the lifecycle — connect (create or join a keyring), unlock after a
//! restart, resume from the OS keychain, one engine pass, and the status
//! snapshot the UI renders. Keeping them here means the on-disk shapes and
//! the user-facing error wording cannot drift between shells.
//!
//! Since ADR-0008 there is one password per vault: it both signs the account
//! in (as an Argon2id verifier the server never sees the plaintext of) and
//! unlocks the keyring. Joining a keyring created under an older password
//! takes that old credential as a *fallback* ([`KeyringCredential`]) and
//! appends a generation the new password opens; [`change_password`] and
//! [`new_recovery_code`] do the same deliberately from the settings.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::keychain;
use crate::keys::KeyringError;
use crate::server::{
    ServerConnection, ServerError, ServerHttpTransport, ServerLogin, ServerTarget,
};
use crate::{
    FolderTarget, KeyringPurpose, S3Config, S3HttpTransport, S3Target, SyncReport, SyncState,
    SyncTarget, TargetError, bootstrap_keyring, generate_recovery_code, keyring_generations,
    push_generation, rewrap_generation, unlock_keyring,
};

/// File name of the local sync state inside the app-data directory. It is
/// shared by both shells on purpose: a device has one identity whatever
/// shell opened the vault.
pub const SYNC_STATE_FILE: &str = "sync-state.json";

/// The shortest the one password may be (ADR-0008 §1). The rule lives in the
/// clients: the server only ever sees the verifier.
pub const PASSWORD_MIN_LEN: usize = 8;

/// The longest the one password may be (ADR-0008 §1): Argon2 cost grows with
/// the input, so an enormous password is refused rather than hashed.
pub const PASSWORD_MAX_LEN: usize = 1024;

/// Which kind of target notes sync through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncKind {
    #[default]
    Folder,
    S3,
    Server,
}

impl SyncKind {
    /// The lowercase name used in `config.json` and the status DTO.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::S3 => "s3",
            Self::Server => "server",
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

/// Noteserver details kept in `config.json` (ADR-0007 §11). The device token
/// never lives here — only in the OS keychain
/// ([`keychain::recall_server_token`]).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ServerConfigFile {
    pub url: String,
    pub email: String,
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
    /// The address and account of the noteserver (server targets); the
    /// device token is in the keychain.
    #[serde(default)]
    pub server: Option<ServerConfigFile>,
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
    /// Display string: the folder path, `s3://<bucket>`, or the server
    /// address.
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
    #[error("set a password first")]
    PasswordFirst,
    #[error("the password must be at least {PASSWORD_MIN_LEN} characters")]
    PasswordTooShort,
    #[error("the password must be at most {PASSWORD_MAX_LEN} characters")]
    PasswordTooLong,
    #[error("enter the current password first")]
    CurrentPasswordFirst,
    #[error("the current password is not right")]
    CurrentPasswordWrong,
    #[error(
        "this vault was created with a different password — enter the old sync password or the recovery code to bring it forward"
    )]
    KeyringDifferentPassword,
    #[error("set the S3 endpoint first")]
    EndpointFirst,
    #[error("set the bucket first")]
    BucketFirst,
    #[error("set the access key and secret first")]
    AccessKeyFirst,
    #[error("the S3 secret key is not on this device — connect sync again")]
    MissingS3Secret,
    #[error("enter the server address first (it looks like https://notes.example.com)")]
    ServerUrlFirst,
    #[error("enter the account email first")]
    EmailFirst,
    #[error("enter the pairing code first")]
    PairingCodeFirst,
    #[error("this device has no server token — sign in again")]
    MissingServerToken,
    #[error("could not reach the server: {0}")]
    ReachServer(ServerError),
    #[error("could not read the keyring from the server: {0}")]
    ServerKeyring(KeyringError),
    #[error("{0}")]
    Server(#[from] ServerError),
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

/// The configured target for one engine pass: the shared folder, the
/// S3-compatible bucket (ADR-0006), or a writan-server (ADR-0007).
pub enum AnyTarget {
    Folder(FolderTarget),
    S3(S3Target<S3HttpTransport>),
    Server(ServerTarget<ServerHttpTransport>),
}

impl SyncTarget for AnyTarget {
    fn list(&self) -> Result<Vec<String>, TargetError> {
        match self {
            Self::Folder(target) => target.list(),
            Self::S3(target) => target.list(),
            Self::Server(target) => target.list(),
        }
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError> {
        match self {
            Self::Folder(target) => target.read(path),
            Self::S3(target) => target.read(path),
            Self::Server(target) => target.read(path),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError> {
        match self {
            Self::Folder(target) => target.write(path, bytes),
            Self::S3(target) => target.write(path, bytes),
            Self::Server(target) => target.write(path, bytes),
        }
    }
}

/// Open the configured target. `vault_id` is the vault the caller's session
/// is bound to: folder and S3 targets ignore it, a noteserver target checks
/// it against the account (ADR-0007 §5). A noteserver target reads its device
/// token from the OS keychain, so a config restored without one fails with
/// [`SessionError::MissingServerToken`]; S3 targets pull the secret key from
/// there too and fail with [`SessionError::MissingS3Secret`].
pub fn open_target(config: &SyncConfig, vault_id: Option<Uuid>) -> Result<AnyTarget, SessionError> {
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
        SyncKind::Server => {
            let server = config.server.as_ref().ok_or(SessionError::NotConfigured)?;
            let token = keychain::recall_server_token(&server.url, &server.email)
                .ok_or(SessionError::MissingServerToken)?;
            let connection = ServerConnection {
                base_url: server.url.clone(),
                token,
                vault_id,
            };
            Ok(AnyTarget::Server(ServerTarget::new(
                ServerHttpTransport::new(connection),
            )))
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

/// A credential that can open a keyring: the one password (ADR-0008 §1) or a
/// recovery code. Joining a keyring that predates the one password accepts
/// either as the *fallback* credential (ADR-0008 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringCredential {
    Password(String),
    Recovery(String),
}

impl KeyringCredential {
    /// Which keyring wrap this credential opens.
    pub fn purpose(&self) -> KeyringPurpose {
        match self {
            Self::Password(_) => KeyringPurpose::Password,
            Self::Recovery(_) => KeyringPurpose::Recovery,
        }
    }

    /// The secret itself.
    pub fn secret(&self) -> &str {
        match self {
            Self::Password(secret) | Self::Recovery(secret) => secret,
        }
    }
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
    check_password(password)?;
    let folder = FolderTarget::open(target)?;
    let generations = keyring_generations(&folder)?;
    let (vault_id, vault_key, recovery_code) =
        create_or_join(&generations, &folder, password, None, None)?;
    keychain::remember_vault_key(vault_id, &vault_key);
    let vault_path = vault_path.ok_or(SessionError::NoVaultOpen)?.to_path_buf();
    let config = SyncConfig {
        kind: SyncKind::Folder,
        target: Some(target.to_path_buf()),
        s3: None,
        server: None,
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
    check_password(password)?;
    let cfg = S3Config {
        endpoint: endpoint.clone(),
        region: region.clone(),
        bucket: bucket.clone(),
        access_key: access_key.clone(),
        secret_key: secret_key.clone(),
    };
    let target = S3Target::new(S3HttpTransport::new(cfg));
    let generations = keyring_generations(&target).map_err(SessionError::ReachBucket)?;
    let (vault_id, vault_key, recovery_code) =
        create_or_join(&generations, &target, password, None, None)?;
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
        server: None,
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

/// The raw noteserver connection details as typed by the user; trimmed and
/// validated here (ADR-0007 §§2–3, 5). The password is the *one* password
/// (ADR-0008 §1): it signs the account in (as a verifier) and unlocks the
/// keyring.
#[derive(Debug, Clone, Default)]
pub struct ServerConnect {
    /// The server root, e.g. `https://notes.example.com`.
    pub url: String,
    pub email: String,
    pub device_name: String,
    /// Register the account when the server does not know the email yet.
    pub create_account: bool,
}

/// Connect this vault to a writan-server (ADR-0007, ADR-0008). Signs in — or
/// registers, with `create_account` — this device, makes sure the account has
/// a vault (creating a fresh one on the first device), then runs the keyring
/// flow: create fresh (returning the recovery code once) or unlock with the
/// one password. A keyring created under an older password is adopted when
/// `fallback` (the old sync password or the recovery code) opens it
/// (ADR-0008 §5). The device token goes to the OS keychain, never to
/// `config.json`.
pub fn connect_server(
    details: &ServerConnect,
    password: &str,
    fallback: Option<&KeyringCredential>,
    vault_path: Option<&Path>,
    state_path: &Path,
) -> Result<Connected, SessionError> {
    let url = details.url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(SessionError::ServerUrlFirst);
    }
    let email = details.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(SessionError::EmailFirst);
    }
    check_password(password)?;
    // The server sees this device's sync id, so signing in here keeps the
    // identity the engine uses (ADR-0007 §3); the state file remembers it
    // before the first call so a later bind_session reuses the same uuid.
    let mut state = SyncState::load(state_path).unwrap_or_default();
    let device = state.device.unwrap_or_else(Uuid::new_v4);
    state.device = Some(device);
    state.save(state_path)?;
    let login = ServerLogin {
        url: url.clone(),
        email: email.clone(),
        password: password.to_string(),
        device_id: device,
        device_name: details.device_name.trim().to_string(),
    };
    let token = if details.create_account {
        crate::server::register(&login)?
    } else {
        crate::server::login(&login)?
    };
    keychain::remember_server_token(&url, &email, &token);
    let vault_id = match crate::server::discover_vault(&url, &token)? {
        Some(existing) => existing,
        None => {
            let fresh = Uuid::new_v4();
            crate::server::ensure_vault(&url, &token, fresh)?;
            fresh
        }
    };
    let target = ServerTarget::new(ServerHttpTransport::new(ServerConnection {
        base_url: url.clone(),
        token,
        vault_id: Some(vault_id),
    }));
    let generations = keyring_generations(&target).map_err(SessionError::ServerKeyring)?;
    let (keyring_vault, vault_key, recovery_code) =
        create_or_join(&generations, &target, password, fallback, Some(vault_id))?;
    if keyring_vault != vault_id {
        return Err(SessionError::DifferentVault);
    }
    keychain::remember_vault_key(vault_id, &vault_key);
    let vault_path = vault_path.ok_or(SessionError::NoVaultOpen)?.to_path_buf();
    let config = SyncConfig {
        kind: SyncKind::Server,
        target: None,
        s3: None,
        server: Some(ServerConfigFile { url, email }),
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

/// The raw pairing details a scanning device has: whatever the QR carried
/// plus the name this device will show as (ADR-0007 §4).
#[derive(Debug, Clone, Default)]
pub struct ServerPairing {
    /// Servers root from the QR, e.g. `https://notes.example.com`.
    pub url: String,
    /// The single-use pairing code.
    pub code: String,
    /// The account the code belongs to; the QR carries it.
    pub email: String,
    pub device_name: String,
}

/// Join an account with a pairing code instead of its password: redeem the
/// code for this device's token, discover the account's vault, then run the
/// same keyring exchange as [`connect_server`]. The code grants blob access
/// only — the vault key never travels, so the one password (or a recovery
/// code) is still needed to open the keyring (ADR-0007 §4, ADR-0008 §§1, 5).
pub fn join_with_code(
    details: &ServerPairing,
    password: &str,
    fallback: Option<&KeyringCredential>,
    vault_path: Option<&Path>,
    state_path: &Path,
) -> Result<Connected, SessionError> {
    let url = details.url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(SessionError::ServerUrlFirst);
    }
    let code = details.code.trim();
    if code.is_empty() {
        return Err(SessionError::PairingCodeFirst);
    }
    let email = details.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(SessionError::EmailFirst);
    }
    check_password(password)?;
    let device_name = if details.device_name.trim().is_empty() {
        "paired device"
    } else {
        details.device_name.trim()
    };
    // The same device id rule as connect_server: the state file remembers it
    // before the first call, so the engine and the server agree (ADR-0007 §3).
    let mut state = SyncState::load(state_path).unwrap_or_default();
    let device = state.device.unwrap_or_else(Uuid::new_v4);
    state.device = Some(device);
    state.save(state_path)?;
    let token = crate::server::redeem_pairing(&url, code, device, device_name)?;
    keychain::remember_server_token(&url, &email, &token);
    let vault_id = match crate::server::discover_vault(&url, &token)? {
        Some(existing) => existing,
        None => {
            let fresh = Uuid::new_v4();
            crate::server::ensure_vault(&url, &token, fresh)?;
            fresh
        }
    };
    let target = ServerTarget::new(ServerHttpTransport::new(ServerConnection {
        base_url: url.clone(),
        token,
        vault_id: Some(vault_id),
    }));
    let generations = keyring_generations(&target).map_err(SessionError::ServerKeyring)?;
    let (keyring_vault, vault_key, recovery_code) =
        create_or_join(&generations, &target, password, fallback, Some(vault_id))?;
    if keyring_vault != vault_id {
        return Err(SessionError::DifferentVault);
    }
    keychain::remember_vault_key(vault_id, &vault_key);
    let vault_path = vault_path.ok_or(SessionError::NoVaultOpen)?.to_path_buf();
    let config = SyncConfig {
        kind: SyncKind::Server,
        target: None,
        s3: None,
        server: Some(ServerConfigFile { url, email }),
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
    let target = open_target(&config, None)?;
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
        SyncKind::Server => config.server.as_ref().map(|server| server.url.clone()),
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
/// that must be shown once), otherwise unlock it with the one password. When
/// the keyring predates the one password, `fallback` (the old sync password or
/// the recovery code) may open it once: a generation for the new password is
/// then appended — the vault key is unchanged — so the vault adopts it
/// (ADR-0008 §5).
fn create_or_join(
    generations: &[(u32, Vec<u8>)],
    target: &impl SyncTarget,
    password: &str,
    fallback: Option<&KeyringCredential>,
    wanted: Option<Uuid>,
) -> Result<(Uuid, [u8; 32], Option<String>), SessionError> {
    if generations.is_empty() {
        // A writan-server vault id is assigned by the account (ADR-0007 §5);
        // folder and bucket targets let the fresh keyring pick its own.
        let vault_id = wanted.unwrap_or_else(Uuid::new_v4);
        let code = generate_recovery_code();
        let (_file, key) = bootstrap_keyring(target, vault_id, password, &code)
            .map_err(SessionError::CreateKeyring)?;
        Ok((vault_id, key, Some(code)))
    } else {
        match unlock_keyring(generations, KeyringPurpose::Password, password, None) {
            Ok(unlocked) => Ok((unlocked.vault_id, unlocked.vault_key, None)),
            Err(KeyringError::Locked) => {
                let Some(fallback) = fallback else {
                    return Err(SessionError::KeyringDifferentPassword);
                };
                let adopted =
                    unlock_keyring(generations, fallback.purpose(), fallback.secret(), None)
                        .map_err(SessionError::JoinKeyring)?;
                let generation = next_generation(generations);
                let file = rewrap_generation(
                    adopted.vault_id,
                    &adopted.vault_key,
                    generation,
                    KeyringPurpose::Password,
                    password,
                )
                .map_err(SessionError::CreateKeyring)?;
                push_generation(target, &file).map_err(SessionError::CreateKeyring)?;
                Ok((adopted.vault_id, adopted.vault_key, None))
            }
            Err(other) => Err(SessionError::JoinKeyring(other)),
        }
    }
}

/// The generation number after the newest one on the target.
fn next_generation(generations: &[(u32, Vec<u8>)]) -> u32 {
    generations
        .iter()
        .map(|(generation, _)| *generation)
        .max()
        .unwrap_or(0)
        + 1
}

/// Change the one password (ADR-0008 §6): keyring first — the current password
/// must open it, which yields the vault key — then a generation only the new
/// password opens, then the account verifier on a server target. An
/// interrupted change leaves an extra generation, never a vault the new
/// password cannot open; older generations keep opening with the old password
/// (the recorded v1 limitation).
pub fn change_password(
    config: &SyncConfig,
    session: &SyncSession,
    old_password: &str,
    new_password: &str,
) -> Result<(), SessionError> {
    if old_password.is_empty() {
        return Err(SessionError::CurrentPasswordFirst);
    }
    check_password(new_password)?;
    let target = open_target(config, Some(session.vault_id))?;
    let generations = keyring_generations(&target)?;
    if generations.is_empty() {
        return Err(SessionError::NoKeyring);
    }
    let unlocked = unlock_keyring(
        &generations,
        KeyringPurpose::Password,
        old_password,
        Some(session.vault_id),
    )
    .map_err(|error| match error {
        KeyringError::Locked => SessionError::CurrentPasswordWrong,
        other => SessionError::Unlock(other),
    })?;
    let generation = next_generation(&generations);
    let file = rewrap_generation(
        unlocked.vault_id,
        &unlocked.vault_key,
        generation,
        KeyringPurpose::Password,
        new_password,
    )
    .map_err(SessionError::CreateKeyring)?;
    push_generation(&target, &file).map_err(SessionError::CreateKeyring)?;
    if matches!(config.kind, SyncKind::Server) {
        let server = config.server.as_ref().ok_or(SessionError::NotConfigured)?;
        let token = keychain::recall_server_token(&server.url, &server.email)
            .ok_or(SessionError::MissingServerToken)?;
        crate::server::change_verifier(&server.url, &token, old_password, new_password)?;
    }
    Ok(())
}

/// Mint a fresh recovery code and append a generation only it opens
/// (ADR-0008 §7). The code is returned for the one-time display; the old code
/// keeps opening older generations. Requires an unlocked session: the vault
/// key is already in memory, so no password is asked for.
pub fn new_recovery_code(
    config: &SyncConfig,
    session: &SyncSession,
) -> Result<String, SessionError> {
    let target = open_target(config, Some(session.vault_id))?;
    let generations = keyring_generations(&target)?;
    if generations.is_empty() {
        return Err(SessionError::NoKeyring);
    }
    let code = generate_recovery_code();
    let generation = next_generation(&generations);
    let file = rewrap_generation(
        session.vault_id,
        &session.vault_key,
        generation,
        KeyringPurpose::Recovery,
        &code,
    )
    .map_err(SessionError::CreateKeyring)?;
    push_generation(&target, &file).map_err(SessionError::CreateKeyring)?;
    Ok(code)
}

/// The client-side password policy (ADR-0008 §1): at least
/// [`PASSWORD_MIN_LEN`] and at most [`PASSWORD_MAX_LEN`] bytes. The server
/// cannot enforce it — it only ever sees the verifier.
fn check_password(password: &str) -> Result<(), SessionError> {
    if password.is_empty() {
        return Err(SessionError::PasswordFirst);
    }
    if password.len() < PASSWORD_MIN_LEN {
        return Err(SessionError::PasswordTooShort);
    }
    if password.len() > PASSWORD_MAX_LEN {
        return Err(SessionError::PasswordTooLong);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn server_config() -> SyncConfig {
        SyncConfig {
            kind: SyncKind::Server,
            target: None,
            s3: None,
            server: Some(ServerConfigFile {
                url: "https://notes.example.com".to_string(),
                email: "ada@example.com".to_string(),
            }),
            vault_path: None,
            enabled: true,
        }
    }

    fn missing_state() -> PathBuf {
        PathBuf::from("/nonexistent/writan-test/sync-state.json")
    }

    fn folder_config(dir: &Path) -> SyncConfig {
        SyncConfig {
            kind: SyncKind::Folder,
            target: Some(dir.to_path_buf()),
            s3: None,
            server: None,
            vault_path: None,
            enabled: true,
        }
    }

    #[test]
    fn server_config_roundtrips_and_older_configs_load() {
        let config = server_config();
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("\"kind\":\"server\""), "{json}");
        let back: SyncConfig = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back.kind, SyncKind::Server));
        assert_eq!(
            back.server.expect("server details").url,
            "https://notes.example.com"
        );

        // A config written before server sync existed still loads.
        let older: SyncConfig =
            serde_json::from_str(r#"{"kind":"s3","enabled":true}"#).expect("older config");
        assert!(older.server.is_none());
        assert!(matches!(older.kind, SyncKind::S3));
    }

    #[test]
    fn server_status_shows_the_address() {
        let status = status(&server_config(), None, &missing_state());
        assert!(matches!(status.kind, SyncKind::Server));
        assert_eq!(status.target.as_deref(), Some("https://notes.example.com"));
        assert!(status.enabled);
        assert!(!status.unlocked);
    }

    #[test]
    fn server_connect_checks_its_inputs_before_any_call() {
        let base = ServerConnect {
            url: "https://notes.example.com".to_string(),
            email: "ada@example.com".to_string(),
            device_name: "laptop".to_string(),
            create_account: true,
        };
        let no_scheme = ServerConnect {
            url: "notes.example.com".to_string(),
            ..base.clone()
        };
        assert!(matches!(
            connect_server(&no_scheme, "one-password", None, None, &missing_state()),
            Err(SessionError::ServerUrlFirst)
        ));
        let bad_email = ServerConnect {
            email: "not-an-email".to_string(),
            ..base.clone()
        };
        assert!(matches!(
            connect_server(&bad_email, "one-password", None, None, &missing_state()),
            Err(SessionError::EmailFirst)
        ));
        assert!(matches!(
            connect_server(&base, "", None, None, &missing_state()),
            Err(SessionError::PasswordFirst)
        ));
        assert!(matches!(
            connect_server(&base, "short", None, None, &missing_state()),
            Err(SessionError::PasswordTooShort)
        ));
    }

    #[test]
    fn a_fallback_password_adopts_the_new_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("target");
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) =
            bootstrap_keyring(&target, vault_id, "old password", &code).expect("bootstrap");
        let generations = keyring_generations(&target).expect("generations");

        // The new password alone does not open the existing keyring.
        let locked = create_or_join(&generations, &target, "new password", None, Some(vault_id));
        assert!(matches!(
            locked,
            Err(SessionError::KeyringDifferentPassword)
        ));

        // The old password as the fallback does — and the vault adopts the new
        // one, so the next sign-in only needs it.
        let adopted = create_or_join(
            &generations,
            &target,
            "new password",
            Some(&KeyringCredential::Password("old password".to_string())),
            Some(vault_id),
        )
        .expect("adopt");
        assert_eq!(adopted.0, vault_id);
        assert_eq!(adopted.1, key);
        assert!(adopted.2.is_none(), "adoption is not a creation");
        let generations = keyring_generations(&target).expect("generations");
        let unlocked = unlock_keyring(
            &generations,
            KeyringPurpose::Password,
            "new password",
            Some(vault_id),
        )
        .expect("the new password opens the newest generation");
        assert_eq!(unlocked.generation, 2);
        assert_eq!(unlocked.vault_key, key);
    }

    #[test]
    fn a_fallback_recovery_code_adopts_the_new_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("target");
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) =
            bootstrap_keyring(&target, vault_id, "old password", &code).expect("bootstrap");
        let generations = keyring_generations(&target).expect("generations");

        let adopted = create_or_join(
            &generations,
            &target,
            "new password",
            Some(&KeyringCredential::Recovery(code.clone())),
            Some(vault_id),
        )
        .expect("adopt");
        assert_eq!(adopted.0, vault_id);
        assert_eq!(adopted.1, key);
        let generations = keyring_generations(&target).expect("generations");
        let unlocked = unlock_keyring(
            &generations,
            KeyringPurpose::Password,
            "new password",
            Some(vault_id),
        )
        .expect("the new password opens the newest generation");
        assert_eq!(unlocked.generation, 2);
    }

    #[test]
    fn change_password_appends_a_generation_for_the_new_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("target");
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) =
            bootstrap_keyring(&target, vault_id, "old password", &code).expect("bootstrap");
        let config = folder_config(dir.path());
        let session =
            bind_session(&dir.path().join("app-data/state.json"), vault_id, key).expect("session");

        // A wrong current password changes nothing.
        let wrong = change_password(&config, &session, "not the password", "brand new password");
        assert!(matches!(wrong, Err(SessionError::CurrentPasswordWrong)));
        assert_eq!(
            keyring_generations(&target).expect("generations").len(),
            1,
            "nothing is written when the current password does not open the keyring"
        );
        assert!(matches!(
            change_password(&config, &session, "old password", "short"),
            Err(SessionError::PasswordTooShort)
        ));

        change_password(&config, &session, "old password", "brand new password").expect("change");
        let generations = keyring_generations(&target).expect("generations");
        let newest = unlock_keyring(
            &generations,
            KeyringPurpose::Password,
            "brand new password",
            Some(vault_id),
        )
        .expect("the new password opens the newest generation");
        assert_eq!(newest.generation, 2);
        assert_eq!(newest.vault_key, key);
        // The old password keeps opening the older generation (the recorded
        // v1 limitation: no vault-key rollover).
        let older = unlock_keyring(
            &generations,
            KeyringPurpose::Password,
            "old password",
            Some(vault_id),
        )
        .expect("the old password still opens generation 1");
        assert_eq!(older.generation, 1);
        assert_eq!(older.vault_key, key);
    }

    #[test]
    fn new_recovery_code_appends_a_recovery_generation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = FolderTarget::open(dir.path()).expect("target");
        let vault_id = Uuid::new_v4();
        let first = generate_recovery_code();
        let (_file, key) =
            bootstrap_keyring(&target, vault_id, "old password", &first).expect("bootstrap");
        let config = folder_config(dir.path());
        let session =
            bind_session(&dir.path().join("app-data/state.json"), vault_id, key).expect("session");

        let fresh = new_recovery_code(&config, &session).expect("a new code");
        assert_ne!(fresh, first);
        let generations = keyring_generations(&target).expect("generations");
        let unlocked = unlock_keyring(
            &generations,
            KeyringPurpose::Recovery,
            &fresh,
            Some(vault_id),
        )
        .expect("the new code opens the newest generation");
        assert_eq!(unlocked.generation, 2);
        assert_eq!(unlocked.vault_key, key);
        // The password still opens it too, by the older wrap.
        let password = unlock_keyring(
            &generations,
            KeyringPurpose::Password,
            "old password",
            Some(vault_id),
        )
        .expect("the password still opens generation 1");
        assert_eq!(password.generation, 1);
    }
}
