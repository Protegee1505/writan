use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};
use uuid::Uuid;
use writan_core::{Note, SearchQuery, Workspace};
use writan_sync::{
    FolderTarget, FolderVault, KeyringPurpose, S3Config, S3HttpTransport, S3Target, SyncReport,
    SyncState, SyncTarget, TargetError, bootstrap_keyring, generate_recovery_code,
    keyring_generations, sync as run_engine, unlock_keyring,
};

use crate::state::{AppState, Inner, S3ConfigFile, SyncConfig, SyncKind};
use crate::sync::{
    SyncSession, forget_s3_secret, forget_vault_key, recall_s3_secret, remember_s3_secret,
    remember_vault_key,
};

type CmdResult<T> = Result<T, String>;

fn parse_id(id: &str) -> CmdResult<Uuid> {
    Uuid::parse_str(id).map_err(|e| format!("invalid note id: {e}"))
}

fn with_ws<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&mut Workspace) -> CmdResult<T>,
) -> CmdResult<T> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let workspace = inner
        .workspace
        .as_mut()
        .ok_or_else(|| "no vault is open".to_string())?;
    f(workspace)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub vault_path: Option<String>,
    pub note_count: usize,
    pub data_dir: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryDto {
    pub id: String,
    pub rel_path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created: Option<String>,
    pub updated: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub id: String,
    pub rel_path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub snippet: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDto {
    pub id: String,
    pub rel_path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub body: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashDto {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub deleted_at: String,
    pub original_path: String,
    pub size: u64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileDto {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub errors: usize,
}

fn note_dto(note: &Note) -> NoteDto {
    NoteDto {
        id: note.meta.id.to_string(),
        rel_path: note.rel_path.clone(),
        title: note.meta.title.clone(),
        tags: note.meta.tags.clone(),
        pinned: note.meta.pinned,
        created: note.meta.created.clone(),
        updated: note.meta.updated.clone(),
        body: note.body.clone(),
    }
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> CmdResult<StatusDto> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    match inner.workspace.as_ref() {
        Some(workspace) => Ok(StatusDto {
            vault_path: inner
                .config
                .vault_path
                .as_ref()
                .map(|p| p.display().to_string()),
            note_count: workspace.index().count().map_err(|e| e.to_string())? as usize,
            data_dir: state.state_dir.display().to_string(),
        }),
        None => Ok(StatusDto {
            vault_path: None,
            note_count: 0,
            data_dir: state.state_dir.display().to_string(),
        }),
    }
}

#[tauri::command]
pub fn open_vault(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<StatusDto> {
    let dir = PathBuf::from(&path);
    state.open_vault(&dir)?;
    crate::spawn_watcher(&app, dir);
    get_status(state)
}

#[tauri::command]
pub fn list_notes(state: State<'_, AppState>, limit: Option<usize>) -> CmdResult<Vec<SummaryDto>> {
    with_ws(&state, |workspace| {
        let notes = workspace
            .list(limit.unwrap_or(500))
            .map_err(|e| e.to_string())?;
        Ok(notes
            .iter()
            .map(|n| SummaryDto {
                id: n.id.to_string(),
                rel_path: n.rel_path.clone(),
                title: n.title.clone(),
                tags: n.tags.clone(),
                pinned: n.pinned,
                created: n.created.clone(),
                updated: n.updated.clone(),
            })
            .collect())
    })
}

#[tauri::command]
pub fn search_notes(
    state: State<'_, AppState>,
    text: String,
    tags: Option<Vec<String>>,
    folder: Option<String>,
    pinned_only: Option<bool>,
    limit: Option<usize>,
) -> CmdResult<Vec<HitDto>> {
    let text = text.trim().to_string();
    let tags = tags.unwrap_or_default();
    let folder = folder.filter(|f| !f.trim().is_empty());
    let pinned_only = pinned_only.unwrap_or(false);
    with_ws(&state, |workspace| {
        if text.is_empty() && tags.is_empty() && folder.is_none() && !pinned_only {
            let notes = workspace
                .list(limit.unwrap_or(500))
                .map_err(|e| e.to_string())?;
            return Ok(notes
                .iter()
                .map(|n| HitDto {
                    id: n.id.to_string(),
                    rel_path: n.rel_path.clone(),
                    title: n.title.clone(),
                    tags: n.tags.clone(),
                    pinned: n.pinned,
                    snippet: String::new(),
                })
                .collect());
        }
        let query = SearchQuery {
            text,
            tags,
            folder,
            pinned_only,
            limit,
        };
        let hits = workspace.search(&query).map_err(|e| e.to_string())?;
        Ok(hits
            .iter()
            .map(|h| HitDto {
                id: h.id.to_string(),
                rel_path: h.rel_path.clone(),
                title: h.title.clone(),
                tags: h.tags.clone(),
                pinned: h.pinned,
                snippet: h.snippet.clone(),
            })
            .collect())
    })
}

#[tauri::command]
pub fn get_note(state: State<'_, AppState>, id: String) -> CmdResult<Option<NoteDto>> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        Ok(workspace
            .get_note(&id)
            .map_err(|e| e.to_string())?
            .map(|note| note_dto(&note)))
    })
}

#[tauri::command]
pub fn create_note(state: State<'_, AppState>, title: Option<String>) -> CmdResult<NoteDto> {
    let title = title.unwrap_or_else(|| "Untitled".to_string());
    with_ws(&state, |workspace| {
        let note = workspace
            .create_note(&title, &[])
            .map_err(|e| e.to_string())?;
        Ok(note_dto(&note))
    })
}

#[tauri::command]
pub fn save_note(
    state: State<'_, AppState>,
    id: String,
    body: String,
    tags: Option<Vec<String>>,
) -> CmdResult<NoteDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let note = workspace
            .update_note(&id, Some(&body), tags)
            .map_err(|e| e.to_string())?;
        Ok(note_dto(&note))
    })
}

#[tauri::command]
pub fn rename_note(state: State<'_, AppState>, id: String, title: String) -> CmdResult<NoteDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let mut note = workspace
            .get_note(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "note not found".to_string())?;
        workspace
            .rename_note(&mut note, &title)
            .map_err(|e| e.to_string())?;
        Ok(note_dto(&note))
    })
}

#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, id: String) -> CmdResult<TrashDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let entry = workspace.delete_note(&id).map_err(|e| e.to_string())?;
        Ok(TrashDto {
            id: entry.id.to_string(),
            title: entry.title,
            tags: entry.tags,
            pinned: entry.pinned,
            deleted_at: entry.deleted_at,
            original_path: entry.original_path,
            size: entry.size,
        })
    })
}

#[tauri::command]
pub fn list_trash(state: State<'_, AppState>) -> CmdResult<Vec<TrashDto>> {
    with_ws(&state, |workspace| {
        let entries = workspace.trash().list().map_err(|e| e.to_string())?;
        Ok(entries
            .iter()
            .map(|entry| TrashDto {
                id: entry.id.to_string(),
                title: entry.title.clone(),
                tags: entry.tags.clone(),
                pinned: entry.pinned,
                deleted_at: entry.deleted_at.clone(),
                original_path: entry.original_path.clone(),
                size: entry.size,
            })
            .collect())
    })
}

#[tauri::command]
pub fn restore_note(state: State<'_, AppState>, id: String) -> CmdResult<NoteDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let note = workspace.restore_note(&id).map_err(|e| e.to_string())?;
        Ok(note_dto(&note))
    })
}

#[tauri::command]
pub fn purge_note(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        workspace.trash().purge(&id).map_err(|e| e.to_string())
    })
}

/// Delete every trashed note forever; returns how many were removed.
#[tauri::command]
pub fn empty_trash(state: State<'_, AppState>) -> CmdResult<usize> {
    with_ws(&state, |workspace| {
        workspace.trash().empty().map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub fn set_pinned(state: State<'_, AppState>, id: String, pinned: bool) -> CmdResult<NoteDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let note = workspace
            .set_pinned(&id, pinned)
            .map_err(|e| e.to_string())?;
        Ok(note_dto(&note))
    })
}

#[tauri::command]
pub fn rebuild_index(state: State<'_, AppState>) -> CmdResult<ReconcileDto> {
    with_ws(&state, |workspace| {
        let stats = workspace.rebuild_index().map_err(|e| e.to_string())?;
        Ok(ReconcileDto {
            added: stats.added,
            updated: stats.updated,
            removed: stats.removed,
            unchanged: stats.unchanged,
            errors: stats.errors,
        })
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountDto {
    pub name: String,
    pub count: i64,
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> CmdResult<Vec<CountDto>> {
    with_ws(&state, |workspace| {
        Ok(workspace
            .tag_counts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(name, count)| CountDto { name, count })
            .collect())
    })
}

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> CmdResult<Vec<CountDto>> {
    with_ws(&state, |workspace| {
        Ok(workspace
            .folder_counts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(name, count)| CountDto { name, count })
            .collect())
    })
}

/// Store pasted/dropped bytes under the vault's `attachments/` folder; the
/// naming and size rules live in `writan_core::attachments` (shared with the
/// GTK shell).
fn write_attachment(workspace: &Workspace, file_name: &str, bytes: &[u8]) -> CmdResult<String> {
    writan_core::attachments::write_attachment(workspace.vault().root(), file_name, bytes)
        .map_err(|error| error.to_string())
}

/// Store pasted/dropped bytes as a vault attachment; returns its vault-relative path.
#[tauri::command]
pub fn save_attachment(
    state: State<'_, AppState>,
    file_name: String,
    bytes: Vec<u8>,
) -> CmdResult<String> {
    with_ws(&state, |workspace| {
        write_attachment(workspace, &file_name, &bytes)
    })
}

/// Copy a file from disk into the vault's attachments folder.
#[tauri::command]
pub fn import_attachment(state: State<'_, AppState>, source: String) -> CmdResult<String> {
    with_ws(&state, |workspace| {
        let data = std::fs::read(&source).map_err(|e| format!("read {source}: {e}"))?;
        let name = Path::new(&source)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        write_attachment(workspace, &name, &data)
    })
}

/// The persisted UI preferences (opaque JSON owned by the frontend registry).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    Ok(state.settings())
}

/// Replace the UI preferences; validated as a size-capped JSON object.
#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: serde_json::Value) -> CmdResult<()> {
    state.save_settings(settings)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReportDto {
    pub published: usize,
    pub merged: usize,
    pub adopted: usize,
    pub conflict_copies: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub unreadable: usize,
}

impl SyncReportDto {
    fn from_report(report: &SyncReport) -> Self {
        Self {
            published: report.published,
            merged: report.merged,
            adopted: report.adopted,
            conflict_copies: report.conflict_copies,
            deleted: report.deleted,
            unchanged: report.unchanged,
            unreadable: report.unreadable,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    pub enabled: bool,
    pub unlocked: bool,
    /// "folder" or "s3" — the kind of configured target.
    pub kind: String,
    pub target: Option<String>,
    pub vault_id: Option<String>,
    pub device_id: Option<String>,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReportDto>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConnectDto {
    /// Shown exactly once when a new keyring is created; `None` when joining
    /// an existing one.
    pub recovery_code: Option<String>,
    pub status: SyncStatusDto,
}

fn sync_status_from(state: &AppState, inner: &Inner) -> SyncStatusDto {
    let (kind, target) = match inner.config.sync.kind {
        SyncKind::Folder => (
            "folder",
            inner
                .config
                .sync
                .target
                .as_ref()
                .map(|path| path.display().to_string()),
        ),
        SyncKind::S3 => (
            "s3",
            inner
                .config
                .sync
                .s3
                .as_ref()
                .map(|s3| format!("s3://{}", s3.bucket)),
        ),
    };
    let mut dto = SyncStatusDto {
        enabled: inner.config.sync.enabled,
        unlocked: inner.sync.is_some(),
        kind: kind.to_string(),
        target,
        vault_id: None,
        device_id: None,
        last_sync: None,
        last_report: None,
    };
    if let Some(session) = &inner.sync {
        dto.vault_id = Some(session.vault_id.to_string());
        dto.device_id = Some(session.device.to_string());
        dto.last_sync = session.last_sync.clone();
        dto.last_report = session.last_report.as_ref().map(SyncReportDto::from_report);
    } else if let Ok(stored) = SyncState::load(&state.sync_state_path) {
        dto.vault_id = stored.vault_id.map(|id| id.to_string());
        dto.device_id = stored.device.map(|id| id.to_string());
    }
    dto
}

/// The configured target for one engine pass: the shared folder, or the
/// S3-compatible bucket (ADR-0006).
enum SyncTargetImpl {
    Folder(FolderTarget),
    S3(S3Target<S3HttpTransport>),
}

impl SyncTarget for SyncTargetImpl {
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

fn build_sync_target(inner: &Inner) -> CmdResult<SyncTargetImpl> {
    match inner.config.sync.kind {
        SyncKind::Folder => {
            let path = inner
                .config
                .sync
                .target
                .as_ref()
                .ok_or_else(|| "sync is not configured yet".to_string())?;
            let target = FolderTarget::open(path).map_err(|e| e.to_string())?;
            Ok(SyncTargetImpl::Folder(target))
        }
        SyncKind::S3 => {
            let s3 = inner
                .config
                .sync
                .s3
                .as_ref()
                .ok_or_else(|| "sync is not configured yet".to_string())?;
            let secret = recall_s3_secret(&s3.endpoint, &s3.bucket).ok_or_else(|| {
                "the S3 secret key is not on this device — connect sync again".to_string()
            })?;
            let cfg = S3Config {
                endpoint: s3.endpoint.clone(),
                region: s3.region.clone(),
                bucket: s3.bucket.clone(),
                access_key: s3.access_key.clone(),
                secret_key: secret,
            };
            Ok(SyncTargetImpl::S3(S3Target::new(S3HttpTransport::new(cfg))))
        }
    }
}

#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> CmdResult<SyncStatusDto> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    Ok(sync_status_from(&state, &inner))
}

/// One engine pass while `inner` is already locked; reconciles the index so
/// notes merged in from other devices show up in the UI.
fn run_sync_locked(state: &AppState, inner: &mut Inner) -> CmdResult<SyncReportDto> {
    if !inner.config.sync.enabled {
        return Err("sync is disabled".to_string());
    }
    let target = build_sync_target(inner)?;
    let bound_vault = inner.config.sync.vault_path.clone();
    let current_vault = inner
        .workspace
        .as_ref()
        .map(|workspace| workspace.vault().root().to_path_buf());
    if let Some(bound) = bound_vault
        && current_vault.as_deref() != Some(bound.as_path())
    {
        return Err("this sync belongs to a different vault folder".to_string());
    }
    let Some(workspace) = inner.workspace.as_mut() else {
        return Err("no vault is open".to_string());
    };
    let Some(session) = inner.sync.as_mut() else {
        return Err("sync is locked; unlock it first".to_string());
    };
    let vault = FolderVault::new(workspace.vault());
    let report = run_engine(
        &session.vault_key,
        session.device,
        session.vault_id,
        &mut session.state,
        &vault,
        &target,
    )
    .map_err(|e| e.to_string())?;
    let dto = SyncReportDto::from_report(&report);
    session
        .state
        .save(&state.sync_state_path)
        .map_err(|e| e.to_string())?;
    session.last_sync = Some(writan_core::now_rfc3339());
    session.last_report = Some(report);
    let _ = workspace.reconcile();
    Ok(dto)
}

/// Connect this vault to a shared folder: create a fresh keyring there (the
/// recovery code is returned exactly once) or unlock an existing one with the
/// password. Runs a first sync immediately.
#[tauri::command]
pub fn sync_connect(
    state: State<'_, AppState>,
    target: String,
    password: String,
) -> CmdResult<SyncConnectDto> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("choose a sync folder first".to_string());
    }
    if password.is_empty() {
        return Err("set a sync password first".to_string());
    }
    let target_path = PathBuf::from(trimmed);
    let folder = FolderTarget::open(&target_path).map_err(|e| e.to_string())?;
    let generations = keyring_generations(&folder).map_err(|e| e.to_string())?;
    let (vault_id, vault_key, recovery_code) = if generations.is_empty() {
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) = bootstrap_keyring(&folder, vault_id, &password, &code)
            .map_err(|e| format!("failed to create the keyring: {e}"))?;
        (vault_id, key, Some(code))
    } else {
        let unlocked = unlock_keyring(&generations, KeyringPurpose::Password, &password, None)
            .map_err(|e| format!("could not unlock the existing keyring: {e}"))?;
        (unlocked.vault_id, unlocked.vault_key, None)
    };
    remember_vault_key(vault_id, &vault_key);

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let Some(vault_path) = inner
        .workspace
        .as_ref()
        .map(|workspace| workspace.vault().root().to_path_buf())
    else {
        return Err("open a vault before setting up sync".to_string());
    };
    let mut sync_state = SyncState::load(&state.sync_state_path).unwrap_or_default();
    if let Some(existing) = sync_state.vault_id
        && existing != vault_id
    {
        return Err("this device already syncs a different vault".to_string());
    }
    sync_state.vault_id = Some(vault_id);
    let device = sync_state.device.unwrap_or_else(Uuid::new_v4);
    sync_state.device = Some(device);
    inner.config.sync = SyncConfig {
        kind: SyncKind::Folder,
        target: Some(target_path),
        s3: None,
        vault_path: Some(vault_path),
        enabled: true,
    };
    inner.sync = Some(SyncSession {
        vault_id,
        device,
        vault_key: zeroize::Zeroizing::new(vault_key),
        state: sync_state,
        last_sync: None,
        last_report: None,
    });
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code,
        status: sync_status_from(&state, &inner),
    })
}

/// Connect this vault to an S3-compatible bucket (ADR-0006). Same keyring
/// flow as the shared folder; the secret key goes to the OS keychain and the
/// rest of the connection details to `config.json`.
#[tauri::command]
pub fn sync_connect_s3(
    state: State<'_, AppState>,
    endpoint: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    password: String,
) -> CmdResult<SyncConnectDto> {
    let endpoint = endpoint.trim().to_string();
    let bucket = bucket.trim().to_string();
    let region = match region.trim() {
        "" => "us-east-1".to_string(),
        region => region.to_string(),
    };
    let access_key = access_key.trim().to_string();
    let secret_key = secret_key.trim().to_string();
    if endpoint.is_empty() {
        return Err("set the S3 endpoint first".to_string());
    }
    if bucket.is_empty() {
        return Err("set the bucket first".to_string());
    }
    if access_key.is_empty() || secret_key.is_empty() {
        return Err("set the access key and secret first".to_string());
    }
    if password.is_empty() {
        return Err("set a sync password first".to_string());
    }
    let cfg = S3Config {
        endpoint: endpoint.clone(),
        region: region.clone(),
        bucket: bucket.clone(),
        access_key: access_key.clone(),
        secret_key: secret_key.clone(),
    };
    let target = S3Target::new(S3HttpTransport::new(cfg));
    let generations =
        keyring_generations(&target).map_err(|e| format!("could not reach the bucket: {e}"))?;
    let (vault_id, vault_key, recovery_code) = if generations.is_empty() {
        let vault_id = Uuid::new_v4();
        let code = generate_recovery_code();
        let (_file, key) = bootstrap_keyring(&target, vault_id, &password, &code)
            .map_err(|e| format!("failed to create the keyring: {e}"))?;
        (vault_id, key, Some(code))
    } else {
        let unlocked = unlock_keyring(&generations, KeyringPurpose::Password, &password, None)
            .map_err(|e| format!("could not unlock the existing keyring: {e}"))?;
        (unlocked.vault_id, unlocked.vault_key, None)
    };
    remember_vault_key(vault_id, &vault_key);
    remember_s3_secret(&endpoint, &bucket, &secret_key);

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let Some(vault_path) = inner
        .workspace
        .as_ref()
        .map(|workspace| workspace.vault().root().to_path_buf())
    else {
        return Err("open a vault before setting up sync".to_string());
    };
    let mut sync_state = SyncState::load(&state.sync_state_path).unwrap_or_default();
    if let Some(existing) = sync_state.vault_id
        && existing != vault_id
    {
        return Err("this device already syncs a different vault".to_string());
    }
    sync_state.vault_id = Some(vault_id);
    let device = sync_state.device.unwrap_or_else(Uuid::new_v4);
    sync_state.device = Some(device);
    inner.config.sync = SyncConfig {
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
    inner.sync = Some(SyncSession {
        vault_id,
        device,
        vault_key: zeroize::Zeroizing::new(vault_key),
        state: sync_state,
        last_sync: None,
        last_report: None,
    });
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code,
        status: sync_status_from(&state, &inner),
    })
}

/// Unlock sync after a restart with the password.
#[tauri::command]
pub fn sync_unlock(state: State<'_, AppState>, password: String) -> CmdResult<SyncStatusDto> {
    unlock_sync(&state, KeyringPurpose::Password, password.trim())
}

/// Unlock sync with the recovery code (forgotten password).
#[tauri::command]
pub fn sync_unlock_recovery(state: State<'_, AppState>, code: String) -> CmdResult<SyncStatusDto> {
    unlock_sync(&state, KeyringPurpose::Recovery, code.trim())
}

fn unlock_sync(
    state: &AppState,
    purpose: KeyringPurpose,
    credential: &str,
) -> CmdResult<SyncStatusDto> {
    if credential.is_empty() {
        return Err("enter the password or recovery code".to_string());
    }
    let target = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        build_sync_target(&inner)?
    };
    let generations = keyring_generations(&target).map_err(|e| e.to_string())?;
    if generations.is_empty() {
        return Err("the sync folder has no keyring yet".to_string());
    }
    let expected = SyncState::load(&state.sync_state_path)
        .ok()
        .and_then(|stored| stored.vault_id);
    let unlocked = unlock_keyring(&generations, purpose, credential, expected)
        .map_err(|e| format!("could not unlock: {e}"))?;
    remember_vault_key(unlocked.vault_id, &unlocked.vault_key);

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let mut sync_state = SyncState::load(&state.sync_state_path).unwrap_or_default();
    sync_state.vault_id = Some(unlocked.vault_id);
    let device = sync_state.device.unwrap_or_else(Uuid::new_v4);
    sync_state.device = Some(device);
    inner.sync = Some(SyncSession {
        vault_id: unlocked.vault_id,
        device,
        vault_key: zeroize::Zeroizing::new(unlocked.vault_key),
        state: sync_state,
        last_sync: None,
        last_report: None,
    });
    inner.config.sync.enabled = true;
    state.save_config(&inner);
    Ok(sync_status_from(state, &inner))
}

/// Run one sync pass now.
#[tauri::command]
pub fn sync_now(state: State<'_, AppState>) -> CmdResult<SyncReportDto> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    run_sync_locked(&state, &mut inner)
}

/// Lock sync: drop the in-memory key and forget the keychain entry.
#[tauri::command]
pub fn sync_lock(state: State<'_, AppState>) -> CmdResult<SyncStatusDto> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    if let Some(session) = &inner.sync {
        forget_vault_key(session.vault_id);
    }
    inner.sync = None;
    Ok(sync_status_from(&state, &inner))
}

/// Turn sync off for this vault (the target keeps its blobs and keyring).
#[tauri::command]
pub fn sync_disable(state: State<'_, AppState>) -> CmdResult<SyncStatusDto> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    if let Some(session) = &inner.sync {
        forget_vault_key(session.vault_id);
    }
    if let Some(s3) = &inner.config.sync.s3 {
        forget_s3_secret(&s3.endpoint, &s3.bucket);
    }
    inner.sync = None;
    inner.config.sync.enabled = false;
    state.save_config(&inner);
    Ok(sync_status_from(&state, &inner))
}

#[cfg(test)]
mod tests {
    #[test]
    fn attachment_names_use_the_core_rules() {
        let name = writan_core::attachments::attachment_name("My Photo (1).PNG", b"hello");
        assert!(name.starts_with("my-photo"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        let other = writan_core::attachments::attachment_name("renamed.image.png", b"hello");
        assert_eq!(
            name.rsplit_once('-').map(|(_, r)| r.to_string()),
            other.rsplit_once('-').map(|(_, r)| r.to_string()),
        );
        let no_ext = writan_core::attachments::attachment_name("notes", b"x");
        assert!(!no_ext.contains('.'), "{no_ext}");
    }
}
