//! Appearance: theme (system/light/dark), accent presets and the link colour
//! the preview uses. The accent table mirrors `ACCENTS` in
//! `app/src/lib/settings.svelte.ts` (registry owns the schema) — a drift test
//! keeps the two in step, like the keymap drift test.

pub struct Accent {
    pub id: &'static str,
    pub label: &'static str,
    /// Filled accent colour on light backgrounds.
    pub light: &'static str,
    /// Filled accent colour on dark backgrounds.
    pub dark: &'static str,
}

/// Presets must match `ACCENTS` in the settings registry, same order.
pub const ACCENTS: [Accent; 6] = [
    Accent {
        id: "indigo",
        label: "Indigo",
        light: "#5b6ee0",
        dark: "#93a0ff",
    },
    Accent {
        id: "violet",
        label: "Violet",
        light: "#7c5ce0",
        dark: "#b6a4ff",
    },
    Accent {
        id: "teal",
        label: "Teal",
        light: "#0f9d8f",
        dark: "#5fd7c6",
    },
    Accent {
        id: "green",
        label: "Green",
        light: "#3f9b4f",
        dark: "#7fd48d",
    },
    Accent {
        id: "amber",
        label: "Amber",
        light: "#b9811a",
        dark: "#e9b95c",
    },
    Accent {
        id: "rose",
        label: "Rose",
        light: "#c04b73",
        dark: "#f09ab5",
    },
];

/// Look up a preset, falling back to the registry default (indigo).
pub fn accent(id: &str) -> &'static Accent {
    ACCENTS
        .iter()
        .find(|accent| accent.id == id)
        .unwrap_or(&ACCENTS[0])
}

/// The colour for the current scheme — also the preview's link colour.
pub fn accent_color(id: &str, dark: bool) -> &'static str {
    let accent = accent(id);
    if dark { accent.dark } else { accent.light }
}

/// Whether the shell should render dark. `adw::StyleManager` sits below
/// GTK_THEME, so honour the user's GTK preference too: a dark GTK_THEME must
/// not leave a light Writan window behind (M3.5 polish item).
#[cfg(target_os = "linux")]
pub fn effective_dark(manager: &adw::StyleManager) -> bool {
    if manager.is_dark() {
        return true;
    }
    if let Ok(theme) = std::env::var("GTK_THEME") {
        return theme.to_ascii_lowercase().ends_with(":dark");
    }
    gtk::Settings::default()
        .map(|settings| settings.is_gtk_application_prefer_dark_theme())
        .unwrap_or(false)
}

/// Apply the persisted choice; "system" follows the desktop again.
#[cfg(target_os = "linux")]
pub fn apply_theme(manager: &adw::StyleManager, theme: &str) {
    let scheme = match theme {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::Default,
    };
    manager.set_color_scheme(scheme);
}

/// CSS that paints the accent where libadwaita uses its own: switches,
/// suggested buttons, selected rows, checked toggles and the focus ring.
///
/// `@define-color accent_bg_color` does not reach libadwaita's stylesheet
/// (named colours resolve when each stylesheet is parsed, and libadwaita
/// defines these itself), so the rules below are spelled out. Verified in a
/// probe on GTK 4.14: only direct rules carry.
pub fn accent_css(id: &str, dark: bool) -> String {
    let color = accent_color(id, dark);
    let fg = contrast_fg(color);
    format!(
        "switch:checked {{ background-color: {color}; }}\n\
         switch:checked > slider {{ background-color: {fg}; }}\n\
         button.suggested-action {{ background-color: {color}; color: {fg}; }}\n\
         row:selected {{ background-color: alpha({color}, 0.18); color: {color}; }}\n\
         togglebutton:checked {{ background-color: alpha({color}, 0.18); color: {color}; }}\n\
         :focus-visible {{ outline-color: {color}; }}\n"
    )
}

/// Black or white, whichever reads better on `color` ("#rrggbb").
pub fn contrast_fg(color: &str) -> &'static str {
    let digits = color.trim_start_matches('#');
    let channel = |start: usize| {
        u8::from_str_radix(&digits[start..start + 2], 16).unwrap_or(0) as f64 / 255.0
    };
    if digits.len() != 6 {
        return "#ffffff";
    }
    let luminance = 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
    if luminance > 0.55 {
        "#000000"
    } else {
        "#ffffff"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_accent_falls_back_to_the_default() {
        assert_eq!(accent("no-such-preset").id, "indigo");
        assert_eq!(accent_color("no-such-preset", false), "#5b6ee0");
        assert_eq!(accent_color("teal", true), "#5fd7c6");
    }

    #[test]
    fn contrast_picks_readable_text() {
        assert_eq!(contrast_fg("#93a0ff"), "#000000");
        assert_eq!(contrast_fg("#3f9b4f"), "#ffffff");
        assert_eq!(contrast_fg("not-a-color"), "#ffffff");
    }

    #[test]
    fn accent_css_paints_the_shell_accents() {
        let css = accent_css("teal", true);
        assert!(css.contains("switch:checked { background-color: #5fd7c6; }"));
        assert!(
            css.contains("button.suggested-action { background-color: #5fd7c6; color: #000000; }")
        );
        assert!(
            css.contains(
                "row:selected { background-color: alpha(#5fd7c6, 0.18); color: #5fd7c6; }"
            )
        );
        assert!(
            !css.contains("@define-color"),
            "named colours do not reach libadwaita's stylesheet"
        );
    }

    /// Drift guard: the Rust presets must equal the settings registry's.
    #[test]
    fn accent_presets_match_the_settings_registry() {
        let source = include_str!("../../app/src/lib/settings.svelte.ts");
        let parsed = parse_presets(source);
        assert_eq!(parsed.len(), ACCENTS.len(), "preset count");
        for (found, ours) in parsed.iter().zip(ACCENTS.iter()) {
            assert_eq!((found.0.as_str(), found.1.as_str()), (ours.id, ours.label));
            assert_eq!(
                (found.2.as_str(), found.3.as_str()),
                (ours.light, ours.dark)
            );
        }
    }

    /// Minimal line parser for `{ id: "…", label: "…", light: "…", dark: "…" }`.
    fn parse_presets(source: &str) -> Vec<(String, String, String, String)> {
        let start = source
            .find("export const ACCENTS: AccentPreset[] = [")
            .expect("ACCENTS block in settings.svelte.ts");
        let rest = &source[start..];
        let end = rest.find("];").expect("end of ACCENTS block");
        let field = |line: &str, name: &str| {
            let key = format!("{name}: \"");
            let from = line.find(&key).expect("field start") + key.len();
            let to = from + line[from..].find('"').expect("field end");
            line[from..to].to_string()
        };
        rest[..end]
            .lines()
            .filter(|line| line.trim_start().starts_with("{ id:"))
            .map(|line| {
                (
                    field(line, "id"),
                    field(line, "label"),
                    field(line, "light"),
                    field(line, "dark"),
                )
            })
            .collect()
    }
}
