//! End-to-end backup tests: a workspace produces the archive, a second
//! workspace imports it and can search what arrived.

use std::fs;

use writan_core::{SearchQuery, Workspace, backup};

fn workspace() -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
    let vault = tempfile::tempdir().expect("vault dir");
    let state = tempfile::tempdir().expect("state dir");
    let ws = Workspace::open(vault.path(), state.path()).expect("workspace");
    (vault, state, ws)
}

#[test]
fn a_backup_moves_a_vault_to_another_machine() {
    let (source_dir, _source_state, mut source) = workspace();
    let mut note = source
        .create_note("Trip plan", &["travel".to_string()])
        .expect("create");
    note.body = "# Trip\n\nagenda: museums and noodles\n".to_string();
    source.save_note(&mut note).expect("save");
    fs::create_dir_all(source_dir.path().join("attachments")).expect("attachments");
    fs::write(source_dir.path().join("attachments/map.png"), b"png").expect("asset");

    let out = tempfile::tempdir().expect("out");
    let archive = out.path().join("trip.zip");
    let export = backup::export_archive(source_dir.path(), &archive).expect("export");
    assert_eq!(export.notes, 1);
    assert_eq!(export.attachments, 1);

    let (target_dir, _target_state, mut target) = workspace();
    let import = backup::import_archive(target_dir.path(), &archive).expect("import");
    assert_eq!(import.notes, 1);
    assert_eq!(import.attachments, 1);
    assert_eq!(import.renamed, 0);

    let stats = target.reconcile().expect("reconcile");
    assert_eq!(stats.added, 1);
    let hits = target
        .search(&SearchQuery {
            text: "noodles".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Trip plan");
    assert_eq!(
        fs::read(target_dir.path().join("attachments/map.png")).expect("asset"),
        b"png"
    );
}

#[test]
fn a_folder_import_adopts_plain_markdown() {
    let source = tempfile::tempdir().expect("source");
    fs::create_dir_all(source.path().join("journal")).expect("dir");
    fs::write(
        source.path().join("journal/day one.md"),
        "# Day one\n\nplain words\n",
    )
    .expect("write");
    let (target_dir, _state, mut target) = workspace();

    let import = backup::import_folder(target_dir.path(), source.path()).expect("import");
    assert_eq!(import.notes, 1);
    assert_eq!(import.renamed, 0);

    target.reconcile().expect("reconcile");
    let notes = target.list(10).expect("list");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Day one");
    let text = fs::read_to_string(target_dir.path().join(&notes[0].rel_path)).expect("read");
    assert!(text.starts_with("---\n"), "{text}");
    assert!(text.contains("id:"), "{text}");
}
