//! The sync engine: reconciles one vault with one target.
//!
//! One pass = target scan → local decisions → frontier publish:
//!
//! 1. **Scan** reads every blob, opens items and tombstones, and keeps the
//!    best version per note (highest vector, then device as tie-break).
//! 2. **Local** walks the vault: dominant tombstones delete unchanged files,
//!    edits after a delete resurrect (publishing a vector that dominates the
//!    tombstone), missing/local-only versions publish, and concurrent ones
//!    go through the [`LwwDiff3`] merge policy.
//! 3. **Remote-only** items whose content is not in the vault are adopted as
//!    new notes; items identical to content the vault already holds (under a
//!    different id — e.g. a conflict copy) are skipped.
//! 4. **Deletion** publishes a tombstone for every note this device last
//!    published whose file is gone and whose target version was integrated.
//!
//! The engine never overwrites silently: a merge always leaves both sides on
//! disk (kept note + conflict copy), and unreadable blobs are counted, not
//! deleted. [`SyncState`] is a local cache in the app-data directory — the
//! target carries everything needed to rebuild it (ADR-0005).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use writan_core::NoteMeta;
use writan_core::vault::{Note, Vault, slugify};

use crate::blob::{self, BlobKind, NoteBlob, TombstoneBlob};
use crate::merge::{LwwDiff3, MergeStrategy};
use crate::target::{SyncTarget, TargetError};
use crate::{DeviceId, NoteId, VersionVector};

/// Errors the engine can raise.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("target: {0}")]
    Target(#[from] TargetError),
    #[error("vault: {0}")]
    Vault(String),
    #[error("sync state: {0}")]
    State(String),
    #[error("sync state belongs to another vault")]
    VaultMismatch,
    #[error("sync state belongs to another device")]
    DeviceMismatch,
    #[error("blob: {0}")]
    Blob(#[from] blob::BlobError),
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
}

impl From<String> for EngineError {
    fn from(message: String) -> Self {
        Self::Vault(message)
    }
}

/// What one [`sync`] pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Versions this device sealed and stored.
    pub published: usize,
    /// Concurrent versions reconciled through the merge policy.
    pub merged: usize,
    /// Remote versions written into the vault.
    pub adopted: usize,
    /// Conflict copies written (and published) alongside a kept note.
    pub conflict_copies: usize,
    /// Deletions applied locally or published as tombstones.
    pub deleted: usize,
    /// Notes already in sync.
    pub unchanged: usize,
    /// Blobs that could not be parsed, opened or read.
    pub unreadable: usize,
}

/// A note file loaded from a vault.
#[derive(Debug, Clone)]
pub struct NoteFile {
    pub id: NoteId,
    pub rel: String,
    pub note: Note,
}

/// The vault side of the engine, abstracted for tests and alternative vaults.
pub trait VaultIo {
    /// Every identified note file in the vault.
    fn load(&self) -> Result<Vec<NoteFile>, String>;
    /// First free path for a new note with this title and id.
    fn free_path(&self, title: &str, id: NoteId, current: Option<&str>) -> String;
    /// Write the whole note back to its `rel_path`.
    fn write(&self, note: &Note) -> Result<(), String>;
    /// Delete the note's file.
    fn remove(&self, note: &Note) -> Result<(), String>;
}

/// [`VaultIo`] backed by a real Markdown vault.
pub struct FolderVault<'a> {
    vault: &'a Vault,
}

impl<'a> FolderVault<'a> {
    pub fn new(vault: &'a Vault) -> Self {
        Self { vault }
    }
}

impl VaultIo for FolderVault<'_> {
    fn load(&self) -> Result<Vec<NoteFile>, String> {
        let mut files = Vec::new();
        for rel in self.vault.walk_notes().map_err(|e| e.to_string())? {
            let parsed = self
                .vault
                .parse_file(&rel)
                .map_err(|e| format!("{rel}: {e}"))?;
            let Some(meta) = parsed.meta else {
                continue; // no id yet: not a synced note
            };
            let note = Note {
                meta,
                body: parsed.body,
                rel_path: rel.clone(),
                extra: parsed.extra,
            };
            files.push(NoteFile {
                id: note.meta.id,
                rel,
                note,
            });
        }
        Ok(files)
    }

    fn free_path(&self, title: &str, id: NoteId, current: Option<&str>) -> String {
        self.vault.unique_rel_path(&slugify(title), id, current)
    }

    fn write(&self, note: &Note) -> Result<(), String> {
        self.vault.write(note).map_err(|e| e.to_string())
    }

    fn remove(&self, note: &Note) -> Result<(), String> {
        self.vault.remove(&note.rel_path).map_err(|e| e.to_string())
    }
}

/// A blob this device (or another) wrote: target path components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRef {
    pub device: DeviceId,
    pub counter: u64,
}

/// Local sync bookkeeping; lives in the app-data directory, never the vault.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncState {
    /// The device this state file belongs to.
    pub device: Option<DeviceId>,
    /// The vault this state file belongs to.
    pub vault_id: Option<Uuid>,
    /// Last integrated vector per note.
    pub vectors: BTreeMap<NoteId, VersionVector>,
    /// Blob the last merge window was computed against.
    pub bases: BTreeMap<NoteId, BlobRef>,
    /// Content digest per note as of the last publish or adopt.
    pub digests: BTreeMap<NoteId, String>,
}

impl SyncState {
    /// Read state, or start fresh if the file does not exist.
    pub fn load(path: &Path) -> Result<Self, EngineError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| EngineError::State(error.to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Persist state after a successful pass.
    pub fn save(&self, path: &Path) -> Result<(), EngineError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| EngineError::State(error.to_string()))?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

struct Scan {
    items: BTreeMap<NoteId, ItemRef>,
    tombstones: BTreeMap<NoteId, TombstoneBlob>,
}

struct ItemRef {
    device: DeviceId,
    counter: u64,
    note: NoteBlob,
}

struct Publisher<'a, T: SyncTarget> {
    key: &'a [u8; 32],
    device: DeviceId,
    target: &'a T,
}

impl<T: SyncTarget> Publisher<'_, T> {
    fn publish_note(&self, mut note: NoteBlob) -> Result<(BlobRef, NoteBlob), EngineError> {
        let counter = note.vv.bump(&self.device);
        let bytes = blob::seal_note(self.key, &note)?;
        let path = blob::blob_path(BlobKind::Item, note.id, self.device, counter);
        self.target.write(&path, &bytes)?;
        Ok((
            BlobRef {
                device: self.device,
                counter,
            },
            note,
        ))
    }

    fn publish_tombstone(
        &self,
        id: NoteId,
        deleted_at: String,
        mut vv: VersionVector,
    ) -> Result<(), EngineError> {
        let counter = vv.bump(&self.device);
        let tombstone = TombstoneBlob::new(id, deleted_at, vv);
        let bytes = blob::seal_tombstone(self.key, &tombstone)?;
        let path = blob::blob_path(BlobKind::Tombstone, id, self.device, counter);
        self.target.write(&path, &bytes)?;
        Ok(())
    }

    fn load_note(&self, id: NoteId, reference: &BlobRef) -> Option<NoteBlob> {
        let path = blob::blob_path(BlobKind::Item, id, reference.device, reference.counter);
        let bytes = self.target.read(&path).ok()?;
        blob::open_note(self.key, id, &bytes).ok()
    }
}

/// Reconcile `vault` with `target` once; `state` is updated in place and the
/// caller persists it with [`SyncState::save`] after a successful pass.
pub fn sync(
    vault_key: &[u8; 32],
    device: DeviceId,
    vault_id: Uuid,
    state: &mut SyncState,
    vault: &impl VaultIo,
    target: &impl SyncTarget,
) -> Result<SyncReport, EngineError> {
    if state.vault_id.is_some_and(|id| id != vault_id) {
        return Err(EngineError::VaultMismatch);
    }
    if state.device.is_some_and(|id| id != device) {
        return Err(EngineError::DeviceMismatch);
    }
    state.vault_id = Some(vault_id);
    state.device = Some(device);

    let mut report = SyncReport::default();
    let publisher = Publisher {
        key: vault_key,
        device,
        target,
    };

    let local = vault.load()?;
    let local_digests: BTreeMap<String, NoteId> = local
        .iter()
        .map(|file| (digest_of_note(&file.note), file.id))
        .collect();

    let Scan { items, tombstones } = scan_target(vault_key, target, &mut report)?;

    // A deletion dominates while its tombstone covers both the local vector
    // and every item version on the target.
    let mut deleted_ids = BTreeSet::new();
    for (id, tombstone) in &tombstones {
        let covers_local = state
            .vectors
            .get(id)
            .is_none_or(|v| tombstone.vv.contains(v));
        let covers_items = items
            .get(id)
            .is_none_or(|item| tombstone.vv.contains(&item.note.vv));
        if covers_local && covers_items {
            deleted_ids.insert(*id);
        }
    }

    let mut local_ids: BTreeSet<NoteId> = BTreeSet::new();
    for file in &local {
        if !local_ids.insert(file.id) {
            continue; // duplicate id: first file wins
        }
        let digest_now = digest_of_note(&file.note);
        let locally_changed = state.digests.get(&file.id) != Some(&digest_now);

        if deleted_ids.contains(&file.id) {
            if locally_changed {
                // An edit after the delete resurrects the note everywhere.
                let mut seed = state
                    .vectors
                    .get(&file.id)
                    .cloned()
                    .unwrap_or_else(VersionVector::new);
                if let Some(tombstone) = tombstones.get(&file.id) {
                    seed.merge(&tombstone.vv);
                }
                let blob = note_blob_from(file, seed);
                publish_and_record(&publisher, state, blob)?;
                report.published += 1;
            } else {
                vault.remove(&file.note)?;
                state.vectors.remove(&file.id);
                state.bases.remove(&file.id);
                state.digests.remove(&file.id);
                report.deleted += 1;
            }
            continue;
        }

        let Some(item) = items.get(&file.id) else {
            // Nothing on the target yet: publish unless we already did.
            let current = state
                .vectors
                .get(&file.id)
                .cloned()
                .unwrap_or_else(VersionVector::new);
            let known = !current.0.is_empty() && !locally_changed;
            if known {
                report.unchanged += 1;
            } else {
                let blob = note_blob_from(file, current);
                publish_and_record(&publisher, state, blob)?;
                report.published += 1;
            }
            continue;
        };

        let best = &item.note;
        let local_vv = state
            .vectors
            .get(&file.id)
            .cloned()
            .unwrap_or_else(VersionVector::new);

        if best.vv.contains(&local_vv) {
            let best_digest = digest_of_blob(best);
            if best_digest == digest_now {
                report.unchanged += 1;
                state.vectors.insert(file.id, best.vv.clone());
                state.bases.insert(
                    file.id,
                    BlobRef {
                        device: item.device,
                        counter: item.counter,
                    },
                );
                state.digests.insert(file.id, best_digest);
            } else if state.digests.get(&file.id) == Some(&best_digest) {
                // Our own edit on top of the version we integrated: publish it.
                let blob = note_blob_from(file, local_vv);
                publish_and_record(&publisher, state, blob)?;
                report.published += 1;
            } else if !locally_changed {
                // A remote edit on top of the version we integrated: adopt it.
                let note = note_from_blob(best, &file.rel);
                vault.write(&note)?;
                state.vectors.insert(file.id, best.vv.clone());
                state.bases.insert(
                    file.id,
                    BlobRef {
                        device: item.device,
                        counter: item.counter,
                    },
                );
                state.digests.insert(file.id, best_digest);
                report.adopted += 1;
            } else {
                merge_flow(
                    &publisher,
                    state,
                    vault,
                    file,
                    best,
                    &local_digests,
                    &mut report,
                )?;
            }
        } else if !local_vv.0.is_empty() && local_vv.contains(&best.vv) {
            if digest_of_blob(best) == digest_now {
                report.unchanged += 1;
            } else {
                let blob = note_blob_from(file, local_vv);
                publish_and_record(&publisher, state, blob)?;
                report.published += 1;
            }
        } else {
            merge_flow(
                &publisher,
                state,
                vault,
                file,
                best,
                &local_digests,
                &mut report,
            )?;
        }
    }

    // Remote-only notes: adopt new content, skip content the vault already
    // holds, and leave deletions to the tombstone pass below.
    for (id, item) in &items {
        if local_ids.contains(id) || deleted_ids.contains(id) {
            continue;
        }
        let best = &item.note;
        let best_digest = digest_of_blob(best);
        if local_digests.contains_key(&best_digest) {
            report.unchanged += 1;
            continue;
        }
        if state.digests.get(id) == Some(&best_digest) {
            continue; // content we integrated before; the file is gone locally
        }
        let rel = vault.free_path(&best.title, *id, None);
        let note = note_from_blob(best, &rel);
        vault.write(&note)?;
        local_ids.insert(*id);
        state.vectors.insert(*id, best.vv.clone());
        state.bases.insert(
            *id,
            BlobRef {
                device: item.device,
                counter: item.counter,
            },
        );
        state.digests.insert(*id, best_digest);
        report.adopted += 1;
    }

    // Deletions: notes this device published whose file is gone and whose
    // target version carries nothing new.
    for (id, vector) in &state.vectors {
        let skip = vector.0.is_empty() || local_ids.contains(id) || tombstones.contains_key(id);
        if skip {
            continue;
        }
        match items.get(id) {
            None => {
                publisher.publish_tombstone(*id, now_rfc3339(), vector.clone())?;
                report.deleted += 1;
            }
            Some(item) if state.digests.get(id) == Some(&digest_of_blob(&item.note)) => {
                let mut seed = vector.clone();
                seed.merge(&item.note.vv);
                publisher.publish_tombstone(*id, now_rfc3339(), seed)?;
                report.deleted += 1;
            }
            Some(_) => {} // an un-integrated remote version wins; it was adopted
        }
    }

    Ok(report)
}

/// Reconcile concurrent local and remote versions of one note.
#[allow(clippy::too_many_arguments)]
fn merge_flow<T: SyncTarget, V: VaultIo>(
    publisher: &Publisher<'_, T>,
    state: &mut SyncState,
    vault: &V,
    file: &NoteFile,
    remote: &NoteBlob,
    local_digests: &BTreeMap<String, NoteId>,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    let base = state
        .bases
        .get(&file.id)
        .and_then(|reference| publisher.load_note(file.id, reference));
    let local_vv = state
        .vectors
        .get(&file.id)
        .cloned()
        .unwrap_or_else(VersionVector::new);
    let local_blob = note_blob_from(file, local_vv);
    let plan = LwwDiff3::default().resolve(base.as_ref(), &local_blob, remote);

    let kept = plan.kept;
    let kept_note = note_from_blob(&kept, &file.rel);
    vault.write(&kept_note)?;

    if let Some(copy) = plan.conflict_copy {
        let copy_digest = digest_of_blob(&copy);
        if !local_digests.contains_key(&copy_digest) {
            let rel = vault.free_path(&copy.title, copy.id, None);
            let mut copy = copy;
            copy.rel_path = rel.clone();
            let copy_note = note_from_blob(&copy, &rel);
            vault.write(&copy_note)?;
            let copy_id = copy.id;
            let (reference, published) = publisher.publish_note(copy)?;
            state.vectors.insert(copy_id, published.vv);
            state.bases.insert(copy_id, reference);
            state.digests.insert(copy_id, copy_digest);
            report.conflict_copies += 1;
        }
    }

    let kept_id = kept.id;
    let (reference, published) = publisher.publish_note(kept)?;
    state.vectors.insert(kept_id, published.vv);
    state.bases.insert(kept_id, reference);
    state.digests.insert(kept_id, digest_of_blob(&published));
    report.merged += 1;
    Ok(())
}

fn publish_and_record<T: SyncTarget>(
    publisher: &Publisher<'_, T>,
    state: &mut SyncState,
    blob: NoteBlob,
) -> Result<(), EngineError> {
    let id = blob.id;
    let (reference, published) = publisher.publish_note(blob)?;
    state.vectors.insert(id, published.vv.clone());
    state.bases.insert(id, reference);
    state.digests.insert(id, digest_of_blob(&published));
    Ok(())
}

fn scan_target<T: SyncTarget>(
    key: &[u8; 32],
    target: &T,
    report: &mut SyncReport,
) -> Result<Scan, EngineError> {
    let mut items: BTreeMap<NoteId, ItemRef> = BTreeMap::new();
    let mut tombstones: BTreeMap<NoteId, TombstoneBlob> = BTreeMap::new();
    for path in target.list()? {
        let Some(parsed) = blob::parse_blob_path(&path) else {
            continue; // unknown files are ignored, never touched
        };
        if parsed.counter == 0 {
            report.unreadable += 1;
            continue;
        }
        let Ok(bytes) = target.read(&path) else {
            report.unreadable += 1;
            continue;
        };
        match parsed.kind {
            BlobKind::Item => match blob::open_note(key, parsed.note, &bytes) {
                Ok(note) if note.id == parsed.note => {
                    let better = items
                        .get(&parsed.note)
                        .is_none_or(|current| vv_key(&note.vv) > vv_key(&current.note.vv));
                    if better {
                        items.insert(
                            parsed.note,
                            ItemRef {
                                device: parsed.device,
                                counter: parsed.counter,
                                note,
                            },
                        );
                    }
                }
                _ => report.unreadable += 1,
            },
            BlobKind::Tombstone => match blob::open_tombstone(key, parsed.note, &bytes) {
                Ok(tombstone) => {
                    let better = tombstones
                        .get(&parsed.note)
                        .is_none_or(|current| vv_key(&tombstone.vv) > vv_key(&current.vv));
                    if better {
                        tombstones.insert(parsed.note, tombstone);
                    }
                }
                Err(_) => report.unreadable += 1,
            },
        }
    }
    Ok(Scan { items, tombstones })
}

fn note_blob_from(file: &NoteFile, vv: VersionVector) -> NoteBlob {
    let mut blob = NoteBlob::new(
        file.id,
        file.rel.clone(),
        file.note.meta.title.clone(),
        file.note.body.clone(),
        vv,
    );
    blob.tags = file.note.meta.tags.clone();
    blob.pinned = file.note.meta.pinned;
    blob.created = file.note.meta.created.clone();
    blob.updated = file.note.meta.updated.clone();
    blob
}

fn note_from_blob(blob: &NoteBlob, rel: &str) -> Note {
    Note {
        meta: NoteMeta {
            id: blob.id,
            title: blob.title.clone(),
            tags: blob.tags.clone(),
            pinned: blob.pinned,
            created: blob.created.clone(),
            updated: blob.updated.clone(),
        },
        body: blob.body.clone(),
        rel_path: rel.to_string(),
        extra: serde_json::Map::new(),
    }
}

/// Stable content digest: everything that travels in a version, excluding
/// the rel-path hint (paths differ per device and are not identity).
fn content_digest(
    title: &str,
    tags: &[String],
    pinned: bool,
    created: Option<&str>,
    updated: Option<&str>,
    body: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"writan-sync/digest/1");
    for part in [title, created.unwrap_or(""), updated.unwrap_or(""), body] {
        hasher.update(part.as_bytes());
        hasher.update(&[0x1f]);
    }
    hasher.update(&[u8::from(pinned), 0x1e]);
    for tag in tags {
        hasher.update(tag.as_bytes());
        hasher.update(&[0x1d]);
    }
    hasher.finalize().to_hex().to_string()
}

fn digest_of_note(note: &Note) -> String {
    content_digest(
        &note.meta.title,
        &note.meta.tags,
        note.meta.pinned,
        note.meta.created.as_deref(),
        note.meta.updated.as_deref(),
        &note.body,
    )
}

fn digest_of_blob(blob: &NoteBlob) -> String {
    content_digest(
        &blob.title,
        &blob.tags,
        blob.pinned,
        blob.created.as_deref(),
        blob.updated.as_deref(),
        &blob.body,
    )
}

/// Ordering key for "which version is furthest ahead": total work, device
/// count, then the last device id for determinism.
fn vv_key(vv: &VersionVector) -> (u64, usize, DeviceId) {
    let total = vv.0.values().copied().sum::<u64>();
    let devices = vv.0.len();
    let tail =
        vv.0.keys()
            .next_back()
            .copied()
            .unwrap_or_else(DeviceId::nil);
    (total, devices, tail)
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_ignores_rel_path_but_tracks_content() {
        let id = Uuid::from_u128(1);
        let a = NoteBlob::new(
            id,
            "a.md".to_string(),
            "Title".to_string(),
            "body".to_string(),
            VersionVector::new(),
        );
        let mut b = a.clone();
        b.rel_path = "deep/b.md".to_string();
        assert_eq!(digest_of_blob(&a), digest_of_blob(&b));

        let mut c = a.clone();
        c.body = "other".to_string();
        assert_ne!(digest_of_blob(&a), digest_of_blob(&c));

        let mut d = a.clone();
        d.tags = vec!["x".to_string()];
        assert_ne!(digest_of_blob(&a), digest_of_blob(&d));
    }

    #[test]
    fn vv_key_prefers_more_work() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut one = VersionVector::new();
        one.bump(&a);
        let mut two = VersionVector::new();
        two.bump(&a);
        two.bump(&a);
        assert!(vv_key(&two) > vv_key(&one));

        let mut spread = VersionVector::new();
        spread.bump(&a);
        spread.bump(&b);
        assert!(vv_key(&spread) > vv_key(&two));
    }
}
