//! Sync primitives for Writan.
//!
//! The sync model is Joplin-style: every note version is an encrypted blob
//! stored on a dumb target (S3, WebDAV, folder, writan-server), named by a
//! per-item [`VersionVector`]. Merging is a bounded three-way diff over the
//! Markdown body with an explicit conflict copy as the fallback, never a
//! silent overwrite.
//!
//! Blob layout, payloads and binding are fixed by [`blob`] and
//! `docs/adr/0005-sync-blob-format.md`; reconciliation of concurrent versions
//! is [`merge`]'s bounded diff3 with a last-writer-wins window (ADR-0001).

#![forbid(unsafe_code)]

pub mod blob;
pub mod engine;
pub mod merge;
pub mod target;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use blob::{
    BlobError, BlobKind, BlobPath, FORMAT_NOTE, FORMAT_TOMBSTONE, NoteBlob, TombstoneBlob,
    blob_aad, blob_path, open_blob, open_note, open_tombstone, parse_blob_path, seal_blob,
    seal_note, seal_tombstone,
};
pub use merge::{LwwDiff3, MergePlan, MergePolicy, MergeStrategy, Resolution};
pub use engine::{EngineError, FolderVault, NoteFile, SyncReport, SyncState, VaultIo, sync};
pub use target::{FolderTarget, SyncTarget, TargetError};

pub type DeviceId = Uuid;
pub type NoteId = Uuid;

/// Per-item logical clock: one counter per device that ever touched the item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionVector(pub BTreeMap<DeviceId, u64>);

impl VersionVector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Counter last observed for `device` (0 if never).
    pub fn get(&self, device: &DeviceId) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    /// Advance `device`'s counter by one and return the new value.
    pub fn bump(&mut self, device: &DeviceId) -> u64 {
        let entry = self.0.entry(*device).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Record that `device` reached at least `counter`.
    pub fn observe(&mut self, device: &DeviceId, counter: u64) {
        let entry = self.0.entry(*device).or_insert(0);
        *entry = (*entry).max(counter);
    }

    /// Pointwise maximum: after `merge`, `self` dominates both inputs.
    pub fn merge(&mut self, other: &Self) {
        for (device, counter) in &other.0 {
            self.observe(device, *counter);
        }
    }

    /// True when `self` dominates `other` (no counter in `other` is ahead).
    pub fn contains(&self, other: &Self) -> bool {
        other
            .0
            .iter()
            .all(|(device, counter)| self.get(device) >= *counter)
    }

    /// True when neither vector dominates the other (a real conflict).
    pub fn is_concurrent_with(&self, other: &Self) -> bool {
        !self.contains(other) && !other.contains(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devs() -> (DeviceId, DeviceId) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn bump_and_observe() {
        let (a, b) = devs();
        let mut vv = VersionVector::new();
        assert_eq!(vv.bump(&a), 1);
        assert_eq!(vv.bump(&a), 2);
        assert_eq!(vv.bump(&b), 1);
        vv.observe(&a, 1);
        assert_eq!(vv.get(&a), 2, "observe must never go backwards");
        vv.observe(&a, 5);
        assert_eq!(vv.get(&a), 5);
        assert_eq!(vv.get(&Uuid::new_v4()), 0);
    }

    #[test]
    fn merge_takes_pointwise_max() {
        let (a, b) = devs();
        let mut left = VersionVector::new();
        left.observe(&a, 3);
        let mut right = VersionVector::new();
        right.observe(&a, 2);
        right.observe(&b, 7);
        left.merge(&right);
        assert_eq!(left.get(&a), 3);
        assert_eq!(left.get(&b), 7);
        assert!(left.contains(&right));
    }

    #[test]
    fn dominance() {
        let (a, b) = devs();
        let mut v1 = VersionVector::new();
        v1.observe(&a, 2);
        let mut v2 = VersionVector::new();
        v2.observe(&a, 2);
        let mut v3 = VersionVector::new();
        v3.observe(&a, 2);
        v3.observe(&b, 1);
        assert!(v3.contains(&v1));
        assert!(!v1.contains(&v3));
        assert!(
            !v3.is_concurrent_with(&v1),
            "a dominated vector is not concurrent"
        );
        assert!(
            !v1.is_concurrent_with(&v2),
            "equal vectors are not concurrent"
        );
        let mut v4 = VersionVector::new();
        v4.observe(&b, 1);
        assert!(v1.is_concurrent_with(&v4));
        assert!(v4.is_concurrent_with(&v1));
    }

    #[test]
    fn serde_roundtrip() {
        let (a, b) = devs();
        let mut vv = VersionVector::new();
        vv.observe(&a, 1);
        vv.observe(&b, 9);
        let json = serde_json::to_string(&vv).unwrap();
        let back: VersionVector = serde_json::from_str(&json).unwrap();
        assert_eq!(vv, back);
    }
}
