//! The workspace ties a vault, its index, and its trash together and keeps the
//! index in step with the files — whether they changed through the app or
//! outside it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::index::{Index, IndexError, IndexedNote, SearchHit, SearchQuery};
use crate::trash::{Trash, TrashEntry, TrashError};
use crate::vault::{Note, ParsedFile, Vault, VaultError, derive_title};
use crate::{NoteId, NoteMeta, now_rfc3339};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("vault error: {0}")]
    Vault(#[from] VaultError),
    #[error("index error: {0}")]
    Index(#[from] IndexError),
    #[error("trash error: {0}")]
    Trash(#[from] TrashError),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("note not found: {0}")]
    NotFound(NoteId),
}

/// What a reconcile pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Files that could not be read or parsed; they are skipped, not fatal.
    pub errors: usize,
}

/// A vault with its index and trash.
pub struct Workspace {
    vault: Vault,
    index: Index,
    trash: Trash,
    state_dir: PathBuf,
}

impl Workspace {
    /// Open `vault_root`, creating it if needed, and store index/trash under `state_dir`.
    pub fn open(
        vault_root: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
    ) -> Result<Self, WorkspaceError> {
        let state_dir = state_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&state_dir).map_err(|source| WorkspaceError::Io {
            path: state_dir.clone(),
            source,
        })?;
        let vault = Vault::open(vault_root)?;
        let index = Index::open(&state_dir.join("index.sqlite"))?;
        let trash = Trash::open(state_dir.join("trash"))?;
        Ok(Self {
            vault,
            index,
            trash,
            state_dir,
        })
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn trash(&self) -> &Trash {
        &self.trash
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Walk the whole vault: adopt files without ids, update changed notes,
    /// drop index rows for files that no longer exist.
    pub fn reconcile(&mut self) -> Result<ReconcileStats, WorkspaceError> {
        let mut stats = ReconcileStats::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for rel in self.vault.walk_notes()? {
            seen.insert(rel.clone());
            self.reconcile_file(&rel, &mut stats)?;
        }
        for path in self.index.all_paths()? {
            if !seen.contains(&path) {
                self.index.remove_by_path(&path)?;
                stats.removed += 1;
            }
        }
        Ok(stats)
    }

    /// Reconcile a batch of vault-relative paths (e.g. from the file watcher).
    pub fn reconcile_paths(
        &mut self,
        rel_paths: &[String],
    ) -> Result<ReconcileStats, WorkspaceError> {
        let mut stats = ReconcileStats::default();
        for rel in rel_paths {
            self.reconcile_file(rel, &mut stats)?;
        }
        Ok(stats)
    }

    /// Reconcile one vault-relative path.
    pub fn reconcile_one(&mut self, rel: &str) -> Result<ReconcileStats, WorkspaceError> {
        let mut stats = ReconcileStats::default();
        self.reconcile_file(rel, &mut stats)?;
        Ok(stats)
    }

    fn reconcile_file(
        &mut self,
        rel: &str,
        stats: &mut ReconcileStats,
    ) -> Result<(), WorkspaceError> {
        let abs = self.vault.abs_path(rel);
        if !abs.exists() {
            if self.index.remove_by_path(rel)?.is_some() {
                stats.removed += 1;
            }
            return Ok(());
        }
        let stat = match self.vault.stat(rel) {
            Ok(stat) => stat,
            Err(_) => {
                stats.errors += 1;
                return Ok(());
            }
        };
        let existing = self.index.get_by_path(rel)?;
        if let Some(row) = &existing {
            // Self-write suppression: our own saves land here with a known
            // (mtime, size) pair, so they count as unchanged.
            if row.mtime_ns == stat.mtime_ns && row.size == stat.size {
                stats.unchanged += 1;
                return Ok(());
            }
        }
        let parsed = match self.vault.parse_file(rel) {
            Ok(parsed) => parsed,
            Err(_) => {
                stats.errors += 1;
                return Ok(());
            }
        };
        let (note, stat, hash) = if let Some(meta) = parsed.meta.clone() {
            (
                Note {
                    meta,
                    body: parsed.body,
                    rel_path: rel.to_string(),
                    extra: parsed.extra,
                },
                stat,
                parsed.hash,
            )
        } else {
            // Adoption: give the file a Writan identity.
            let note = adopt(&self.vault, rel, &parsed)?;
            // Writing changed the file: stat and hash again.
            let stat = self.vault.stat(rel).unwrap_or(stat);
            let hash = self
                .vault
                .parse_file(rel)
                .map(|p| p.hash)
                .unwrap_or_default();
            (note, stat, hash)
        };
        let row = IndexedNote {
            id: note.meta.id,
            rel_path: note.rel_path.clone(),
            title: note.meta.title.clone(),
            tags: note.meta.tags.clone(),
            pinned: note.meta.pinned,
            created: note.meta.created.clone(),
            updated: note.meta.updated.clone(),
            mtime_ns: stat.mtime_ns,
            size: stat.size,
            hash,
        };
        self.index.upsert(&row, &note.body)?;
        if existing.is_some() {
            stats.updated += 1;
        } else {
            stats.added += 1;
        }
        Ok(())
    }

    /// Save an edited note; stamps `updated` and refreshes the index.
    pub fn save_note(&mut self, note: &mut Note) -> Result<(), WorkspaceError> {
        note.meta.updated = Some(now_rfc3339());
        self.vault.write(note)?;
        let rel = note.rel_path.clone();
        self.reconcile_one(&rel)?;
        Ok(())
    }

    pub fn create_note(&mut self, title: &str, tags: &[String]) -> Result<Note, WorkspaceError> {
        let note = self.vault.create(title, tags)?;
        let rel = note.rel_path.clone();
        self.reconcile_one(&rel)?;
        Ok(note)
    }

    /// Rename a note's title (and file); `note` is updated in place.
    pub fn rename_note(&mut self, note: &mut Note, new_title: &str) -> Result<(), WorkspaceError> {
        let renamed = self.vault.rename(note, new_title)?;
        // Old path is gone now; new path needs indexing.
        let old_rel = note.rel_path.clone();
        self.reconcile_one(&old_rel)?;
        self.reconcile_one(&renamed.rel_path)?;
        *note = renamed;
        Ok(())
    }

    /// Move a note to the trash.
    pub fn delete_note(&mut self, id: &NoteId) -> Result<TrashEntry, WorkspaceError> {
        let row = self.index.get(id)?.ok_or(WorkspaceError::NotFound(*id))?;
        let abs = self.vault.abs_path(&row.rel_path);
        let meta = NoteMeta {
            id: row.id,
            title: row.title.clone(),
            tags: row.tags.clone(),
            pinned: row.pinned,
            created: row.created.clone(),
            updated: row.updated.clone(),
        };
        let entry = self.trash.send(&abs, &meta, &row.rel_path)?;
        self.index.remove(id)?;
        Ok(entry)
    }

    /// Restore a note from the trash into the vault.
    pub fn restore_note(&mut self, trash_id: &NoteId) -> Result<Note, WorkspaceError> {
        let note = self.trash.restore(trash_id, &self.vault)?;
        let rel = note.rel_path.clone();
        self.reconcile_one(&rel)?;
        Ok(note)
    }

    pub fn set_pinned(&mut self, id: &NoteId, pinned: bool) -> Result<Note, WorkspaceError> {
        let Some(row) = self.index.get(id)? else {
            return Err(WorkspaceError::NotFound(*id));
        };
        let mut note = self.vault.read(&row.rel_path)?;
        note.meta.pinned = pinned;
        self.save_note(&mut note)?;
        Ok(note)
    }

    pub fn get_note(&self, id: &NoteId) -> Result<Option<Note>, WorkspaceError> {
        let Some(row) = self.index.get(id)? else {
            return Ok(None);
        };
        match self.vault.read(&row.rel_path) {
            Ok(note) => Ok(Some(note)),
            // File vanished behind our back; the next reconcile prunes the row.
            Err(VaultError::Io { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list(&self, limit: usize) -> Result<Vec<IndexedNote>, WorkspaceError> {
        Ok(self.index.list(limit)?)
    }

    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, WorkspaceError> {
        Ok(self.index.search(query)?)
    }

    /// Drop the index and rebuild it from the files.
    pub fn rebuild_index(&mut self) -> Result<ReconcileStats, WorkspaceError> {
        self.index.clear()?;
        self.reconcile()
    }
}

/// Give a file a Writan identity by writing frontmatter with a fresh id.
fn adopt(vault: &Vault, rel: &str, parsed: &ParsedFile) -> Result<Note, WorkspaceError> {
    let now = now_rfc3339();
    let meta = NoteMeta {
        id: Uuid::new_v4(),
        title: parsed
            .title
            .clone()
            .unwrap_or_else(|| derive_title(&parsed.body, rel)),
        tags: parsed.tags.clone(),
        pinned: parsed.pinned,
        created: Some(now.clone()),
        updated: Some(now),
    };
    let note = Note {
        meta,
        body: parsed.body.clone(),
        rel_path: rel.to_string(),
        extra: parsed.extra.clone(),
    };
    vault.write(&note)?;
    Ok(note)
}
