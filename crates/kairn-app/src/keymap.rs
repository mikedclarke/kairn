//! Actions, key bindings, and the platform-correct chord labels.

use gpui::{App, KeyBinding, actions};

actions!(
    kairn,
    [
        ToggleSidebar,
        ToggleTerminalFull,
        ToggleWriting,
        ToggleSwitcher,
        CloseOverlay,
        ToggleThemeMode,
        OpenSettings,
        Capture,
        SaveNote,
        NewLocalSession,
        Quit,
        InputUp,
        InputDown,
        LineEditLeft,
        LineEditRight,
        LineEditBackspace,
        LineEditDelete,
        Session1,
        Session2,
        Session3,
        Session4,
        Session5,
        Session6,
        Session7,
        Session8,
        Session9
    ]
);

pub fn init(cx: &mut App) {
    // Primary chords: Cmd on macOS, Ctrl on Linux. On Linux, plain Ctrl+letter
    // combos are shell control characters (Ctrl+J accept-line, Ctrl+N
    // next-history, Ctrl+Q XON resume) and bindings win over the terminal, so
    // letter chords take Ctrl+Shift instead (the GNOME Terminal / VS Code
    // convention). Digits and punctuation stay plain Ctrl: the terminal emits
    // nothing for them, and shifted punctuation resolves to a different key
    // per layout (Ctrl+Shift+\ arrives as ctrl-|), so it can't be bound
    // reliably.
    let p = |k: &str| {
        if cfg!(target_os = "macos") {
            format!("cmd-{k}")
        } else if k.len() == 1 && k.chars().next().unwrap().is_ascii_alphabetic() {
            format!("ctrl-shift-{k}")
        } else {
            format!("ctrl-{k}")
        }
    };
    cx.bind_keys([
        KeyBinding::new(&p("\\"), ToggleSidebar, None),
        KeyBinding::new(&p("shift-enter"), ToggleTerminalFull, None),
        KeyBinding::new(&p("alt-enter"), ToggleWriting, None),
        KeyBinding::new(&p("j"), ToggleSwitcher, None),
        KeyBinding::new(&p(","), OpenSettings, None),
        KeyBinding::new(&p("shift-k"), Capture, None),
        KeyBinding::new(&p("s"), SaveNote, None),
        KeyBinding::new(&p("n"), NewLocalSession, None),
        KeyBinding::new(&p("q"), Quit, None),
        KeyBinding::new(&p("1"), Session1, None),
        KeyBinding::new(&p("2"), Session2, None),
        KeyBinding::new(&p("3"), Session3, None),
        KeyBinding::new(&p("4"), Session4, None),
        KeyBinding::new(&p("5"), Session5, None),
        KeyBinding::new(&p("6"), Session6, None),
        KeyBinding::new(&p("7"), Session7, None),
        KeyBinding::new(&p("8"), Session8, None),
        KeyBinding::new(&p("9"), Session9, None),
        KeyBinding::new("escape", CloseOverlay, Some("Overlay")),
        // Movement keys inside any Input, bound in the Input context AFTER
        // gpui-component's own bindings so they match first. Whichever
        // surface is active handles them (the line editor moves the edit
        // across lines, the switcher moves its selection); anywhere else the
        // handler propagates and the input's normal binding runs instead.
        KeyBinding::new("up", InputUp, Some("Input")),
        KeyBinding::new("down", InputDown, Some("Input")),
        KeyBinding::new("left", LineEditLeft, Some("Input")),
        KeyBinding::new("right", LineEditRight, Some("Input")),
        KeyBinding::new("backspace", LineEditBackspace, Some("Input")),
        KeyBinding::new("delete", LineEditDelete, Some("Input")),
    ]);
}

// Every keybinding hint in the app goes through the chord family below, so
// labels stay platform-correct: mac glyphs on macOS, plain modifier words on
// Linux (where letter chords ride Ctrl+Shift, matching `init`).

/// Primary chord: `⌘J` / `Ctrl+Shift+J`, `⌘1` / `Ctrl+1`.
pub fn chord(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else if key.chars().count() == 1 && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        format!("Ctrl+Shift+{}", key.to_uppercase())
    } else {
        format!("Ctrl+{}", linux_key(key))
    }
}

/// Primary+Shift chord: `⇧⌘⏎` / `Ctrl+Shift+Enter`.
pub fn chord_shift(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⇧⌘{key}")
    } else {
        format!("Ctrl+Shift+{}", linux_key(key))
    }
}

/// Primary+Alt chord: `⌥⌘⏎` / `Ctrl+Alt+Enter`.
pub fn chord_alt(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌥⌘{key}")
    } else {
        format!("Ctrl+Alt+{}", linux_key(key))
    }
}

fn linux_key(key: &str) -> String {
    match key {
        "⏎" => "Enter".to_string(),
        k => k.to_uppercase(),
    }
}
