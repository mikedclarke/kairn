//! The in-place line editor: the only editing model. Every commit path
//! goes through kairn-core's never-clobber write functions, and text that
//! cannot be saved is surfaced as an orphan banner, never dropped.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AppContext as _, Context, Pixels, Point, Window};
use gpui_component::input::{InputEvent, InputState, Position};
use kairn_core as notes;

use crate::keymap::{LineEditBackspace, LineEditDelete, LineEditLeft, LineEditRight, SaveNote};
use crate::workspace::{PaneView, Workspace};

/// An in-place edit of one rendered line: the raw markdown in a single-line
/// input sitting where the styled line was.
pub struct LineEdit {
    pub line_idx: usize,
    /// The line as last saved, for safe relocation if the file shifts.
    expected: String,
    /// This edit appends a line the file doesn't have yet.
    appending: bool,
    pub input: gpui::Entity<InputState>,
    path: PathBuf,
}

impl Workspace {
    /// Start editing a line of the pane document in place, cursor at the end.
    pub fn edit_line(&mut self, line_idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_line_at(line_idx, None, window, cx);
    }

    /// The raw-markdown cursor column (in characters) for a click at `pos` on
    /// rendered line `idx`, from the line's text layout. `None` falls back to
    /// the end of the line.
    pub fn line_click_col(&self, idx: usize, pos: Point<Pixels>) -> Option<usize> {
        let layouts = self.line_layouts.borrow();
        let layout = layouts.get(&idx)?;
        let display_ix = layout.index_for_position(pos).unwrap_or_else(|ix| ix);
        let display = layout.text();
        let display_chars = display
            .get(..display_ix)
            .map(|s| s.chars().count())
            .unwrap_or_else(|| display.chars().count());
        let raw = self.doc_text.as_deref()?.lines().nth(idx)?;
        let byte = notes::raw_col_for_display_char(raw, display_chars);
        Some(raw.get(..byte).map(|s| s.chars().count()).unwrap_or(0))
    }

    /// The link span (wiki link or date ref) under a click at `pos` on
    /// rendered line `idx`. Only exact hits count: a click in the empty space
    /// past a line must edit, never follow a link.
    pub fn line_click_link(&self, idx: usize, pos: Point<Pixels>) -> Option<notes::Span> {
        let layouts = self.line_layouts.borrow();
        let layout = layouts.get(&idx)?;
        let display_ix = layout.index_for_position(pos).ok()?;
        let display = layout.text();
        let display_chars = display.get(..display_ix)?.chars().count();
        let raw = self.doc_text.as_deref()?.lines().nth(idx)?;
        let span = notes::span_at_display_char(raw, display_chars)?;
        matches!(span.0, notes::SpanKind::WikiLink | notes::SpanKind::DateRef).then_some(span)
    }

    /// Start editing a line of the pane document in place, placing the cursor
    /// `cursor_chars` characters in (end of line when `None`). `line_idx` at
    /// or past the end starts a new line appended to the file.
    pub fn edit_line_at(
        &mut self,
        line_idx: usize,
        cursor_chars: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.view, PaneView::Tasks(_)) || self.root_missing {
            return;
        }
        self.commit_line_edit(true, cx);
        self.materialize_seed();
        let path = match &self.doc_path {
            Some(p) => p.clone(),
            None => match &self.view {
                PaneView::Day => notes::daily_path(&self.notes_root, self.selected_day),
                _ => return,
            },
        };
        // A file that exists but couldn't be read must not be edited.
        if self.doc_text.is_none() && path.exists() {
            return;
        }
        let line_count = self.doc_text.as_deref().map(|t| t.lines().count()).unwrap_or(0);
        let (line_idx, expected, appending) = if line_idx < line_count {
            let line = self
                .doc_text
                .as_deref()
                .and_then(|t| t.lines().nth(line_idx))
                .unwrap_or_default()
                .to_string();
            (line_idx, line, false)
        } else {
            (line_count, String::new(), true)
        };

        // Auto-grow so a wrapped paragraph is edited in full view instead of
        // scrolling horizontally through a single-line box. The note's line
        // model is untouched: Enter still commits/splits (the literal
        // newline the input inserts is consumed by the Enter handler).
        let input = cx.new(|cx| InputState::new(window, cx).auto_grow(1, 40));
        input.update(cx, |s, cx| {
            // set_value puts the cursor at the end on single-line inputs.
            s.set_value(expected.clone(), window, cx);
            if let Some(chars) = cursor_chars {
                // position_to_offset clamps past-the-end columns to the line.
                let col = chars.min(u32::MAX as usize) as u32;
                s.set_cursor_position(Position::new(0, col), window, cx);
            }
            s.focus(window, cx);
        });
        self._line_edit_sub = Some(cx.subscribe_in(
            &input,
            window,
            |this, state, ev: &InputEvent, window, cx| {
                // Events from an input this edit no longer owns are stale.
                let current = this.line_edit.as_ref().map(|le| le.input.entity_id());
                if current != Some(state.entity_id()) {
                    return;
                }
                match ev {
                    InputEvent::Change => {
                        // A newline can only arrive via paste (Enter is
                        // handled below before it lands): commit it as a
                        // real line split instead of autosaving a value the
                        // single-line model can't track.
                        if state.read(cx).value().contains('\n') {
                            this.commit_line_edit(true, cx);
                            return;
                        }
                        this._autosave = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(800))
                                .await;
                            let _ = this.update(cx, |ws, cx| ws.commit_line_edit(false, cx));
                        }));
                    }
                    InputEvent::PressEnter { .. } => this.on_line_edit_enter(window, cx),
                    InputEvent::Blur => this.commit_line_edit(true, cx),
                    _ => {}
                }
            },
        ));
        self.line_edit = Some(LineEdit { line_idx, expected, appending, input, path });
        cx.notify();
    }

    /// Save the in-place line edit if its text changed; optionally keep the
    /// editor open (autosave keeps it open, blur and navigation close it).
    pub fn commit_line_edit(&mut self, close: bool, cx: &mut Context<Self>) {
        let Some(mut le) = self.line_edit.take() else { return };
        self._autosave = None;
        let value = le.input.read(cx).value().to_string();
        if value != le.expected {
            let written = if le.appending {
                // Appending re-reads the file, so a pane snapshot gone stale
                // (an agent or sync wrote meanwhile) is never clobbered.
                notes::append_line(&le.path, &value).map(Some)
            } else {
                notes::replace_line_on_disk(&le.path, le.line_idx, &le.expected, &value)
            };
            match written {
                Ok(Some(idx)) => {
                    self.note_self_write(&le.path);
                    le.expected = value;
                    le.appending = false;
                    le.line_idx = idx;
                }
                Ok(None) => {
                    // The line vanished or moved ambiguously under the edit;
                    // keep the user's text visible instead of dropping it.
                    self.orphaned = Some((le.path.clone(), value));
                    self.reload_notes();
                    cx.notify();
                    return;
                }
                Err(e) => eprintln!("kairn: could not save {}: {e}", le.path.display()),
            }
            self.reload_notes();
        }
        if !close {
            self.line_edit = Some(le);
        }
        cx.notify();
    }

    /// Enter inside a line edit: NotePlan behaviour. The auto-grow input has
    /// already inserted a literal newline at the cursor; consume it and
    /// apply the line model: at the end of a line with content it commits
    /// and continues the list on a new line below; a bare list marker
    /// clears itself instead; mid-line it splits at the cursor, the
    /// remainder keeping the list style.
    fn on_line_edit_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(le) = self.line_edit.take() else { return };
        self._autosave = None;
        let raw = le.input.read(cx).value().to_string();
        let (head, tail) = match raw.find('\n') {
            Some(i) => (raw[..i].to_string(), raw[i + 1..].to_string()),
            None => (raw, String::new()),
        };
        let value = format!("{head}{tail}");
        // A split inside the list marker or task bracket would corrupt the
        // line; the earliest split point is the start of the content.
        let split = head.len().max(notes::content_start_col(&value)).min(value.len());
        let (head, tail) = value.split_at(split);
        let (combined, next_col) = if !tail.is_empty() {
            let prefix = notes::continuation_prefix(head);
            (format!("{head}\n{prefix}{tail}"), Some(prefix.chars().count()))
        } else {
            let prefix = notes::continuation_prefix(head);
            if !head.is_empty() && prefix == head {
                le.input.update(cx, |s, cx| s.set_value("", window, cx));
                self.line_edit = Some(le);
                self.commit_line_edit(false, cx);
                return;
            }
            (format!("{head}\n{prefix}"), None)
        };
        let written = if le.appending {
            notes::append_line(&le.path, &combined).map(Some)
        } else {
            notes::replace_line_on_disk(&le.path, le.line_idx, &le.expected, &combined)
        };
        match written {
            Ok(Some(idx)) => {
                self.note_self_write(&le.path);
                self.reload_notes();
                self.edit_line_at(idx + 1, next_col, window, cx);
            }
            Ok(None) => {
                if value != le.expected {
                    self.orphaned = Some((le.path.clone(), value));
                }
                self.reload_notes();
                cx.notify();
            }
            Err(e) => {
                eprintln!("kairn: could not save {}: {e}", le.path.display());
                cx.notify();
            }
        }
    }

    /// Up/Down inside a line edit: move the edit to the adjacent line,
    /// keeping the cursor column. Above the first line the cursor goes to the
    /// start; below the last, to the end.
    pub fn line_edit_vertical(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(le) = &self.line_edit else { return };
        let col = le.input.read(cx).cursor_position().character as usize;
        let line_count = self.doc_text.as_deref().map(|t| t.lines().count()).unwrap_or(0);
        let target = le.line_idx as i64 + delta;
        if target < 0 {
            le.input.update(cx, |s, cx| {
                s.set_cursor_position(Position::new(0, 0), window, cx);
            });
            return;
        }
        if target >= line_count as i64 {
            let input = le.input.clone();
            input.update(cx, |s, cx| {
                let end = s.value().chars().count() as u32;
                s.set_cursor_position(Position::new(0, end), window, cx);
            });
            return;
        }
        self.commit_line_edit(true, cx);
        self.edit_line_at(target as usize, Some(col), window, cx);
    }

    pub(crate) fn on_line_edit_left(
        &mut self,
        _: &LineEditLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(le) = &self.line_edit else {
            cx.propagate();
            return;
        };
        if le.input.read(cx).cursor() != 0 || le.line_idx == 0 {
            cx.propagate();
            return;
        }
        let target = le.line_idx - 1;
        self.commit_line_edit(true, cx);
        let end = self
            .doc_text
            .as_deref()
            .and_then(|t| t.lines().nth(target))
            .map(|l| l.chars().count());
        self.edit_line_at(target, end.or(Some(0)), window, cx);
    }

    pub(crate) fn on_line_edit_right(
        &mut self,
        _: &LineEditRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(le) = &self.line_edit else {
            cx.propagate();
            return;
        };
        let state = le.input.read(cx);
        let at_end = state.cursor() == state.text().len();
        let line_count = self.doc_text.as_deref().map(|t| t.lines().count()).unwrap_or(0);
        if !at_end || le.appending || le.line_idx + 1 >= line_count {
            cx.propagate();
            return;
        }
        let target = le.line_idx + 1;
        self.commit_line_edit(true, cx);
        self.edit_line_at(target, Some(0), window, cx);
    }

    /// Backspace at the very start of a line edit merges the line into the
    /// previous one, cursor at the junction.
    pub(crate) fn on_line_edit_backspace(
        &mut self,
        _: &LineEditBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(le) = &self.line_edit else {
            cx.propagate();
            return;
        };
        if le.input.read(cx).cursor() != 0 || le.line_idx == 0 {
            cx.propagate();
            return;
        }
        let prev_idx = le.line_idx - 1;
        let Some(prev_line) = self
            .doc_text
            .as_deref()
            .and_then(|t| t.lines().nth(prev_idx))
            .map(str::to_string)
        else {
            cx.propagate();
            return;
        };
        let value = le.input.read(cx).value().to_string();
        let (appending, path, expected) = (le.appending, le.path.clone(), le.expected.clone());
        self._autosave = None;
        self.line_edit = None;
        let junction = prev_line.chars().count();
        let merged = format!("{prev_line}{value}");
        let written = if appending {
            // The appended line was never written; fold its text (if any)
            // onto the last real line.
            if value.is_empty() {
                Ok(Some(prev_idx))
            } else {
                notes::replace_line_on_disk(&path, prev_idx, &prev_line, &merged)
            }
        } else {
            notes::join_lines_on_disk(&path, prev_idx, &prev_line, &expected, &merged)
        };
        match written {
            Ok(Some(idx)) => {
                self.note_self_write(&path);
                self.reload_notes();
                self.edit_line_at(idx, Some(junction), window, cx);
            }
            Ok(None) => {
                if value != expected {
                    self.orphaned = Some((path, value));
                }
                self.reload_notes();
                cx.notify();
            }
            Err(e) => {
                eprintln!("kairn: could not save {}: {e}", path.display());
                cx.notify();
            }
        }
    }

    /// Delete at the very end of a line edit merges the next line into this
    /// one, cursor staying at the junction.
    pub(crate) fn on_line_edit_delete(
        &mut self,
        _: &LineEditDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(le) = &self.line_edit else {
            cx.propagate();
            return;
        };
        let state = le.input.read(cx);
        let at_end = state.cursor() == state.text().len();
        if !at_end || le.appending {
            cx.propagate();
            return;
        }
        let next_idx = le.line_idx + 1;
        let Some(next_line) = self
            .doc_text
            .as_deref()
            .and_then(|t| t.lines().nth(next_idx))
            .map(str::to_string)
        else {
            cx.propagate();
            return;
        };
        let value = le.input.read(cx).value().to_string();
        let (idx, path, expected) = (le.line_idx, le.path.clone(), le.expected.clone());
        self._autosave = None;
        self.line_edit = None;
        let junction = value.chars().count();
        let merged = format!("{value}{next_line}");
        match notes::join_lines_on_disk(&path, idx, &expected, &next_line, &merged) {
            Ok(Some(resolved)) => {
                self.note_self_write(&path);
                self.reload_notes();
                self.edit_line_at(resolved, Some(junction), window, cx);
            }
            Ok(None) => {
                if value != expected {
                    self.orphaned = Some((path, value));
                }
                self.reload_notes();
                cx.notify();
            }
            Err(e) => {
                eprintln!("kairn: could not save {}: {e}", path.display());
                cx.notify();
            }
        }
    }

    pub(crate) fn on_save_note(&mut self, _: &SaveNote, _: &mut Window, cx: &mut Context<Self>) {
        // Flush a pending line edit now instead of waiting for the autosave.
        self.commit_line_edit(false, cx);
    }

    /// Dismiss the orphaned-line banner, optionally appending its text to
    /// the note it was bound for so nothing typed is lost.
    pub fn resolve_orphan(&mut self, append: bool, cx: &mut Context<Self>) {
        let Some((path, text)) = self.orphaned.take() else { return };
        if append {
            if let Err(e) = notes::append_line(&path, &text) {
                eprintln!("kairn: could not save {}: {e}", path.display());
                self.orphaned = Some((path, text));
                return;
            }
            self.note_self_write(&path);
            self.reload_notes();
        }
        cx.notify();
    }
}
