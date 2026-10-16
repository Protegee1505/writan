//! App-data config for the GTK shell (same shape as the Tauri app: the
//! `config.json` owned by ADR-0003, with `vault_path` and an opaque
//! `settings` object). GTK-free so it is testable on any target.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// `$XDG_CONFIG_HOME/com.writan.app` (default `~/.config/com.writan.app`).
pub fn config_dir() -> PathBuf {
    base_dir("XDG_CONFIG_HOME", ".config").join("com.writan.app")
}

/// `$XDG_DATA_HOME/com.writan.app` (default `~/.local/share/com.writan.app`).
/// Index and trash live here, never in the vault (ADR-0003).
pub fn state_dir() -> PathBuf {
    base_dir("XDG_DATA_HOME", ".local/share").join("com.writan.app")
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn base_dir(env: &str, fallback: &str) -> PathBuf {
    let from_env = std::env::var_os(env)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    from_env
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Read `config.json` as a JSON object; a missing or malformed file yields an
/// empty object (defaults win, ADR-0003).
pub fn load_object(path: &Path) -> serde_json::Map<String, Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// The vault chosen last time, if any.
pub fn load_vault_path(path: &Path) -> Option<PathBuf> {
    load_object(path)
        .get("vault_path")?
        .as_str()
        .map(PathBuf::from)
}

/// Record the vault path with an atomic temp + rename write, preserving every
/// other key in the file (`settings`, etc. — ADR-0003).
pub fn save_vault_path(path: &Path, vault: &Path) -> std::io::Result<()> {
    let mut object = load_object(path);
    object.insert(
        "vault_path".to_string(),
        Value::String(vault.display().to_string()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let pretty = serde_json::to_string_pretty(&Value::Object(object))?;
    std::fs::write(&temporary, pretty)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "writan-linux-config-{}-{nanos}.json",
            std::process::id()
        ))
    }

    #[test]
    fn vault_path_roundtrip_preserves_other_keys() {
        let file = temp_file();
        std::fs::write(
            &file,
            r#"{"settings":{"appearance":{"theme":"dark"}},"vault_path":"/old"}"#,
        )
        .expect("write fixture");
        save_vault_path(&file, Path::new("/notes")).expect("save");
        assert_eq!(load_vault_path(&file), Some(PathBuf::from("/notes")));
        let raw = std::fs::read_to_string(&file).expect("read");
        assert!(raw.contains("\"theme\""), "settings must survive: {raw}");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn missing_file_yields_none_and_malformed_yields_defaults() {
        assert_eq!(load_vault_path(&temp_file()), None);
        let file = temp_file();
        std::fs::write(&file, "not json").expect("write fixture");
        assert!(load_object(&file).is_empty());
        std::fs::remove_file(&file).ok();
    }
}
