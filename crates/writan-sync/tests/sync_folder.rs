//! Two vaults, one folder target: edits, deletions and conflicts converge.
//!
//! Each [`End`] is one device — a vault, a local [`SyncState`] file and the
//! shared target. The engine's storage rules (ADR-0005) are exercised
//! end-to-end: publish, adopt, tombstone, resurrect, bounded diff3 and the
//! explicit conflict copy.

use std::path::{Path, PathBuf};

use uuid::Uuid;
use writan_core::vault::{Note, Vault};
use writan_sync::{FolderTarget, FolderVault, SyncReport, SyncState, sync};

const KEY: [u8; 32] = [7u8; 32];

fn vault_id() -> Uuid {
    Uuid::from_u128(0x5eed)
}

struct End {
    device: Uuid,
    vault: Vault,
    state_path: PathBuf,
    target: FolderTarget,
}

fn end(root: &Path, name: &str, device: Uuid) -> End {
    let vault = Vault::open(root.join(name)).expect("vault");
    let target = FolderTarget::open(root.join("target")).expect("target");
    End {
        device,
        vault,
        state_path: root.join(format!("{name}.state.json")),
        target,
    }
}

impl End {
    /// One sync pass, persisting the local state.
    fn sync(&self) -> SyncReport {
        let mut state = SyncState::load(&self.state_path).expect("state loads");
        let io = FolderVault::new(&self.vault);
        let report =
            sync(&KEY, self.device, vault_id(), &mut state, &io, &self.target).expect("sync pass");
        state.save(&self.state_path).expect("state saves");
        report
    }

    fn notes(&self) -> Vec<Note> {
        self.vault
            .walk_notes()
            .expect("walk")
            .iter()
            .map(|rel| self.vault.read(rel).expect("read"))
            .collect()
    }

    fn note(&self, title: &str) -> Option<Note> {
        self.notes().into_iter().find(|n| n.meta.title == title)
    }

    fn body_of(&self, title: &str) -> Option<String> {
        self.note(title).map(|n| n.body)
    }

    fn edit(&self, title: &str, body: &str, updated: &str) {
        let mut note = self.note(title).expect("note to edit");
        note.body = body.to_string();
        note.meta.updated = Some(updated.to_string());
        self.vault.write(&note).expect("write");
    }

    fn delete(&self, title: &str) {
        let note = self.note(title).expect("note to delete");
        self.vault.remove(&note.rel_path).expect("remove");
    }

    fn conflict_copy(&self) -> Option<Note> {
        self.notes()
            .into_iter()
            .find(|n| n.meta.title.contains("(conflict copy"))
    }
}

#[test]
fn create_edit_and_adopt_propagate() {
    let root = tempfile::tempdir().expect("tempdir");
    let a = end(root.path(), "a", Uuid::from_u128(1));
    let b = end(root.path(), "b", Uuid::from_u128(2));

    a.vault.create("Alpha", &[]).expect("create");
    a.edit("Alpha", "first\n", "2026-03-01T10:00:00Z");
    assert_eq!(a.sync().published, 1);

    let report = b.sync();
    assert_eq!(report.adopted, 1);
    assert_eq!(b.body_of("Alpha").as_deref(), Some("first\n"));

    // B edits on top of the adopted version and publishes; A adopts back.
    b.edit("Alpha", "second\n", "2026-03-02T10:00:00Z");
    assert_eq!(b.sync().published, 1);

    let report = a.sync();
    assert_eq!(report.adopted, 1);
    assert_eq!(a.body_of("Alpha").as_deref(), Some("second\n"));

    // Both sides idle: nothing left to reconcile.
    assert_eq!(a.sync().unchanged, 1);
    assert_eq!(b.sync().unchanged, 1);
}

#[test]
fn deletion_propagates_and_an_edit_resurrects() {
    let root = tempfile::tempdir().expect("tempdir");
    let a = end(root.path(), "a", Uuid::from_u128(1));
    let b = end(root.path(), "b", Uuid::from_u128(2));

    a.vault.create("Doomed", &[]).expect("create");
    assert_eq!(a.sync().published, 1);
    assert_eq!(b.sync().adopted, 1);

    // A deletes and publishes the tombstone.
    a.delete("Doomed");
    assert_eq!(a.sync().deleted, 1);
    assert!(a.notes().is_empty());

    // B edits before it ever hears about the deletion: the edit resurrects
    // the note with a version that dominates the tombstone.
    b.edit("Doomed", "edited after delete\n", "2026-02-01T10:00:00Z");
    let report = b.sync();
    assert_eq!(report.published, 1, "an edit after the delete resurrects");
    assert_eq!(
        b.body_of("Doomed").as_deref(),
        Some("edited after delete\n")
    );

    // A adopts the resurrected note; nothing is deleted a second time.
    let report = a.sync();
    assert_eq!(report.adopted, 1);
    assert_eq!(report.deleted, 0);
    assert_eq!(
        a.body_of("Doomed").as_deref(),
        Some("edited after delete\n")
    );

    assert_eq!(b.sync().unchanged, 1);
}

#[test]
fn disjoint_edits_merge_through_diff3() {
    let root = tempfile::tempdir().expect("tempdir");
    let a = end(root.path(), "a", Uuid::from_u128(1));
    let b = end(root.path(), "b", Uuid::from_u128(2));

    a.vault.create("Fight", &[]).expect("create");
    a.edit("Fight", "one\ntwo\nthree\n", "2026-01-01T10:00:00Z");
    assert_eq!(a.sync().published, 1);
    assert_eq!(b.sync().adopted, 1);

    a.edit("Fight", "ONE\ntwo\nthree\n", "2026-01-05T10:00:00Z");
    assert_eq!(a.sync().published, 1);

    // B edits a different line from the version it still holds.
    b.edit("Fight", "one\ntwo\nTHREE\n", "2026-01-06T10:00:00Z");
    let report = b.sync();
    assert_eq!(report.merged, 1, "concurrent edits go through the diff3");
    assert_eq!(report.conflict_copies, 0);
    assert_eq!(b.body_of("Fight").as_deref(), Some("ONE\ntwo\nTHREE\n"));

    let report = a.sync();
    assert_eq!(report.adopted, 1);
    assert_eq!(a.body_of("Fight").as_deref(), Some("ONE\ntwo\nTHREE\n"));
    assert_eq!(a.sync().unchanged, 1);
    assert_eq!(b.sync().unchanged, 1);
}

#[test]
fn same_line_edits_keep_an_explicit_conflict_copy() {
    // Temporary diagnostics: `list` comes from the `SyncTarget` trait.
    use writan_sync::SyncTarget;

    let root = tempfile::tempdir().expect("tempdir");
    let a = end(root.path(), "a", Uuid::from_u128(1));
    let b = end(root.path(), "b", Uuid::from_u128(2));

    a.vault.create("Same", &[]).expect("create");
    a.edit("Same", "alpha\nbeta\ngamma\n", "2026-01-01T10:00:00Z");
    assert_eq!(a.sync().published, 1);
    assert_eq!(b.sync().adopted, 1);

    a.edit("Same", "alpha\nbeta-a\ngamma\n", "2026-01-06T10:00:00Z");
    assert_eq!(a.sync().published, 1);

    // Both sides touched the same line; the later edit keeps the note and the
    // earlier one is materialised as an explicit conflict copy.
    b.edit("Same", "alpha\nbeta-b\ngamma\n", "2026-01-07T10:00:00Z");
    let report = b.sync();
    assert_eq!(report.merged, 1);
    assert_eq!(report.conflict_copies, 1);
    assert_eq!(b.body_of("Same").as_deref(), Some("alpha\nbeta-b\ngamma\n"));
    let copy = b.conflict_copy().expect("copy materialised on B");
    assert_eq!(copy.body, "alpha\nbeta-a\ngamma\n");
    assert!(copy.meta.title.contains("(conflict copy 2026-01-06)"));
    eprintln!("B report: {report:?}");
    eprintln!("B copy id: {:?}", copy.meta.id);
    eprintln!("B target: {:?}", b.target.list().expect("list"));

    // A first takes the kept version, then adopts the copy on the next pass.
    let report = a.sync();
    eprintln!("A pass1 report: {report:?}");
    eprintln!(
        "A pass1 notes: {:?}",
        a.notes()
            .iter()
            .map(|note| (note.meta.title.clone(), note.meta.id))
            .collect::<Vec<_>>()
    );
    eprintln!("A pass1 target: {:?}", a.target.list().expect("list"));
    eprintln!(
        "A pass1 state: {:?}",
        SyncState::load(&a.state_path).expect("state")
    );
    assert_eq!(report.adopted, 1);
    assert_eq!(a.body_of("Same").as_deref(), Some("alpha\nbeta-b\ngamma\n"));
    let report = a.sync();
    eprintln!("A pass2 report: {report:?}");
    eprintln!(
        "A pass2 notes: {:?}",
        a.notes()
            .iter()
            .map(|note| (note.meta.title.clone(), note.meta.id))
            .collect::<Vec<_>>()
    );
    eprintln!("A pass2 target: {:?}", a.target.list().expect("list"));
    assert_eq!(report.adopted, 1);
    let copy = a.conflict_copy().expect("copy materialised on A");
    assert_eq!(copy.body, "alpha\nbeta-a\ngamma\n");

    // Converged: no side has anything left to do.
    let report = a.sync();
    assert_eq!(report.unchanged, 2);
    assert_eq!(
        report.published + report.merged + report.adopted + report.conflict_copies + report.deleted,
        0
    );
    assert_eq!(b.sync().unchanged, 2);
}
