//! Actions, key bindings, and the platform-correct chord labels.

use gpui::{App, KeyBinding, NoAction, actions};

actions!(
    kairn,
    [
        ToggleSidebar,
        ToggleTerminalFull,
        ToggleWriting,
        LayoutNotes,
        LayoutSplit,
        LayoutTerminal,
        LayoutWriting,
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
        EditorEnter,
        EditorBackspace,
        EditorDelete,
        EditorLeft,
        EditorRight,
        EditorUp,
        EditorDown,
        EditorUndo,
        EditorRedo,
        EditorPaste,
        EditorCopy,
        EditorCut,
        EditorSelectAll,
        EditorSelectLeft,
        EditorSelectRight,
        EditorSelectUp,
        EditorSelectDown,
        EditorWordLeft,
        EditorWordRight,
        EditorSelectWordLeft,
        EditorSelectWordRight,
        EditorLineStart,
        EditorLineEnd,
        EditorSelectLineStart,
        EditorSelectLineEnd,
        EditorDocStart,
        EditorDocEnd,
        EditorSelectDocStart,
        EditorSelectDocEnd,
        EditorDeleteWordBack,
        EditorDeleteToLineStart,
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
    // Primary+Alt chords (labelled by `chord_alt`): Cmd+Option on macOS,
    // Ctrl+Alt on Linux, where they are free on both the shell and the
    // desktop for digits.
    let pa = |k: &str| {
        if cfg!(target_os = "macos") {
            format!("cmd-alt-{k}")
        } else {
            format!("ctrl-alt-{k}")
        }
    };
    cx.bind_keys([
        // gpui-component's Root binds tab/shift-tab to focus cycling, which
        // consumes them before a focused terminal can forward them to the
        // PTY (Tab completion, backtab \x1b[Z). A NoAction binding in the
        // deeper Terminal context disables the Root binding there, letting
        // the raw keystrokes fall through to the terminal's key handler.
        KeyBinding::new("tab", NoAction, Some("Terminal")),
        KeyBinding::new("shift-tab", NoAction, Some("Terminal")),
        KeyBinding::new(&p("\\"), ToggleSidebar, None),
        KeyBinding::new(&p("shift-enter"), ToggleTerminalFull, None),
        KeyBinding::new(&p("alt-enter"), ToggleWriting, None),
        // Direct layout chords, numbered in the titlebar switcher's
        // left-to-right order. Alt+digit so they can't collide with the
        // plain-digit session chords on either platform.
        KeyBinding::new(&pa("1"), LayoutNotes, None),
        KeyBinding::new(&pa("2"), LayoutSplit, None),
        KeyBinding::new(&pa("3"), LayoutTerminal, None),
        KeyBinding::new(&pa("4"), LayoutWriting, None),
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
        // gpui-component's own bindings so they match first. The switcher
        // moves its selection; anywhere else the handler propagates and the
        // input's normal binding runs instead.
        KeyBinding::new("up", InputUp, Some("Input")),
        KeyBinding::new("down", InputDown, Some("Input")),
        // The single-buffer note editor: its own context so nothing here
        // collides with the Input-context bindings.
        KeyBinding::new("enter", EditorEnter, Some("NoteEditor")),
        KeyBinding::new("backspace", EditorBackspace, Some("NoteEditor")),
        KeyBinding::new("delete", EditorDelete, Some("NoteEditor")),
        KeyBinding::new("left", EditorLeft, Some("NoteEditor")),
        KeyBinding::new("right", EditorRight, Some("NoteEditor")),
        KeyBinding::new("up", EditorUp, Some("NoteEditor")),
        KeyBinding::new("down", EditorDown, Some("NoteEditor")),
        KeyBinding::new(&p("z"), EditorUndo, Some("NoteEditor")),
        KeyBinding::new(&p("shift-z"), EditorRedo, Some("NoteEditor")),
        KeyBinding::new(&p("v"), EditorPaste, Some("NoteEditor")),
        KeyBinding::new(&p("c"), EditorCopy, Some("NoteEditor")),
        KeyBinding::new(&p("x"), EditorCut, Some("NoteEditor")),
        KeyBinding::new(&p("a"), EditorSelectAll, Some("NoteEditor")),
        KeyBinding::new("shift-left", EditorSelectLeft, Some("NoteEditor")),
        KeyBinding::new("shift-right", EditorSelectRight, Some("NoteEditor")),
        KeyBinding::new("shift-up", EditorSelectUp, Some("NoteEditor")),
        KeyBinding::new("shift-down", EditorSelectDown, Some("NoteEditor")),
        // Line start/end on the dedicated keys, both platforms.
        KeyBinding::new("home", EditorLineStart, Some("NoteEditor")),
        KeyBinding::new("end", EditorLineEnd, Some("NoteEditor")),
        KeyBinding::new("shift-home", EditorSelectLineStart, Some("NoteEditor")),
        KeyBinding::new("shift-end", EditorSelectLineEnd, Some("NoteEditor")),
    ]);
    // Word, line, and document motions on each platform's native text
    // chords: Option/Cmd arrows on macOS, Ctrl arrows and Home/End on Linux.
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("alt-left", EditorWordLeft, Some("NoteEditor")),
            KeyBinding::new("alt-right", EditorWordRight, Some("NoteEditor")),
            KeyBinding::new("alt-shift-left", EditorSelectWordLeft, Some("NoteEditor")),
            KeyBinding::new("alt-shift-right", EditorSelectWordRight, Some("NoteEditor")),
            KeyBinding::new("cmd-left", EditorLineStart, Some("NoteEditor")),
            KeyBinding::new("cmd-right", EditorLineEnd, Some("NoteEditor")),
            KeyBinding::new("cmd-shift-left", EditorSelectLineStart, Some("NoteEditor")),
            KeyBinding::new("cmd-shift-right", EditorSelectLineEnd, Some("NoteEditor")),
            KeyBinding::new("cmd-up", EditorDocStart, Some("NoteEditor")),
            KeyBinding::new("cmd-down", EditorDocEnd, Some("NoteEditor")),
            KeyBinding::new("cmd-shift-up", EditorSelectDocStart, Some("NoteEditor")),
            KeyBinding::new("cmd-shift-down", EditorSelectDocEnd, Some("NoteEditor")),
            KeyBinding::new("alt-backspace", EditorDeleteWordBack, Some("NoteEditor")),
            KeyBinding::new("cmd-backspace", EditorDeleteToLineStart, Some("NoteEditor")),
        ]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-left", EditorWordLeft, Some("NoteEditor")),
            KeyBinding::new("ctrl-right", EditorWordRight, Some("NoteEditor")),
            KeyBinding::new("ctrl-shift-left", EditorSelectWordLeft, Some("NoteEditor")),
            KeyBinding::new("ctrl-shift-right", EditorSelectWordRight, Some("NoteEditor")),
            KeyBinding::new("ctrl-home", EditorDocStart, Some("NoteEditor")),
            KeyBinding::new("ctrl-end", EditorDocEnd, Some("NoteEditor")),
            KeyBinding::new("ctrl-shift-home", EditorSelectDocStart, Some("NoteEditor")),
            KeyBinding::new("ctrl-shift-end", EditorSelectDocEnd, Some("NoteEditor")),
            KeyBinding::new("ctrl-backspace", EditorDeleteWordBack, Some("NoteEditor")),
        ]);
    }
}

/// Every binding the app answers to, grouped for the settings Keybinds tab:
/// `(group, [(chord label, what it does)])`. Kept next to [`init`] so the
/// two lists can't drift apart silently.
pub fn keybind_list() -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    vec![
        (
            "App",
            vec![
                (chord("J"), "Jump to a session, day, or note"),
                (chord("\\"), "Show or hide the sidebar"),
                (chord(","), "Open Settings"),
                (chord_shift("K"), "Capture a thought into today's note"),
                (chord("Q"), "Quit"),
            ],
        ),
        (
            "Layout",
            vec![
                (chord_alt("1"), "Notes layout"),
                (chord_alt("2"), "Notes + terminal layout"),
                (chord_alt("3"), "Terminal layout"),
                (chord_alt("4"), "Writing layout"),
                (chord_shift("⏎"), "Full-screen terminal on/off"),
                (chord_alt("⏎"), "Writing mode on/off"),
            ],
        ),
        (
            "Sessions",
            vec![
                (chord("N"), "New local shell"),
                (chord("1–9"), "Switch to session 1–9"),
            ],
        ),
        (
            "Notes",
            vec![
                (chord("S"), "Save the open note now"),
                (chord("Z"), "Undo"),
                (chord_shift("Z"), "Redo"),
                (chord("C"), "Copy"),
                (chord("X"), "Cut"),
                (chord("V"), "Paste"),
                (chord("A"), "Select all"),
            ],
        ),
        (
            "Notes: cursor",
            if cfg!(target_os = "macos") {
                vec![
                    ("⌥← ⌥→".to_string(), "Previous or next word"),
                    ("⌘← ⌘→".to_string(), "Start or end of line"),
                    ("⌘↑ ⌘↓".to_string(), "Top or bottom of note"),
                    ("⇧ + any move".to_string(), "Extend the selection"),
                    ("⌥⌫".to_string(), "Delete the previous word"),
                    ("⌘⌫".to_string(), "Delete to the start of the line"),
                ]
            } else {
                vec![
                    ("Ctrl+← Ctrl+→".to_string(), "Previous or next word"),
                    ("Home End".to_string(), "Start or end of line"),
                    ("Ctrl+Home Ctrl+End".to_string(), "Top or bottom of note"),
                    ("Shift + any move".to_string(), "Extend the selection"),
                    ("Ctrl+⌫".to_string(), "Delete the previous word"),
                ]
            },
        ),
    ]
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
