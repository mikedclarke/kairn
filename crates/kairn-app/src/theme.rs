use gpui::{App, Global, Hsla, Window, rgb, rgba};
use gpui_component::theme::{Theme, ThemeMode};
use gpui_terminal::ColorPalette;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "light" => Mode::Light,
            _ => Mode::Dark,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        }
    }
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// The sage/amber palette from the locked design spec, one struct per mode.
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
}

impl KairnTheme {
    pub fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            bg: c(0x1a1b15),
            panel: c(0x20221a),
            panel2: c(0x262920),
            hover: c(0x2c2f25),
            border: c(0x31352a),
            text: c(0xdadcd1),
            dim: c(0x90957f),
            faint: c(0x5f6355),
            accent: c(0xa8b48d),
            amber: c(0xd9a75c),
            on_amber: c(0x1a1a14),
            red: c(0xc97b6d),
            term_bg: c(0x14150f),
            sel: ca(0xa8b48d26),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: Mode::Light,
            bg: c(0xf4f4ea),
            panel: c(0xebecdf),
            panel2: c(0xe3e4d6),
            hover: c(0xdcded0),
            border: c(0xd4d6c5),
            text: c(0x2c2e26),
            dim: c(0x6e7263),
            faint: c(0x9ba18b),
            accent: c(0x5f7247),
            amber: c(0xae7c2c),
            on_amber: c(0x1a1a14),
            red: c(0xa8574a),
            term_bg: c(0x22231c),
            sel: ca(0x5f724721),
        }
    }

    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        }
    }
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

/// Install the palette globally and skin gpui-component's widgets (dialogs,
/// inputs, buttons) to match, so the few stock components don't look foreign.
pub fn apply(mode: Mode, window: Option<&mut Window>, cx: &mut App) {
    let t = KairnTheme::for_mode(mode);

    Theme::change(
        match t.mode {
            Mode::Dark => ThemeMode::Dark,
            Mode::Light => ThemeMode::Light,
        },
        window,
        cx,
    );

    let theme = cx.global_mut::<Theme>();
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

/// Resolve the app's fonts against the families actually installed, once at
/// startup. Asking for a family that isn't there makes gpui fall back per
/// glyph with mismatched advance widths — on Linux the terminal renders with
/// broken letter spacing rather than failing loudly.
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
}

pub fn mono_font() -> &'static str {
    MONO_FONT.get().map(String::as_str).unwrap_or("monospace")
}

/// Terminal colors: sage-tinted ANSI ramp; the terminal stays dark in both
/// app themes (per the design spec), only its background shade follows.
pub fn terminal_palette(mode: Mode) -> ColorPalette {
    let (bg_r, bg_g, bg_b) = match mode {
        Mode::Dark => (0x14, 0x15, 0x0f),
        Mode::Light => (0x22, 0x23, 0x1c),
    };
    ColorPalette::builder()
        .background(bg_r, bg_g, bg_b)
        .foreground(0xc9, 0xcc, 0xbf)
        .cursor(0xa8, 0xb4, 0x8d)
        .black(0x10, 0x10, 0x10)
        .red(0xc9, 0x7b, 0x6d)
        .green(0xa8, 0xb4, 0x8d)
        .yellow(0xd9, 0xa7, 0x5c)
        .blue(0xa3, 0xb8, 0xef)
        .magenta(0xe6, 0xa3, 0xdc)
        .cyan(0x50, 0xca, 0xcd)
        .white(0xb0, 0xb0, 0xb0)
        .bright_black(0x5d, 0x61, 0x56)
        .bright_red(0xf2, 0xb4, 0xb0)
        .bright_green(0xbc, 0xc8, 0xa0)
        .bright_yellow(0xe8, 0xc0, 0x84)
        .bright_blue(0xb8, 0xc8, 0xf4)
        .bright_magenta(0xf2, 0xb8, 0xe8)
        .bright_cyan(0x74, 0xd8, 0xdc)
        .bright_white(0xe0, 0xe0, 0xe0)
        .build()
}
