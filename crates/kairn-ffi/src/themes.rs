//! Vault theme files (`.kairn/themes/*.json`): listing, loading, and hex
//! color parsing, so the phone renders a custom theme byte-for-byte the way
//! the desktop does. The schema and tolerant parsing live in
//! [`kairn_core::themes`]; colors cross the FFI as the raw strings the file
//! carries plus a shared parser, and terminal colors stay behind (no
//! terminal on the phone yet).

use std::path::Path;

/// A theme available to pick: its id (file stem, what settings store), its
/// display name, and whether it declares itself `dark` or `light`.
#[derive(uniffi::Record)]
pub struct FfiThemeEntry {
    pub id: String,
    pub name: String,
    pub mode: String,
}

/// The color overrides a theme file carries, raw hex strings as written.
/// Unset fields keep the built-in palette the theme's `mode` names.
#[derive(uniffi::Record)]
pub struct FfiThemeColors {
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
    pub sel: Option<String>,
    pub highlight: Option<String>,
    pub heading: Option<String>,
    pub bold: Option<String>,
}

/// The font overrides a theme file carries. Unset fields keep the app's
/// defaults.
#[derive(uniffi::Record)]
pub struct FfiThemeFonts {
    pub ui: Option<String>,
    pub editor: Option<String>,
    pub mono: Option<String>,
    pub editor_size: Option<f32>,
}

/// One parsed theme file.
#[derive(uniffi::Record)]
pub struct FfiThemeSpec {
    pub name: String,
    pub mode: String,
    pub colors: FfiThemeColors,
    pub fonts: FfiThemeFonts,
}

/// Every readable theme file in the vault, sorted by display name.
/// Unparseable files are skipped rather than hiding the whole list behind
/// one bad edit.
#[uniffi::export]
pub fn list_vault_themes(root: String) -> Vec<FfiThemeEntry> {
    kairn_core::list_themes(Path::new(&root))
        .into_iter()
        .map(|e| FfiThemeEntry { id: e.id, name: e.name, mode: e.mode })
        .collect()
}

/// Load a vault theme by id. `None` when the file is missing or malformed —
/// the caller falls back to a built-in.
#[uniffi::export]
pub fn load_vault_theme(root: String, id: String) -> Option<FfiThemeSpec> {
    let spec = kairn_core::load_theme(Path::new(&root), &id)?;
    let c = spec.colors;
    let f = spec.fonts;
    Some(FfiThemeSpec {
        name: spec.name,
        mode: spec.mode,
        colors: FfiThemeColors {
            bg: c.bg,
            panel: c.panel,
            panel2: c.panel2,
            hover: c.hover,
            border: c.border,
            text: c.text,
            dim: c.dim,
            faint: c.faint,
            accent: c.accent,
            amber: c.amber,
            on_amber: c.on_amber,
            red: c.red,
            sel: c.sel,
            highlight: c.highlight,
            heading: c.heading,
            bold: c.bold,
        },
        fonts: FfiThemeFonts {
            ui: f.ui,
            editor: f.editor,
            mono: f.mono,
            editor_size: f.editor_size,
        },
    })
}

/// An RGBA color, one byte per channel.
#[derive(uniffi::Record)]
pub struct FfiRgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// `#rrggbb` or `#rrggbbaa` (leading `#` optional) to RGBA bytes, exactly
/// as the desktop parses theme colors. `None` for anything else.
#[uniffi::export]
pub fn parse_hex_color(hex: String) -> Option<FfiRgba> {
    let [r, g, b, a] = kairn_core::parse_hex_color(&hex)?;
    Some(FfiRgba { r, g, b, a })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_rgb_and_rgba() {
        let c = parse_hex_color("#a8b48d".into()).unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (0xa8, 0xb4, 0x8d, 0xff));
        let c = parse_hex_color("d9a75c48".into()).unwrap();
        assert_eq!(c.a, 0x48);
        assert!(parse_hex_color("#fff".into()).is_none());
    }

    #[test]
    fn listing_and_loading_a_vault_theme() {
        let root = std::env::temp_dir().join(format!("kairn-ffi-themes-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".kairn/themes")).unwrap();
        std::fs::write(
            root.join(".kairn/themes/test.json"),
            r##"{"name":"Test","mode":"dark","colors":{"accent":"#73b3c0"}}"##,
        )
        .unwrap();
        let entries = list_vault_themes(root.to_string_lossy().into_owned());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "test");
        assert_eq!(entries[0].name, "Test");
        let spec = load_vault_theme(root.to_string_lossy().into_owned(), "test".into()).unwrap();
        assert_eq!(spec.colors.accent.as_deref(), Some("#73b3c0"));
        assert!(spec.colors.bg.is_none());
        assert!(load_vault_theme(root.to_string_lossy().into_owned(), "missing".into()).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
