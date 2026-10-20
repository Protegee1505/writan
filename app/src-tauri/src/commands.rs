use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};
use uuid::Uuid;
use writan_core::{Note, SearchQuery, Workspace};
use writan_sync::{
    FolderVault, KeyringCredential, KeyringPurpose, S3Connect, ServerConnect, ServerPairing,
    SyncReport, SyncStatus, change_password, check_vault, connect_folder, connect_s3,
    connect_server, create_pairing, join_with_code, keychain, new_recovery_code, open_target,
    pair_payload, parse_pair_payload, qr_svg, run_pass, sync_status as session_status,
    unlock as unlock_session,
};

use crate::state::{AppState, Inner};

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
    /// The OS the app runs on ("linux", "macos", "windows", "android", "ios").
    pub platform: String,
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
    let platform = std::env::consts::OS.to_string();
    match inner.workspace.as_ref() {
        Some(workspace) => Ok(StatusDto {
            vault_path: inner
                .config
                .vault_path
                .as_ref()
                .map(|p| p.display().to_string()),
            note_count: workspace.index().count().map_err(|e| e.to_string())? as usize,
            data_dir: state.state_dir.display().to_string(),
            platform,
        }),
        None => Ok(StatusDto {
            vault_path: None,
            note_count: 0,
            data_dir: state.state_dir.display().to_string(),
            platform,
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

/// Open the vault inside the app's private data directory. The Android build
/// has no folder picker, so a fresh install starts here (ADR-0009).
#[tauri::command]
pub fn open_default_vault(app: AppHandle, state: State<'_, AppState>) -> CmdResult<StatusDto> {
    let dir = state.state_dir.join("vault");
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

/// Create a note from text shared into the app (Android share target).
///
/// Returns `None` when nothing is pending — and also when no vault is open
/// yet: the native layer keeps the share until the user has a vault, so a
/// first-run share is not dropped in favour of an "open a vault first" error.
#[tauri::command]
pub async fn take_shared(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<NoteDto>> {
    if !state.has_workspace() {
        return Ok(None);
    }
    let Some(payload) = crate::share::take_pending(&app).await? else {
        return Ok(None);
    };
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let workspace = inner
        .workspace
        .as_mut()
        .ok_or_else(|| "no vault is open".to_string())?;
    let note = workspace
        .create_note_from_share(&payload.text, payload.subject.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(Some(note_dto(&note)))
}

#[tauri::command]
pub fn duplicate_note(state: State<'_, AppState>, id: String) -> CmdResult<NoteDto> {
    let id = parse_id(&id)?;
    with_ws(&state, |workspace| {
        let note = workspace.duplicate_note(&id).map_err(|e| e.to_string())?;
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

/// Export the vault as one plaintext zip (ADR-0011). The file holds exactly
/// what is already on disk, so this runs off the UI thread: a big vault takes
/// a while to compress.
#[tauri::command]
pub async fn export_backup(
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<writan_core::ExportReport> {
    let root = state
        .workspace_root()
        .ok_or_else(|| "no vault is open".to_string())?;
    let dest = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || writan_core::backup::export_archive(&root, &dest))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Import a backup zip or a Markdown file (ADR-0011). Nothing is overwritten;
/// the index is reconciled before the UI refreshes, so the new notes show up.
#[tauri::command]
pub async fn import_backup(
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<writan_core::ImportReport> {
    let root = state
        .workspace_root()
        .ok_or_else(|| "no vault is open".to_string())?;
    let source = PathBuf::from(path);
    let report = tauri::async_runtime::spawn_blocking(move || {
        writan_core::backup::import_path(&root, &source)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    with_ws(&state, |workspace| {
        workspace.reconcile().map_err(|e| e.to_string())?;
        Ok(())
    })?;
    Ok(report)
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
    /// "folder", "s3" or "server" — the kind of configured target.
    pub kind: String,
    pub target: Option<String>,
    /// The stored writan-server address and account so the setup form can
    /// prefill them (never a secret; the token stays in the keychain).
    pub server_url: Option<String>,
    pub server_email: Option<String>,
    pub vault_id: Option<String>,
    pub device_id: Option<String>,
    pub last_sync: Option<String>,
    pub last_report: Option<SyncReportDto>,
    /// The Android opt-in for the 15-minute background sync job (M5 slice 2c).
    pub background: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConnectDto {
    /// Shown exactly once when a new keyring is created; `None` when joining
    /// an existing one.
    pub recovery_code: Option<String>,
    pub status: SyncStatusDto,
}

impl SyncStatusDto {
    fn from_status(status: SyncStatus) -> Self {
        Self {
            enabled: status.enabled,
            unlocked: status.unlocked,
            kind: status.kind.as_str().to_string(),
            target: status.target,
            server_url: default_server_url(),
            server_email: None,
            vault_id: status.vault_id,
            device_id: status.device_id,
            last_sync: status.last_sync,
            last_report: status.last_report.as_ref().map(SyncReportDto::from_report),
            background: false,
        }
    }
}

/// The build-time default server address (`WRITAN_DEFAULT_SERVER`, ADR-0008
/// §9): a personal build bakes its own domain so the form arrives filled in;
/// a public release build compiles no value. Never a credential.
fn default_server_url() -> Option<String> {
    option_env!("WRITAN_DEFAULT_SERVER")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn sync_status_from(state: &AppState, inner: &Inner) -> SyncStatusDto {
    let mut dto = SyncStatusDto::from_status(session_status(
        &inner.config.sync,
        inner.sync.as_ref(),
        &state.sync_state_path,
    ));
    if let Some(server) = inner.config.sync.server.as_ref() {
        dto.server_url = Some(server.url.clone());
        dto.server_email = Some(server.email.clone());
    }
    dto.background = inner.config.background_sync;
    dto
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
/// notes merged in from other devices show up in the UI. Also the entry point
/// of the Android background worker (M5 slice 2c).
pub(crate) fn run_sync_locked(state: &AppState, inner: &mut Inner) -> CmdResult<SyncReportDto> {
    if !inner.config.sync.enabled {
        return Err("sync is disabled".to_string());
    }
    let target = open_target(
        &inner.config.sync,
        inner.sync.as_ref().map(|session| session.vault_id),
    )
    .map_err(|e| e.to_string())?;
    let current_vault = inner
        .workspace
        .as_ref()
        .map(|workspace| workspace.vault().root().to_path_buf());
    check_vault(&inner.config.sync, current_vault.as_deref()).map_err(|e| e.to_string())?;
    let Some(workspace) = inner.workspace.as_mut() else {
        return Err("no vault is open".to_string());
    };
    let Some(session) = inner.sync.as_mut() else {
        return Err("sync is locked; unlock it first".to_string());
    };
    let vault = FolderVault::new(workspace.vault());
    let report =
        run_pass(&state.sync_state_path, session, &vault, &target).map_err(|e| e.to_string())?;
    let dto = SyncReportDto::from_report(&report);
    let _ = workspace.reconcile();
    Ok(dto)
}

/// The optional older credential a retry carries after a connect asked for it
/// ("this vault was created with a different password", ADR-0008 §5): the
/// password the vault's keyring was made with, or its recovery code. Both
/// halves must be present; an empty credential means the user asked to retry
/// without one.
fn keyring_fallback(
    purpose: Option<String>,
    credential: Option<String>,
) -> CmdResult<Option<KeyringCredential>> {
    let (Some(purpose), Some(credential)) = (purpose, credential) else {
        return Ok(None);
    };
    if credential.is_empty() {
        return Ok(None);
    }
    match purpose.as_str() {
        "password" => Ok(Some(KeyringCredential::Password(credential))),
        "recovery" => Ok(Some(KeyringCredential::Recovery(credential))),
        other => Err(format!("unknown keyring credential kind: {other}")),
    }
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
    let vault_path = state.workspace_root();
    let connected = connect_folder(
        Path::new(trimmed),
        &password,
        vault_path.as_deref(),
        &state.sync_state_path,
    )
    .map_err(|e| e.to_string())?;

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    inner.config.sync = connected.config;
    inner.sync = Some(connected.session);
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code: connected.recovery_code,
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
    let details = S3Connect {
        endpoint,
        bucket,
        region,
        access_key,
        secret_key,
    };
    let vault_path = state.workspace_root();
    let connected = connect_s3(
        &details,
        &password,
        vault_path.as_deref(),
        &state.sync_state_path,
    )
    .map_err(|e| e.to_string())?;

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    inner.config.sync = connected.config;
    inner.sync = Some(connected.session);
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code: connected.recovery_code,
        status: sync_status_from(&state, &inner),
    })
}

/// The device name the server shows for this machine (ADR-0007 §3), from the
/// OS host name; the state file's device id is what really identifies it.
fn default_device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        "this device".to_string()
    } else {
        name.chars().take(64).collect()
    }
}

/// Connect this vault to a writan-server (ADR-0007): sign in — or register,
/// with `create_account` — as this device, then run the same keyring flow as
/// the other targets. One password serves both the sign-in verifier and the
/// vault key's wrap (ADR-0008): it never leaves the device, and the server
/// only ever sees a verifier derived from it.
#[tauri::command]
pub fn sync_connect_server(
    state: State<'_, AppState>,
    url: String,
    email: String,
    create_account: bool,
    password: String,
    keyring_purpose: Option<String>,
    keyring_credential: Option<String>,
) -> CmdResult<SyncConnectDto> {
    let details = ServerConnect {
        url,
        email,
        device_name: default_device_name(),
        create_account,
    };
    let fallback = keyring_fallback(keyring_purpose, keyring_credential)?;
    let vault_path = state.workspace_root();
    let connected = connect_server(
        &details,
        &password,
        fallback.as_ref(),
        vault_path.as_deref(),
        &state.sync_state_path,
    )
    .map_err(|e| e.to_string())?;

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    inner.config.sync = connected.config;
    inner.sync = Some(connected.session);
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code: connected.recovery_code,
        status: sync_status_from(&state, &inner),
    })
}

/// Join an account with a pairing code instead of its password: paste the
/// text the other device's QR carried. The code grants blob access only, so
/// the one password is still required (ADR-0007 §4, ADR-0008 §3).
#[tauri::command]
pub fn sync_join_server(
    state: State<'_, AppState>,
    payload: String,
    password: String,
    keyring_purpose: Option<String>,
    keyring_credential: Option<String>,
) -> CmdResult<SyncConnectDto> {
    let parsed = parse_pair_payload(payload.trim()).map_err(|e| e.to_string())?;
    let details = ServerPairing {
        url: parsed.url,
        code: parsed.code,
        email: parsed.email,
        device_name: default_device_name(),
    };
    let fallback = keyring_fallback(keyring_purpose, keyring_credential)?;
    let vault_path = state.workspace_root();
    let connected = join_with_code(
        &details,
        &password,
        fallback.as_ref(),
        vault_path.as_deref(),
        &state.sync_state_path,
    )
    .map_err(|e| e.to_string())?;

    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    inner.config.sync = connected.config;
    inner.sync = Some(connected.session);
    state.save_config(&inner);
    run_sync_locked(&state, &mut inner)?;
    Ok(SyncConnectDto {
        recovery_code: connected.recovery_code,
        status: sync_status_from(&state, &inner),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingDto {
    /// The single-use code another device types or scans.
    pub code: String,
    /// When it stops working, RFC 3339.
    pub expires_at: String,
    /// The exact text the QR carries, for devices without a camera.
    pub payload: String,
    /// The QR as a standalone SVG document.
    pub svg: String,
}

/// Mint a single-use pairing code and its QR (ADR-0007 §4). The code grants
/// blob access only; the vault key never travels, and the joining device
/// still needs the same password or its recovery code.
#[tauri::command]
pub fn sync_pairing_code(state: State<'_, AppState>) -> CmdResult<PairingDto> {
    let (url, email) = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        let server = inner
            .config
            .sync
            .server
            .as_ref()
            .ok_or_else(|| "sync is not set up to a writan-server".to_string())?;
        (server.url.clone(), server.email.clone())
    };
    let token = keychain::recall_server_token(&url, &email)
        .ok_or_else(|| "this device has no server token — unlock sync and try again".to_string())?;
    let pairing = create_pairing(&url, &token).map_err(|e| e.to_string())?;
    let payload = pair_payload(&url, &pairing.code, &email);
    let svg = qr_svg(&payload, 4).map_err(|e| e.to_string())?;
    Ok(PairingDto {
        code: pairing.code,
        expires_at: pairing.expires_at,
        payload,
        svg,
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
    let config = {
        let inner = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        inner.config.sync.clone()
    };
    let (config, session) = unlock_session(config, &state.sync_state_path, purpose, credential)
        .map_err(|e| e.to_string())?;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    inner.config.sync = config;
    inner.sync = Some(session);
    state.save_config(&inner);
    Ok(sync_status_from(state, &inner))
}

/// Change the one password (ADR-0008 §6): first the keyring (a new generation
/// only the new password opens, appended on this device), then the account's
/// verifier on a server target. Other devices must sign in again; this one
/// keeps its token.
#[tauri::command]
pub fn sync_change_password(
    state: State<'_, AppState>,
    old_password: String,
    new_password: String,
) -> CmdResult<SyncStatusDto> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let config = inner.config.sync.clone();
    let Some(session) = inner.sync.as_ref() else {
        return Err("sync is locked; unlock it first".to_string());
    };
    change_password(&config, session, old_password.trim(), new_password.trim())
        .map_err(|e| e.to_string())?;
    Ok(sync_status_from(&state, &inner))
}

/// Mint a fresh recovery code (ADR-0008 §7) and append a keyring generation
/// only that code opens. The code is shown once and never stored anywhere; the
/// old code keeps opening older generations.
#[tauri::command]
pub fn sync_new_recovery_code(state: State<'_, AppState>) -> CmdResult<String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let config = inner.config.sync.clone();
    let Some(session) = inner.sync.as_ref() else {
        return Err("sync is locked; unlock it first".to_string());
    };
    new_recovery_code(&config, session).map_err(|e| e.to_string())
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

/// Opt in or out of the Android background sync job (M5 slice 2c, ADR-0010
/// §7): persists the flag and lets the Kotlin plugin schedule or cancel the
/// WorkManager job. Desktop stores the flag too, it just never schedules.
#[tauri::command]
pub async fn sync_set_background(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<SyncStatusDto> {
    #[cfg(not(target_os = "android"))]
    let _ = &app;
    state.save_background_sync(enabled)?;
    #[cfg(target_os = "android")]
    crate::android::set_background_sync(&app, enabled).await?;
    let inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    Ok(sync_status_from(&state, &inner))
}

/// Lock sync: drop the in-memory key and forget the keychain entry.
#[tauri::command]
pub fn sync_lock(state: State<'_, AppState>) -> CmdResult<SyncStatusDto> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    if let Some(session) = &inner.sync {
        keychain::forget_vault_key(session.vault_id);
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
        keychain::forget_vault_key(session.vault_id);
    }
    if let Some(s3) = &inner.config.sync.s3 {
        keychain::forget_s3_secret(&s3.endpoint, &s3.bucket);
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
