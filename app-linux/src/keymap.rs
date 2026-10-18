//! Global keyboard shortcuts for the GTK shell: the same action table as the
//! Tauri keymap (`app/src/lib/keymap.ts`), with user overrides from
//! `settings.shortcuts` in the app-data `config.json` (ADR-0003).
//!
//! GTK-free on purpose: `app.rs` turns these bindings into GTK accelerators,
//! everything else (defaults, overrides, reserved keys, conversion) is
//! unit-tested on every target — including a drift guard that parses
//! `app/src/lib/keymap.ts` itself, so the two shells cannot diverge silently.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// A global action; field for field what `KeyAction` holds in keymap.ts.
pub struct KeyAction {
    pub id: &'static str,
    pub label: &'static str,
    /// Canonical binding ("Ctrl+P"); `None` = unbound by default.
    pub default_binding: Option<&'static str>,
    /// Extra bindings that always work but cannot be rebound.
    pub aliases: &'static [&'static str],
}

pub const KEY_ACTIONS: &[KeyAction] = &[
    KeyAction {
        id: "command-palette",
        label: "Command palette",
        default_binding: Some("Ctrl+P"),
        aliases: &["Ctrl+K"],
    },
    KeyAction {
        id: "new-note",
        label: "New note",
        default_binding: Some("Ctrl+N"),
        aliases: &[],
    },
    KeyAction {
        id: "focus-search",
        label: "Find in notes",
        default_binding: Some("Ctrl+F"),
        aliases: &[],
    },
    KeyAction {
        id: "save-note",
        label: "Save note",
        default_binding: Some("Ctrl+S"),
        aliases: &[],
    },
    KeyAction {
        id: "open-settings",
        label: "Open settings",
        default_binding: Some("Ctrl+,"),
        aliases: &[],
    },
    KeyAction {
        id: "toggle-theme",
        label: "Toggle theme",
        default_binding: None,
        aliases: &[],
    },
    KeyAction {
        id: "toggle-trash",
        label: "Toggle trash panel",
        default_binding: None,
        aliases: &[],
    },
    KeyAction {
        id: "pin-note",
        label: "Pin / unpin note",
        default_binding: None,
        aliases: &[],
    },
    KeyAction {
        id: "duplicate-note",
        label: "Duplicate note",
        default_binding: None,
        aliases: &[],
    },
    KeyAction {
        id: "sync-now",
        label: "Sync now",
        default_binding: None,
        aliases: &[],
    },
];

/// Text-editing keys we never hijack at the app level (keymap.ts parity).
pub const RESERVED_BINDINGS: &[&str] = &[
    "Ctrl+C",
    "Ctrl+V",
    "Ctrl+X",
    "Ctrl+Z",
    "Ctrl+Y",
    "Ctrl+A",
    "Ctrl+R",
    "Ctrl+W",
    "Ctrl+Shift+V",
];

/// `settings.shortcuts`: action id → binding override, explicit `null` =
/// unbound. An override wins even when it is `null`, like `hasOwnProperty`
/// in keymap.ts's `effectiveBinding`.
pub type ShortcutOverrides = BTreeMap<String, Option<String>>;

pub fn action(id: &str) -> Option<&'static KeyAction> {
    KEY_ACTIONS.iter().find(|action| action.id == id)
}

pub fn label(id: &str) -> &str {
    action(id).map(|action| action.label).unwrap_or(id)
}

pub fn is_reserved(binding: &str) -> bool {
    RESERVED_BINDINGS.contains(&binding)
}

/// Read `settings.shortcuts` from `config.json`; malformed entries are
/// dropped, like `mergeSettings` in `app/src/lib/settings.svelte.ts`.
pub fn load_overrides(path: &Path) -> ShortcutOverrides {
    let mut overrides = ShortcutOverrides::new();
    let object = crate::config::load_object(path);
    let Some(Value::Object(shortcuts)) = object
        .get("settings")
        .and_then(|settings| settings.get("shortcuts"))
    else {
        return overrides;
    };
    for (id, value) in shortcuts {
        match value {
            Value::String(binding) => {
                overrides.insert(id.clone(), Some(binding.clone()));
            }
            Value::Null => {
                overrides.insert(id.clone(), None);
            }
            _ => {}
        }
    }
    overrides
}

/// The binding that currently applies: the override when present, else the
/// default (keymap.ts `effectiveBinding`).
pub fn effective_binding(id: &str, overrides: &ShortcutOverrides) -> Option<String> {
    match overrides.get(id) {
        Some(override_binding) => override_binding.clone(),
        None => action(id)
            .and_then(|action| action.default_binding)
            .map(str::to_string),
    }
}

/// The binding the shell actually installs for `id`: `effective_binding`,
/// except that an override onto a reserved text-editing key is refused and
/// the default stands. The settings panel enforces the same rule
/// (`isReserved`), but the GTK shell must not let a hand-edited config
/// hijack Ctrl+C and friends.
pub fn installed_binding(id: &str, overrides: &ShortcutOverrides) -> Option<String> {
    if let Some(Some(binding)) = overrides.get(id)
        && is_reserved(binding)
    {
        return action(id)
            .and_then(|action| action.default_binding)
            .map(str::to_string);
    }
    effective_binding(id, overrides)
}

/// The binding shown as an action's key hint (empty when it has none).
pub fn hint(id: &str, overrides: &ShortcutOverrides) -> String {
    installed_binding(id, overrides).unwrap_or_default()
}

/// GTK accelerators for an action: the installed binding plus the always-on
/// aliases (aliases keep working when the action is rebound or unbound).
pub fn accels_for(id: &str, overrides: &ShortcutOverrides) -> Vec<String> {
    let mut accels: Vec<String> = Vec::new();
    if let Some(binding) = installed_binding(id, overrides) {
        accels.push(to_gtk_accel(&binding));
    }
    if let Some(action) = action(id) {
        accels.extend(action.aliases.iter().map(|alias| to_gtk_accel(alias)));
    }
    accels
}

/// Whether a key name is a modifier, or another key the recorder swallows
/// while it waits for a real shortcut (lock keys, the group switch).
pub fn is_modifier_key(name: &str) -> bool {
    matches!(
        name,
        "ISO_Left_Tab" | "VoidSymbol" | "Caps_Lock" | "Num_Lock"
    ) || name.starts_with("Control_")
        || name.starts_with("Shift_")
        || name.starts_with("Alt_")
        || name.starts_with("Super_")
        || name.starts_with("Meta_")
        || name.ends_with("_Lock")
}

/// A key press from the settings recorder → a canonical binding, the same
/// spelling `bindingFromEvent` stores on the Tauri side (`"Ctrl+Shift+P"`).
/// Returns `None` for presses that cannot become a shortcut: no Ctrl or Alt
/// (a shortcut must not shadow plain typing), Escape (the recorder reads it as
/// cancel) and the modifier keys themselves.
pub fn binding_from_parts(ctrl: bool, alt: bool, shift: bool, key_name: &str) -> Option<String> {
    if !ctrl && !alt {
        return None;
    }
    if key_name == "Escape" || is_modifier_key(key_name) {
        return None;
    }
    let key = canonical_key_name(key_name)?;
    let mut binding = String::new();
    if ctrl {
        binding.push_str("Ctrl+");
    }
    if alt {
        binding.push_str("Alt+");
    }
    if shift {
        binding.push_str("Shift+");
    }
    binding.push_str(&key);
    Some(binding)
}

/// GDK key name → the canonical spelling (the inverse of `gtk_key_name`, plus
/// the shifted punctuation names GDK reports).
fn canonical_key_name(name: &str) -> Option<String> {
    let named = match name {
        "space" => "Space",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "semicolon" => ";",
        "apostrophe" => "'",
        "minus" => "-",
        "equal" => "=",
        "bracketleft" => "[",
        "bracketright" => "]",
        "backslash" => "\\",
        "grave" => "`",
        "Up" => "ArrowUp",
        "Down" => "ArrowDown",
        "Left" => "ArrowLeft",
        "Right" => "ArrowRight",
        "Return" => "Enter",
        "BackSpace" => "Backspace",
        "Page_Up" => "PageUp",
        "Page_Down" => "PageDown",
        "exclam" => "!",
        "at" => "@",
        "numbersign" => "#",
        "dollar" => "$",
        "percent" => "%",
        "asciicircum" => "^",
        "ampersand" => "&",
        "asterisk" => "*",
        "parenleft" => "(",
        "parenright" => ")",
        "underscore" => "_",
        "plus" => "+",
        "braceleft" => "{",
        "braceright" => "}",
        "colon" => ":",
        "quotedbl" => "\"",
        "less" => "<",
        "greater" => ">",
        "question" => "?",
        "bar" => "|",
        "asciitilde" => "~",
        other => {
            if other.chars().count() == 1 {
                return Some(other.to_uppercase());
            }
            if let Some(digits) = other.strip_prefix('f')
                && !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
            {
                return Some(format!("F{digits}"));
            }
            return Some(other.to_string());
        }
    };
    Some(named.to_string())
}

/// The other action that already owns `binding` for `id` — its installed
/// binding or one of its always-on aliases (keymap.ts `bindingConflict`).
/// A clash has to be refused: GTK would fire both actions at once.
pub fn binding_conflict(
    id: &str,
    binding: &str,
    overrides: &ShortcutOverrides,
) -> Option<&'static str> {
    KEY_ACTIONS
        .iter()
        .find(|action| {
            action.id != id
                && (installed_binding(action.id, overrides).as_deref() == Some(binding)
                    || action.aliases.contains(&binding))
        })
        .map(|action| action.id)
}

/// Canonical binding → GTK accelerator (`"Ctrl+P"` → `"<Ctrl>p"`), the syntax
/// `gtk_application_set_accels_for_action` parses.
pub fn to_gtk_accel(binding: &str) -> String {
    let mut parts: Vec<&str> = binding.split('+').collect();
    let Some(key) = parts.pop() else {
        return String::new();
    };
    let mut accel = String::new();
    for modifier in parts {
        accel.push_str(match modifier {
            "Ctrl" | "Control" => "<Ctrl>",
            "Alt" => "<Alt>",
            "Shift" => "<Shift>",
            other => other,
        });
    }
    accel.push_str(&gtk_key_name(key));
    accel
}

/// GDK's name for a canonical key, where it differs from the DOM spelling.
fn gtk_key_name(key: &str) -> String {
    let named = match key {
        "Space" => "space",
        "," => "comma",
        "." => "period",
        "/" => "slash",
        ";" => "semicolon",
        "'" => "apostrophe",
        "-" => "minus",
        "=" => "equal",
        "[" => "bracketleft",
        "]" => "bracketright",
        "\\" => "backslash",
        "`" => "grave",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "Enter" => "Return",
        "Backspace" => "BackSpace",
        "PageUp" => "Page_Up",
        "PageDown" => "Page_Down",
        other => {
            return if other.chars().count() == 1 {
                other.to_lowercase()
            } else {
                other.to_string()
            };
        }
    };
    named.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_file() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "writan-linux-keymap-{}-{nanos}.json",
            std::process::id()
        ))
    }

    fn overrides(pairs: &[(&str, Option<&str>)]) -> ShortcutOverrides {
        pairs
            .iter()
            .map(|(id, binding)| (id.to_string(), binding.map(str::to_string)))
            .collect()
    }

    #[test]
    fn defaults_and_overrides_resolve_like_the_tauri_keymap() {
        let none = ShortcutOverrides::new();
        assert_eq!(
            effective_binding("command-palette", &none).as_deref(),
            Some("Ctrl+P")
        );
        assert_eq!(effective_binding("toggle-theme", &none), None);
        assert_eq!(effective_binding("no-such-action", &none), None);

        let custom = overrides(&[("command-palette", Some("Ctrl+Shift+P"))]);
        assert_eq!(
            effective_binding("command-palette", &custom).as_deref(),
            Some("Ctrl+Shift+P")
        );

        let unbound = overrides(&[("command-palette", None)]);
        assert_eq!(
            effective_binding("command-palette", &unbound),
            None,
            "an explicit null unbinds"
        );
    }

    #[test]
    fn accels_include_aliases_and_ignore_them_for_effective_binding() {
        let none = ShortcutOverrides::new();
        assert_eq!(
            accels_for("command-palette", &none),
            vec!["<Ctrl>p", "<Ctrl>k"]
        );
        assert_eq!(accels_for("new-note", &none), vec!["<Ctrl>n"]);
        assert!(accels_for("toggle-theme", &none).is_empty());

        let custom = overrides(&[("command-palette", Some("Ctrl+Shift+P"))]);
        assert_eq!(
            accels_for("command-palette", &custom),
            vec!["<Ctrl><Shift>p", "<Ctrl>k"],
            "the alias survives a rebind"
        );

        let unbound = overrides(&[("command-palette", None)]);
        assert_eq!(
            accels_for("command-palette", &unbound),
            vec!["<Ctrl>k"],
            "aliases are the only accelerator when unbound"
        );
    }

    #[test]
    fn reserved_overrides_are_refused() {
        assert!(is_reserved("Ctrl+C"));
        assert!(!is_reserved("Ctrl+P"));
        let hijack = overrides(&[("new-note", Some("Ctrl+C"))]);
        assert_eq!(
            effective_binding("new-note", &hijack).as_deref(),
            Some("Ctrl+C"),
            "the override is read"
        );
        assert_eq!(
            installed_binding("new-note", &hijack).as_deref(),
            Some("Ctrl+N"),
            "but the default stands"
        );
        assert_eq!(accels_for("new-note", &hijack), vec!["<Ctrl>n"]);
    }

    #[test]
    fn canonical_bindings_become_gtk_accelerators() {
        assert_eq!(to_gtk_accel("Ctrl+P"), "<Ctrl>p");
        assert_eq!(to_gtk_accel("Ctrl+Shift+P"), "<Ctrl><Shift>p");
        assert_eq!(to_gtk_accel("Ctrl+,"), "<Ctrl>comma");
        assert_eq!(to_gtk_accel("Alt+Space"), "<Alt>space");
        assert_eq!(to_gtk_accel("F2"), "F2");
        assert_eq!(to_gtk_accel("Ctrl+ArrowDown"), "<Ctrl>Down");
    }

    #[test]
    fn shortcuts_read_from_config_and_malformed_values_drop() {
        let file = temp_file();
        assert!(load_overrides(&file).is_empty(), "missing file");

        std::fs::write(
            &file,
            r#"{"settings":{"shortcuts":{"command-palette":"Ctrl+Shift+P","new-note":null,"save-note":42}}}"#,
        )
        .expect("write fixture");
        let loaded = load_overrides(&file);
        assert_eq!(
            loaded.get("command-palette").cloned().flatten().as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert!(matches!(loaded.get("new-note"), Some(None)));
        assert!(
            !loaded.contains_key("save-note"),
            "a malformed value falls back to the default"
        );

        std::fs::write(&file, r#"{"settings":{"shortcuts":"nope"}}"#).expect("write fixture");
        assert!(load_overrides(&file).is_empty());
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn recorded_keys_become_canonical_bindings() {
        assert_eq!(binding_from_parts(false, false, false, "p"), None);
        assert_eq!(
            binding_from_parts(true, false, true, "p").as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(
            binding_from_parts(true, false, false, "comma").as_deref(),
            Some("Ctrl+,")
        );
        assert_eq!(
            binding_from_parts(true, false, false, "space").as_deref(),
            Some("Ctrl+Space")
        );
        assert_eq!(
            binding_from_parts(true, false, false, "Return").as_deref(),
            Some("Ctrl+Enter")
        );
        assert_eq!(
            binding_from_parts(true, false, false, "Up").as_deref(),
            Some("Ctrl+ArrowUp")
        );
        assert_eq!(
            binding_from_parts(true, false, false, "F5").as_deref(),
            Some("Ctrl+F5")
        );
        assert_eq!(binding_from_parts(true, false, false, "Escape"), None);
        assert_eq!(binding_from_parts(true, false, false, "Control_L"), None);
        assert_eq!(binding_from_parts(true, false, false, "Shift_L"), None);
        assert!(is_modifier_key("Shift_L"));
        assert!(is_modifier_key("Caps_Lock"));
        assert!(!is_modifier_key("p"), "letters are real keys");

        // Round-trips through the accelerator syntax the shell installs.
        let recorded = binding_from_parts(true, false, true, "p").expect("binding");
        assert_eq!(to_gtk_accel(&recorded), "<Ctrl><Shift>p");
        let comma = binding_from_parts(true, false, false, "comma").expect("binding");
        assert_eq!(to_gtk_accel(&comma), "<Ctrl>comma");
    }

    #[test]
    fn conflicts_include_aliases_and_respect_unbound_overrides() {
        let none = ShortcutOverrides::new();
        assert_eq!(
            binding_conflict("new-note", "Ctrl+P", &none),
            Some("command-palette"),
            "another action's binding clashes"
        );
        assert_eq!(
            binding_conflict("new-note", "Ctrl+K", &none),
            Some("command-palette"),
            "an always-on alias clashes"
        );
        assert_eq!(binding_conflict("new-note", "Ctrl+Shift+K", &none), None);
        assert_eq!(
            binding_conflict("command-palette", "Ctrl+P", &none),
            None,
            "an action does not clash with itself"
        );

        let unbound = overrides(&[("command-palette", None)]);
        assert_eq!(
            binding_conflict("new-note", "Ctrl+P", &unbound),
            None,
            "an unbound action frees its binding"
        );
        assert_eq!(
            binding_conflict("new-note", "Ctrl+K", &unbound),
            Some("command-palette"),
            "its alias stays installed regardless"
        );
    }

    /// The Tauri keymap is the source of truth; this fails the moment the
    /// two tables drift (the reason `docs/PARITY.md` can promise parity).
    #[test]
    fn keymap_ts_table_matches_this_one() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/src/lib/keymap.ts");
        let source = std::fs::read_to_string(&path).expect("read app/src/lib/keymap.ts");
        let actions_block = slice_between(&source, "KEY_ACTIONS", "];");
        let reserved_block = slice_between(&source, "RESERVED_BINDINGS", "]);");

        let segments = action_segments(actions_block);
        assert_eq!(
            segments.len(),
            KEY_ACTIONS.len(),
            "keymap.ts has actions this table does not"
        );

        for action in KEY_ACTIONS {
            let needle = format!("id: \"{}\"", action.id);
            let segment = segments
                .iter()
                .find(|segment| segment.contains(&needle))
                .unwrap_or_else(|| panic!("{} is missing from keymap.ts", action.id));
            assert_eq!(
                quoted(segment, "label:").as_deref(),
                Some(action.label),
                "label drift for {}",
                action.id
            );
            assert_eq!(
                binding_field(segment),
                action.default_binding.map(str::to_string),
                "default binding drift for {}",
                action.id
            );
            assert_eq!(
                string_list(segment, "aliases:"),
                action
                    .aliases
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>(),
                "alias drift for {}",
                action.id
            );
        }

        let reserved: Vec<String> = reserved_block
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        assert_eq!(
            reserved,
            RESERVED_BINDINGS
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>(),
            "reserved set drift"
        );
    }

    fn slice_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let begin = source.find(start).expect("start marker");
        let rest = &source[begin..];
        let stop = rest.find(end).expect("end marker");
        &rest[..stop]
    }

    /// Split the array body on `id: "` so an entry reformatted over several
    /// lines still parses.
    fn action_segments(block: &str) -> Vec<&str> {
        let starts: Vec<usize> = block
            .match_indices("id: \"")
            .map(|(index, _)| index)
            .collect();
        starts
            .iter()
            .enumerate()
            .map(|(position, start)| {
                let end = starts.get(position + 1).copied().unwrap_or(block.len());
                &block[*start..end]
            })
            .collect()
    }

    fn after<'a>(segment: &'a str, needle: &str) -> Option<&'a str> {
        segment
            .split_once(needle)
            .map(|(_, rest)| rest.trim_start())
    }

    fn quoted(segment: &str, needle: &str) -> Option<String> {
        let rest = after(segment, needle)?.strip_prefix('"')?;
        rest.split('"').next().map(str::to_string)
    }

    fn binding_field(segment: &str) -> Option<String> {
        let rest = after(segment, "defaultBinding:")?;
        if rest.starts_with("null") {
            None
        } else {
            rest.strip_prefix('"')?
                .split('"')
                .next()
                .map(str::to_string)
        }
    }

    fn string_list(segment: &str, needle: &str) -> Vec<String> {
        let Some(rest) = after(segment, needle) else {
            return Vec::new();
        };
        let inner = rest
            .strip_prefix('[')
            .and_then(|rest| rest.split(']').next())
            .unwrap_or("");
        inner
            .split(',')
            .filter_map(|piece| {
                let piece = piece.trim();
                piece
                    .strip_prefix('"')?
                    .strip_suffix('"')
                    .map(str::to_string)
            })
            .collect()
    }
}
