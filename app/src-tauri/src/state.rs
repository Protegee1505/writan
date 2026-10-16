use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use writan_core::Workspace;

/// Persisted app configuration (stored next to the index, not in the vault).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub vault_path: Option<PathBuf>,
    /// UI preferences: opaque JSON owned by the frontend settings registry.
    #[serde(default)]
    pub settings: serde_json::Value,
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

    /// The persisted UI preferences (`{}` when nothing was saved yet).
    pub fn settings(&self) -> serde_json::Value {
        match self.inner.lock() {
            Ok(inner) => {
                if inner.config.settings.is_null() {
                    serde_json::json!({})
                } else {
                    inner.config.settings.clone()
                }
            }
            Err(_) => serde_json::json!({}),
        }
    }

    /// Replace the UI preferences. Must be a JSON object, size-capped.
    pub fn save_settings(&self, value: serde_json::Value) -> Result<(), String> {
        if !value.is_object() {
            return Err("settings must be a JSON object".to_string());
        }
        if value.to_string().len() > 256 * 1024 {
            return Err("settings payload too large".to_string());
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        inner.config.settings = value;
        self.save_config(&inner);
        Ok(())
    }

    fn save_config(&self, inner: &Inner) {
        let Ok(json) = serde_json::to_string_pretty(&inner.config) else {
            return;
        };
        let _ = fs::create_dir_all(&self.state_dir);
        // Write a temp file first so a crash mid-write cannot corrupt config.json.
        let tmp = self.config_path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &self.config_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state() -> (AppState, PathBuf) {
        let dir = std::env::temp_dir().join(format!("writan-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (AppState::new(dir.clone()), dir)
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let (state, dir) = temp_state();
        assert_eq!(state.settings(), serde_json::json!({}));
        let value = serde_json::json!({
            "appearance": { "theme": "system", "accent": "indigo" },
            "shortcuts": { "command-palette": "Ctrl+P" },
        });
        state.save_settings(value.clone()).unwrap();
        assert_eq!(state.settings(), value);
        let reloaded = AppState::new(dir.clone());
        reloaded.load_config();
        assert_eq!(reloaded.settings(), value);
        assert!(state.save_settings(serde_json::json!(5)).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_without_settings_key_still_loads() {
        let (state, dir) = temp_state();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), r#"{ "vault_path": null }"#).unwrap();
        state.load_config();
        assert_eq!(state.settings(), serde_json::json!({}));
        let _ = fs::remove_dir_all(&dir);
    }
}
