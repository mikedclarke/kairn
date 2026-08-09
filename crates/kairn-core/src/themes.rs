//! Theme files: `.kairn/themes/*.json` inside the notes root, so custom
//! looks sync with the vault. A file names a mode ("dark" or "light"),
//! then overrides any subset of the palette, the font choices, and the
//! terminal's ANSI colors; everything it leaves out falls back to the
//! built-in palette for that mode. The UI layer owns the built-ins and
//! the hex-to-color mapping; this module only reads and validates files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSpec {
    /// Display name; falls back to the file stem when empty.
    pub name: String,
    /// "dark" or "light": which built-in palette fills the gaps, and how
    /// stock widgets render. Anything else reads as "dark".
    pub mode: String,
    pub colors: ThemeColors,
    pub fonts: ThemeFonts,
    pub terminal: ThemeTerminal,
}

/// Palette overrides as `#rrggbb` / `#rrggbbaa` strings (leading `#`
/// optional). Field names mirror the app's `KairnTheme`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeColors {
    pub bg: Option<String>,
    pub panel: Option<String>,
    pub panel2: Option<String>,
    pub hover: Option<String>,
    pub border: Option<String>,
    pub text: Option<String>,
    pub dim: Option<String>,
    pub faint: Option<String>,
    pub accent: Option<String>,
    pub amber: Option<String>,
    pub on_amber: Option<String>,
    pub red: Option<String>,
    pub term_bg: Option<String>,
    pub sel: Option<String>,
    /// The `==highlight==` background; alpha respected.
    pub highlight: Option<String>,
    /// Heading and note-title text; defaults to the sage accent.
    pub heading: Option<String>,
    /// `**bold**` text; defaults to the amber accent so bold reads as a
    /// distinct colour, not just a heavier weight.
    pub bold: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeFonts {
    /// UI chrome family. Unset keeps the system font.
    pub ui: Option<String>,
    /// Notes editor family. Unset follows the UI font.
    pub editor: Option<String>,
    /// Terminal and mono family. Unset keeps the auto-resolved mono.
    pub mono: Option<String>,
    /// Editor body size in px; headings scale with it.
    pub editor_size: Option<f32>,
}

/// ANSI-16 (plus background/foreground/cursor) overrides for the terminal.
/// Unset fields keep the built-in ramp, with the background following the
/// theme's `term_bg`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeTerminal {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub cursor: Option<String>,
    pub black: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
    pub cyan: Option<String>,
    pub white: Option<String>,
    pub bright_black: Option<String>,
    pub bright_red: Option<String>,
    pub bright_green: Option<String>,
    pub bright_yellow: Option<String>,
    pub bright_blue: Option<String>,
    pub bright_magenta: Option<String>,
    pub bright_cyan: Option<String>,
    pub bright_white: Option<String>,
}

/// A theme available to pick: its id (file stem, what settings store) and
/// display name.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    pub id: String,
    pub name: String,
    pub mode: String,
}

pub fn themes_dir(root: &Path) -> PathBuf {
    root.join(".kairn").join("themes")
}

/// Every readable theme file in the vault, sorted by display name.
/// Unparseable files are skipped with a note on stderr rather than hiding
/// the whole list behind one bad edit.
pub fn list_themes(root: &Path) -> Vec<ThemeEntry> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(themes_dir(root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match load_theme_file(&path) {
            Ok(spec) => out.push(ThemeEntry {
                id: stem.to_string(),
                name: if spec.name.trim().is_empty() {
                    stem.to_string()
                } else {
                    spec.name.trim().to_string()
                },
                mode: spec.mode.clone(),
            }),
            Err(e) => eprintln!("kairn: skipping theme {}: {e:#}", path.display()),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Load a theme by id (file stem). `None`, with the reason on stderr, when
/// the file is missing or malformed — the caller falls back to a built-in.
pub fn load_theme(root: &Path, id: &str) -> Option<ThemeSpec> {
    let path = themes_dir(root).join(format!("{id}.json"));
    match load_theme_file(&path) {
        Ok(spec) => Some(spec),
        Err(e) => {
            eprintln!("kairn: could not load theme {}: {e:#}", path.display());
            None
        }
    }
}

fn load_theme_file(path: &Path) -> Result<ThemeSpec> {
    let text = fs::read_to_string(path).context("reading")?;
    serde_json::from_str(&text).context("parsing")
}

/// `#rrggbb` or `#rrggbbaa` (leading `#` optional) to RGBA bytes.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => {
            let [_, r, g, b] = v.to_be_bytes();
            Some([r, g, b, 0xff])
        }
        8 => Some(v.to_be_bytes()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(parse_hex_color("#a8b48d"), Some([0xa8, 0xb4, 0x8d, 0xff]));
        assert_eq!(parse_hex_color("d9a75c48"), Some([0xd9, 0xa7, 0x5c, 0x48]));
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("not a color"), None);
    }

    #[test]
    fn themes_list_and_load_from_the_vault() {
        let root = ScratchRoot::new("themes");
        assert!(list_themes(&root.0).is_empty());
        root.write(
            ".kairn/themes/gruvbox.json",
            r##"{ "name": "Gruvbox", "mode": "dark",
                 "colors": { "bg": "#282828", "accent": "#b8bb26" },
                 "fonts": { "editor_size": 14.5 } }"##,
        );
        // Malformed files are skipped, not fatal to the listing.
        root.write(".kairn/themes/broken.json", "{ nope");
        // Name falls back to the file stem.
        root.write(".kairn/themes/plain.json", r#"{ "mode": "light" }"#);

        let listed = list_themes(&root.0);
        assert_eq!(
            listed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["gruvbox", "plain"]
        );
        assert_eq!(listed[0].name, "Gruvbox");
        assert_eq!(listed[1].name, "plain");

        let spec = load_theme(&root.0, "gruvbox").expect("load");
        assert_eq!(spec.colors.bg.as_deref(), Some("#282828"));
        assert_eq!(spec.fonts.editor_size, Some(14.5));
        assert_eq!(spec.colors.panel, None);
        assert!(load_theme(&root.0, "missing").is_none());
    }
}
