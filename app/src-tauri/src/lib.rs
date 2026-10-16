mod commands;
mod state;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use writan_core::{VaultEvent, VaultWatcher};

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&state_dir)?;
            let state = AppState::new(state_dir);
            state.load_config();
            app.manage(state);
            start_watcher_if_open(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::open_vault,
            commands::list_notes,
            commands::search_notes,
            commands::get_note,
            commands::create_note,
            commands::save_note,
            commands::rename_note,
            commands::delete_note,
            commands::list_trash,
            commands::restore_note,
            commands::purge_note,
            commands::set_pinned,
            commands::rebuild_index,
            commands::list_tags,
            commands::list_folders,
            commands::save_attachment,
            commands::import_attachment,
            commands::get_settings,
            commands::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn start_watcher_if_open(app: &AppHandle) {
    if let Some(root) = app.state::<AppState>().workspace_root() {
        spawn_watcher(app, root);
    }
}

/// Watch the vault folder and notify the frontend about external changes.
pub(crate) fn spawn_watcher(app: &AppHandle, root: PathBuf) {
    let app = app.clone();
    let generation = app
        .state::<AppState>()
        .watcher_generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    std::thread::spawn(move || {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let watcher = match VaultWatcher::spawn(&root) {
            Ok(watcher) => watcher,
            Err(e) => {
                eprintln!("writan: failed to watch {}: {e}", root.display());
                return;
            }
        };
        loop {
            if app
                .state::<AppState>()
                .watcher_generation
                .load(Ordering::SeqCst)
                != generation
            {
                return;
            }
            match watcher.recv_timeout(Duration::from_millis(750)) {
                Ok(events) => {
                    let changed: Vec<String> = {
                        let state = app.state::<AppState>();
                        let mut inner = match state.inner.lock() {
                            Ok(inner) => inner,
                            Err(_) => return,
                        };
                        let Some(workspace) = inner.workspace.as_mut() else {
                            continue;
                        };
                        let rels: Vec<String> = events
                            .iter()
                            .filter_map(|event| {
                                let path = match event {
                                    VaultEvent::Upserted(path) | VaultEvent::Removed(path) => path,
                                };
                                workspace.vault().rel_path(path)
                            })
                            .collect();
                        if rels.is_empty() {
                            Vec::new()
                        } else {
                            let _ = workspace.reconcile_paths(&rels);
                            rels
                        }
                    };
                    if !changed.is_empty() {
                        let _ = app.emit("vault-changed", changed);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}
