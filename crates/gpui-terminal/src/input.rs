//! Keyboard input handling for the terminal emulator.
//!
//! This module provides [`keystroke_to_bytes`], which converts GPUI keyboard
//! events into terminal escape sequences that can be written to the PTY.
//!
//! # Key Mappings
//!
//! ## Special Keys
//!
//! | Key | Sequence | Notes |
//! |-----|----------|-------|
//! | Enter | `\r` (0x0D) | Carriage return; Alt prefixes ESC |
//! | Escape | `\x1b` (0x1B) | ESC; Alt sends ESC ESC |
//! | Backspace | `\x7f` (0x7F) | DEL; Alt prefixes ESC |
//! | Tab | `\t` (0x09) | Horizontal tab |
//! | Shift+Tab | `\x1b[Z` | Backtab |
//! | Space | ` ` (0x20) | Space |
//! | Ctrl+Space | `\x00` | NUL |
//!
//! ## Arrow Keys
//!
//! Arrow key sequences depend on application cursor mode:
//!
//! | Key | Normal Mode | App Cursor Mode |
//! |-----|-------------|-----------------|
//! | Up | `\x1b[A` | `\x1bOA` |
//! | Down | `\x1b[B` | `\x1bOB` |
//! | Right | `\x1b[C` | `\x1bOC` |
//! | Left | `\x1b[D` | `\x1bOD` |
//!
//! ## Navigation Keys
//!
//! | Key | Sequence |
//! |-----|----------|
//! | Home | `\x1b[H` |
//! | End | `\x1b[F` |
//! | PageUp | `\x1b[5~` |
//! | PageDown | `\x1b[6~` |
//! | Insert | `\x1b[2~` |
//! | Delete | `\x1b[3~` |
//!
//! ## Function Keys
//!
//! | Key | Sequence |
//! |-----|----------|
//! | F1-F4 | `\x1bOP` - `\x1bOS` |
//! | F5-F12 | `\x1b[15~` - `\x1b[24~` |
//!
//! ## Modified Special Keys
//!
//! Shift, Alt, and Ctrl on arrows, navigation, and function keys use the
//! xterm modifier parameter `1 + shift(1) + alt(2) + ctrl(4)`:
//!
//! | Combination | Sequence |
//! |-------------|----------|
//! | Ctrl+Right | `\x1b[1;5C` |
//! | Shift+Up | `\x1b[1;2A` |
//! | Alt+Delete | `\x1b[3;3~` |
//! | Ctrl+F5 | `\x1b[15;5~` |
//!
//! Modified arrows always use the CSI form, even in application cursor mode,
//! matching xterm.
//!
//! ## Control Combinations
//!
//! Ctrl+A through Ctrl+Z map to ASCII control characters 0x01-0x1A:
//!
//! | Combination | Byte |
//! |-------------|------|
//! | Ctrl+A | 0x01 |
//! | Ctrl+C | 0x03 (interrupt) |
//! | Ctrl+D | 0x04 (EOF) |
//! | Ctrl+Z | 0x1A (suspend) |
//!
//! ## Alt Combinations
//!
//! Alt+key sends ESC followed by the key: `\x1b` + key. Ctrl+Alt+key sends
//! ESC followed by the control byte.
//!
//! # Terminal Mode Effects
//!
//! The [`TermMode`] flags affect key sequences:
//!
//! - **APP_CURSOR**: Changes unmodified arrow key sequences from CSI to SS3
//!   format
//!
//! # Example
//!
//! ```
//! use gpui::Keystroke;
//! use alacritty_terminal::term::TermMode;
//! use gpui_terminal::input::keystroke_to_bytes;
//!
//! // Enter key
//! let keystroke = Keystroke::parse("enter").unwrap();
//! assert_eq!(keystroke_to_bytes(&keystroke, TermMode::empty()), Some(b"\r".to_vec()));
//!
//! // Ctrl+C (interrupt)
//! let keystroke = Keystroke::parse("ctrl-c").unwrap();
//! assert_eq!(keystroke_to_bytes(&keystroke, TermMode::empty()), Some(vec![0x03]));
//! ```

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// Convert a GPUI keystroke to terminal escape sequence bytes.
///
/// This function translates GPUI keyboard events into the appropriate byte sequences
/// expected by terminal applications. It handles special keys, control characters,
/// modified keys (xterm modifier encoding), and application cursor mode.
///
/// # Arguments
///
/// * `keystroke` - The GPUI keystroke to convert
/// * `mode` - The current terminal mode (affects arrow key sequences)
///
/// # Returns
///
/// An optional vector of bytes representing the terminal escape sequence.
/// Returns `None` if the keystroke should not produce any output.
///
/// # Examples
///
/// ```
/// use gpui::Keystroke;
/// use alacritty_terminal::term::TermMode;
/// use gpui_terminal::input::keystroke_to_bytes;
///
/// let keystroke = Keystroke::parse("enter").unwrap();
/// let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
/// assert_eq!(bytes, Some(b"\r".to_vec()));
/// ```
pub fn keystroke_to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let mods = keystroke.modifiers;
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let param = 1 + u8::from(mods.shift) + u8::from(mods.alt) * 2 + u8::from(mods.control) * 4;
    let modified = param > 1;

    // CSI keys whose final byte is a letter: `\x1b[H`, or `\x1b[1;5H` when
    // a modifier is held.
    let csi_letter = |letter: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{param}{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    // CSI keys with a numeric code and `~` final: `\x1b[3~`, or `\x1b[3;3~`
    // when a modifier is held.
    let csi_tilde = |num: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{num};{param}~").into_bytes()
        } else {
            format!("\x1b[{num}~").into_bytes()
        }
    };

    // Handle special keys first
    match keystroke.key.as_str() {
        // Basic control characters. Alt prefixes ESC (meta), matching xterm.
        "space" => {
            let mut bytes = Vec::new();
            if mods.alt {
                bytes.push(0x1b);
            }
            bytes.push(if mods.control { 0x00 } else { b' ' });
            return Some(bytes);
        }
        "enter" => {
            if mods.alt {
                return Some(b"\x1b\r".to_vec());
            }
            return Some(b"\r".to_vec());
        }
        "escape" => {
            if mods.alt {
                return Some(b"\x1b\x1b".to_vec());
            }
            return Some(b"\x1b".to_vec());
        }
        "backspace" => {
            if mods.alt {
                return Some(b"\x1b\x7f".to_vec());
            }
            return Some(b"\x7f".to_vec());
        }
        "tab" => {
            // Shift+Tab sends a different sequence
            if mods.shift {
                return Some(b"\x1b[Z".to_vec());
            }
            return Some(b"\t".to_vec());
        }

        // Arrow keys - modified arrows always take the CSI form with the
        // xterm modifier parameter; unmodified arrows check APP_CURSOR mode.
        "up" | "down" | "right" | "left" => {
            let letter = match keystroke.key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                _ => 'D',
            };
            if modified {
                return Some(csi_letter(letter));
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(format!("\x1bO{letter}").into_bytes());
            }
            return Some(format!("\x1b[{letter}").into_bytes());
        }

        // Navigation keys
        "home" => return Some(csi_letter('H')),
        "end" => return Some(csi_letter('F')),
        "pageup" => return Some(csi_tilde(5)),
        "pagedown" => return Some(csi_tilde(6)),
        "insert" => return Some(csi_tilde(2)),
        "delete" => return Some(csi_tilde(3)),

        // Function keys - unmodified F1-F4 use SS3, modified use CSI,
        // matching xterm.
        "f1" | "f2" | "f3" | "f4" => {
            let letter = match keystroke.key.as_str() {
                "f1" => 'P',
                "f2" => 'Q',
                "f3" => 'R',
                _ => 'S',
            };
            if modified {
                return Some(csi_letter(letter));
            }
            return Some(format!("\x1bO{letter}").into_bytes());
        }
        "f5" => return Some(csi_tilde(15)),
        "f6" => return Some(csi_tilde(17)),
        "f7" => return Some(csi_tilde(18)),
        "f8" => return Some(csi_tilde(19)),
        "f9" => return Some(csi_tilde(20)),
        "f10" => return Some(csi_tilde(21)),
        "f11" => return Some(csi_tilde(23)),
        "f12" => return Some(csi_tilde(24)),

        _ => {}
    }

    // Handle Ctrl+key combinations. Alt on top prefixes ESC.
    if mods.control {
        let key = keystroke.key.as_str();

        if key.len() == 1 {
            let ch = key.chars().next().unwrap();

            // Ctrl+A through Ctrl+Z map to 0x01 through 0x1a; the rest are
            // the special punctuation control combinations.
            let ctrl_byte = if ch.is_ascii_alphabetic() {
                Some((ch.to_ascii_uppercase() as u8) - b'@')
            } else {
                match ch {
                    '[' => Some(0x1b),  // Ctrl+[
                    '\\' => Some(0x1c), // Ctrl+\
                    ']' => Some(0x1d),  // Ctrl+]
                    '^' => Some(0x1e),  // Ctrl+^
                    '_' => Some(0x1f),  // Ctrl+_
                    '?' => Some(0x7f),  // Ctrl+?
                    _ => None,
                }
            };
            if let Some(byte) = ctrl_byte {
                let mut bytes = Vec::new();
                if mods.alt {
                    bytes.push(0x1b);
                }
                bytes.push(byte);
                return Some(bytes);
            }
        }
    }

    // Handle Alt+key combinations
    if mods.alt {
        let key = keystroke.key.as_str();
        if key.len() == 1 {
            // Alt+key sends ESC followed by the key
            let ch = key.chars().next().unwrap();
            if ch.is_ascii() {
                let mut bytes = vec![b'\x1b'];
                bytes.push(ch as u8);
                return Some(bytes);
            }
        }
    }

    // Handle regular printable characters
    // Use key_char if available (contains the actual typed character with modifiers like Shift)
    if let Some(key_char) = &keystroke.key_char
        && !mods.control
        && !mods.alt
    {
        return Some(key_char.as_bytes().to_vec());
    }

    // Fallback to key for single characters
    let key = keystroke.key.as_str();
    if key.len() == 1 {
        let ch = key.chars().next().unwrap();
        if ch.is_ascii() && !mods.control {
            // Handle shift modifier for uppercase
            let ch = if mods.shift {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            return Some(vec![ch as u8]);
        }
        // For non-ASCII characters, encode as UTF-8
        if !mods.control && !mods.alt {
            return Some(key.as_bytes().to_vec());
        }
    }

    // If we get here, the keystroke doesn't produce any output
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enter_key() {
        let keystroke = Keystroke::parse("enter").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\r".to_vec()));
    }

    #[test]
    fn test_escape_key() {
        let keystroke = Keystroke::parse("escape").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x1b".to_vec()));
    }

    #[test]
    fn test_backspace_key() {
        let keystroke = Keystroke::parse("backspace").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x7f".to_vec()));
    }

    #[test]
    fn test_tab_key() {
        let keystroke = Keystroke::parse("tab").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\t".to_vec()));
    }

    #[test]
    fn test_shift_tab() {
        let keystroke = Keystroke::parse("shift-tab").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn test_arrow_keys_normal_mode() {
        let mode = TermMode::empty();

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_to_bytes(&up, mode), Some(b"\x1b[A".to_vec()));

        let down = Keystroke::parse("down").unwrap();
        assert_eq!(keystroke_to_bytes(&down, mode), Some(b"\x1b[B".to_vec()));

        let right = Keystroke::parse("right").unwrap();
        assert_eq!(keystroke_to_bytes(&right, mode), Some(b"\x1b[C".to_vec()));

        let left = Keystroke::parse("left").unwrap();
        assert_eq!(keystroke_to_bytes(&left, mode), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn test_arrow_keys_app_cursor_mode() {
        let mode = TermMode::APP_CURSOR;

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_to_bytes(&up, mode), Some(b"\x1bOA".to_vec()));

        let down = Keystroke::parse("down").unwrap();
        assert_eq!(keystroke_to_bytes(&down, mode), Some(b"\x1bOB".to_vec()));

        let right = Keystroke::parse("right").unwrap();
        assert_eq!(keystroke_to_bytes(&right, mode), Some(b"\x1bOC".to_vec()));

        let left = Keystroke::parse("left").unwrap();
        assert_eq!(keystroke_to_bytes(&left, mode), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn test_modified_arrow_keys() {
        let mode = TermMode::empty();

        // Ctrl+Right = word forward in most shells
        let ctrl_right = Keystroke::parse("ctrl-right").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_right, mode),
            Some(b"\x1b[1;5C".to_vec())
        );

        // Shift+Up = extend selection in TUIs
        let shift_up = Keystroke::parse("shift-up").unwrap();
        assert_eq!(
            keystroke_to_bytes(&shift_up, mode),
            Some(b"\x1b[1;2A".to_vec())
        );

        // Alt+Left
        let alt_left = Keystroke::parse("alt-left").unwrap();
        assert_eq!(
            keystroke_to_bytes(&alt_left, mode),
            Some(b"\x1b[1;3D".to_vec())
        );

        // Ctrl+Shift+Down combines to parameter 6
        let ctrl_shift_down = Keystroke::parse("ctrl-shift-down").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_shift_down, mode),
            Some(b"\x1b[1;6B".to_vec())
        );
    }

    #[test]
    fn test_modified_arrows_ignore_app_cursor_mode() {
        // xterm sends the CSI form for modified arrows even in application
        // cursor mode.
        let mode = TermMode::APP_CURSOR;

        let ctrl_up = Keystroke::parse("ctrl-up").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_up, mode),
            Some(b"\x1b[1;5A".to_vec())
        );
    }

    #[test]
    fn test_navigation_keys() {
        let mode = TermMode::empty();

        let home = Keystroke::parse("home").unwrap();
        assert_eq!(keystroke_to_bytes(&home, mode), Some(b"\x1b[H".to_vec()));

        let end = Keystroke::parse("end").unwrap();
        assert_eq!(keystroke_to_bytes(&end, mode), Some(b"\x1b[F".to_vec()));

        let pageup = Keystroke::parse("pageup").unwrap();
        assert_eq!(keystroke_to_bytes(&pageup, mode), Some(b"\x1b[5~".to_vec()));

        let pagedown = Keystroke::parse("pagedown").unwrap();
        assert_eq!(
            keystroke_to_bytes(&pagedown, mode),
            Some(b"\x1b[6~".to_vec())
        );

        let insert = Keystroke::parse("insert").unwrap();
        assert_eq!(keystroke_to_bytes(&insert, mode), Some(b"\x1b[2~".to_vec()));

        let delete = Keystroke::parse("delete").unwrap();
        assert_eq!(keystroke_to_bytes(&delete, mode), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn test_modified_navigation_keys() {
        let mode = TermMode::empty();

        // Ctrl+Home = top of buffer
        let ctrl_home = Keystroke::parse("ctrl-home").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_home, mode),
            Some(b"\x1b[1;5H".to_vec())
        );

        // Alt+Delete
        let alt_delete = Keystroke::parse("alt-delete").unwrap();
        assert_eq!(
            keystroke_to_bytes(&alt_delete, mode),
            Some(b"\x1b[3;3~".to_vec())
        );

        // Shift+PageUp
        let shift_pageup = Keystroke::parse("shift-pageup").unwrap();
        assert_eq!(
            keystroke_to_bytes(&shift_pageup, mode),
            Some(b"\x1b[5;2~".to_vec())
        );
    }

    #[test]
    fn test_function_keys() {
        let mode = TermMode::empty();

        let f1 = Keystroke::parse("f1").unwrap();
        assert_eq!(keystroke_to_bytes(&f1, mode), Some(b"\x1bOP".to_vec()));

        let f2 = Keystroke::parse("f2").unwrap();
        assert_eq!(keystroke_to_bytes(&f2, mode), Some(b"\x1bOQ".to_vec()));

        let f5 = Keystroke::parse("f5").unwrap();
        assert_eq!(keystroke_to_bytes(&f5, mode), Some(b"\x1b[15~".to_vec()));

        let f12 = Keystroke::parse("f12").unwrap();
        assert_eq!(keystroke_to_bytes(&f12, mode), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn test_modified_function_keys() {
        let mode = TermMode::empty();

        // Modified F1-F4 switch from SS3 to CSI
        let ctrl_f1 = Keystroke::parse("ctrl-f1").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_f1, mode),
            Some(b"\x1b[1;5P".to_vec())
        );

        let ctrl_f5 = Keystroke::parse("ctrl-f5").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_f5, mode),
            Some(b"\x1b[15;5~".to_vec())
        );
    }

    #[test]
    fn test_ctrl_combinations() {
        let mode = TermMode::empty();

        // Ctrl+A = 0x01
        let ctrl_a = Keystroke::parse("ctrl-a").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_a, mode), Some(vec![0x01]));

        // Ctrl+C = 0x03
        let ctrl_c = Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_c, mode), Some(vec![0x03]));

        // Ctrl+Z = 0x1a
        let ctrl_z = Keystroke::parse("ctrl-z").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_z, mode), Some(vec![0x1a]));

        // Ctrl+Space = 0x00
        let ctrl_space = Keystroke::parse("ctrl-space").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_space, mode), Some(vec![0x00]));
    }

    #[test]
    fn test_ctrl_alt_combinations() {
        let mode = TermMode::empty();

        // Ctrl+Alt+A = ESC then 0x01
        let ctrl_alt_a = Keystroke::parse("ctrl-alt-a").unwrap();
        assert_eq!(
            keystroke_to_bytes(&ctrl_alt_a, mode),
            Some(vec![0x1b, 0x01])
        );
    }

    #[test]
    fn test_alt_combinations() {
        let mode = TermMode::empty();

        // Alt+a sends ESC followed by 'a'
        let alt_a = Keystroke::parse("alt-a").unwrap();
        assert_eq!(keystroke_to_bytes(&alt_a, mode), Some(b"\x1ba".to_vec()));

        // Alt+x sends ESC followed by 'x'
        let alt_x = Keystroke::parse("alt-x").unwrap();
        assert_eq!(keystroke_to_bytes(&alt_x, mode), Some(b"\x1bx".to_vec()));
    }

    #[test]
    fn test_alt_special_keys() {
        let mode = TermMode::empty();

        // Alt+Backspace = ESC DEL (delete word back in shells)
        let alt_backspace = Keystroke::parse("alt-backspace").unwrap();
        assert_eq!(
            keystroke_to_bytes(&alt_backspace, mode),
            Some(b"\x1b\x7f".to_vec())
        );

        // Alt+Enter = ESC CR
        let alt_enter = Keystroke::parse("alt-enter").unwrap();
        assert_eq!(
            keystroke_to_bytes(&alt_enter, mode),
            Some(b"\x1b\r".to_vec())
        );

        // Alt+Escape = ESC ESC
        let alt_escape = Keystroke::parse("alt-escape").unwrap();
        assert_eq!(
            keystroke_to_bytes(&alt_escape, mode),
            Some(b"\x1b\x1b".to_vec())
        );
    }

    #[test]
    fn test_regular_characters() {
        let mode = TermMode::empty();

        let a = Keystroke::parse("a").unwrap();
        assert_eq!(keystroke_to_bytes(&a, mode), Some(b"a".to_vec()));

        let z = Keystroke::parse("z").unwrap();
        assert_eq!(keystroke_to_bytes(&z, mode), Some(b"z".to_vec()));

        let zero = Keystroke::parse("0").unwrap();
        assert_eq!(keystroke_to_bytes(&zero, mode), Some(b"0".to_vec()));
    }

    #[test]
    fn test_space_key() {
        let mode = TermMode::empty();

        let space = Keystroke::parse("space").unwrap();
        assert_eq!(keystroke_to_bytes(&space, mode), Some(b" ".to_vec()));
    }
}
