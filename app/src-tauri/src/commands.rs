use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};
use uuid::Uuid;
use writan_core::{Note, SearchQuery, Workspace};

use crate::state::AppState;

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
        let mut note = workspace
            .get_note(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "note not found".to_string())?;
        note.body = body;
        if let Some(tags) = tags {
            note.meta.tags = tags;
        }
        workspace.save_note(&mut note).map_err(|e| e.to_string())?;
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

const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// Deterministic attachment file name: `<slug>-<hash8>.<ext>`.
fn attachment_name(file_name: &str, bytes: &[u8]) -> String {
    let path = Path::new(file_name);
    let mut stem =
        writan_core::vault::slugify(path.file_stem().and_then(|s| s.to_str()).unwrap_or("file"));
    if stem.is_empty() {
        stem = "file".to_string();
    }
    let ext: String = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let hash = blake3::hash(bytes);
    let short = &hash.to_hex()[..8];
    if ext.is_empty() {
        format!("{stem}-{short}")
    } else {
        format!("{stem}-{short}.{ext}")
    }
}

fn write_attachment(workspace: &Workspace, file_name: &str, bytes: &[u8]) -> CmdResult<String> {
    if bytes.is_empty() {
        return Err("attachment is empty".to_string());
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err("attachment is larger than 25 MB".to_string());
    }
    let name = attachment_name(file_name, bytes);
    let dir = workspace.vault().root().join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create attachments dir: {e}"))?;
    let path = dir.join(&name);
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|e| format!("write attachment: {e}"))?;
    }
    Ok(format!("attachments/{name}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_are_slugged_and_hashed() {
        let name = attachment_name("My Photo (1).PNG", b"hello");
        assert!(name.starts_with("my-photo"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        let other = attachment_name("renamed.image.png", b"hello");
        assert_eq!(
            name.rsplit_once('-').map(|(_, r)| r.to_string()),
            other.rsplit_once('-').map(|(_, r)| r.to_string()),
        );
        let no_ext = attachment_name("notes", b"x");
        assert!(!no_ext.contains('.'), "{no_ext}");
    }
}
