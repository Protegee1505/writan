use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use writan_core::Workspace;
use writan_sync::SyncState;

use crate::sync::SyncSession;

/// Persisted app configuration (stored next to the index, not in the vault).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub vault_path: Option<PathBuf>,
    /// UI preferences: opaque JSON owned by the frontend settings registry.
    #[serde(default)]
    pub settings: serde_json::Value,
    /// E2EE sync through a shared folder target (docs/adr/0005).
    #[serde(default)]
    pub sync: SyncConfig,
}

/// How and where this device syncs; the vault key and bookkeeping live in the
/// OS keychain and `sync-state.json` (ADR-0005 §§6–10).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SyncConfig {
    /// The shared folder notes sync through.
    pub target: Option<PathBuf>,
    /// The vault this sync was set up for; opening another vault keeps sync
    /// locked rather than mixing notes between vaults.
    pub vault_path: Option<PathBuf>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Default)]
pub struct Inner {
    pub workspace: Option<Workspace>,
    pub config: AppConfig,
    /// The unlocked sync session (vault key + engine state); `None` = locked.
    pub sync: Option<SyncSession>,
}

pub struct AppState {
    pub inner: Mutex<Inner>,
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
    /// Local-only sync bookkeeping (device id, vectors, merge bases).
    pub sync_state_path: PathBuf,
    /// Bumped whenever a new watcher thread takes over, so old ones stop.
    pub watcher_generation: AtomicU64,
}

impl AppState {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            config_path: state_dir.join("config.json"),
            sync_state_path: state_dir.join("sync-state.json"),
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
        if let Some(path) = config.vault_path.as_deref()
            && path.is_dir()
        {
            match Workspace::open(path, &self.state_dir) {
                Ok(mut workspace) => {
                    let _ = workspace.reconcile();
                    inner.workspace = Some(workspace);
                }
                Err(e) => eprintln!("writan: failed to open vault {}: {e}", path.display()),
            }
        }
        inner.sync = self.auto_unlock_sync(&config);
    }

    /// Restore an enabled sync session from the OS keychain, if possible.
    /// Failing just leaves sync locked; the UI asks for the password.
    fn auto_unlock_sync(&self, config: &AppConfig) -> Option<SyncSession> {
        if !config.sync.enabled {
            return None;
        }
        let state = SyncState::load(&self.sync_state_path).ok()?;
        let vault_id = state.vault_id?;
        let device = state.device?;
        let key = crate::sync::recall_vault_key(vault_id)?;
        Some(SyncSession {
            vault_id,
            device,
            vault_key: zeroize::Zeroizing::new(key),
            state,
            last_sync: None,
            last_report: None,
        })
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

    pub(crate) fn save_config(&self, inner: &Inner) {
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

    #[test]
    fn sync_config_roundtrips_and_stays_locked() {
        let (state, dir) = temp_state();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            r#"{ "vault_path": null, "sync": { "enabled": true, "target": "C:/sync-notes" } }"#,
        )
        .unwrap();
        state.load_config();
        let inner = state.inner.lock().unwrap();
        assert!(inner.config.sync.enabled);
        assert_eq!(
            inner.config.sync.target.as_deref(),
            Some(Path::new("C:/sync-notes"))
        );
        assert!(inner.config.sync.vault_path.is_none());
        // No keychain entry and no state file: sync stays locked, no error.
        assert!(inner.sync.is_none());
        drop(inner);
        let _ = fs::remove_dir_all(&dir);
    }
}
