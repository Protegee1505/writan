//! Trash: deleted notes are moved aside (file + metadata sidecar) so they can
//! be listed, restored, or purged. Lives in the app state dir, never in the vault.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::vault::{Note, Vault, VaultError};
use crate::{NoteId, NoteMeta, now_rfc3339};

#[derive(Debug, thiserror::Error)]
pub enum TrashError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("corrupt trash record: {0}")]
    Corrupt(String),
    #[error("note not found in trash: {0}")]
    NotFound(NoteId),
    #[error("vault error: {0}")]
    Vault(#[from] VaultError),
}

/// A note that was deleted from the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    pub id: NoteId,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
    /// RFC 3339 timestamp of deletion.
    pub deleted_at: String,
    /// The vault-relative path the note had when it was deleted.
    pub original_path: String,
    pub size: u64,
}

pub struct Trash {
    dir: PathBuf,
}

impl Trash {
    /// Open (creating if needed) the trash directory.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, TrashError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(|source| TrashError::Io {
            path: dir.clone(),
            source,
        })?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn note_path(&self, id: &NoteId) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    fn meta_path(&self, id: &NoteId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Move `note_path` into the trash, recording its metadata.
    pub fn send(
        &self,
        note_path: &Path,
        meta: &NoteMeta,
        original_path: &str,
    ) -> Result<TrashEntry, TrashError> {
        let size = fs::metadata(note_path)
            .map_err(|source| TrashError::Io {
                path: note_path.to_path_buf(),
                source,
            })?
            .len();
        let entry = TrashEntry {
            id: meta.id,
            title: meta.title.clone(),
            tags: meta.tags.clone(),
            pinned: meta.pinned,
            deleted_at: now_rfc3339(),
            original_path: original_path.to_string(),
            size,
        };
        let dest = self.note_path(&entry.id);
        move_file(note_path, &dest)?;
        let json =
            serde_json::to_string_pretty(&entry).map_err(|e| TrashError::Corrupt(e.to_string()))?;
        let meta_path = self.meta_path(&entry.id);
        fs::write(&meta_path, json).map_err(|source| TrashError::Io {
            path: meta_path,
            source,
        })?;
        Ok(entry)
    }

    /// All trash entries, most recently deleted first.
    pub fn list(&self) -> Result<Vec<TrashEntry>, TrashError> {
        let mut out = Vec::new();
        let read = fs::read_dir(&self.dir).map_err(|source| TrashError::Io {
            path: self.dir.clone(),
            source,
        })?;
        for item in read {
            let item = item.map_err(|source| TrashError::Io {
                path: self.dir.clone(),
                source,
            })?;
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|source| TrashError::Io {
                path: path.clone(),
                source,
            })?;
            let entry: TrashEntry = serde_json::from_str(&text)
                .map_err(|e| TrashError::Corrupt(format!("{}: {e}", path.display())))?;
            out.push(entry);
        }
        out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        Ok(out)
    }

    pub fn get(&self, id: &NoteId) -> Result<TrashEntry, TrashError> {
        let path = self.meta_path(id);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TrashError::NotFound(*id));
            }
            Err(source) => return Err(TrashError::Io { path, source }),
        };
        serde_json::from_str(&text)
            .map_err(|e| TrashError::Corrupt(format!("{}: {e}", path.display())))
    }

    /// Move a trashed note back into the vault; returns the restored note.
    pub fn restore(&self, id: &NoteId, vault: &Vault) -> Result<Note, TrashError> {
        let entry = self.get(id)?;
        let source = self.note_path(id);
        if !source.exists() {
            return Err(TrashError::NotFound(*id));
        }
        let rel = free_restore_path(vault, &entry.original_path);
        let dest = vault.abs_path(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|source| TrashError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        move_file(&source, &dest)?;
        let _ = fs::remove_file(self.meta_path(id));
        let parsed = vault.parse_file(&rel)?;
        let Some(meta) = parsed.meta else {
            return Err(TrashError::Corrupt(format!(
                "restored note {rel} has no frontmatter id"
            )));
        };
        Ok(Note {
            meta,
            body: parsed.body,
            rel_path: rel,
            extra: parsed.extra,
        })
    }

    /// Delete one trashed note for good.
    pub fn purge(&self, id: &NoteId) -> Result<(), TrashError> {
        let note = self.note_path(id);
        let meta = self.meta_path(id);
        if !note.exists() && !meta.exists() {
            return Err(TrashError::NotFound(*id));
        }
        for path in [note, meta] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(TrashError::Io { path, source }),
            }
        }
        Ok(())
    }

    /// Delete everything in the trash; returns how many notes were removed.
    pub fn empty(&self) -> Result<usize, TrashError> {
        let mut count = 0usize;
        let read = fs::read_dir(&self.dir).map_err(|source| TrashError::Io {
            path: self.dir.clone(),
            source,
        })?;
        for item in read {
            let item = item.map_err(|source| TrashError::Io {
                path: self.dir.clone(),
                source,
            })?;
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                count += 1;
            }
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(TrashError::Io { path, source }),
            }
        }
        Ok(count)
    }
}

fn move_file(from: &Path, to: &Path) -> Result<(), TrashError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            // rename fails across volumes (e.g. vault on D:, state dir on C:).
            fs::copy(from, to).map_err(|source| TrashError::Io {
                path: to.to_path_buf(),
                source,
            })?;
            fs::remove_file(from).map_err(|source| TrashError::Io {
                path: from.to_path_buf(),
                source,
            })?;
            Ok(())
        }
    }
}

/// The original path if free, else `<stem> (restored).md`, `<stem> (restored 2).md`, …
fn free_restore_path(vault: &Vault, original: &str) -> String {
    if !vault.exists(original) {
        return original.to_string();
    }
    let original = original.replace('\\', "/");
    let (parent, file) = match original.rsplit_once('/') {
        Some((parent, file)) => (Some(parent.to_string()), file.to_string()),
        None => (None, original.clone()),
    };
    let stem = file.strip_suffix(".md").unwrap_or(&file).to_string();
    for n in 1..=200usize {
        let suffix = if n == 1 {
            " (restored)".to_string()
        } else {
            format!(" (restored {n})")
        };
        let candidate_file = format!("{stem}{suffix}.md");
        let candidate = match &parent {
            Some(parent) => format!("{parent}/{candidate_file}"),
            None => candidate_file,
        };
        if !vault.exists(&candidate) {
            return candidate;
        }
    }
    format!("{stem}-{}.md", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, Vault, Trash) {
        let vault_dir = tempfile::tempdir().expect("vault dir");
        let state_dir = tempfile::tempdir().expect("state dir");
        let vault = Vault::open(vault_dir.path()).expect("vault");
        let trash = Trash::open(state_dir.path().join("trash")).expect("trash");
        (vault_dir, state_dir, vault, trash)
    }

    #[test]
    fn send_list_restore_roundtrip() {
        let (_v, _s, vault, trash) = setup();
        let note = vault
            .create("Buy milk", &["errands".to_string()])
            .expect("create");
        let abs = vault.abs_path(&note.rel_path);
        let entry = trash.send(&abs, &note.meta, &note.rel_path).expect("send");
        assert!(!abs.exists());
        let listed = trash.list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Buy milk");
        assert_eq!(listed[0].original_path, note.rel_path);

        let restored = trash.restore(&entry.id, &vault).expect("restore");
        assert_eq!(restored.meta.id, note.meta.id);
        assert_eq!(restored.rel_path, note.rel_path);
        assert!(vault.abs_path(&restored.rel_path).exists());
        assert!(trash.list().expect("list").is_empty());
    }

    #[test]
    fn restore_uses_free_name_when_occupied() {
        let (_v, _s, vault, trash) = setup();
        let note = vault.create("Same", &[]).expect("create");
        let entry = trash
            .send(&vault.abs_path(&note.rel_path), &note.meta, &note.rel_path)
            .expect("send");
        // Occupy the original path with unrelated content.
        std::fs::write(vault.abs_path(&note.rel_path), "occupied").expect("occupy");
        let restored = trash.restore(&entry.id, &vault).expect("restore");
        assert_ne!(restored.rel_path, note.rel_path);
        assert!(
            restored.rel_path.contains("(restored)"),
            "{}",
            restored.rel_path
        );
        assert!(vault.abs_path(&restored.rel_path).exists());
    }

    #[test]
    fn purge_and_empty() {
        let (_v, _s, vault, trash) = setup();
        let a = vault.create("A", &[]).expect("a");
        let a_entry = trash
            .send(&vault.abs_path(&a.rel_path), &a.meta, &a.rel_path)
            .expect("send a");
        let b = vault.create("B", &[]).expect("b");
        trash
            .send(&vault.abs_path(&b.rel_path), &b.meta, &b.rel_path)
            .expect("send b");

        trash.purge(&a_entry.id).expect("purge");
        assert!(matches!(
            trash.get(&a_entry.id),
            Err(TrashError::NotFound(_))
        ));
        assert_eq!(trash.empty().expect("empty"), 1);
        assert!(trash.list().expect("list").is_empty());
        assert!(matches!(trash.empty().expect("empty again"), 0));
    }
}
