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

/// Bind a letter primary chord. Cmd on macOS. On Linux, Super (the Windows
/// key) is the primary, matching macOS Cmd and the "Super acts like Cmd"
/// convention Omarchy and similar setups adopt; Ctrl+Shift stays bound as a
/// fallback for desktops (stock GNOME/KDE) that reserve Super for the
/// compositor. Plain Ctrl+letter is never usable here: those are shell control
/// characters in a focused terminal (Ctrl+J accept-line, Ctrl+N next-history,
/// Ctrl+Q XON resume). `tail` is a bare letter (`"c"`) or a shifted letter
/// (`"shift-k"`); the Ctrl+Shift fallback matches the pre-Super chord exactly.
fn letter<A: gpui::Action + Clone>(
    keys: &mut Vec<KeyBinding>,
    tail: &str,
    action: A,
    context: Option<&'static str>,
) {
    if cfg!(target_os = "macos") {
        keys.push(KeyBinding::new(&format!("cmd-{tail}"), action, context));
    } else {
        let fallback = if tail.starts_with("shift-") {
            format!("ctrl-{tail}")
        } else {
            format!("ctrl-shift-{tail}")
        };
        keys.push(KeyBinding::new(&format!("super-{tail}"), action.clone(), context));
        keys.push(KeyBinding::new(&fallback, action, context));
    }
}

pub fn init(cx: &mut App) {
    // Non-letter primary chords: Cmd on macOS, Ctrl on Linux. Digits and
    // punctuation stay plain Ctrl on Linux: the terminal emits nothing for
    // them, and shifted punctuation resolves to a different key per layout
    // (Ctrl+Shift+\ arrives as ctrl-|), so it can't be bound reliably. Letter
    // chords go through `letter()` instead. Digits deliberately stay Ctrl, not
    // Super: Super+1..9 is workspace switching on most Linux compositors
    // (Hyprland/Omarchy included).
    let p = |k: &str| {
        if cfg!(target_os = "macos") {
            format!("cmd-{k}")
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
    let mut keys = vec![
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
        KeyBinding::new(&pa("1"), LayoutWriting, None),
        KeyBinding::new(&pa("2"), LayoutNotes, None),
        KeyBinding::new(&pa("3"), LayoutSplit, None),
        KeyBinding::new(&pa("4"), LayoutTerminal, None),
        KeyBinding::new(&p(","), OpenSettings, None),
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
        KeyBinding::new("shift-left", EditorSelectLeft, Some("NoteEditor")),
        KeyBinding::new("shift-right", EditorSelectRight, Some("NoteEditor")),
        KeyBinding::new("shift-up", EditorSelectUp, Some("NoteEditor")),
        KeyBinding::new("shift-down", EditorSelectDown, Some("NoteEditor")),
        // Line start/end on the dedicated keys, both platforms.
        KeyBinding::new("home", EditorLineStart, Some("NoteEditor")),
        KeyBinding::new("end", EditorLineEnd, Some("NoteEditor")),
        KeyBinding::new("shift-home", EditorSelectLineStart, Some("NoteEditor")),
        KeyBinding::new("shift-end", EditorSelectLineEnd, Some("NoteEditor")),
    ];
    // Letter primary chords (see `letter` for the Super/Ctrl+Shift split).
    letter(&mut keys, "j", ToggleSwitcher, None);
    letter(&mut keys, "shift-k", Capture, None);
    letter(&mut keys, "s", SaveNote, None);
    letter(&mut keys, "n", NewLocalSession, None);
    letter(&mut keys, "q", Quit, None);
    // Undo/redo live in the Workspace context, not the editor's: the workspace
    // coordinates buffer undo with its cross-note move history, and the chord
    // must work right after a mouse drag, when nothing keyboard-focused holds
    // the editor context. Text inputs keep their own undo (gpui-component binds
    // it in the deeper Input context, which wins).
    letter(&mut keys, "z", EditorUndo, Some("Workspace"));
    // Redo is Shift + the undo chord: Super+Shift+Z / Cmd+Shift+Z. On Linux the
    // undo *fallback* is already Ctrl+Shift+Z, so redo's fallback takes
    // Ctrl+Shift+Y to avoid colliding with it.
    if cfg!(target_os = "macos") {
        keys.push(KeyBinding::new("cmd-shift-z", EditorRedo, Some("Workspace")));
    } else {
        keys.push(KeyBinding::new("super-shift-z", EditorRedo, Some("Workspace")));
        keys.push(KeyBinding::new("ctrl-shift-y", EditorRedo, Some("Workspace")));
    }
    letter(&mut keys, "v", EditorPaste, Some("NoteEditor"));
    letter(&mut keys, "c", EditorCopy, Some("NoteEditor"));
    letter(&mut keys, "x", EditorCut, Some("NoteEditor"));
    letter(&mut keys, "a", EditorSelectAll, Some("NoteEditor"));
    cx.bind_keys(keys);
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
                (chord_alt("1"), "Writing layout"),
                (chord_alt("2"), "Notes layout"),
                (chord_alt("3"), "Notes + terminal layout"),
                (chord_alt("4"), "Terminal layout"),
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
                (chord("Z"), "Undo (edits and task moves)"),
                (
                    if cfg!(target_os = "macos") {
                        chord_shift("Z")
                    } else {
                        "Super+Shift+Z".to_string()
                    },
                    "Redo",
                ),
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
// Linux. Letter chords ride Super (the primary, matching `init`); digits,
// punctuation, and Enter combos stay on Ctrl / Ctrl+Shift. The Ctrl+Shift
// fallback that `init` also binds for letters is intentionally not labelled.

/// Whether `key` is a single letter (Super chord) rather than a digit,
/// punctuation, or a named key like `⏎` (Ctrl chord).
fn is_letter(key: &str) -> bool {
    key.chars().count() == 1 && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// Primary chord: `⌘J` / `Super+J` (letters), `⌘1` / `Ctrl+1` (digits).
pub fn chord(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else if is_letter(key) {
        format!("Super+{}", key.to_uppercase())
    } else {
        format!("Ctrl+{}", linux_key(key))
    }
}

/// Primary+Shift chord: `⇧⌘K` / `Super+Shift+K` (letters), `⇧⌘⏎` /
/// `Ctrl+Shift+Enter` (other keys).
pub fn chord_shift(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⇧⌘{key}")
    } else if is_letter(key) {
        format!("Super+Shift+{}", key.to_uppercase())
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
