//! End-to-end tests for the workspace: vault files, index, and trash together.

use std::fs;

use writan_core::{SearchQuery, Workspace};

fn workspace() -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
    let vault = tempfile::tempdir().expect("vault dir");
    let state = tempfile::tempdir().expect("state dir");
    let ws = Workspace::open(vault.path(), state.path()).expect("workspace");
    (vault, state, ws)
}

#[test]
fn create_reconcile_and_search() {
    let (_v, _s, mut ws) = workspace();
    let note = ws
        .create_note("Shopping List", &["errands".to_string()])
        .expect("create");
    let hits = ws
        .search(&SearchQuery {
            text: "Shopping".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, note.meta.id);
    let stats = ws.reconcile().expect("reconcile");
    assert_eq!(stats.unchanged, 1);
    assert_eq!(ws.list(10).expect("list").len(), 1);
}

#[test]
fn external_edit_is_picked_up() {
    let (_v, _s, mut ws) = workspace();
    let note = ws.create_note("Original", &[]).expect("create");
    let abs = ws.vault().abs_path(&note.rel_path);
    let text = fs::read_to_string(&abs).expect("read");
    fs::write(&abs, format!("{text}\nnew words here")).expect("write");
    let stats = ws.reconcile_one(&note.rel_path).expect("reconcile");
    assert_eq!(stats.updated, 1);
    let hits = ws
        .search(&SearchQuery {
            text: "new words".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
}

#[test]
fn external_removal_is_picked_up() {
    let (_v, _s, mut ws) = workspace();
    let note = ws.create_note("Doomed", &[]).expect("create");
    fs::remove_file(ws.vault().abs_path(&note.rel_path)).expect("remove");
    let stats = ws.reconcile_one(&note.rel_path).expect("reconcile");
    assert_eq!(stats.removed, 1);
    assert!(ws.list(10).expect("list").is_empty());
}

#[test]
fn adoption_gives_plain_files_an_id() {
    let (vault_dir, _s, mut ws) = workspace();
    fs::write(
        vault_dir.path().join("plain note.md"),
        "# Plain\n\nbody text",
    )
    .expect("write");
    let stats = ws.reconcile().expect("reconcile");
    assert_eq!(stats.added, 1);
    let listed = ws.list(10).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Plain");
    let text = fs::read_to_string(vault_dir.path().join("plain note.md")).expect("read");
    assert!(text.starts_with("---\n"), "{text}");
    assert!(text.contains("id:"), "{text}");
    // Second pass changes nothing.
    let stats = ws.reconcile().expect("reconcile");
    assert_eq!(stats.unchanged, 1);
    assert_eq!(stats.added, 0);
}

#[test]
fn delete_restore_roundtrip() {
    let (_v, _s, mut ws) = workspace();
    let note = ws.create_note("Temp", &[]).expect("create");
    let entry = ws.delete_note(&note.meta.id).expect("delete");
    assert!(!ws.vault().abs_path(&note.rel_path).exists());
    assert!(
        ws.search(&SearchQuery {
            text: "Temp".into(),
            ..Default::default()
        })
        .expect("search")
        .is_empty()
    );
    let restored = ws.restore_note(&entry.id).expect("restore");
    assert_eq!(restored.meta.id, note.meta.id);
    assert!(ws.vault().abs_path(&restored.rel_path).exists());
    let hits = ws
        .search(&SearchQuery {
            text: "Temp".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
}

#[test]
fn pin_and_filter() {
    let (_v, _s, mut ws) = workspace();
    let a = ws.create_note("Alpha", &[]).expect("a");
    ws.create_note("Beta", &[]).expect("b");
    let pinned = ws.set_pinned(&a.meta.id, true).expect("pin");
    assert!(pinned.meta.pinned);
    let hits = ws
        .search(&SearchQuery {
            pinned_only: true,
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Alpha");
}

#[test]
fn set_tags_sanitizes_and_reaches_file_and_index() {
    let (_v, _s, mut ws) = workspace();
    let note = ws.create_note("Tagged", &[]).expect("create");
    let updated = ws
        .set_tags(
            &note.meta.id,
            vec![
                " #dev ".to_string(),
                "dev".to_string(),
                String::new(),
                "ops".to_string(),
                "#".to_string(),
            ],
        )
        .expect("set_tags");
    assert_eq!(updated.meta.tags, vec!["dev", "ops"]);
    // The file is the source of truth: frontmatter carries the cleaned tags.
    let text = fs::read_to_string(ws.vault().abs_path(&note.rel_path)).expect("read");
    assert!(text.contains("- dev"), "{text}");
    assert!(text.contains("- ops"), "{text}");
    // A body-only update keeps the tags.
    let edited = ws
        .update_note(&note.meta.id, Some("fresh words"), None)
        .expect("update body");
    assert_eq!(edited.body.trim(), "fresh words");
    assert_eq!(edited.meta.tags, vec!["dev", "ops"]);
    let hits = ws
        .search(&SearchQuery {
            tags: vec!["ops".to_string()],
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, note.meta.id);
}

#[test]
fn rename_note_moves_file_and_updates_index() {
    let (_v, _s, mut ws) = workspace();
    let mut note = ws.create_note("Old Name", &[]).expect("create");
    let old_rel = note.rel_path.clone();
    ws.rename_note(&mut note, "New Name").expect("rename");
    assert_ne!(note.rel_path, old_rel);
    assert!(!ws.vault().abs_path(&old_rel).exists());
    assert!(ws.vault().abs_path(&note.rel_path).exists());
    let hits = ws
        .search(&SearchQuery {
            text: "New Name".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
    let stale = ws
        .search(&SearchQuery {
            text: "Old Name".into(),
            ..Default::default()
        })
        .expect("search");
    assert!(stale.is_empty(), "{stale:?}");
}

#[test]
fn rebuild_index_matches_files() {
    let (_v, _s, mut ws) = workspace();
    ws.create_note("One", &[]).expect("1");
    ws.create_note("Two", &[]).expect("2");
    let stats = ws.rebuild_index().expect("rebuild");
    assert_eq!(stats.added, 2);
    assert_eq!(ws.list(10).expect("list").len(), 2);
}

#[test]
fn save_note_updates_updated_and_index() {
    let (_v, _s, mut ws) = workspace();
    let mut note = ws.create_note("Draft", &[]).expect("create");
    let first_updated = note.meta.updated.clone();
    note.body = "rewritten content".to_string();
    ws.save_note(&mut note).expect("save");
    assert!(note.meta.updated.is_some());
    assert_ne!(note.meta.updated, first_updated);
    let hits = ws
        .search(&SearchQuery {
            text: "rewritten".into(),
            ..Default::default()
        })
        .expect("search");
    assert_eq!(hits.len(), 1);
}

#[test]
fn facets_reports_tags_and_folders() {
    let (v, _s, mut ws) = workspace();
    ws.create_note("Alpha", &["work".to_string(), "rust".to_string()])
        .expect("a");
    ws.create_note("Beta", &["work".to_string()]).expect("b");
    fs::create_dir_all(v.path().join("projects")).expect("mkdir");
    fs::write(v.path().join("projects/external.md"), "# External\n\nbody").expect("write");
    ws.reconcile().expect("reconcile");

    let tags = ws.tag_counts().expect("tags");
    assert_eq!(
        tags.iter().find(|(t, _)| t == "work").map(|(_, n)| *n),
        Some(2)
    );
    assert_eq!(
        tags.iter().find(|(t, _)| t == "rust").map(|(_, n)| *n),
        Some(1)
    );

    let folders = ws.folder_counts().expect("folders");
    assert_eq!(folders, vec![("projects".to_string(), 1)]);
}
