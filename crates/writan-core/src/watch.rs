//! Filesystem watcher: raw `notify` events in, debounced vault events out.
//!
//! Editors save in different ways (truncate-in-place, temp file + rename,
//! autosave bursts), so the watcher waits for a quiet period per path and
//! reports only the settled state: file exists → `Upserted`, gone → `Removed`.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// How long a path must be quiet before its event is reported.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// A settled change to a vault file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEvent {
    /// The file exists (created or modified).
    Upserted(PathBuf),
    /// The file no longer exists.
    Removed(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("watcher error: {0}")]
    Notify(#[from] notify::Error),
}

/// A running recursive watch on a vault root.
pub struct VaultWatcher {
    rx: Receiver<Vec<VaultEvent>>,
    _watcher: RecommendedWatcher,
}

impl VaultWatcher {
    /// Watch `root` recursively with the default debounce.
    pub fn spawn(root: &Path) -> Result<Self, WatchError> {
        Self::spawn_with_debounce(root, DEFAULT_DEBOUNCE)
    }

    /// Watch `root` recursively, coalescing changes within `debounce`.
    pub fn spawn_with_debounce(root: &Path, debounce: Duration) -> Result<Self, WatchError> {
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Event>();
        let (event_tx, event_rx) = mpsc::channel::<Vec<VaultEvent>>();

        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    let _ = raw_tx.send(event);
                }
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        let root = root.to_path_buf();
        thread::spawn(move || {
            let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
            loop {
                // Wait for the first relevant event; exit when the watcher drops.
                let Ok(first) = raw_rx.recv() else {
                    return;
                };
                collect(&root, &first, &mut pending);
                // Then keep absorbing until the paths have been quiet for `debounce`.
                loop {
                    match raw_rx.recv_timeout(debounce) {
                        Ok(event) => collect(&root, &event, &mut pending),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                if pending.is_empty() {
                    continue;
                }
                let events: Vec<VaultEvent> = pending
                    .iter()
                    .map(|path| {
                        if path.exists() {
                            VaultEvent::Upserted(path.clone())
                        } else {
                            VaultEvent::Removed(path.clone())
                        }
                    })
                    .collect();
                pending.clear();
                if event_tx.send(events).is_err() {
                    return;
                }
            }
        });

        Ok(Self {
            rx: event_rx,
            _watcher: watcher,
        })
    }

    /// Block up to `timeout` for the next batch of settled events.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Vec<VaultEvent>, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    /// Take the next batch of settled events if one is ready.
    pub fn try_recv(&self) -> Result<Vec<VaultEvent>, TryRecvError> {
        self.rx.try_recv()
    }
}

fn collect(root: &Path, event: &notify::Event, pending: &mut BTreeSet<PathBuf>) {
    // Pure metadata updates (access times, permissions) cannot change content.
    if event.kind.is_access() {
        return;
    }
    for path in &event.paths {
        if is_candidate(root, path) {
            pending.insert(path.clone());
        }
    }
}

/// True if `path` is a Markdown file inside `root`, hidden files excluded.
pub fn is_candidate(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false;
    }
    for component in rel.components() {
        match component {
            Component::Normal(name) => {
                if name.to_string_lossy().starts_with('.') {
                    return false;
                }
            }
            _ => return false,
        }
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn candidates() {
        let root = Path::new("/vault");
        assert!(is_candidate(root, Path::new("/vault/a.md")));
        assert!(is_candidate(root, Path::new("/vault/sub/b.MD")));
        assert!(!is_candidate(root, Path::new("/vault/.hidden.md")));
        assert!(!is_candidate(root, Path::new("/vault/.git/c.md")));
        assert!(!is_candidate(root, Path::new("/vault/note.txt")));
        assert!(!is_candidate(root, Path::new("/other/a.md")));
    }

    #[test]
    fn watcher_reports_create_and_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let watcher = VaultWatcher::spawn_with_debounce(dir.path(), Duration::from_millis(50))
            .expect("watch");
        let file = dir.path().join("note.md");
        fs::write(&file, "# hi").expect("write");
        assert!(wait_for(&watcher, "note.md", true), "no upsert event seen");
        fs::remove_file(&file).expect("remove");
        assert!(
            wait_for(&watcher, "note.md", false),
            "no removal event seen"
        );
    }

    /// Wait until a settled event for `name` with the given existence appears.
    fn wait_for(watcher: &VaultWatcher, name: &str, exists: bool) -> bool {
        for _ in 0..10 {
            let Ok(batch) = watcher.recv_timeout(Duration::from_secs(1)) else {
                return false;
            };
            for event in batch {
                let (path, is_upsert) = match &event {
                    VaultEvent::Upserted(path) => (path, true),
                    VaultEvent::Removed(path) => (path, false),
                };
                if is_upsert == exists && path.file_name().and_then(|n| n.to_str()) == Some(name) {
                    return true;
                }
            }
        }
        false
    }
}
