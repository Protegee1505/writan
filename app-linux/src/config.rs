//! App-data config for the GTK shell (same shape as the Tauri app: the
//! `config.json` owned by ADR-0003, with `vault_path` and an opaque
//! `settings` object). GTK-free so it is testable on any target.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// `$XDG_CONFIG_HOME/com.writan.desktop` (default `~/.config/com.writan.desktop`).
pub fn config_dir() -> PathBuf {
    base_dir("XDG_CONFIG_HOME", ".config").join("com.writan.desktop")
}

/// `$XDG_DATA_HOME/com.writan.desktop` (default `~/.local/share/com.writan.desktop`).
/// Index and trash live here, never in the vault (ADR-0003).
pub fn state_dir() -> PathBuf {
    base_dir("XDG_DATA_HOME", ".local/share").join("com.writan.desktop")
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

/// Default autosave debounce, matching the Tauri settings registry's 700 ms.
pub const DEFAULT_AUTOSAVE_MS: u64 = 700;

/// The autosave delay from `settings.editor.autosaveMs`. Defensively clamped
/// (100–5000 ms) so a hand-edited config cannot stall or spin saves; anything
/// malformed falls back to the default.
pub fn load_autosave_ms(path: &Path) -> u64 {
    load_object(path)
        .get("settings")
        .and_then(|settings| settings.get("editor"))
        .and_then(|editor| editor.get("autosaveMs"))
        .and_then(Value::as_u64)
        .map(|ms| ms.clamp(100, 5000))
        .unwrap_or(DEFAULT_AUTOSAVE_MS)
}

/// Read a boolean from `settings.<section>.<key>`; anything missing or malformed
/// falls back to the settings registry's default (ADR-0003).
fn setting_bool(object: &serde_json::Map<String, Value>, section: &str, key: &str) -> Option<bool> {
    object.get("settings")?.get(section)?.get(key)?.as_bool()
}

/// `settings.general.confirmDelete` — the Tauri registry defaults it to true,
/// so the trash button arms before deleting.
pub fn load_confirm_delete(path: &Path) -> bool {
    setting_bool(&load_object(path), "general", "confirmDelete").unwrap_or(true)
}

/// `settings.view.showWordCount` — default true in the Tauri registry.
pub fn load_show_word_count(path: &Path) -> bool {
    setting_bool(&load_object(path), "view", "showWordCount").unwrap_or(true)
}

/// `settings.view.dateFormat` — the Tauri registry defaults it to `relative`.
pub fn load_date_format(path: &Path) -> crate::dates::DateFormat {
    let object = load_object(path);
    let raw = object
        .get("settings")
        .and_then(|settings| settings.get("view"))
        .and_then(|view| view.get("dateFormat"))
        .and_then(Value::as_str);
    match raw {
        Some("absolute") => crate::dates::DateFormat::Absolute,
        _ => crate::dates::DateFormat::Relative,
    }
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
    fn autosave_defaults_and_clamps() {
        let file = temp_file();
        assert_eq!(load_autosave_ms(&file), DEFAULT_AUTOSAVE_MS);
        std::fs::write(&file, r#"{"settings":{"editor":{"autosaveMs":1000}}}"#).expect("write");
        assert_eq!(load_autosave_ms(&file), 1000);
        std::fs::write(&file, r#"{"settings":{"editor":{"autosaveMs":50}}}"#).expect("write");
        assert_eq!(load_autosave_ms(&file), 100);
        std::fs::write(&file, r#"{"settings":{"editor":{"autosaveMs":"soon"}}}"#).expect("write");
        assert_eq!(load_autosave_ms(&file), DEFAULT_AUTOSAVE_MS);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn display_settings_default_true_and_read_config() {
        let file = temp_file();
        assert!(load_confirm_delete(&file));
        assert!(load_show_word_count(&file));
        std::fs::write(
            &file,
            r#"{"settings":{"general":{"confirmDelete":false},"view":{"showWordCount":false}}}"#,
        )
        .expect("write");
        assert!(!load_confirm_delete(&file));
        assert!(!load_show_word_count(&file));
        std::fs::write(
            &file,
            r#"{"settings":{"general":{},"view":{"showWordCount":"yes"}}}"#,
        )
        .expect("write");
        assert!(
            load_confirm_delete(&file),
            "missing key falls back to default"
        );
        assert!(load_show_word_count(&file), "malformed value falls back");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn date_format_defaults_to_relative_and_reads_absolute() {
        use crate::dates::DateFormat;
        let file = temp_file();
        assert_eq!(load_date_format(&file), DateFormat::Relative);
        std::fs::write(&file, r#"{"settings":{"view":{"dateFormat":"absolute"}}}"#).expect("write");
        assert_eq!(load_date_format(&file), DateFormat::Absolute);
        std::fs::write(&file, r#"{"settings":{"view":{"dateFormat":"fancy"}}}"#).expect("write");
        assert_eq!(load_date_format(&file), DateFormat::Relative);
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
