use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use writan_core::Workspace;

/// Persisted app configuration (stored next to the index, not in the vault).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub vault_path: Option<PathBuf>,
}

#[derive(Default)]
pub struct Inner {
    pub workspace: Option<Workspace>,
    pub config: AppConfig,
}

pub struct AppState {
    pub inner: Mutex<Inner>,
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
    /// Bumped whenever a new watcher thread takes over, so old ones stop.
    pub watcher_generation: AtomicU64,
}

impl AppState {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            config_path: state_dir.join("config.json"),
            state_dir,
            inner: Mutex::new(Inner::default()),
            watcher_generation: AtomicU64::new(0),
        }
    }

    /// Read the config file and reopen the last vault, if any.
    pub fn load_config(&self) {
        let Ok(raw) = fs::read_to_string(&self.config_path) else {
            return;
        };
        let Ok(config) = serde_json::from_str::<AppConfig>(&raw) else {
            return;
        };
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return,
        };
        inner.config = config.clone();
        if let Some(path) = config.vault_path
            && path.is_dir()
        {
            match Workspace::open(&path, &self.state_dir) {
                Ok(mut workspace) => {
                    let _ = workspace.reconcile();
                    inner.workspace = Some(workspace);
                }
                Err(e) => eprintln!("writan: failed to open vault {}: {e}", path.display()),
            }
        }
    }

    pub fn workspace_root(&self) -> Option<PathBuf> {
        let inner = self.inner.lock().ok()?;
        inner
            .workspace
            .as_ref()
            .map(|workspace| workspace.vault().root().to_path_buf())
    }

    /// Open (or create) a vault folder and make it the current one.
    pub fn open_vault(&self, path: &Path) -> Result<(), String> {
        fs::create_dir_all(path).map_err(|e| format!("failed to create vault folder: {e}"))?;
        let mut workspace = Workspace::open(path, &self.state_dir).map_err(|e| e.to_string())?;
        workspace.reconcile().map_err(|e| e.to_string())?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        inner.workspace = Some(workspace);
        inner.config.vault_path = Some(path.to_path_buf());
        self.save_config(&inner);
        Ok(())
    }

    fn save_config(&self, inner: &Inner) {
        if let Ok(json) = serde_json::to_string_pretty(&inner.config) {
            let _ = fs::write(&self.config_path, json);
        }
    }
}
