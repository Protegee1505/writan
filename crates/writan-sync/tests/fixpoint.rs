//! A merge must leave a version that neither device has to merge again.
//!
//! Regression for the soak loop found on 2026-10-19 (the nightly
//! `three_devices_converge_through_a_*` runs stalled at `m=1` per sweep on two
//! devices): when two devices hold the *same* note content under different
//! `updated` stamps and incomparable vectors, `LwwDiff3`'s fast-forward used to
//! keep one side's vector alone, so the version it published never dominated
//! the other device's — the pair re-merged that note on every sweep, forever.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use writan_core::vault::{Note, Vault};
use writan_sync::{FolderTarget, FolderVault, SyncReport, SyncState, sync as run_engine};

const KEY: [u8; 32] = [9u8; 32];
const VAULT_ID: Uuid = Uuid::from_u128(0x51D3_0001);
const BODY: &str = "One shared body.\n";

/// A synthetic, strictly increasing timestamp: `minutes` after a fixed epoch.
fn stamp(minutes: i64) -> String {
    OffsetDateTime::from_unix_timestamp(1_767_225_600 + minutes * 60)
        .expect("valid timestamp")
        .format(&Rfc3339)
        .expect("RFC 3339 formats")
}

struct Device {
    vault: Vault,
    state_path: PathBuf,
    id: Uuid,
}

impl Device {
    fn new(root: &Path, name: &str, seed: u128) -> Self {
        Self {
            vault: Vault::open(root.join(name)).expect("vault opens"),
            state_path: root.join(format!("{name}-state.json")),
            id: Uuid::from_u128(seed),
        }
    }

    fn sync(&self, target: &FolderTarget) -> SyncReport {
        let mut state = SyncState::load(&self.state_path).unwrap_or_default();
        let report = run_engine(
            &KEY,
            self.id,
            VAULT_ID,
            &mut state,
            &FolderVault::new(&self.vault),
            target,
        )
        .expect("sync pass");
        state.save(&self.state_path).expect("state saved");
        report
    }

    fn note(&self) -> Note {
        let rel = self
            .vault
            .walk_notes()
            .expect("walk notes")
            .pop()
            .expect("one note");
        self.vault.read(&rel).expect("read note")
    }

    /// Rewrite the note as an editor would: the body stays as it is, the
    /// `updated` stamp moves on.
    fn edit(&self, updated: &str) {
        let mut note = self.note();
        note.meta.updated = Some(updated.to_string());
        self.vault.write(&note).expect("write note");
    }
}

#[test]
fn a_same_content_merge_reaches_a_fixpoint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let target = FolderTarget::open(root.join("shared")).expect("folder target");
    let (u10, u20, u30) = (stamp(10), stamp(20), stamp(30));

    // A creates the note and publishes it.
    let a = Device::new(root, "a", 0xA110);
    let mut note = a.vault.create("Shared", &[]).expect("create note");
    note.body = BODY.to_string();
    note.meta.updated = Some(u10.clone());
    a.vault.write(&note).expect("write note");
    let report = a.sync(&target);
    assert_eq!(report.published, 1, "the first pass publishes the note");

    // B adopts it, then edits with the *same* body: same content, a newer
    // stamp, and a vector of its own.
    let b = Device::new(root, "b", 0xB220);
    b.sync(&target);
    b.edit(&u20);
    let report = b.sync(&target);
    assert_eq!(report.merged, 0, "B merely published on top");
    assert_eq!(report.published, 1, "B publishes its own stamp");

    // A edits the same body once more: two versions with equal content,
    // different stamps, neither vector covering the other. This is the pair
    // the loop formed on; the merge must claim both vectors.
    a.edit(&u30);
    let report = a.sync(&target);
    assert_eq!(report.merged, 1, "the pair merges once");

    // B must fast-forward to the merged version rather than merge again: with
    // the fixed fast-forward the merged version covers B's vector.
    let report = b.sync(&target);
    assert_eq!(report.merged, 0, "B must not re-merge the same note");
    assert_eq!(report.adopted, 1, "B adopts the merged version");

    // And the fixpoint holds: one more pass on either device is a no-op, with
    // the newer stamp settled on both.
    for device in [&a, &b] {
        assert_eq!(
            device.note().meta.updated.as_deref(),
            Some(u30.as_str()),
            "both devices settle on the newer stamp"
        );
        let report = device.sync(&target);
        assert_eq!(report.merged, 0, "steady state merges nothing");
        assert_eq!(report.published, 0, "steady state publishes nothing");
        assert_eq!(report.adopted, 0, "steady state adopts nothing");
        assert_eq!(report.unchanged, 1, "the note is simply in sync");
    }
}
