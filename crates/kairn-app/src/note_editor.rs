//! The single-buffer note editor: the note pane as one continuous text
//! editor over the raw markdown, with NotePlan-style styling painted over
//! the text. No edit mode, no per-line swap: text is always just text.
//!
//! Structure: [`NoteEditor`] (an entity) owns the [`NoteBuffer`], cursor,
//! IME state, and autosave; [`NoteEditorElement`] is a custom element that
//! shapes each line with its markdown styling (cached), paints text and
//! cursor, and registers the IME input handler. The line under the cursor
//! renders its raw markdown; every other line renders styled display text,
//! with clicks mapped back to raw bytes through kairn-core's span math.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Corners, Edges, Element, ElementId, ElementInputHandler, Entity,
    EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId, Hsla,
    InteractiveElement as _, IntoElement, LayoutId, MouseButton, MouseDownEvent, ParentElement
    as _, Pixels, Point, Render, ScrollHandle, SharedString, StrikethroughStyle, Style, Styled
    as _, Task, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, div, fill, point,
    px, size,
};
use kairn_core as notes;
use notes::{Line, NoteBuffer, SpanKind, TaskState};

use crate::keymap::{
    EditorBackspace, EditorDelete, EditorDown, EditorEnter, EditorLeft, EditorPaste, EditorRedo,
    EditorRight, EditorUndo, EditorUp,
};
use crate::theme::{self, KairnTheme, KairnThemeExt as _};

/// Debounce before unsaved changes autosave, matching the old line editor.
const AUTOSAVE_MS: u64 = 800;
const CURSOR_WIDTH: Pixels = px(2.);
const BLINK_MS: u64 = 550;

pub enum NoteEditorEvent {
    /// The editor wrote its file; the workspace should note the self-write
    /// and refresh sidebar state.
    Saved(PathBuf),
    /// A merge collision: typed text that lost to a disk change and must be
    /// surfaced, never dropped.
    Conflicts(PathBuf, Vec<String>),
    OpenWikiLink(String),
    OpenDate(chrono::NaiveDate),
}

pub struct NoteEditor {
    pub path: PathBuf,
    buffer: NoteBuffer,
    /// The file used CRLF endings; the buffer holds LF and saves convert back.
    crlf: bool,
    cursor: usize,
    ime_marked: Option<Range<usize>>,
    focus_handle: FocusHandle,
    pub scroll_handle: ScrollHandle,
    /// Focus as of the last frame; edge-detected in prepaint (the blink
    /// task has no window to ask).
    focused: bool,
    blink_visible: bool,
    blink_epoch: u64,
    _blink_task: Option<Task<()>>,
    _autosave: Option<Task<()>>,
    /// Scroll the cursor into view on the next frame (set by anything that
    /// moves the cursor; consumed by prepaint).
    follow_cursor: Cell<bool>,
    /// Layout of the last frame, shared with the element and the IME
    /// handler. Content-relative y positions plus the element's bounds.
    layout: Rc<RefCell<Option<EditorLayout>>>,
    cache: RefCell<ShapeCache>,
}

pub(crate) struct EditorLayout {
    pub bounds: Bounds<Pixels>,
    pub slots: Vec<LineSlot>,
}

/// One raw line of the document, laid out.
pub(crate) struct LineSlot {
    /// Byte range of the raw line in the buffer (newline excluded).
    pub raw_start: usize,
    pub raw_len: usize,
    /// Content-relative top of the line's block (including its top pad).
    pub y: Pixels,
    pub height: Pixels,
    pub entry: Rc<ShapedEntry>,
}

impl LineSlot {
    fn text_origin_in(&self, bounds: &Bounds<Pixels>) -> Point<Pixels> {
        point(bounds.origin.x + self.entry.indent, bounds.origin.y + self.y + self.entry.pad_top)
    }
}

pub(crate) enum Glyph {
    None,
    Task(TaskState),
    Bullet,
    Rule,
    QuoteBar,
}

pub(crate) struct ShapedEntry {
    pub display: SharedString,
    pub wrapped: Option<WrappedLine>,
    pub line_height: Pixels,
    pub text_height: Pixels,
    pub pad_top: Pixels,
    pub pad_bottom: Pixels,
    pub indent: Pixels,
    pub glyph: Glyph,
    /// Shaped from raw markdown (the cursor line).
    pub active: bool,
}

#[derive(Default)]
struct ShapeCache {
    width: Pixels,
    dark: bool,
    entries: HashMap<(String, bool), Rc<ShapedEntry>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl NoteEditor {
    /// `text` is the document as read from disk (or the rendered template
    /// for a day with no file yet); `path` is where saves go, which may not
    /// exist until the first save creates it.
    pub fn new(path: PathBuf, text: &str, cx: &mut Context<Self>) -> Self {
        let crlf = text.contains("\r\n");
        let text = if crlf { text.replace("\r\n", "\n") } else { text.to_string() };
        Self {
            path,
            buffer: NoteBuffer::new(text),
            crlf,
            cursor: 0,
            ime_marked: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            focused: false,
            blink_visible: true,
            blink_epoch: 0,
            _blink_task: None,
            _autosave: None,
            follow_cursor: Cell::new(false),
            layout: Rc::new(RefCell::new(None)),
            cache: RefCell::new(ShapeCache::default()),
        }
    }

    /// Absorb the document as it now stands on disk (watcher event or
    /// navigation reload). Merges into any in-flight edits; conflicts are
    /// emitted, never dropped.
    pub fn reconcile_from_disk(&mut self, disk: &str, cx: &mut Context<Self>) {
        let disk = if disk.contains("\r\n") { disk.replace("\r\n", "\n") } else { disk.to_string() };
        let (cursor, conflicts) = self.buffer.reconcile(&disk, self.cursor);
        self.cursor = cursor;
        if !conflicts.is_empty() {
            cx.emit(NoteEditorEvent::Conflicts(self.path.clone(), conflicts));
        }
        self.cache.borrow_mut().entries.clear();
        cx.notify();
    }

    /// Write the buffer now if it holds unsaved edits: the flush point for
    /// navigation, Cmd+S, and quit.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self._autosave = None;
        if self.ime_marked.is_some() || !self.buffer.is_dirty() {
            return;
        }
        match std::fs::read_to_string(&self.path) {
            Ok(disk) => {
                let disk =
                    if disk.contains("\r\n") { disk.replace("\r\n", "\n") } else { disk };
                let (cursor, conflicts) = self.buffer.reconcile(&disk, self.cursor);
                self.cursor = cursor;
                if !conflicts.is_empty() {
                    cx.emit(NoteEditorEvent::Conflicts(self.path.clone(), conflicts));
                    self.cache.borrow_mut().entries.clear();
                }
            }
            // A file that never existed (template seed, fresh note) or was
            // deleted externally: writing the buffer is creation or an
            // explicit resurrect; nothing on disk can be clobbered.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("kairn: could not read {}: {e}", self.path.display());
                return;
            }
        }
        let out = if self.crlf {
            self.buffer.text().replace('\n', "\r\n")
        } else {
            self.buffer.text().to_string()
        };
        match notes::write_note(&self.path, &out) {
            Ok(()) => {
                self.buffer.mark_saved();
                cx.emit(NoteEditorEvent::Saved(self.path.clone()));
            }
            Err(e) => eprintln!("kairn: could not save {}: {e}", self.path.display()),
        }
        cx.notify();
    }

    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self._autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(AUTOSAVE_MS)).await;
            let _ = this.update(cx, |ed, cx| ed.save_now(cx));
        }));
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.reset_blink(cx);
        self.follow_cursor.set(true);
        self.cache.borrow_mut().entries.clear();
        self.schedule_autosave(cx);
        cx.notify();
    }

    fn after_cursor_move(&mut self, cx: &mut Context<Self>) {
        self.buffer.break_undo_group();
        self.reset_blink(cx);
        self.follow_cursor.set(true);
        cx.notify();
    }

    /// Track focus changes seen at render time: focus starts the blink,
    /// blur stops it (no per-frame timers while unfocused).
    fn sync_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        if focused {
            self.reset_blink(cx);
        } else {
            self.blink_epoch += 1;
            self._blink_task = None;
            self.blink_visible = true;
        }
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_visible = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self._blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(BLINK_MS)).await;
                let ok = this.update(cx, |ed, cx| {
                    if ed.blink_epoch != epoch {
                        return false;
                    }
                    if ed.focused {
                        ed.blink_visible = !ed.blink_visible;
                        cx.notify();
                    }
                    true
                });
                match ok {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    // --- text/coordinate helpers -------------------------------------------

    fn text(&self) -> &str {
        self.buffer.text()
    }

    fn line_range_at(&self, offset: usize) -> Range<usize> {
        let text = self.text();
        let start = text[..offset.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        start..end
    }

    fn prev_char_start(&self, offset: usize) -> usize {
        self.text()[..offset].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_char_end(&self, offset: usize) -> usize {
        let text = self.text();
        text[offset..].chars().next().map_or(text.len(), |c| offset + c.len_utf8())
    }

    // --- actions -----------------------------------------------------------

    fn on_enter(&mut self, _: &EditorEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.buffer.split_line(self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_backspace(&mut self, _: &EditorBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.prev_char_start(self.cursor);
        self.cursor = self.buffer.edit(prev..self.cursor, "", self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_delete(&mut self, _: &EditorDelete, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.next_char_end(self.cursor);
        if next == self.cursor {
            return;
        }
        self.cursor = self.buffer.edit(self.cursor..next, "", self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_left(&mut self, _: &EditorLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.prev_char_start(self.cursor);
        self.after_cursor_move(cx);
    }

    fn on_right(&mut self, _: &EditorRight, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.next_char_end(self.cursor);
        self.after_cursor_move(cx);
    }

    fn on_up(&mut self, _: &EditorUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn on_down(&mut self, _: &EditorDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, cx);
    }

    fn move_vertical(&mut self, delta: i64, cx: &mut Context<Self>) {
        let line = self.line_range_at(self.cursor);
        let col = self.text()[line.start..self.cursor].chars().count();
        let target_start = if delta < 0 {
            if line.start == 0 {
                self.cursor = 0;
                self.after_cursor_move(cx);
                return;
            }
            self.line_range_at(line.start - 1).start
        } else {
            if line.end >= self.text().len() {
                self.cursor = self.text().len();
                self.after_cursor_move(cx);
                return;
            }
            line.end + 1
        };
        let target = self.line_range_at(target_start);
        let byte = self.text()[target.clone()]
            .char_indices()
            .nth(col)
            .map_or(target.end, |(i, _)| target.start + i);
        self.cursor = byte;
        self.after_cursor_move(cx);
    }

    fn on_undo(&mut self, _: &EditorUndo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cursor) = self.buffer.undo() {
            self.cursor = cursor;
            self.after_edit(cx);
        }
    }

    fn on_redo(&mut self, _: &EditorRedo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cursor) = self.buffer.redo() {
            self.cursor = cursor;
            self.after_edit(cx);
        }
    }

    fn on_paste(&mut self, _: &EditorPaste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        self.cursor = self.buffer.edit(self.cursor..self.cursor, &text, self.cursor, now_ms());
        self.after_edit(cx);
    }

    // --- mouse -------------------------------------------------------------

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        enum Click {
            Toggle(Range<usize>),
            Link(SpanKind, String),
            Cursor(usize),
        }
        let click = {
            let layout = self.layout.borrow();
            let Some(layout) = layout.as_ref() else { return };
            let pos = event.position;
            let mut click = Click::Cursor(self.text().len());
            for slot in &layout.slots {
                let top = layout.bounds.origin.y + slot.y;
                if pos.y < top || pos.y >= top + slot.height {
                    continue;
                }
                let raw_line = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
                // Checkbox hit: the glyph column of a task line toggles.
                if let Glyph::Task(state) = &slot.entry.glyph
                    && matches!(state, TaskState::Open | TaskState::Done)
                    && pos.x < layout.bounds.origin.x + slot.entry.indent
                {
                    click = Click::Toggle(slot.raw_start..slot.raw_start + slot.raw_len);
                    break;
                }
                let origin = slot.text_origin_in(&layout.bounds);
                let local = point(pos.x - origin.x, pos.y - origin.y);
                let index = slot
                    .entry
                    .wrapped
                    .as_ref()
                    .and_then(|w| w.index_for_position(local, slot.entry.line_height).ok());
                let raw_col = match index {
                    Some(display_ix) if !slot.entry.active => {
                        let display = &slot.entry.display;
                        let display_chars =
                            display.get(..display_ix).map_or(0, |s| s.chars().count());
                        // An exact hit on a link navigates; anywhere else edits.
                        if let Some((kind, text)) =
                            notes::span_at_display_char(raw_line, display_chars)
                            && matches!(kind, SpanKind::WikiLink | SpanKind::DateRef)
                        {
                            click = Click::Link(kind, text);
                            break;
                        }
                        notes::raw_col_for_display_char(raw_line, display_chars)
                    }
                    Some(raw_ix) => raw_ix.min(raw_line.len()),
                    // Past the end of the line's text: cursor to line end.
                    None => raw_line.len(),
                };
                click = Click::Cursor(slot.raw_start + raw_col);
                break;
            }
            click
        };
        match click {
            Click::Toggle(range) => self.toggle_task_in(range, cx),
            Click::Link(SpanKind::WikiLink, text) => {
                cx.emit(NoteEditorEvent::OpenWikiLink(
                    notes::wiki_link_title(&text).to_string(),
                ));
            }
            Click::Link(_, text) => {
                if let Some(date) = text
                    .strip_prefix('>')
                    .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                {
                    cx.emit(NoteEditorEvent::OpenDate(date));
                }
            }
            Click::Cursor(target) => {
                self.cursor = target.min(self.text().len());
                window.focus(&self.focus_handle);
                self.after_cursor_move(cx);
            }
        }
    }

    fn toggle_task_in(&mut self, raw_range: Range<usize>, cx: &mut Context<Self>) {
        let line = self.text()[raw_range.clone()].to_string();
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let Some(toggled) = notes::toggle_task_line(&line, &now) else { return };
        self.buffer.edit(raw_range, &toggled, self.cursor, now_ms());
        self.after_edit(cx);
    }

    // --- layout ------------------------------------------------------------

    /// Shape every line at `width` and rebuild the slot list. Returns the
    /// content height. Called from the element's measured layout, so the
    /// shaping cache keeps a keystroke from re-shaping the whole note.
    fn layout_for_width(&mut self, width: Pixels, window: &mut Window, cx: &mut App) -> Pixels {
        let t = cx.kairn().clone();
        let dark = matches!(t.mode, theme::Mode::Dark);
        {
            let mut cache = self.cache.borrow_mut();
            if cache.width != width || cache.dark != dark {
                cache.entries.clear();
                cache.width = width;
                cache.dark = dark;
            }
        }
        let text = self.buffer.text().to_string();
        let cursor_line = self.line_range_at(self.cursor);
        let mut slots = Vec::new();
        let mut y = px(0.);
        let mut start = 0usize;
        for raw in text.split('\n') {
            let active = start == cursor_line.start;
            let entry = self.entry_for(raw, active, width, &t, window);
            let height = entry.pad_top + entry.text_height + entry.pad_bottom;
            slots.push(LineSlot { raw_start: start, raw_len: raw.len(), y, height, entry });
            y += height;
            start += raw.len() + 1;
        }
        let mut layout = self.layout.borrow_mut();
        let bounds = layout.as_ref().map(|l| l.bounds).unwrap_or_default();
        *layout = Some(EditorLayout { bounds, slots });
        y
    }

    fn entry_for(
        &self,
        raw: &str,
        active: bool,
        width: Pixels,
        t: &KairnTheme,
        window: &mut Window,
    ) -> Rc<ShapedEntry> {
        let key = (raw.to_string(), active);
        if let Some(hit) = self.cache.borrow().entries.get(&key) {
            return hit.clone();
        }
        let entry = Rc::new(shape_entry(
            raw,
            active,
            width,
            self.ime_marked.as_ref().and_then(|r| {
                // The marked range lives on the cursor line; express it in
                // line-local bytes for the shaper.
                let line = self.line_range_at(self.cursor);
                (active && r.start >= line.start && r.end <= line.end)
                    .then(|| r.start - line.start..r.end - line.start)
            }),
            t,
            window,
        ));
        self.cache.borrow_mut().entries.insert(key, entry.clone());
        entry
    }

    /// Keep the cursor visible inside the note's scroll viewport.
    fn follow_cursor_now(&self) {
        let Some(layout) = &*self.layout.borrow() else { return };
        let Some(slot) = layout
            .slots
            .iter()
            .find(|s| {
                (s.raw_start..=s.raw_start + s.raw_len).contains(&self.cursor)
            })
        else {
            return;
        };
        let viewport = self.scroll_handle.bounds();
        if viewport.size.height <= px(0.) {
            return;
        }
        let top = layout.bounds.origin.y + slot.y;
        let bottom = top + slot.height;
        let margin = px(8.);
        let mut offset = self.scroll_handle.offset();
        if bottom + margin > viewport.origin.y + viewport.size.height {
            offset.y -= bottom + margin - (viewport.origin.y + viewport.size.height);
            self.scroll_handle.set_offset(offset);
        } else if top - margin < viewport.origin.y {
            offset.y += viewport.origin.y - (top - margin);
            self.scroll_handle.set_offset(offset);
        }
    }

    // --- utf16 helpers for the IME contract --------------------------------

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text()[..offset.min(self.text().len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn offset_from_utf16(&self, target: usize) -> usize {
        let mut units = 0usize;
        for (i, c) in self.text().char_indices() {
            if units >= target {
                return i;
            }
            units += c.len_utf16();
        }
        self.text().len()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

impl Focusable for NoteEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NoteEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("NoteEditor")
            .track_focus(&self.focus_handle)
            .cursor_text()
            // Room below the last line so clicking the empty space under a
            // short note lands in the editor and appends.
            .pb(px(140.))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_left))
            .on_action(cx.listener(Self::on_right))
            .on_action(cx.listener(Self::on_up))
            .on_action(cx.listener(Self::on_down))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_paste))
            .child(NoteEditorElement { editor: cx.entity().clone() })
    }
}

// --- IME / text input ------------------------------------------------------

impl gpui::EntityInputHandler for NoteEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        Some(self.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&(self.cursor..self.cursor)),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.ime_marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.cursor = self.buffer.edit(range, new_text, self.cursor, now_ms());
        self.ime_marked = None;
        self.after_edit(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.ime_marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        let start = range.start;
        self.cursor = self.buffer.edit(range, new_text, self.cursor, now_ms());
        if new_text.is_empty() {
            self.ime_marked = None;
            self.cursor = start;
        } else {
            self.ime_marked = Some(start..start + new_text.len());
            self.cursor = new_selected_range_utf16
                .map(|r| {
                    let local = self.range_from_utf16(&r);
                    (start + local.end).min(self.text().len())
                })
                .unwrap_or(start + new_text.len());
        }
        self.after_edit(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let slot = layout
            .slots
            .iter()
            .find(|s| (s.raw_start..=s.raw_start + s.raw_len).contains(&range.start))?;
        let wrapped = slot.entry.wrapped.as_ref()?;
        let local = wrapped
            .position_for_index(range.start - slot.raw_start, slot.entry.line_height)?;
        let origin = slot.text_origin_in(&element_bounds) + local;
        Some(Bounds::new(origin, size(px(2.), slot.entry.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        pos: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        for slot in &layout.slots {
            let top = layout.bounds.origin.y + slot.y;
            if pos.y < top || pos.y >= top + slot.height {
                continue;
            }
            let origin = slot.text_origin_in(&layout.bounds);
            let local = point(pos.x - origin.x, pos.y - origin.y);
            if let Some(ix) = slot
                .entry
                .wrapped
                .as_ref()
                .and_then(|w| w.index_for_position(local, slot.entry.line_height).ok())
            {
                let raw = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
                let raw_ix = if slot.entry.active {
                    ix.min(raw.len())
                } else {
                    let chars =
                        slot.entry.display.get(..ix).map_or(0, |s| s.chars().count());
                    notes::raw_col_for_display_char(raw, chars)
                };
                return Some(self.offset_to_utf16(slot.raw_start + raw_ix));
            }
        }
        None
    }
}

// --- shaping ---------------------------------------------------------------

/// Per-kind metrics, matching the retired per-line renderer so the flag flip
/// is visually seamless: body 13px at 1.58 line height, H1 serif 19, section
/// headings 11 uppercase, task/bullet indents from the glyph column.
struct KindStyle {
    size: Pixels,
    line_height: Pixels,
    pad_top: Pixels,
    pad_bottom: Pixels,
    indent: Pixels,
    color: Hsla,
    weight: FontWeight,
    serif: bool,
    uppercase: bool,
}

fn kind_style(line: &Line, t: &KairnTheme) -> (KindStyle, Glyph) {
    let body = |color| KindStyle {
        size: px(13.),
        line_height: px(20.5),
        pad_top: px(1.),
        pad_bottom: px(1.),
        indent: px(0.),
        color,
        weight: FontWeight::NORMAL,
        serif: false,
        uppercase: false,
    };
    match line {
        Line::Heading { level: 1, .. } => (
            KindStyle {
                size: px(19.),
                line_height: px(27.),
                pad_top: px(18.),
                pad_bottom: px(6.),
                weight: FontWeight::BOLD,
                serif: true,
                ..body(t.text)
            },
            Glyph::None,
        ),
        Line::Heading { .. } => (
            KindStyle {
                size: px(11.),
                line_height: px(17.),
                pad_top: px(18.),
                pad_bottom: px(8.),
                weight: FontWeight::SEMIBOLD,
                uppercase: true,
                ..body(t.faint)
            },
            Glyph::None,
        ),
        Line::Task { state, .. } => {
            let color = match state {
                TaskState::Open => t.text,
                TaskState::Scheduled => t.dim,
                TaskState::Done | TaskState::Cancelled => t.faint,
            };
            (
                KindStyle {
                    pad_top: px(2.5),
                    pad_bottom: px(2.5),
                    indent: px(22.),
                    ..body(color)
                },
                Glyph::Task(*state),
            )
        }
        Line::Bullet { .. } => (
            KindStyle {
                pad_top: px(2.5),
                pad_bottom: px(2.5),
                indent: px(18.),
                ..body(t.text)
            },
            Glyph::Bullet,
        ),
        Line::Quote { .. } => (
            KindStyle { pad_top: px(4.), pad_bottom: px(4.), indent: px(14.), ..body(t.dim) },
            Glyph::QuoteBar,
        ),
        Line::Rule => (
            KindStyle { pad_top: px(14.), pad_bottom: px(14.), ..body(t.faint) },
            Glyph::Rule,
        ),
        Line::Blank | Line::Text { .. } => (body(t.text), Glyph::None),
    }
}

fn span_style(kind: SpanKind, base: Hsla, t: &KairnTheme) -> (Hsla, Option<Hsla>, FontWeight, FontStyle) {
    match kind {
        SpanKind::Text => (base, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::WikiLink => (t.accent, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Tag | SpanKind::DateRef => (t.amber, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Mention => (t.faint, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Highlight => (t.text, Some(t.amber.opacity(0.28)), FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Bold => (base, None, FontWeight::BOLD, FontStyle::Normal),
        SpanKind::Italic => (base, None, FontWeight::NORMAL, FontStyle::Italic),
        SpanKind::Marker => (t.faint, None, FontWeight::NORMAL, FontStyle::Normal),
    }
}

fn shape_entry(
    raw: &str,
    active: bool,
    width: Pixels,
    marked_local: Option<Range<usize>>,
    t: &KairnTheme,
    window: &mut Window,
) -> ShapedEntry {
    let parsed = notes::parse_line(raw);
    let (style, glyph) = kind_style(&parsed, t);
    let strikethrough = matches!(
        parsed,
        Line::Task { state: TaskState::Done | TaskState::Cancelled, .. }
    );

    // Inactive blank lines are an 8px breather, matching the old renderer;
    // the cursor line always has full text height so the caret has a home.
    if !active && matches!(parsed, Line::Blank) {
        return ShapedEntry {
            display: SharedString::default(),
            wrapped: None,
            line_height: style.line_height,
            text_height: px(8.),
            pad_top: px(0.),
            pad_bottom: px(0.),
            indent: px(0.),
            glyph: Glyph::None,
            active,
        };
    }
    // Inactive rules paint a line, no text.
    if !active && matches!(parsed, Line::Rule) {
        return ShapedEntry {
            display: SharedString::default(),
            wrapped: None,
            line_height: style.line_height,
            text_height: px(1.),
            pad_top: style.pad_top,
            pad_bottom: style.pad_bottom,
            indent: px(0.),
            glyph: Glyph::Rule,
            active,
        };
    }

    let base_font = {
        let mut f = window.text_style().font();
        if style.serif {
            f.family = theme::serif_font().to_string().into();
        }
        f.weight = style.weight;
        f
    };
    let strike = strikethrough.then_some(StrikethroughStyle {
        thickness: px(1.),
        color: Some(style.color),
    });

    let (display, runs, indent, glyph) = if active {
        // The cursor line shows its raw markdown in the line's own style,
        // markers and all: what NotePlan does, and what makes the mapping
        // between clicks, cursor, and bytes exact while editing.
        let mut runs = Vec::new();
        let mut push = |len: usize, underline: bool| {
            if len == 0 {
                return;
            }
            runs.push(TextRun {
                len,
                font: base_font.clone(),
                color: style.color,
                background_color: None,
                underline: underline.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(style.color),
                    wavy: false,
                }),
                strikethrough: None,
            });
        };
        match &marked_local {
            Some(m) => {
                push(m.start.min(raw.len()), false);
                push(m.end.min(raw.len()).saturating_sub(m.start.min(raw.len())), true);
                push(raw.len().saturating_sub(m.end.min(raw.len())), false);
            }
            None => push(raw.len(), false),
        }
        (SharedString::from(raw.to_string()), runs, px(0.), Glyph::None)
    } else {
        let spans = match &parsed {
            Line::Heading { spans, .. }
            | Line::Task { spans, .. }
            | Line::Bullet { spans }
            | Line::Quote { spans }
            | Line::Text { spans } => spans.as_slice(),
            Line::Rule | Line::Blank => &[],
        };
        let mut display = String::new();
        let mut runs = Vec::new();
        for (kind, text) in spans {
            let piece = if style.uppercase { text.to_uppercase() } else { text.clone() };
            let (color, bg, weight, font_style) = span_style(*kind, style.color, t);
            let mut font = base_font.clone();
            if weight > font.weight {
                font.weight = weight;
            }
            font.style = font_style;
            runs.push(TextRun {
                len: piece.len(),
                font,
                color,
                background_color: bg,
                underline: None,
                strikethrough: strike,
            });
            display.push_str(&piece);
        }
        (SharedString::from(display), runs, style.indent, glyph)
    };

    let wrap_width = (width - indent).max(px(50.));
    let wrapped = window
        .text_system()
        .shape_text(display.clone(), style.size, &runs, Some(wrap_width), None)
        .ok()
        .and_then(|mut lines| {
            debug_assert!(lines.len() <= 1);
            lines.pop()
        });
    let text_height = wrapped
        .as_ref()
        .map(|w| w.size(style.line_height).height)
        .unwrap_or(style.line_height);

    ShapedEntry {
        display,
        wrapped,
        line_height: style.line_height,
        text_height,
        pad_top: style.pad_top,
        pad_bottom: style.pad_bottom,
        indent,
        glyph,
        active,
    }
}

// --- the element -----------------------------------------------------------

pub struct NoteEditorElement {
    pub editor: Entity<NoteEditor>,
}

impl IntoElement for NoteEditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NoteEditorElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let editor = self.editor.clone();
        let layout_id = window.request_measured_layout(
            Style::default(),
            move |known, available, window, cx| {
                let width = known.width.unwrap_or(match available.width {
                    gpui::AvailableSpace::Definite(w) => w,
                    _ => px(640.),
                });
                let height =
                    editor.update(cx, |ed, cx| ed.layout_for_width(width, window, cx));
                size(width, height)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.editor.update(cx, |ed, cx| {
            let stale = ed
                .layout
                .borrow()
                .as_ref()
                .is_none_or(|l| l.bounds.size.width != bounds.size.width);
            if stale {
                ed.layout_for_width(bounds.size.width, window, cx);
            }
            if let Some(layout) = ed.layout.borrow_mut().as_mut() {
                layout.bounds = bounds;
            }
            let focused = ed.focus_handle.is_focused(window);
            ed.sync_focus(focused, cx);
            if ed.follow_cursor.take() {
                ed.follow_cursor_now();
            }
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let t = cx.kairn().clone();
        let (focus_handle, cursor, blink_visible) = {
            let ed = self.editor.read(cx);
            (ed.focus_handle.clone(), ed.cursor, ed.blink_visible)
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        let layout = self.editor.read(cx).layout.clone();
        let layout = layout.borrow();
        let Some(layout) = layout.as_ref() else { return };
        let focused = focus_handle.is_focused(window);

        for slot in &layout.slots {
            let block_top = bounds.origin.y + slot.y;
            let text_origin = slot.text_origin_in(&bounds);

            match &slot.entry.glyph {
                Glyph::Rule => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.origin.x, block_top + slot.entry.pad_top),
                            size(bounds.size.width, px(1.)),
                        ),
                        t.border,
                    ));
                }
                Glyph::Task(state) => {
                    paint_task_box(
                        point(bounds.origin.x, text_origin.y + px(4.)),
                        state,
                        &t,
                        window,
                        cx,
                    );
                }
                Glyph::Bullet => {
                    let dash = window.text_system().shape_line(
                        SharedString::from("–"),
                        px(13.),
                        &[TextRun {
                            len: "–".len(),
                            font: window.text_style().font(),
                            color: t.faint,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    let _ = dash.paint(
                        point(bounds.origin.x, text_origin.y),
                        slot.entry.line_height,
                        window,
                        cx,
                    );
                }
                Glyph::QuoteBar => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.origin.x, block_top + slot.entry.pad_top),
                            size(px(2.), slot.entry.text_height),
                        ),
                        t.border,
                    ));
                }
                Glyph::None => {}
            }

            if let Some(wrapped) = &slot.entry.wrapped {
                let _ = wrapped.paint(
                    text_origin,
                    slot.entry.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }

            // The caret: on the slot that owns the cursor offset.
            if focused
                && blink_visible
                && (slot.raw_start..=slot.raw_start + slot.raw_len).contains(&cursor)
            {
                let local = slot
                    .entry
                    .wrapped
                    .as_ref()
                    .and_then(|w| {
                        w.position_for_index(cursor - slot.raw_start, slot.entry.line_height)
                    })
                    .unwrap_or(point(px(0.), px(0.)));
                window.paint_quad(fill(
                    Bounds::new(
                        text_origin + local - point(px(0.5), px(0.)),
                        size(CURSOR_WIDTH, slot.entry.line_height),
                    ),
                    t.accent,
                ));
            }
        }
    }
}

fn paint_task_box(
    origin: Point<Pixels>,
    state: &TaskState,
    t: &KairnTheme,
    window: &mut Window,
    cx: &mut App,
) {
    let box_bounds = Bounds::new(origin, size(px(13.), px(13.)));
    let mut quad = fill(box_bounds, gpui::transparent_black());
    quad.corner_radii = Corners::all(px(4.));
    match state {
        TaskState::Done => {
            quad.background = t.accent.into();
        }
        _ => {
            quad.border_widths = Edges::all(px(1.));
            quad.border_color = t.faint;
        }
    }
    window.paint_quad(quad);

    let (mark, color, size_px) = match state {
        TaskState::Done => ("✓", t.bg, px(9.)),
        TaskState::Cancelled => ("✕", t.faint, px(8.)),
        TaskState::Scheduled => ("›", t.faint, px(8.)),
        TaskState::Open => return,
    };
    let mut font = window.text_style().font();
    font.weight = FontWeight::BOLD;
    let line = window.text_system().shape_line(
        SharedString::from(mark),
        size_px,
        &[TextRun {
            len: mark.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let inset = point(
        origin.x + (px(13.) - line.width).max(px(0.)) / 2.,
        origin.y + (px(13.) - size_px * 1.2) / 2.,
    );
    let _ = line.paint(inset, size_px * 1.2, window, cx);
}
