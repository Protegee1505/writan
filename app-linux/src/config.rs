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

/// Read a string from `settings.<section>.<key>`; anything missing or malformed
/// yields `None`, so the caller applies the registry's default.
fn setting_str<'a>(
    object: &'a serde_json::Map<String, Value>,
    section: &str,
    key: &str,
) -> Option<&'a str> {
    object.get("settings")?.get(section)?.get(key)?.as_str()
}

/// `settings.general.confirmDelete` — the Tauri registry defaults it to true,
/// so the trash button arms before deleting.
pub fn load_confirm_delete(path: &Path) -> bool {
    setting_bool(&load_object(path), "general", "confirmDelete").unwrap_or(true)
}

/// `settings.general.reopenLastNote` — default true in the Tauri registry, so
/// an untouched `config.json` reopens the last note.
pub fn load_reopen_last_note(path: &Path) -> bool {
    setting_bool(&load_object(path), "general", "reopenLastNote").unwrap_or(true)
}

/// `settings.appearance.theme` — `system` (the registry's default), `light`
/// or `dark`; anything else reads as `system`.
pub fn load_theme(path: &Path) -> String {
    let object = load_object(path);
    match setting_str(&object, "appearance", "theme") {
        Some(theme @ ("light" | "dark")) => theme.to_string(),
        _ => "system".to_string(),
    }
}

/// `settings.appearance.accent` — a preset id; unknown ids fall back to the
/// first preset in `appearance::ACCENTS`.
pub fn load_accent(path: &Path) -> String {
    let object = load_object(path);
    match setting_str(&object, "appearance", "accent") {
        Some(id) => id.to_string(),
        None => "indigo".to_string(),
    }
}

/// `settings.session.lastNoteId` — the note to reopen at startup, if the
/// Tauri shell (or an earlier run) left one.
pub fn load_last_note(path: &Path) -> Option<String> {
    let object = load_object(path);
    object
        .get("settings")?
        .get("session")?
        .get("lastNoteId")?
        .as_str()
        .map(str::to_string)
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

/// Atomically write the object as pretty JSON (temp file + rename), creating
/// the parent directory as needed.
fn write_object(path: &Path, object: serde_json::Map<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let pretty = serde_json::to_string_pretty(&Value::Object(object))?;
    std::fs::write(&temporary, pretty)?;
    std::fs::rename(&temporary, path)
}

/// Record the vault path with an atomic temp + rename write, preserving every
/// other key in the file (`settings`, etc. — ADR-0003).
pub fn save_vault_path(path: &Path, vault: &Path) -> std::io::Result<()> {
    let mut object = load_object(path);
    object.insert(
        "vault_path".to_string(),
        Value::String(vault.display().to_string()),
    );
    write_object(path, object)
}

/// Write `settings.<section>.<key> = value`, preserving every other key
/// (ADR-0003). A malformed `settings` or section value is replaced by an
/// object, so a hand-edited file cannot block a settings change.
pub fn set_setting(path: &Path, section: &str, key: &str, value: Value) -> std::io::Result<()> {
    let mut object = load_object(path);
    let settings = object
        .entry("settings".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !settings.is_object() {
        *settings = Value::Object(serde_json::Map::new());
    }
    let section_value = settings
        .as_object_mut()
        .expect("settings is an object")
        .entry(section.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !section_value.is_object() {
        *section_value = Value::Object(serde_json::Map::new());
    }
    section_value
        .as_object_mut()
        .expect("section is an object")
        .insert(key.to_string(), value);
    write_object(path, object)
}

/// Remove `settings.<section>.<key>` (a no-op when absent), preserving every
/// other key.
pub fn remove_setting(path: &Path, section: &str, key: &str) -> std::io::Result<()> {
    let mut object = load_object(path);
    if let Some(settings) = object.get_mut("settings").and_then(Value::as_object_mut)
        && let Some(section_value) = settings.get_mut(section).and_then(Value::as_object_mut)
    {
        section_value.remove(key);
    }
    write_object(path, object)
}

/// `settings.shortcuts.<id>`: `Some(binding)` rebinds the action, `None`
/// persists an explicit `null` (unbound) — the same shape `mergeSettings`
/// keeps on the Tauri side.
pub fn set_shortcut(path: &Path, id: &str, binding: Option<&str>) -> std::io::Result<()> {
    let value = match binding {
        Some(binding) => Value::String(binding.to_string()),
        None => Value::Null,
    };
    set_setting(path, "shortcuts", id, value)
}

/// Drop the `settings.shortcuts.<id>` override so the default binding returns.
pub fn remove_shortcut(path: &Path, id: &str) -> std::io::Result<()> {
    remove_setting(path, "shortcuts", id)
}

/// Persist `settings.session.lastNoteId` (the same key the Tauri shell's
/// settings store writes), preserving every other key.
pub fn save_last_note(path: &Path, id: &str) -> std::io::Result<()> {
    set_setting(path, "session", "lastNoteId", Value::String(id.to_string()))
}

/// Remove the whole `settings` object: every shell's preferences return to
/// their defaults, while notes and the vault path stay untouched (the Tauri
/// panel's "Reset all settings").
pub fn clear_settings(path: &Path) -> std::io::Result<()> {
    let mut object = load_object(path);
    object.remove("settings");
    write_object(path, object)
}

/// The `sync` object of `config.json` (ADR-0005 §§6–10), the same shape the
/// Tauri shell writes. A missing or malformed object reads as "not set up".
pub fn load_sync(path: &Path) -> writan_sync::SyncConfig {
    load_object(path)
        .remove("sync")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Persist the `sync` object (connect, unlock, turn off), preserving every
/// other key.
pub fn save_sync(path: &Path, sync: &writan_sync::SyncConfig) -> std::io::Result<()> {
    let mut object = load_object(path);
    match serde_json::to_value(sync) {
        Ok(value) => object.insert("sync".to_string(), value),
        Err(_) => object.remove("sync"),
    };
    write_object(path, object)
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
    fn appearance_and_session_read_the_tauri_shapes() {
        let file = temp_file();
        assert_eq!(load_theme(&file), "system");
        assert_eq!(load_accent(&file), "indigo");
        assert!(load_reopen_last_note(&file));
        assert_eq!(load_last_note(&file), None);

        std::fs::write(
            &file,
            r#"{"vault_path":"/notes","settings":{"appearance":{"theme":"dark","accent":"violet"},"general":{"reopenLastNote":false},"session":{"lastNoteId":"abc-123"}}}"#,
        )
        .expect("write");
        assert_eq!(load_theme(&file), "dark");
        assert_eq!(load_accent(&file), "violet");
        assert!(!load_reopen_last_note(&file));
        assert_eq!(load_last_note(&file), Some("abc-123".to_string()));

        // Malformed values fall back to the registry defaults.
        std::fs::write(
            &file,
            r#"{"settings":{"appearance":{"theme":"neon"},"session":{"lastNoteId":7}}}"#,
        )
        .expect("write");
        assert_eq!(load_theme(&file), "system");
        assert_eq!(load_last_note(&file), None);

        // The GTK shell writes the same session key, preserving neighbours.
        std::fs::write(&file, r#"{"vault_path":"/notes"}"#).expect("write");
        save_last_note(&file, "dead-beef").expect("save");
        assert_eq!(load_last_note(&file), Some("dead-beef".to_string()));
        assert_eq!(load_vault_path(&file), Some(PathBuf::from("/notes")));
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

    #[test]
    fn setting_writes_preserve_other_keys_and_round_trip() {
        let file = temp_file();
        std::fs::write(
            &file,
            r#"{"vault_path":"/notes","settings":{"appearance":{"theme":"dark"}}}"#,
        )
        .expect("write fixture");
        set_setting(&file, "editor", "autosaveMs", Value::from(1200)).expect("set");
        set_setting(&file, "view", "showWordCount", Value::Bool(false)).expect("set");
        assert_eq!(load_autosave_ms(&file), 1200);
        assert!(!load_show_word_count(&file));
        let raw = std::fs::read_to_string(&file).expect("read");
        assert!(raw.contains("\"theme\""), "settings must survive: {raw}");
        assert!(raw.contains("/notes"), "vault_path must survive: {raw}");

        remove_setting(&file, "view", "showWordCount").expect("remove");
        assert!(load_show_word_count(&file), "removed key falls back");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn malformed_settings_are_replaced_not_fatal() {
        let file = temp_file();
        std::fs::write(&file, r#"{"settings":7}"#).expect("write fixture");
        set_setting(&file, "general", "confirmDelete", Value::Bool(false)).expect("set");
        assert!(!load_confirm_delete(&file));
        std::fs::write(&file, r#"{"settings":{"view":"yes"}}"#).expect("write fixture");
        set_setting(&file, "view", "dateFormat", Value::from("absolute")).expect("set");
        assert_eq!(load_date_format(&file), crate::dates::DateFormat::Absolute);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn shortcuts_override_null_and_removal_round_trip() {
        use crate::keymap::load_overrides;
        let file = temp_file();
        set_shortcut(&file, "command-palette", Some("Ctrl+Shift+P")).expect("set");
        assert_eq!(
            load_overrides(&file)
                .get("command-palette")
                .map(Option::as_deref),
            Some(Some("Ctrl+Shift+P"))
        );
        set_shortcut(&file, "command-palette", None).expect("unbind");
        assert_eq!(
            load_overrides(&file)
                .get("command-palette")
                .map(Option::as_deref),
            Some(None),
            "null persists as unbound"
        );
        remove_shortcut(&file, "command-palette").expect("remove");
        assert!(load_overrides(&file).is_empty());
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn sync_object_round_trips_and_malformed_reads_as_unset() {
        use writan_sync::{S3ConfigFile, SyncConfig, SyncKind};
        let file = temp_file();
        assert!(!load_sync(&file).enabled);

        let config = SyncConfig {
            kind: SyncKind::S3,
            target: None,
            s3: Some(S3ConfigFile {
                endpoint: "http://127.0.0.1:9000".to_string(),
                region: "us-east-1".to_string(),
                bucket: "notes".to_string(),
                access_key: "key".to_string(),
            }),
            vault_path: Some(PathBuf::from("/notes")),
            enabled: true,
        };
        std::fs::write(
            &file,
            r#"{"vault_path":"/notes","settings":{"editor":{"autosaveMs":1200}}}"#,
        )
        .expect("write fixture");
        save_sync(&file, &config).expect("save");
        let loaded = load_sync(&file);
        assert!(loaded.enabled);
        assert_eq!(loaded.kind, SyncKind::S3);
        assert_eq!(
            loaded.s3.as_ref().map(|s3| s3.bucket.as_str()),
            Some("notes")
        );
        assert_eq!(load_autosave_ms(&file), 1200);

        // A malformed sync object reads as "not set up", like the Tauri shell.
        std::fs::write(&file, r#"{"sync":{"enabled":"yes"}}"#).expect("write");
        assert!(!load_sync(&file).enabled);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn clear_settings_drops_only_settings() {
        let file = temp_file();
        std::fs::write(
            &file,
            r#"{"vault_path":"/notes","settings":{"editor":{"autosaveMs":1200}}}"#,
        )
        .expect("write fixture");
        clear_settings(&file).expect("clear");
        assert_eq!(load_autosave_ms(&file), DEFAULT_AUTOSAVE_MS);
        assert_eq!(load_vault_path(&file), Some(PathBuf::from("/notes")));
        let raw = std::fs::read_to_string(&file).expect("read");
        assert!(!raw.contains("\"settings\""), "settings gone: {raw}");
        std::fs::remove_file(&file).ok();
    }
}
