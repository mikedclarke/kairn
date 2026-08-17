use std::path::Path;

use gpui::{App, Global, Hsla, SharedString, Window, rgb, rgba};
use gpui_component::theme::{Theme, ThemeMode};
use gpui_terminal::ColorPalette;
use kairn_core::settings::Settings;
use kairn_core::themes::{ThemeSpec, ThemeTerminal, parse_hex_color};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "light" => Mode::Light,
            _ => Mode::Dark,
        }
    }
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// The active look: colors, fonts, and the terminal palette. The built-in
/// pair is the sage/amber palette from the locked design spec; theme files
/// (`.kairn/themes/*.json`) override any subset of it.
#[derive(Clone)]
pub struct KairnTheme {
    pub mode: Mode,
    pub bg: Hsla,
    pub panel: Hsla,
    pub panel2: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub dim: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub amber: Hsla,
    pub on_amber: Hsla,
    pub red: Hsla,
    pub term_bg: Hsla,
    pub sel: Hsla,
    /// `==highlight==` background, alpha included.
    pub highlight: Hsla,
    /// Heading and note-title text.
    pub heading: Hsla,
    /// `**bold**` text: a distinct colour, not just a heavier weight.
    pub bold: Hsla,
    /// UI chrome family; `None` keeps the system font.
    pub ui_font: Option<SharedString>,
    /// Notes editor family; `None` follows the UI font.
    pub editor_font: Option<SharedString>,
    /// Terminal and mono family.
    pub mono_font: SharedString,
    /// Editor body size in px; headings scale with it.
    pub editor_size: f32,
    /// Interface text size in px; the whole app chrome scales from it via
    /// [`KairnTheme::ui_px`].
    pub ui_size: f32,
    pub term_colors: ColorPalette,
}

/// The default editor body size the metrics in the note editor are drawn
/// against; `editor_size / EDITOR_BASE_SIZE` scales them.
pub const EDITOR_BASE_SIZE: f32 = 13.0;

/// The default interface size the hard-coded chrome sizes are authored
/// against; `ui_size / UI_BASE_SIZE` scales them (see [`KairnTheme::ui_px`]).
pub const UI_BASE_SIZE: f32 = 13.0;

impl KairnTheme {
    /// Scale a chrome text size (authored against [`UI_BASE_SIZE`]) by the
    /// active interface size, so every `text_size(t.ui_px(n))` in the UI
    /// tracks the one setting. The editor uses `editor_size` instead.
    pub fn ui_px(&self, base: f32) -> gpui::Pixels {
        gpui::px(base * self.ui_size / UI_BASE_SIZE)
    }
}

impl KairnTheme {
    pub fn dark() -> Self {
        let amber = c(0xd9a75c);
        let text = c(0xdadcd1);
        let accent = c(0xa8b48d);
        Self {
            mode: Mode::Dark,
            bg: c(0x1a1b15),
            panel: c(0x20221a),
            panel2: c(0x262920),
            hover: c(0x2c2f25),
            border: c(0x31352a),
            text,
            dim: c(0x90957f),
            faint: c(0x5f6355),
            accent,
            amber,
            on_amber: c(0x1a1a14),
            red: c(0xc97b6d),
            term_bg: c(0x14150f),
            sel: ca(0xa8b48d40),
            highlight: amber.opacity(0.28),
            heading: accent,
            bold: amber,
            ui_font: None,
            editor_font: None,
            mono_font: auto_mono().into(),
            editor_size: EDITOR_BASE_SIZE,
            ui_size: UI_BASE_SIZE,
            term_colors: terminal_palette(&ThemeTerminal::default(), (0x14, 0x15, 0x0f)),
        }
    }

    pub fn light() -> Self {
        let amber = c(0xae7c2c);
        let text = c(0x2c2e26);
        let accent = c(0x5f7247);
        Self {
            mode: Mode::Light,
            bg: c(0xf4f4ea),
            panel: c(0xebecdf),
            panel2: c(0xe3e4d6),
            hover: c(0xdcded0),
            border: c(0xd4d6c5),
            text,
            dim: c(0x6e7263),
            faint: c(0x9ba18b),
            accent,
            amber,
            on_amber: c(0x1a1a14),
            red: c(0xa8574a),
            // The terminal stays dark in both modes per the design spec;
            // only its background shade follows.
            term_bg: c(0x22231c),
            sel: ca(0x5f724733),
            highlight: amber.opacity(0.28),
            heading: accent,
            bold: amber,
            ui_font: None,
            editor_font: None,
            mono_font: auto_mono().into(),
            editor_size: EDITOR_BASE_SIZE,
            ui_size: UI_BASE_SIZE,
            term_colors: terminal_palette(&ThemeTerminal::default(), (0x22, 0x23, 0x1c)),
        }
    }

    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        }
    }

    /// A theme file layered over the built-in palette for its mode.
    pub fn from_spec(spec: &ThemeSpec) -> Self {
        fn set(dst: &mut Hsla, src: &Option<String>) {
            if let Some(v) = src.as_deref().and_then(parse_hex_color) {
                *dst = rgba(u32::from_be_bytes(v)).into();
            }
        }
        let mut t = Self::for_mode(Mode::from_str(&spec.mode));
        let colors = &spec.colors;
        set(&mut t.bg, &colors.bg);
        set(&mut t.panel, &colors.panel);
        set(&mut t.panel2, &colors.panel2);
        set(&mut t.hover, &colors.hover);
        set(&mut t.border, &colors.border);
        set(&mut t.text, &colors.text);
        set(&mut t.dim, &colors.dim);
        set(&mut t.faint, &colors.faint);
        set(&mut t.accent, &colors.accent);
        set(&mut t.amber, &colors.amber);
        set(&mut t.on_amber, &colors.on_amber);
        set(&mut t.red, &colors.red);
        set(&mut t.term_bg, &colors.term_bg);
        set(&mut t.sel, &colors.sel);
        // The derived defaults follow their sources when the file moves
        // amber/accent but doesn't pin highlight/heading/bold explicitly.
        t.highlight = t.amber.opacity(0.28);
        t.heading = t.accent;
        t.bold = t.amber;
        set(&mut t.highlight, &colors.highlight);
        set(&mut t.heading, &colors.heading);
        set(&mut t.bold, &colors.bold);
        if let Some(f) = &spec.fonts.ui {
            t.ui_font = Some(f.clone().into());
        }
        if let Some(f) = &spec.fonts.editor {
            t.editor_font = Some(f.clone().into());
        }
        if let Some(f) = &spec.fonts.mono {
            t.mono_font = f.clone().into();
        }
        if let Some(s) = spec.fonts.editor_size {
            t.editor_size = s.clamp(9., 32.);
        }
        let term_bg = colors
            .term_bg
            .as_deref()
            .and_then(parse_hex_color)
            .map(|[r, g, b, _]| (r, g, b))
            .unwrap_or(match t.mode {
                Mode::Dark => (0x14, 0x15, 0x0f),
                Mode::Light => (0x22, 0x23, 0x1c),
            });
        t.term_colors = terminal_palette(&spec.terminal, term_bg);
        t
    }
}

/// Built-in themes offered in the picker, in display order. Menlo (the
/// fresh-install default) is a full embedded theme spec, fonts and
/// terminal ramp included. Sage and Sage Light are the original sage/amber
/// base palettes; they keep the historical "dark"/"light" ids so stored
/// settings keep resolving. The rest are the dark base with a different
/// accent family, Ocean additionally swapping the base's olive-tinted
/// surfaces for untinted greys, so only its accent carries any colour.
/// Ids are stable (settings store them); names are shown.
pub const BUILTIN_THEMES: &[(&str, &str)] = &[
    ("menlo", "Menlo"),
    ("ocean", "Ocean"),
    ("dark", "Sage"),
    ("light", "Sage Light"),
    ("rose", "Rose"),
    ("forest", "Forest"),
];

/// The Menlo preset's full spec, in the same format as `.kairn/themes`
/// files; a preset lookup shadows any vault theme with the same id.
const MENLO_SPEC: &str = include_str!("themes/menlo.json");

/// Resolve a built-in preset id to its theme, or `None` for anything that
/// isn't one (a base mode or a `.kairn/themes` file id).
fn preset(id: &str) -> Option<KairnTheme> {
    if id == "menlo" {
        let spec: ThemeSpec =
            serde_json::from_str(MENLO_SPEC).expect("embedded menlo theme parses");
        return Some(KairnTheme::from_spec(&spec));
    }
    let (accent, amber) = match id {
        "ocean" => (c(0x7fa8c9), c(0xd9a75c)),
        "rose" => (c(0xc98fa8), c(0xd9a75c)),
        "forest" => (c(0x8fbf8f), c(0xcbb26a)),
        _ => return None,
    };
    let mut t = KairnTheme::dark();
    t.accent = accent;
    t.amber = amber;
    t.heading = accent;
    t.bold = amber;
    t.sel = accent.opacity(0.25);
    t.highlight = amber.opacity(0.28);
    if id == "ocean" {
        t.bg = c(0x151515);
        t.panel = c(0x1b1b1b);
        t.panel2 = c(0x212121);
        t.hover = c(0x272727);
        t.border = c(0x2e2e2e);
        t.text = c(0xd9d9d9);
        t.dim = c(0x8f8f8f);
        t.faint = c(0x5e5e5e);
        t.term_bg = c(0x101010);
        t.term_colors = terminal_palette(&ThemeTerminal::default(), (0x10, 0x10, 0x10));
    }
    Some(t)
}

pub struct ActiveKairnTheme(pub KairnTheme);

impl Global for ActiveKairnTheme {}

pub trait KairnThemeExt {
    fn kairn(&self) -> &KairnTheme;
}

impl KairnThemeExt for App {
    fn kairn(&self) -> &KairnTheme {
        &self.global::<ActiveKairnTheme>().0
    }
}

/// Resolve the configured theme — a built-in name or a `.kairn/themes`
/// file id — with the settings' font overrides on top, install it as the
/// active palette, and skin gpui-component's widgets (dialogs, inputs,
/// buttons) to match, so the few stock components don't look foreign.
pub fn apply(settings: &Settings, notes_root: &Path, window: Option<&mut Window>, cx: &mut App) {
    let mut t = match settings.theme.as_str() {
        "light" => KairnTheme::light(),
        "dark" => KairnTheme::dark(),
        // Presets first: they have no file, so trying to load them as one
        // would just spew a "could not load theme" line every apply.
        id => match preset(id) {
            Some(t) => t,
            None => match kairn_core::themes::load_theme(notes_root, id) {
                Some(spec) => KairnTheme::from_spec(&spec),
                None => KairnTheme::dark(),
            },
        },
    };
    if let Some(f) = &settings.ui_font {
        t.ui_font = Some(f.clone().into());
    }
    if let Some(f) = &settings.editor_font {
        t.editor_font = Some(f.clone().into());
    }
    if let Some(f) = &settings.mono_font {
        t.mono_font = f.clone().into();
    }
    // Styles must never carry a family the platform can't load: gpui's
    // resolve_font formats an error per missing family on every text run,
    // every frame, and on Linux the fallback walk pays that several times
    // over before landing (GDL-710). Unset UI resolves here too — the old
    // .SystemUIFont default only exists on macOS.
    t.ui_font = Some(
        t.ui_font
            .take()
            .filter(|f| loads_as_itself(f, cx))
            .unwrap_or_else(|| auto_ui().into()),
    );
    if let Some(f) = t.editor_font.take() {
        t.editor_font =
            Some(if loads_as_itself(&f, cx) { f } else { auto_ui().into() });
    }
    if !loads_as_itself(&t.mono_font, cx) {
        t.mono_font = auto_mono().into();
    }
    if let Some(s) = settings.editor_font_size {
        t.editor_size = s.clamp(9., 32.);
    }
    if let Some(s) = settings.ui_font_size {
        t.ui_size = s.clamp(9., 32.);
    }

    Theme::change(
        match t.mode {
            Mode::Dark => ThemeMode::Dark,
            Mode::Light => ThemeMode::Light,
        },
        window,
        cx,
    );

    let theme = cx.global_mut::<Theme>();
    theme.font_family = t
        .ui_font
        .clone()
        .unwrap_or_else(|| auto_ui().into());
    theme.mono_font_family = t.mono_font.clone();
    let colors = &mut theme.colors;
    colors.background = t.panel2;
    colors.foreground = t.text;
    colors.border = t.border;
    colors.input = t.border;
    colors.ring = t.accent;
    colors.caret = t.accent;
    colors.selection = t.sel;
    colors.popover = t.panel2;
    colors.popover_foreground = t.text;
    colors.primary = t.accent;
    colors.primary_hover = t.accent.opacity(0.9);
    colors.primary_active = t.accent.opacity(0.8);
    colors.primary_foreground = t.bg;
    colors.secondary = t.hover;
    colors.secondary_hover = t.hover.opacity(0.8);
    colors.secondary_active = t.hover.opacity(0.7);
    colors.secondary_foreground = t.text;
    colors.muted = t.hover;
    colors.muted_foreground = t.dim;
    colors.accent = t.sel;
    colors.accent_foreground = t.text;
    colors.danger = t.red;
    colors.danger_hover = t.red.opacity(0.9);
    colors.danger_active = t.red.opacity(0.8);
    colors.danger_foreground = t.bg;
    colors.title_bar = t.panel;
    colors.title_bar_border = t.border;
    colors.list = t.panel2;
    colors.list_hover = t.hover;
    colors.list_active = t.sel;

    cx.set_global(ActiveKairnTheme(t));
    cx.refresh_windows();
}

static MONO_FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static UI_FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Whether `family` loads as itself rather than through gpui's fallback
/// stack. The round-trip matters: `resolve_font` never fails, so the only
/// tell is whether the id it returns maps back to the family asked for.
fn loads_as_itself(family: &str, cx: &App) -> bool {
    let ts = cx.text_system();
    let id = ts.resolve_font(&gpui::font(family.to_string()));
    ts.get_font_for_id(id).is_some_and(|f| f.family.as_ref() == family)
}

/// Resolve the fallback mono and UI fonts against the families actually
/// installed, once at startup. Asking for a family that isn't there has
/// two costs: gpui falls back per glyph with mismatched advance widths
/// (on Linux the terminal renders with broken letter spacing rather than
/// failing loudly), and every text run pays a formatted-error fallback
/// walk in `resolve_font` on every frame (found pegging the ThinkPad,
/// GDL-710).
pub fn resolve_fonts(cx: &App) {
    let installed: std::collections::HashSet<String> =
        cx.text_system().all_font_names().into_iter().collect();
    let pick = |candidates: &[&str], contains: &str, last: &str| {
        candidates
            .iter()
            .find(|c| installed.contains(**c))
            .map(|c| c.to_string())
            .or_else(|| {
                let mut close: Vec<&String> =
                    installed.iter().filter(|f| f.contains(contains)).collect();
                close.sort();
                close.first().map(|f| f.to_string())
            })
            .unwrap_or_else(|| last.to_string())
    };
    let mono = pick(
        &[
            "Menlo",
            "SF Mono",
            "JetBrains Mono",
            "Fira Code",
            "Hack",
            "Adwaita Mono",
            "DejaVu Sans Mono",
            "Noto Sans Mono",
            "Ubuntu Mono",
            "Liberation Mono",
        ],
        "Mono",
        "monospace",
    );
    let _ = MONO_FONT.set(mono);
    // The UI candidates are probed by actual resolution, not the installed
    // list: gpui appends its own fallback-stack names (and .SystemUIFont)
    // to `all_font_names` whether or not they exist, so membership there
    // proves nothing. .SystemUIFont only genuinely loads on macOS.
    let ui = [
        ".SystemUIFont",
        "Noto Sans",
        "Adwaita Sans",
        "Cantarell",
        "Ubuntu",
        "DejaVu Sans",
        "Liberation Sans",
        "Arial",
        "Helvetica",
    ]
    .iter()
    .find(|f| loads_as_itself(f, cx))
    .copied()
    .unwrap_or("Noto Sans");
    let _ = UI_FONT.set(ui.to_string());
}

/// The auto-resolved mono family: what themes and settings fall back to.
pub fn auto_mono() -> &'static str {
    MONO_FONT.get().map(String::as_str).unwrap_or("monospace")
}

/// The auto-resolved UI family: the system font on macOS, the best
/// installed sans elsewhere.
pub fn auto_ui() -> &'static str {
    UI_FONT.get().map(String::as_str).unwrap_or("Noto Sans")
}

/// Terminal colors: the sage-tinted ANSI ramp with any theme-file
/// overrides on top. The stock ramp's background is the theme's terminal
/// background, so the terminal follows the theme even without overrides.
fn terminal_palette(spec: &ThemeTerminal, bg: (u8, u8, u8)) -> ColorPalette {
    let over = |o: &Option<String>, d: (u8, u8, u8)| {
        o.as_deref()
            .and_then(parse_hex_color)
            .map(|[r, g, b, _]| (r, g, b))
            .unwrap_or(d)
    };
    let background = over(&spec.background, bg);
    let foreground = over(&spec.foreground, (0xc9, 0xcc, 0xbf));
    let cursor = over(&spec.cursor, (0xa8, 0xb4, 0x8d));
    let black = over(&spec.black, (0x10, 0x10, 0x10));
    let red = over(&spec.red, (0xc9, 0x7b, 0x6d));
    let green = over(&spec.green, (0xa8, 0xb4, 0x8d));
    let yellow = over(&spec.yellow, (0xd9, 0xa7, 0x5c));
    let blue = over(&spec.blue, (0xa3, 0xb8, 0xef));
    let magenta = over(&spec.magenta, (0xe6, 0xa3, 0xdc));
    let cyan = over(&spec.cyan, (0x50, 0xca, 0xcd));
    let white = over(&spec.white, (0xb0, 0xb0, 0xb0));
    let bright_black = over(&spec.bright_black, (0x5d, 0x61, 0x56));
    let bright_red = over(&spec.bright_red, (0xf2, 0xb4, 0xb0));
    let bright_green = over(&spec.bright_green, (0xbc, 0xc8, 0xa0));
    let bright_yellow = over(&spec.bright_yellow, (0xe8, 0xc0, 0x84));
    let bright_blue = over(&spec.bright_blue, (0xb8, 0xc8, 0xf4));
    let bright_magenta = over(&spec.bright_magenta, (0xf2, 0xb8, 0xe8));
    let bright_cyan = over(&spec.bright_cyan, (0x74, 0xd8, 0xdc));
    let bright_white = over(&spec.bright_white, (0xe0, 0xe0, 0xe0));
    ColorPalette::builder()
        .background(background.0, background.1, background.2)
        .foreground(foreground.0, foreground.1, foreground.2)
        .cursor(cursor.0, cursor.1, cursor.2)
        .black(black.0, black.1, black.2)
        .red(red.0, red.1, red.2)
        .green(green.0, green.1, green.2)
        .yellow(yellow.0, yellow.1, yellow.2)
        .blue(blue.0, blue.1, blue.2)
        .magenta(magenta.0, magenta.1, magenta.2)
        .cyan(cyan.0, cyan.1, cyan.2)
        .white(white.0, white.1, white.2)
        .bright_black(bright_black.0, bright_black.1, bright_black.2)
        .bright_red(bright_red.0, bright_red.1, bright_red.2)
        .bright_green(bright_green.0, bright_green.1, bright_green.2)
        .bright_yellow(bright_yellow.0, bright_yellow.1, bright_yellow.2)
        .bright_blue(bright_blue.0, bright_blue.1, bright_blue.2)
        .bright_magenta(bright_magenta.0, bright_magenta.1, bright_magenta.2)
        .bright_cyan(bright_cyan.0, bright_cyan.1, bright_cyan.2)
        .bright_white(bright_white.0, bright_white.1, bright_white.2)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menlo_is_the_leading_theme_and_parses() {
        assert_eq!(BUILTIN_THEMES[0], ("menlo", "Menlo"));
        let spec: ThemeSpec =
            serde_json::from_str(MENLO_SPEC).expect("embedded menlo theme parses");
        assert_eq!(spec.name, "Menlo");
        let t = KairnTheme::from_spec(&spec);
        assert_eq!(t.mode, Mode::Dark);
        assert_eq!(t.editor_font.as_ref().map(|f| f.as_ref()), Some("Menlo"));
        assert_eq!(t.mono_font.as_ref(), "Menlo");
    }

    #[test]
    fn every_builtin_theme_resolves() {
        for (id, _) in BUILTIN_THEMES {
            let resolves = matches!(*id, "dark" | "light") || preset(id).is_some();
            assert!(resolves, "theme {id} did not resolve");
        }
    }
}
