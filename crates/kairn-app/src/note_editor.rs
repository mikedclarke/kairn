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
    App, Bounds, ClipboardItem, Context, Corners, DispatchPhase, Edges, Element, ElementId,
    ElementInputHandler, Entity, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight,
    GlobalElementId, Hsla, InteractiveElement as _, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollHandle, SharedString, StrikethroughStyle, Style, Styled as _, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, div, fill, point, px, size,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use kairn_core as notes;
use notes::{Line, NoteBuffer, SpanKind, TaskState};

use crate::keymap::{
    EditorBackspace, EditorCopy, EditorCut, EditorDelete, EditorDeleteToLineStart,
    EditorDeleteWordBack, EditorDocEnd, EditorDocStart, EditorDown, EditorEnter, EditorLeft,
    EditorLineEnd, EditorLineStart, EditorPaste, EditorRedo, EditorRight, EditorSelectAll,
    EditorSelectDocEnd, EditorSelectDocStart, EditorSelectDown, EditorSelectLeft,
    EditorSelectLineEnd, EditorSelectLineStart, EditorSelectRight, EditorSelectUp,
    EditorSelectWordLeft, EditorSelectWordRight, EditorUndo, EditorUp, EditorWordLeft,
    EditorWordRight,
};
use crate::theme::{self, KairnTheme, KairnThemeExt as _};

/// Debounce before unsaved changes autosave, matching the old line editor.
const AUTOSAVE_MS: u64 = 800;
const CURSOR_WIDTH: Pixels = px(2.);
const BLINK_MS: u64 = 550;
/// Pointer travel before a glyph press becomes a line drag, not a click.
const DRAG_THRESHOLD: Pixels = px(3.);
/// The handle gutter left of every line, at the base editor size: the grab
/// zone for dragging any line. Scales with the editor size like the glyph
/// indents.
const HANDLE_GUTTER: f32 = 18.;

pub enum NoteEditorEvent {
    /// The editor wrote its file; the workspace should note the self-write
    /// and refresh sidebar state.
    Saved(PathBuf),
    /// A merge collision: typed text that lost to a disk change and must be
    /// surfaced, never dropped.
    Conflicts(PathBuf, Vec<String>),
    OpenWikiLink(String),
    OpenDate(chrono::NaiveDate),
    OpenUrl(String),
    /// A line drag was released outside the editor; the workspace moves the
    /// block if the pointer sat on a day drop target.
    BlockDropped { range: Range<usize>, position: Point<Pixels> },
}

pub struct NoteEditor {
    pub path: PathBuf,
    buffer: NoteBuffer,
    /// The file used CRLF endings; the buffer holds LF and saves convert back.
    crlf: bool,
    cursor: usize,
    /// The fixed end of the selection; the cursor is the moving end. Equal
    /// offsets mean no selection.
    selection_anchor: Option<usize>,
    /// A mouse drag-select is in progress (mouse down through mouse up).
    selecting: bool,
    /// An in-flight drag that started on a line's handle or glyph: released
    /// in place a glyph grab toggles open/done tasks; moved past the
    /// threshold it drags the line's block.
    line_drag: Option<LineDrag>,
    /// Line-start offset of the line under the pointer, for the hover-
    /// revealed drag handle in the gutter.
    hovered_line: Option<usize>,
    ime_marked: Option<Range<usize>>,
    /// The clickable span under the pointer, underlined so links read as
    /// links: the line's start offset and the span's display-char range.
    hovered_link: Option<(usize, Range<usize>)>,
    /// The link under the last right-click, captured on mouse down so the
    /// context menu (built a frame later) can offer to open it.
    menu_link: Option<notes::LinkTarget>,
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
    /// Width of the handle gutter at the current editor size; every entry's
    /// `indent` already includes it.
    pub gutter: Pixels,
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

struct LineDrag {
    /// Byte range of the dragged block at mouse down: the grabbed line plus
    /// its deeper-indented run (`block_range`), final newline excluded.
    range: Range<usize>,
    origin: Point<Pixels>,
    /// Where the pointer is now, window coordinates: the workspace reads it
    /// to place the drag ghost and light up day drop targets.
    position: Point<Pixels>,
    moved: bool,
    /// Whether releasing without moving toggles the task (glyph grabs on
    /// open/done tasks).
    toggles: bool,
    /// The grab started on the handle gutter, where releasing in place is a
    /// no-op, rather than on a task/bullet glyph.
    from_handle: bool,
    /// Drop position for a reorder: a line-start offset (or the text length
    /// for the end); `None` while the pointer is outside the editor.
    target: Option<usize>,
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
    /// Shaped with inline markers revealed (the cursor line).
    pub active: bool,
    /// Raw bytes hidden at the start of an active line (the list marker or
    /// quote prefix, whose glyph stays painted instead): display index i is
    /// raw column i + prefix_len. Zero on inactive lines, whose mapping goes
    /// through the span math.
    pub prefix_len: usize,
}

#[derive(Default)]
struct ShapeCache {
    width: Pixels,
    dark: bool,
    /// Inactive lines only, keyed by content (the active line's shaping also
    /// depends on IME state, and there is exactly one, so it never caches).
    entries: HashMap<String, Rc<ShapedEntry>>,
}

impl ShapeCache {
    /// Far above any real note's distinct lines; guards against a pathological
    /// document (or a long editing session) growing the map without limit.
    const CAPACITY: usize = 8192;
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the word (or punctuation run) left of `offset`, skipping any
/// whitespace first; crosses newlines like the platform text engines do.
fn prev_word_boundary(text: &str, offset: usize) -> usize {
    let mut i = offset.min(text.len());
    let mut chars = text[..i].chars().rev().peekable();
    while let Some(&c) = chars.peek() {
        if !c.is_whitespace() {
            break;
        }
        i -= c.len_utf8();
        chars.next();
    }
    let Some(&first) = chars.peek() else { return i };
    let word = is_word_char(first);
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || is_word_char(c) != word {
            break;
        }
        i -= c.len_utf8();
        chars.next();
    }
    i
}

/// End of the word (or punctuation run) right of `offset`, skipping any
/// whitespace first.
fn next_word_boundary(text: &str, offset: usize) -> usize {
    let mut i = offset.min(text.len());
    let mut chars = text[i..].chars().peekable();
    while let Some(&c) = chars.peek() {
        if !c.is_whitespace() {
            break;
        }
        i += c.len_utf8();
        chars.next();
    }
    let Some(&first) = chars.peek() else { return i };
    let word = is_word_char(first);
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || is_word_char(c) != word {
            break;
        }
        i += c.len_utf8();
        chars.next();
    }
    i
}

impl NoteEditor {
    /// `text` is the document as read from disk; `seed` is content rendered
    /// over a blank day (the daily template), kept out of the disk baseline
    /// and written only once a real edit lands; `path` is where saves go,
    /// which may not exist until the first save creates it.
    pub fn new(path: PathBuf, text: &str, seed: Option<&str>, cx: &mut Context<Self>) -> Self {
        let crlf = text.contains("\r\n");
        let text = if crlf { text.replace("\r\n", "\n") } else { text.to_string() };
        let buffer = match seed {
            Some(seed) => NoteBuffer::with_seed(text, seed),
            None => NoteBuffer::new(text),
        };
        Self {
            path,
            buffer,
            crlf,
            cursor: 0,
            selection_anchor: None,
            selecting: false,
            line_drag: None,
            hovered_line: None,
            ime_marked: None,
            hovered_link: None,
            menu_link: None,
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
        let before = self.buffer.text().to_string();
        let (cursor, conflicts) = self.buffer.reconcile(&disk, self.cursor);
        self.cursor = cursor;
        if self.buffer.text() != before {
            self.drop_stale_offsets();
        }
        if !conflicts.is_empty() {
            cx.emit(NoteEditorEvent::Conflicts(self.path.clone(), conflicts));
        }
        cx.notify();
    }

    /// The filename stem this note's title (its first heading) implies, or
    /// `None` when there's no usable title yet. Drives title-follows-filename
    /// renaming for regular notes.
    pub fn title_stem(&self) -> Option<String> {
        notes::note_title_stem(self.text())
    }

    /// Re-point the editor at a moved file (after a title rename) without
    /// swapping the entity, so the cursor, undo history, and focus survive.
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

    /// Put the caret at the end of the title line and take focus, so a
    /// freshly created note is ready to type into right after the `# `.
    pub fn focus_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.text().find('\n').unwrap_or(self.text().len());
        self.selection_anchor = None;
        self.follow_cursor.set(true);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// An external change shifted the text under us: any selection or
    /// in-flight line drag holds stale byte offsets, so let go of them.
    fn drop_stale_offsets(&mut self) {
        self.selection_anchor = None;
        self.selecting = false;
        self.line_drag = None;
        self.hovered_line = None;
        self.hovered_link = None;
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
                let before = self.buffer.text().to_string();
                let (cursor, conflicts) = self.buffer.reconcile(&disk, self.cursor);
                self.cursor = cursor;
                if self.buffer.text() != before {
                    self.drop_stale_offsets();
                }
                if !conflicts.is_empty() {
                    cx.emit(NoteEditorEvent::Conflicts(self.path.clone(), conflicts));
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
        self.selection_anchor = None;
        self.hovered_link = None;
        self.reset_blink(cx);
        self.follow_cursor.set(true);
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

    /// Content start of the line spanning `line`, when that line hides its
    /// marker behind a glyph (task, bullet, quote): the region before it is
    /// invisible, so the cursor must never sit inside it.
    fn hidden_marker_end(&self, line: &Range<usize>) -> Option<usize> {
        let raw = &self.text()[line.clone()];
        matches!(
            notes::parse_line(raw),
            Line::Task { .. } | Line::Bullet { .. } | Line::Quote { .. }
        )
        .then(|| line.start + notes::content_start_col(raw).min(raw.len()))
        .filter(|cs| *cs > line.start)
    }

    /// Clamp an offset out of a hidden marker, forward onto the content.
    fn snap_to_content(&self, offset: usize) -> usize {
        let line = self.line_range_at(offset);
        match self.hidden_marker_end(&line) {
            Some(cs) if offset < cs => cs,
            _ => offset,
        }
    }

    /// Where a plain left-arrow goes: the previous character, except that a
    /// hidden marker is atomic — from its content start the cursor hops to
    /// the previous line's end.
    fn left_target(&self) -> usize {
        let line = self.line_range_at(self.cursor);
        if let Some(cs) = self.hidden_marker_end(&line)
            && self.cursor <= cs
        {
            return if line.start > 0 { line.start - 1 } else { self.cursor };
        }
        self.prev_char_start(self.cursor)
    }

    // --- selection ---------------------------------------------------------

    fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    /// Move the cursor; `extend` keeps (or starts) a selection from the
    /// current position, plain movement drops any selection.
    fn move_cursor_to(&mut self, target: usize, extend: bool, cx: &mut Context<Self>) {
        if extend {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
        self.cursor = self.snap_to_content(target.min(self.text().len()));
        self.after_cursor_move(cx);
    }

    /// Delete the selected range if there is one; its own undo step.
    fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selection() else { return false };
        self.buffer.break_undo_group();
        self.cursor = self.buffer.edit(sel, "", self.cursor, now_ms());
        self.buffer.break_undo_group();
        self.selection_anchor = None;
        true
    }

    /// The run of same-class characters (word, whitespace, or punctuation)
    /// around `offset`, never crossing the line.
    fn word_range_at(&self, offset: usize) -> Range<usize> {
        let line = self.line_range_at(offset);
        if line.is_empty() {
            return line;
        }
        let text = self.text();
        let mut offset = offset.clamp(line.start, line.end);
        if offset == line.end {
            offset = self.prev_char_start(offset);
        }
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                0u8
            } else if c.is_whitespace() {
                1
            } else {
                2
            }
        };
        let k = text[offset..].chars().next().map(class).unwrap_or(2);
        let mut start = offset;
        while start > line.start {
            let prev = self.prev_char_start(start);
            if text[prev..].chars().next().map(class) != Some(k) {
                break;
            }
            start = prev;
        }
        let mut end = self.next_char_end(offset);
        while end < line.end {
            if text[end..].chars().next().map(class) != Some(k) {
                break;
            }
            end = self.next_char_end(end);
        }
        start..end
    }

    // --- actions -----------------------------------------------------------

    fn on_enter(&mut self, _: &EditorEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_selection();
        self.cursor = self.buffer.split_line(self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_backspace(&mut self, _: &EditorBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selection() {
            self.after_edit(cx);
            return;
        }
        if self.cursor == 0 {
            return;
        }
        // At the content start of a glyph line the marker is atomic: one
        // press removes the whole checkbox/bullet/quote prefix, leaving a
        // plain line.
        let line = self.line_range_at(self.cursor);
        if let Some(cs) = self.hidden_marker_end(&line)
            && self.cursor <= cs
        {
            self.buffer.break_undo_group();
            self.cursor = self.buffer.edit(line.start..cs, "", self.cursor, now_ms());
            self.buffer.break_undo_group();
            self.after_edit(cx);
            return;
        }
        let prev = self.prev_char_start(self.cursor);
        self.cursor = self.buffer.edit(prev..self.cursor, "", self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_delete(&mut self, _: &EditorDelete, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selection() {
            self.after_edit(cx);
            return;
        }
        let next = self.next_char_end(self.cursor);
        if next == self.cursor {
            return;
        }
        self.cursor = self.buffer.edit(self.cursor..next, "", self.cursor, now_ms());
        self.after_edit(cx);
    }

    fn on_left(&mut self, _: &EditorLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = match self.selection() {
            Some(sel) => sel.start,
            None => self.left_target(),
        };
        self.move_cursor_to(target, false, cx);
    }

    fn on_right(&mut self, _: &EditorRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = match self.selection() {
            Some(sel) => sel.end,
            None => self.next_char_end(self.cursor),
        };
        self.move_cursor_to(target, false, cx);
    }

    fn on_up(&mut self, _: &EditorUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, cx);
    }

    fn on_down(&mut self, _: &EditorDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, cx);
    }

    fn on_select_left(&mut self, _: &EditorSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.left_target();
        self.move_cursor_to(target, true, cx);
    }

    fn on_select_right(&mut self, _: &EditorSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.next_char_end(self.cursor);
        self.move_cursor_to(target, true, cx);
    }

    fn on_select_up(&mut self, _: &EditorSelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }

    fn on_select_down(&mut self, _: &EditorSelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }

    /// Word-left, with a hidden marker treated as part of the gap between
    /// lines: from a glyph line's content start the step continues from the
    /// line start, so it lands on the previous line's last word.
    fn word_left_target(&self) -> usize {
        let mut from = self.cursor;
        let line = self.line_range_at(from);
        if let Some(cs) = self.hidden_marker_end(&line)
            && from <= cs
        {
            from = line.start;
        }
        prev_word_boundary(self.text(), from)
    }

    fn on_word_left(&mut self, _: &EditorWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.word_left_target();
        self.move_cursor_to(target, false, cx);
    }

    fn on_word_right(&mut self, _: &EditorWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = next_word_boundary(self.text(), self.cursor);
        self.move_cursor_to(target, false, cx);
    }

    fn on_select_word_left(
        &mut self,
        _: &EditorSelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.word_left_target();
        self.move_cursor_to(target, true, cx);
    }

    fn on_select_word_right(
        &mut self,
        _: &EditorSelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = next_word_boundary(self.text(), self.cursor);
        self.move_cursor_to(target, true, cx);
    }

    /// Line-start for the home motion: the visible start of the line, which
    /// on a glyph line is its content start.
    fn line_home_target(&self) -> usize {
        let line = self.line_range_at(self.cursor);
        self.hidden_marker_end(&line).unwrap_or(line.start)
    }

    fn on_line_start(&mut self, _: &EditorLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.line_home_target();
        self.move_cursor_to(target, false, cx);
    }

    fn on_line_end(&mut self, _: &EditorLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.line_range_at(self.cursor).end;
        self.move_cursor_to(target, false, cx);
    }

    fn on_select_line_start(
        &mut self,
        _: &EditorSelectLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.line_home_target();
        self.move_cursor_to(target, true, cx);
    }

    fn on_select_line_end(
        &mut self,
        _: &EditorSelectLineEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.line_range_at(self.cursor).end;
        self.move_cursor_to(target, true, cx);
    }

    fn on_doc_start(&mut self, _: &EditorDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor_to(0, false, cx);
    }

    fn on_doc_end(&mut self, _: &EditorDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.text().len();
        self.move_cursor_to(target, false, cx);
    }

    fn on_select_doc_start(
        &mut self,
        _: &EditorSelectDocStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_cursor_to(0, true, cx);
    }

    fn on_select_doc_end(
        &mut self,
        _: &EditorSelectDocEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.text().len();
        self.move_cursor_to(target, true, cx);
    }

    fn on_delete_word_back(
        &mut self,
        _: &EditorDeleteWordBack,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.delete_selection() {
            self.after_edit(cx);
            return;
        }
        let target = self.word_left_target();
        if target >= self.cursor {
            return;
        }
        self.buffer.break_undo_group();
        self.cursor = self.buffer.edit(target..self.cursor, "", self.cursor, now_ms());
        self.buffer.break_undo_group();
        self.after_edit(cx);
    }

    fn on_delete_to_line_start(
        &mut self,
        _: &EditorDeleteToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.delete_selection() {
            self.after_edit(cx);
            return;
        }
        let line = self.line_range_at(self.cursor);
        // At a glyph line's content start there is nothing visible left of
        // the caret but the marker itself, so the press removes it.
        let target = match self.hidden_marker_end(&line) {
            Some(cs) if self.cursor <= cs => line.start,
            Some(cs) => cs,
            None => line.start,
        };
        if target >= self.cursor {
            return;
        }
        self.buffer.break_undo_group();
        self.cursor = self.buffer.edit(target..self.cursor, "", self.cursor, now_ms());
        self.buffer.break_undo_group();
        self.after_edit(cx);
    }

    fn on_select_all(&mut self, _: &EditorSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection_anchor = Some(0);
        self.cursor = self.text().len();
        self.buffer.break_undo_group();
        self.reset_blink(cx);
        cx.notify();
    }

    fn on_copy(&mut self, _: &EditorCopy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy(cx);
    }

    fn on_cut(&mut self, _: &EditorCut, _: &mut Window, cx: &mut Context<Self>) {
        self.cut(cx);
    }

    /// Whether a non-empty selection exists (the context menu greys Cut and
    /// Copy without one).
    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text()[sel].to_string()));
        }
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.selection() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(self.text()[sel].to_string()));
        self.delete_selection();
        self.after_edit(cx);
    }

    fn move_vertical(&mut self, delta: i64, extend: bool, cx: &mut Context<Self>) {
        if extend {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            if let Some(sel) = self.selection() {
                self.cursor = if delta < 0 { sel.start } else { sel.end };
            }
            self.selection_anchor = None;
        }
        let line = self.line_range_at(self.cursor);
        let col = self.text()[line.start..self.cursor].chars().count();
        let target_start = if delta < 0 {
            if line.start == 0 {
                self.cursor = self.snap_to_content(0);
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
        self.cursor = self.snap_to_content(byte);
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
        self.paste(cx);
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.buffer.break_undo_group();
        self.cursor = self.buffer.edit(range, &text, self.cursor, now_ms());
        self.buffer.break_undo_group();
        self.after_edit(cx);
    }

    /// Follow a link target: the shared path behind a left-click on a link
    /// and the context menu's Open item.
    pub fn open_link(&mut self, target: notes::LinkTarget, cx: &mut Context<Self>) {
        match target {
            notes::LinkTarget::Wiki(title) => {
                cx.emit(NoteEditorEvent::OpenWikiLink(
                    notes::wiki_link_title(&title).to_string(),
                ));
            }
            notes::LinkTarget::Date(text) => {
                if let Some(date) = text
                    .strip_prefix('>')
                    .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                {
                    cx.emit(NoteEditorEvent::OpenDate(date));
                }
            }
            notes::LinkTarget::Url(url) => cx.emit(NoteEditorEvent::OpenUrl(url)),
        }
    }

    /// The link under a window position, if any: the same slot walk the
    /// left-click hit-test does, minus everything that isn't a link.
    fn link_at(&self, pos: Point<Pixels>) -> Option<notes::LinkTarget> {
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        for slot in &layout.slots {
            let top = layout.bounds.origin.y + slot.y;
            if pos.y < top || pos.y >= top + slot.height {
                continue;
            }
            // The active line shows raw markdown; links only resolve on
            // styled lines, matching the left-click behaviour.
            if slot.entry.active {
                return None;
            }
            let raw_line = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
            let origin = slot.text_origin_in(&layout.bounds);
            let local = point(pos.x - origin.x, pos.y - origin.y);
            let display_ix = slot
                .entry
                .wrapped
                .as_ref()?
                .index_for_position(local, slot.entry.line_height)
                .ok()?;
            let display_chars = slot
                .entry
                .display
                .get(..display_ix)
                .map_or(0, |s| s.chars().count());
            return notes::link_target_at_display_char(raw_line, display_chars);
        }
        None
    }

    // --- mouse -------------------------------------------------------------

    /// Right mouse down: remember what sits under the pointer so the context
    /// menu (built by the wrapper a frame later) can offer to open it.
    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_link = self.link_at(event.position);
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        enum Click {
            Handle { line_start: usize },
            Glyph { line_start: usize, toggles: bool },
            Nav(notes::LinkTarget),
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
                // The handle gutter: any non-blank line picks up for a drag.
                if pos.x < layout.bounds.origin.x + layout.gutter {
                    if !raw_line.trim().is_empty() {
                        click = Click::Handle { line_start: slot.raw_start };
                        break;
                    }
                }
                // The glyph column of a task or bullet line: a press-and-
                // release toggles (open/done tasks); a drag moves the block.
                else if matches!(slot.entry.glyph, Glyph::Task(_) | Glyph::Bullet)
                    && pos.x < layout.bounds.origin.x + slot.entry.indent
                {
                    click = Click::Glyph {
                        line_start: slot.raw_start,
                        toggles: matches!(
                            slot.entry.glyph,
                            Glyph::Task(TaskState::Open | TaskState::Done)
                        ),
                    };
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
                        if let Some(target) =
                            notes::link_target_at_display_char(raw_line, display_chars)
                        {
                            click = Click::Nav(target);
                            break;
                        }
                        notes::raw_col_for_display_char(raw_line, display_chars)
                    }
                    Some(ix) => (slot.entry.prefix_len + ix).min(raw_line.len()),
                    // Past the end of the line's text: cursor to line end.
                    None => raw_line.len(),
                };
                click = Click::Cursor(slot.raw_start + raw_col);
                break;
            }
            click
        };
        match click {
            Click::Handle { line_start } => {
                self.line_drag = Some(LineDrag {
                    range: notes::block_range(self.text(), line_start),
                    origin: event.position,
                    position: event.position,
                    moved: false,
                    toggles: false,
                    from_handle: true,
                    target: None,
                });
            }
            Click::Glyph { line_start, toggles } => {
                self.line_drag = Some(LineDrag {
                    range: notes::block_range(self.text(), line_start),
                    origin: event.position,
                    position: event.position,
                    moved: false,
                    toggles,
                    from_handle: false,
                    target: None,
                });
            }
            Click::Nav(target) => self.open_link(target, cx),
            Click::Cursor(target) => {
                let target = target.min(self.text().len());
                window.focus(&self.focus_handle);
                match event.click_count {
                    2 => {
                        let word = self.word_range_at(target);
                        self.selection_anchor = Some(word.start);
                        self.cursor = word.end;
                        self.selecting = true;
                        self.after_cursor_move(cx);
                    }
                    n if n >= 3 => {
                        // The whole line, trailing newline included.
                        let line = self.line_range_at(target);
                        self.selection_anchor = Some(line.start);
                        self.cursor = (line.end + 1).min(self.text().len());
                        self.selecting = true;
                        self.after_cursor_move(cx);
                    }
                    _ if event.modifiers.shift => {
                        self.selecting = true;
                        self.move_cursor_to(target, true, cx);
                    }
                    _ => {
                        self.selecting = true;
                        self.selection_anchor = Some(target);
                        self.cursor = target;
                        self.after_cursor_move(cx);
                    }
                }
            }
        }
    }

    /// Window-level mouse move while the primary button is down: extends a
    /// drag-select or tracks a line drag, wherever the pointer goes.
    fn on_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.line_drag.is_none() && !self.selecting {
            return;
        }
        if let Some(mut drag) = self.line_drag.take() {
            let delta = event.position - drag.origin;
            if !drag.moved
                && (delta.x.abs() > DRAG_THRESHOLD || delta.y.abs() > DRAG_THRESHOLD)
            {
                drag.moved = true;
            }
            drag.position = event.position;
            if drag.moved {
                // Inside the editor the drag is a reorder; once the pointer
                // leaves (e.g. up to the week strip) the drop indicator goes
                // away and releasing becomes the workspace's drop to handle.
                let inside = self
                    .layout
                    .borrow()
                    .as_ref()
                    .is_some_and(|l| l.bounds.contains(&event.position));
                drag.target = inside.then(|| self.drop_target_for_y(event.position.y));
                cx.notify();
            }
            self.line_drag = Some(drag);
            return;
        }
        if self.selecting
            && let Some(offset) = self.offset_for_point(event.position)
            && offset != self.cursor
        {
            self.selection_anchor.get_or_insert(self.cursor);
            self.cursor = offset;
            self.follow_cursor.set(true);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.line_drag.take() {
            if !drag.moved {
                if drag.from_handle {
                    // A handle press-and-release does nothing: the handle is
                    // purely a grab point.
                } else if drag.toggles {
                    let line = self.line_range_at(drag.range.start);
                    self.toggle_task_in(line, cx);
                } else {
                    // A glyph without a toggle (bullet, scheduled or
                    // cancelled task): the click still lands the cursor at
                    // the line's content instead of dying.
                    let line = self.line_range_at(drag.range.start);
                    let target = self.hidden_marker_end(&line).unwrap_or(line.start);
                    window.focus(&self.focus_handle);
                    self.move_cursor_to(target, false, cx);
                }
            } else if let Some(target) = drag.target {
                let new_start =
                    self.buffer.move_block(drag.range, target, self.cursor, now_ms());
                self.cursor = new_start;
                self.after_edit(cx);
            } else {
                // Released outside the editor: the workspace decides whether
                // the pointer was over a day drop target.
                cx.emit(NoteEditorEvent::BlockDropped {
                    range: drag.range,
                    position: drag.position,
                });
            }
            cx.notify();
            return;
        }
        if self.selecting {
            self.selecting = false;
            if self.selection_anchor == Some(self.cursor) {
                self.selection_anchor = None;
            }
            cx.notify();
        }
    }

    /// The buffer offset under a window position, clamped into the document:
    /// above the first line is the start, below the last is the end.
    fn offset_for_point(&self, pos: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let first = layout.slots.first()?;
        if pos.y < layout.bounds.origin.y + first.y {
            return Some(0);
        }
        for slot in &layout.slots {
            let top = layout.bounds.origin.y + slot.y;
            if pos.y >= top + slot.height {
                continue;
            }
            let raw_line = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
            let origin = slot.text_origin_in(&layout.bounds);
            let max_y = (slot.entry.text_height - px(1.)).max(px(0.));
            let local = point(
                (pos.x - origin.x).max(px(0.)),
                (pos.y - origin.y).clamp(px(0.), max_y),
            );
            let ix = slot
                .entry
                .wrapped
                .as_ref()
                .map(|w| match w.index_for_position(local, slot.entry.line_height) {
                    // The error carries the nearest row-edge index, which is
                    // exactly what dragging past a line's text should land on.
                    Ok(ix) | Err(ix) => ix,
                })
                .unwrap_or(0);
            let raw_col = if slot.entry.active {
                (slot.entry.prefix_len + ix).min(raw_line.len())
            } else {
                let chars = slot.entry.display.get(..ix).map_or(0, |s| s.chars().count());
                notes::raw_col_for_display_char(raw_line, chars)
            };
            return Some(slot.raw_start + raw_col);
        }
        Some(self.text().len())
    }

    /// Where a dragged line would land for a pointer at `y`: the line-start
    /// offset whose top edge is nearest below, or the text length for the
    /// end of the note.
    fn drop_target_for_y(&self, y: Pixels) -> usize {
        let layout = self.layout.borrow();
        let Some(layout) = layout.as_ref() else { return 0 };
        for slot in &layout.slots {
            let top = layout.bounds.origin.y + slot.y;
            if y < top + slot.height / 2. {
                return slot.raw_start;
            }
        }
        self.text().len()
    }

    /// The clickable span under a window position, for hover styling:
    /// only styled (inactive) lines participate — the cursor line shows raw
    /// markdown where nothing navigates on click.
    fn hover_target(&self, pos: Point<Pixels>) -> Option<(usize, Range<usize>)> {
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        if !layout.bounds.contains(&pos) {
            return None;
        }
        for slot in &layout.slots {
            let top = layout.bounds.origin.y + slot.y;
            if pos.y < top || pos.y >= top + slot.height {
                continue;
            }
            if slot.entry.active {
                return None;
            }
            let origin = slot.text_origin_in(&layout.bounds);
            let local = point(pos.x - origin.x, pos.y - origin.y);
            let ix = slot
                .entry
                .wrapped
                .as_ref()?
                .index_for_position(local, slot.entry.line_height)
                .ok()?;
            let chars = slot.entry.display.get(..ix).map_or(0, |s| s.chars().count());
            let raw_line = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
            let range = notes::link_display_range(raw_line, chars)?;
            return Some((slot.raw_start, range));
        }
        None
    }

    /// Mouse movement with no button down: track the hovered link and the
    /// hovered line (for its drag handle), repainting only on change.
    fn on_hover_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let target = self.hover_target(pos);
        if target != self.hovered_link {
            self.hovered_link = target;
            cx.notify();
        }
        let line = {
            let layout = self.layout.borrow();
            layout.as_ref().filter(|l| l.bounds.contains(&pos)).and_then(|l| {
                l.slots.iter().find(|slot| {
                    let top = l.bounds.origin.y + slot.y;
                    pos.y >= top && pos.y < top + slot.height
                })
            })
            .map(|slot| slot.raw_start)
        };
        if line != self.hovered_line {
            self.hovered_line = line;
            cx.notify();
        }
    }

    fn toggle_task_in(&mut self, raw_range: Range<usize>, cx: &mut Context<Self>) {
        let line = self.text()[raw_range.clone()].to_string();
        let Some(toggled) = notes::toggle_task_line(&line) else { return };
        self.buffer.edit(raw_range, &toggled, self.cursor, now_ms());
        self.after_edit(cx);
    }

    /// The in-flight line drag, once it has actually moved: the block's
    /// first line, how many further lines travel with it, and the pointer's
    /// window position. The workspace reads this to draw the drag ghost and
    /// light day drop targets.
    pub fn line_drag(&self) -> Option<(String, usize, Point<Pixels>)> {
        let drag = self.line_drag.as_ref()?;
        if !drag.moved {
            return None;
        }
        let block = &self.text()[drag.range.clone()];
        let mut lines = block.split('\n');
        let first = lines.next().unwrap_or("").to_string();
        Some((first, lines.count(), drag.position))
    }

    /// The text of a block the editor reported dropped, for the workspace's
    /// cross-note move.
    pub fn block_text(&self, range: Range<usize>) -> String {
        let len = self.text().len();
        self.text()[range.start.min(len)..range.end.min(len)].to_string()
    }

    /// Remove a dropped block from this note (the source half of a move to
    /// another day), together with the newline that separated it from its
    /// neighbours. One undoable step.
    pub fn remove_block(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let len = self.text().len();
        let mut start = range.start.min(len);
        let mut end = range.end.min(len).max(start);
        if end < len {
            end += 1;
        } else {
            // The block ends the file: take the preceding newline so the
            // remaining last line keeps the no-trailing-newline convention.
            start = start.saturating_sub(1);
        }
        self.cursor = self.buffer.edit(start..end, "", self.cursor, now_ms());
        self.after_edit(cx);
    }

    /// Move a block within this note to the line boundary `target` (the
    /// in-buffer half of a drop on the note's own day). One undoable step.
    pub fn move_block_to(&mut self, range: Range<usize>, target: usize, cx: &mut Context<Self>) {
        self.cursor = self.buffer.move_block(range, target, self.cursor, now_ms());
        self.after_edit(cx);
    }

    /// Abandon any in-flight line drag (Escape): nothing has been edited
    /// yet, so letting go of the state is the whole cancel.
    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.line_drag.take().is_some() {
            cx.notify();
        }
    }

    /// The hovered line's slot geometry (content-relative y, height) when
    /// its drag handle should show: a non-blank line with no drag in flight.
    fn hover_handle_slot(&self) -> Option<(Pixels, Pixels)> {
        if self.line_drag.is_some() {
            return None;
        }
        let start = self.hovered_line?;
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let slot = layout.slots.iter().find(|s| s.raw_start == start)?;
        let raw = &self.text()[slot.raw_start..slot.raw_start + slot.raw_len];
        if raw.trim().is_empty() {
            return None;
        }
        Some((slot.y, slot.height))
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
            let hover = self
                .hovered_link
                .as_ref()
                .and_then(|(ls, range)| (*ls == start).then(|| range.clone()));
            let entry = self.entry_for(raw, active, hover, width, &t, window);
            let height = entry.pad_top + entry.text_height + entry.pad_bottom;
            slots.push(LineSlot { raw_start: start, raw_len: raw.len(), y, height, entry });
            y += height;
            start += raw.len() + 1;
        }
        let mut layout = self.layout.borrow_mut();
        let bounds = layout.as_ref().map(|l| l.bounds).unwrap_or_default();
        let gutter = px(HANDLE_GUTTER * (t.editor_size / theme::EDITOR_BASE_SIZE));
        *layout = Some(EditorLayout { bounds, gutter, slots });
        y
    }

    fn entry_for(
        &self,
        raw: &str,
        active: bool,
        hover: Option<Range<usize>>,
        width: Pixels,
        t: &KairnTheme,
        window: &mut Window,
    ) -> Rc<ShapedEntry> {
        // The active line is never cached: its shaping also depends on the
        // IME marked range, which the content key can't see, and there is
        // exactly one such line. A hover-underlined line stays out of the
        // cache too — the underline isn't part of the content key. Everything
        // else is keyed purely by content, which is what lets a keystroke
        // reshape one line, not the note.
        let cacheable = !active && hover.is_none();
        if cacheable
            && let Some(hit) = self.cache.borrow().entries.get(raw)
        {
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
            hover,
            t,
            window,
        ));
        if cacheable {
            let mut cache = self.cache.borrow_mut();
            // Content keys never invalidate, they just stop being hit; the
            // bound keeps edited-away lines from pinning memory forever.
            if cache.entries.len() >= ShapeCache::CAPACITY {
                cache.entries.clear();
            }
            cache.entries.insert(raw.to_string(), entry.clone());
        }
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
        let editor = cx.entity().downgrade();
        let t = cx.kairn();
        // The handle gutter is baked into every line's indent; pulling the
        // element left by the same width keeps the text where it was, with
        // the handles hanging in the note frame's padding.
        let gutter = px(HANDLE_GUTTER * (t.editor_size / theme::EDITOR_BASE_SIZE));
        div()
            .key_context("NoteEditor")
            .track_focus(&self.focus_handle)
            .cursor_text()
            .ml(-gutter)
            // Room below the last line so clicking the empty space under a
            // short note lands in the editor and appends.
            .pb(px(140.))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
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
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_select_left))
            .on_action(cx.listener(Self::on_select_right))
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_word_left))
            .on_action(cx.listener(Self::on_word_right))
            .on_action(cx.listener(Self::on_select_word_left))
            .on_action(cx.listener(Self::on_select_word_right))
            .on_action(cx.listener(Self::on_line_start))
            .on_action(cx.listener(Self::on_line_end))
            .on_action(cx.listener(Self::on_select_line_start))
            .on_action(cx.listener(Self::on_select_line_end))
            .on_action(cx.listener(Self::on_doc_start))
            .on_action(cx.listener(Self::on_doc_end))
            .on_action(cx.listener(Self::on_select_doc_start))
            .on_action(cx.listener(Self::on_select_doc_end))
            .on_action(cx.listener(Self::on_delete_word_back))
            .on_action(cx.listener(Self::on_delete_to_line_start))
            .child(NoteEditorElement { editor: cx.entity().clone() })
            .context_menu(move |menu, _, cx| {
                let Some(strong) = editor.upgrade() else { return menu };
                let (link, has_selection) = {
                    let ed = strong.read(cx);
                    (ed.menu_link.clone(), ed.has_selection())
                };
                let mut menu = menu;
                if let Some(target) = link {
                    let label = match &target {
                        notes::LinkTarget::Wiki(_) => "Open note",
                        notes::LinkTarget::Date(_) => "Open day",
                        notes::LinkTarget::Url(_) => "Open link",
                    };
                    let ed = strong.clone();
                    menu = menu
                        .item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let target = target.clone();
                            ed.update(cx, |ed, cx| ed.open_link(target, cx));
                        }))
                        .separator();
                }
                let ed = strong.clone();
                menu = menu.item(
                    PopupMenuItem::new("Cut").disabled(!has_selection).on_click(
                        move |_, window, cx| {
                            ed.update(cx, |ed, cx| ed.cut(cx));
                            window.focus(&ed.read(cx).focus_handle);
                        },
                    ),
                );
                let ed = strong.clone();
                menu = menu.item(
                    PopupMenuItem::new("Copy").disabled(!has_selection).on_click(
                        move |_, window, cx| {
                            ed.update(cx, |ed, cx| ed.copy(cx));
                            window.focus(&ed.read(cx).focus_handle);
                        },
                    ),
                );
                let ed = strong.clone();
                menu.item(PopupMenuItem::new("Paste").on_click(move |_, window, cx| {
                    ed.update(cx, |ed, cx| ed.paste(cx));
                    window.focus(&ed.read(cx).focus_handle);
                }))
            })
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
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        Some(UTF16Selection {
            range: self.range_to_utf16(&range),
            reversed: self.selection_anchor.is_some_and(|a| a > self.cursor),
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
            .or_else(|| self.selection())
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
            .or_else(|| self.selection())
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
        let ix = (range.start - slot.raw_start).saturating_sub(slot.entry.prefix_len);
        let local = wrapped.position_for_index(ix, slot.entry.line_height)?;
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
                    (slot.entry.prefix_len + ix).min(raw.len())
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

/// Per-kind metrics: body 13px at 1.58 line height, headings on the
/// standard markdown scale (`#` biggest, deeper levels smaller),
/// task/bullet indents from the glyph column.
struct KindStyle {
    size: Pixels,
    line_height: Pixels,
    pad_top: Pixels,
    pad_bottom: Pixels,
    indent: Pixels,
    color: Hsla,
    weight: FontWeight,
}

fn kind_style(line: &Line, t: &KairnTheme) -> (KindStyle, Glyph) {
    // The metrics below are drawn against the default 13px body; a themed
    // editor size scales text, line heights, and the glyph column (indents)
    // together, leaving only the vertical paddings alone. Every indent
    // includes the handle gutter, so text and hit-testing agree without a
    // separate offset.
    let scale = t.editor_size / crate::theme::EDITOR_BASE_SIZE;
    let gutter = HANDLE_GUTTER * scale;
    let body = |color| KindStyle {
        size: px(13. * scale),
        line_height: px(20.5 * scale),
        pad_top: px(1.),
        pad_bottom: px(1.),
        indent: px(gutter),
        color,
        weight: FontWeight::NORMAL,
    };
    match line {
        Line::Heading { level, .. } => {
            // Standard markdown scale: `#` biggest down to `#####` smallest,
            // deeper levels treated as 5. Sizes sit against the 13px body.
            let heading = |size: f32, lh: f32, pad_top: f32, pad_bottom: f32| KindStyle {
                size: px(size * scale),
                line_height: px(lh * scale),
                pad_top: px(pad_top),
                pad_bottom: px(pad_bottom),
                weight: FontWeight::SEMIBOLD,
                ..body(t.heading)
            };
            let style = match level {
                1 => KindStyle { weight: FontWeight::BOLD, ..heading(20., 28., 18., 6.) },
                2 => heading(17., 24., 16., 5.),
                3 => heading(15., 22., 14., 4.),
                4 => heading(13.5, 21., 12., 3.),
                _ => KindStyle { color: t.dim, ..heading(13., 20.5, 10., 2.) },
            };
            (style, Glyph::None)
        }
        Line::Task { state, spans } => {
            let color = match state {
                // A `!`-prefixed open task runs hot so it stands out.
                TaskState::Open if notes::task_priority(spans) > 0 => t.red,
                TaskState::Open => t.text,
                TaskState::Scheduled => t.dim,
                TaskState::Done | TaskState::Cancelled => t.faint,
            };
            (
                KindStyle {
                    pad_top: px(2.5),
                    pad_bottom: px(2.5),
                    indent: px(gutter + 22. * scale),
                    ..body(color)
                },
                Glyph::Task(*state),
            )
        }
        Line::Bullet { .. } => (
            KindStyle {
                pad_top: px(2.5),
                pad_bottom: px(2.5),
                indent: px(gutter + 18. * scale),
                ..body(t.text)
            },
            Glyph::Bullet,
        ),
        Line::Quote { .. } => (
            KindStyle {
                pad_top: px(4.),
                pad_bottom: px(4.),
                indent: px(gutter + 14. * scale),
                ..body(t.dim)
            },
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
        SpanKind::WikiLink | SpanKind::Link | SpanKind::Url => {
            (t.accent, None, FontWeight::NORMAL, FontStyle::Normal)
        }
        SpanKind::Tag | SpanKind::DateRef => (t.amber, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Mention => (t.faint, None, FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Highlight => (t.text, Some(t.highlight), FontWeight::NORMAL, FontStyle::Normal),
        SpanKind::Bold => (t.bold, None, FontWeight::BOLD, FontStyle::Normal),
        SpanKind::Italic => (base, None, FontWeight::NORMAL, FontStyle::Italic),
        SpanKind::Hidden => (t.faint, None, FontWeight::NORMAL, FontStyle::Normal),
    }
}

fn shape_entry(
    raw: &str,
    active: bool,
    width: Pixels,
    marked_local: Option<Range<usize>>,
    hover_local: Option<Range<usize>>,
    t: &KairnTheme,
    window: &mut Window,
) -> ShapedEntry {
    let parsed = notes::parse_line(raw);
    let (style, glyph) = kind_style(&parsed, t);
    let gutter = px(HANDLE_GUTTER * (t.editor_size / crate::theme::EDITOR_BASE_SIZE));
    let strikethrough = matches!(
        parsed,
        Line::Task { state: TaskState::Done | TaskState::Cancelled, .. }
    );

    // Blank lines hold a full text row whether or not the cursor is on
    // them: uniform height keeps the document from shifting as the cursor
    // passes through, and Enter on an empty line drops a full line like
    // any editor. (The active blank falls through to shaping so a
    // whitespace-only line still positions the caret inside its spaces.)
    if !active && matches!(parsed, Line::Blank) {
        return ShapedEntry {
            display: SharedString::default(),
            wrapped: None,
            line_height: style.line_height,
            text_height: style.line_height,
            pad_top: px(1.),
            pad_bottom: px(1.),
            indent: gutter,
            glyph: Glyph::None,
            active,
            prefix_len: 0,
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
            indent: gutter,
            glyph: Glyph::Rule,
            active,
            prefix_len: 0,
        };
    }

    let base_font = {
        let mut f = window.text_style().font();
        f.weight = style.weight;
        f
    };
    let strike = strikethrough.then_some(StrikethroughStyle {
        thickness: px(1.),
        color: Some(style.color),
    });

    let spans = match &parsed {
        Line::Heading { spans, .. }
        | Line::Task { spans, .. }
        | Line::Bullet { spans }
        | Line::Quote { spans }
        | Line::Text { spans } => spans.as_slice(),
        Line::Rule | Line::Blank => &[],
    };
    let (display, runs, indent, glyph, prefix_len) = if active {
        // The cursor line reveals its raw inline markdown, byte for byte, so
        // inline markers can be edited in place — but glyph lines (tasks,
        // bullets, quotes) keep their marker hidden and their glyph painted,
        // exactly like inactive lines: the checkbox never blinks out from
        // under the pointer, and the text never shifts when the cursor
        // arrives. The hidden prefix makes the mapping display index + prefix
        // = raw column. While an IME composition is marked, plain runs with
        // the marked range underlined take over.
        let prefix = match &parsed {
            Line::Task { .. } | Line::Bullet { .. } | Line::Quote { .. } => {
                notes::content_start_col(raw).min(raw.len())
            }
            _ => 0,
        };
        let shown = &raw[prefix..];
        let mut runs = Vec::new();
        if let Some(m) = &marked_local {
            let m = m.start.saturating_sub(prefix)..m.end.saturating_sub(prefix);
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
            push(m.start.min(shown.len()), false);
            push(m.end.min(shown.len()).saturating_sub(m.start.min(shown.len())), true);
            push(shown.len().saturating_sub(m.end.min(shown.len())), false);
        } else {
            let mut push = |len: usize, kind: Option<SpanKind>| {
                if len == 0 {
                    return;
                }
                let (color, bg, weight, font_style) = match kind {
                    Some(k) => span_style(k, style.color, t),
                    // The line's own revealed prefix (heading hashes) and
                    // any bytes past the spans.
                    None => (t.faint, None, FontWeight::NORMAL, FontStyle::Normal),
                };
                let mut font = base_font.clone();
                if weight > font.weight {
                    font.weight = weight;
                }
                font.style = font_style;
                runs.push(TextRun {
                    len,
                    font,
                    color,
                    background_color: bg,
                    underline: None,
                    strikethrough: strike,
                });
            };
            let start = notes::spans_start_col(raw).min(raw.len()).saturating_sub(prefix);
            push(start, None);
            let mut covered = start;
            for (kind, s) in spans {
                let len = s.len().min(shown.len().saturating_sub(covered));
                push(len, Some(*kind));
                covered += len;
            }
            if covered < shown.len() {
                let len = shown.len() - covered;
                let mut font = base_font.clone();
                font.style = FontStyle::Normal;
                runs.push(TextRun {
                    len,
                    font,
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: strike,
                });
            }
        }
        let (indent, glyph) =
            if prefix > 0 { (style.indent, glyph) } else { (gutter, Glyph::None) };
        (SharedString::from(shown.to_string()), runs, indent, glyph, prefix)
    } else {
        let mut display = String::new();
        let mut runs = Vec::new();
        let mut chars_seen = 0usize;
        for (kind, text) in spans {
            // Hidden spans hold raw bytes the styled line does not render.
            if *kind == SpanKind::Hidden {
                continue;
            }
            let (color, bg, weight, font_style) = span_style(*kind, style.color, t);
            let mut font = base_font.clone();
            if weight > font.weight {
                font.weight = weight;
            }
            font.style = font_style;
            // The hovered link underlines so it reads as clickable.
            let chars = text.chars().count();
            let hovered = hover_local
                .as_ref()
                .is_some_and(|h| chars_seen < h.end && chars_seen + chars > h.start);
            runs.push(TextRun {
                len: text.len(),
                font,
                color,
                background_color: bg,
                underline: hovered.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: strike,
            });
            display.push_str(text);
            chars_seen += chars;
        }
        (SharedString::from(display), runs, style.indent, glyph, 0)
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
        prefix_len,
    }
}

#[cfg(test)]
mod tests {
    use super::{next_word_boundary, prev_word_boundary};

    #[test]
    fn word_left_lands_on_word_starts() {
        let t = "hello brave world";
        assert_eq!(prev_word_boundary(t, 17), 12); // from end -> "world"
        assert_eq!(prev_word_boundary(t, 12), 6); // -> "brave"
        assert_eq!(prev_word_boundary(t, 8), 6); // mid-word -> its start
        assert_eq!(prev_word_boundary(t, 6), 0);
        assert_eq!(prev_word_boundary(t, 0), 0);
    }

    #[test]
    fn word_right_lands_on_word_ends() {
        let t = "hello brave world";
        assert_eq!(next_word_boundary(t, 0), 5);
        assert_eq!(next_word_boundary(t, 5), 11);
        assert_eq!(next_word_boundary(t, 8), 11); // mid-word -> its end
        assert_eq!(next_word_boundary(t, 17), 17);
    }

    #[test]
    fn word_motion_crosses_newlines() {
        let t = "one\ntwo";
        assert_eq!(next_word_boundary(t, 3), 7);
        assert_eq!(prev_word_boundary(t, 4), 0);
    }

    #[test]
    fn punctuation_runs_are_their_own_stops() {
        let t = "a -- b";
        assert_eq!(next_word_boundary(t, 1), 4);
        assert_eq!(prev_word_boundary(t, 5), 2);
    }

    #[test]
    fn word_boundaries_respect_utf8() {
        let t = "café über";
        assert_eq!(next_word_boundary(t, 0), 5); // café is 5 bytes
        assert_eq!(prev_word_boundary(t, t.len()), 6);
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
    /// The hovered line's handle-gutter hitbox, carried into paint for the
    /// open-hand cursor (hitboxes can only be inserted during prepaint).
    type PrepaintState = Option<gpui::Hitbox>;

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
            // The hovered line's handle cell gets a hitbox so paint can give
            // it the open-hand cursor.
            let handle_cell = ed.hover_handle_slot().map(|(y, height)| {
                let gutter =
                    ed.layout.borrow().as_ref().map(|l| l.gutter).unwrap_or_default();
                Bounds::new(point(bounds.origin.x, bounds.origin.y + y), size(gutter, height))
            });
            handle_cell.map(|cell| window.insert_hitbox(cell, gpui::HitboxBehavior::Normal))
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let t = cx.kairn().clone();
        let (focus_handle, cursor, blink_visible, selection, text, drag_target, drag_lifted, handle_slot) = {
            let ed = self.editor.read(cx);
            let selection = ed.selection();
            // The document text is only consulted for mapping a selection's
            // raw offsets onto styled display text; don't clone it per frame
            // otherwise.
            let text = selection.is_some().then(|| ed.text().to_string());
            (
                ed.focus_handle.clone(),
                ed.cursor,
                ed.blink_visible,
                selection,
                text,
                ed.line_drag.as_ref().filter(|d| d.moved).and_then(|d| d.target),
                ed.line_drag.as_ref().filter(|d| d.moved).map(|d| d.range.clone()),
                ed.hover_handle_slot(),
            )
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        // Drag tracking lives at the window level so a selection or line
        // drag keeps following the pointer outside the element's bounds;
        // buttonless moves feed link-hover styling instead.
        window.on_mouse_event({
            let editor = self.editor.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                match event.pressed_button {
                    Some(MouseButton::Left) => {
                        editor.update(cx, |ed, cx| ed.on_drag_move(event, cx));
                    }
                    None => {
                        editor.update(cx, |ed, cx| ed.on_hover_move(event.position, cx));
                    }
                    Some(_) => {}
                }
            }
        });
        window.on_mouse_event({
            let editor = self.editor.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                editor.update(cx, |ed, cx| ed.on_mouse_up(window, cx));
            }
        });
        // Escape abandons an in-flight drag. Window-level like the mouse
        // handlers: a handle grab never focuses the editor, so an action
        // binding wouldn't reach it.
        window.on_key_event({
            let editor = self.editor.clone();
            move |event: &gpui::KeyDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.keystroke.key != "escape" {
                    return;
                }
                editor.update(cx, |ed, cx| ed.cancel_drag(cx));
            }
        });

        let layout = self.editor.read(cx).layout.clone();
        let layout = layout.borrow();
        let Some(layout) = layout.as_ref() else { return };
        let focused = focus_handle.is_focused(window);
        let viewport_bottom = window.viewport_size().height;

        for slot in &layout.slots {
            let block_top = bounds.origin.y + slot.y;
            // The element is far taller than the scroll viewport on long
            // notes; slots outside the window contribute nothing visible,
            // so skip their draw calls entirely.
            if block_top + slot.height < px(0.) || block_top > viewport_bottom {
                continue;
            }
            let text_origin = slot.text_origin_in(&bounds);

            // Selection, painted under the text. Offsets are raw bytes;
            // styled lines map them onto their display text through the
            // same span math clicks use.
            if let Some(sel) = &selection {
                let line_end = slot.raw_start + slot.raw_len;
                if sel.start < line_end + 1 && sel.end > slot.raw_start {
                    let s_raw = sel.start.max(slot.raw_start) - slot.raw_start;
                    let e_raw = sel.end.min(line_end) - slot.raw_start;
                    let includes_newline = sel.end > line_end;
                    let color = t.sel;
                    match &slot.entry.wrapped {
                        Some(wrapped) => {
                            let (s_ix, e_ix) = if slot.entry.active {
                                let p = slot.entry.prefix_len;
                                (s_raw.saturating_sub(p), e_raw.saturating_sub(p))
                            } else {
                                // `text` is always present when a selection is.
                                let raw_line = text
                                    .as_deref()
                                    .map(|t| &t[slot.raw_start..line_end])
                                    .unwrap_or("");
                                let display = &slot.entry.display;
                                let to_ix = |raw_col: usize| {
                                    let ch =
                                        notes::display_char_for_raw_col(raw_line, raw_col);
                                    display
                                        .char_indices()
                                        .nth(ch)
                                        .map_or(display.len(), |(i, _)| i)
                                };
                                (to_ix(s_raw), to_ix(e_raw))
                            };
                            let lh = slot.entry.line_height;
                            let zero = point(px(0.), px(0.));
                            let start =
                                wrapped.position_for_index(s_ix, lh).unwrap_or(zero);
                            let end =
                                wrapped.position_for_index(e_ix, lh).unwrap_or(start);
                            let mut end_x = end.x;
                            if includes_newline && e_ix >= slot.entry.display.len() {
                                end_x += px(7.);
                            }
                            let row_width = wrapped.size(lh).width;
                            let rows = ((end.y - start.y) / lh).round() as usize;
                            for i in 0..=rows {
                                let (x0, x1) = if rows == 0 {
                                    (start.x, end_x)
                                } else if i == 0 {
                                    (start.x, row_width)
                                } else if i == rows {
                                    (px(0.), end_x)
                                } else {
                                    (px(0.), row_width)
                                };
                                if x1 <= x0 {
                                    continue;
                                }
                                window.paint_quad(fill(
                                    Bounds::new(
                                        text_origin + point(x0, start.y + lh * i as f32),
                                        size(x1 - x0, lh),
                                    ),
                                    color,
                                ));
                            }
                        }
                        // Blank spacers and rules inside the selection get a
                        // thin stub so the sweep reads as continuous.
                        None => {
                            window.paint_quad(fill(
                                Bounds::new(
                                    text_origin,
                                    size(px(7.), slot.entry.text_height),
                                ),
                                color,
                            ));
                        }
                    }
                }
            }

            // Glyphs sit in the column between the handle gutter and the
            // text.
            let glyph_x = bounds.origin.x + layout.gutter;
            match &slot.entry.glyph {
                Glyph::Rule => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(glyph_x, block_top + slot.entry.pad_top),
                            size(bounds.size.width - layout.gutter, px(1.)),
                        ),
                        t.border,
                    ));
                }
                Glyph::Task(state) => {
                    let scale = t.editor_size / theme::EDITOR_BASE_SIZE;
                    paint_task_box(
                        point(glyph_x, text_origin.y + px(4. * scale)),
                        state,
                        &t,
                        window,
                        cx,
                    );
                }
                Glyph::Bullet => {
                    let scale = t.editor_size / theme::EDITOR_BASE_SIZE;
                    let dash = window.text_system().shape_line(
                        SharedString::from("–"),
                        px(13. * scale),
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
                        point(glyph_x, text_origin.y),
                        slot.entry.line_height,
                        window,
                        cx,
                    );
                }
                Glyph::QuoteBar => {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(glyph_x, block_top + slot.entry.pad_top),
                            size(px(2.), slot.entry.text_height),
                        ),
                        t.border,
                    ));
                }
                Glyph::None => {}
            }

            if let Some(wrapped) = &slot.entry.wrapped {
                // Backgrounds (==highlights==) are a separate pass in gpui:
                // `paint` draws only glyphs and decorations.
                let _ = wrapped.paint_background(
                    text_origin,
                    slot.entry.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                let _ = wrapped.paint(
                    text_origin,
                    slot.entry.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }

            // The caret: on the slot that owns the cursor offset, hidden
            // while a selection is showing.
            if focused
                && blink_visible
                && selection.is_none()
                && (slot.raw_start..=slot.raw_start + slot.raw_len).contains(&cursor)
            {
                let local = slot
                    .entry
                    .wrapped
                    .as_ref()
                    .and_then(|w| {
                        let ix =
                            (cursor - slot.raw_start).saturating_sub(slot.entry.prefix_len);
                        w.position_for_index(ix, slot.entry.line_height)
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

        // The dragged block lifts (dims) while a line drag is in flight, so
        // it reads as picked up rather than duplicated by the ghost.
        if let Some(lifted) = &drag_lifted {
            let mut span: Option<(Pixels, Pixels)> = None;
            for slot in &layout.slots {
                if slot.raw_start >= lifted.start && slot.raw_start < lifted.end.max(lifted.start + 1) {
                    let top = bounds.origin.y + slot.y;
                    let bottom = top + slot.height;
                    span = Some(match span {
                        Some((t0, b0)) => (t0.min(top), b0.max(bottom)),
                        None => (top, bottom),
                    });
                }
            }
            if let Some((top, bottom)) = span {
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.origin.x, top),
                        size(bounds.size.width, bottom - top),
                    ),
                    t.bg.opacity(0.65),
                ));
            }
        }

        // The hovered line's drag handle: a quiet grip in the gutter that
        // invites the pick-up.
        if let Some((y, _height)) = handle_slot {
            let scale = t.editor_size / theme::EDITOR_BASE_SIZE;
            let grip = window.text_system().shape_line(
                SharedString::from("⠿"),
                px(11. * scale),
                &[TextRun {
                    len: "⠿".len(),
                    font: window.text_style().font(),
                    color: t.faint,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let slot = layout.slots.iter().find(|s| s.y == y);
            if let Some(slot) = slot {
                let x = bounds.origin.x + (layout.gutter - grip.width).max(px(0.)) / 2.;
                let _ = grip.paint(
                    point(x, bounds.origin.y + slot.y + slot.entry.pad_top),
                    slot.entry.line_height,
                    window,
                    cx,
                );
            }
        }
        if let Some(hitbox) = prepaint {
            window.set_cursor_style(gpui::CursorStyle::OpenHand, hitbox);
        }

        // The drop indicator for an in-flight glyph drag: a line across the
        // boundary the dragged line would land on.
        if let Some(target) = drag_target {
            let y = layout
                .slots
                .iter()
                .find(|s| s.raw_start == target)
                .map(|s| bounds.origin.y + s.y)
                .or_else(|| {
                    layout.slots.last().map(|s| bounds.origin.y + s.y + s.height)
                })
                .unwrap_or(bounds.origin.y);
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x, y - px(1.)),
                    size(bounds.size.width, px(2.)),
                ),
                t.accent,
            ));
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
    let scale = t.editor_size / theme::EDITOR_BASE_SIZE;
    let side = px(13. * scale);
    let box_bounds = Bounds::new(origin, size(side, side));
    let mut quad = fill(box_bounds, gpui::transparent_black());
    quad.corner_radii = Corners::all(px(4. * scale));
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
        TaskState::Done => ("✓", t.bg, px(9. * scale)),
        TaskState::Cancelled => ("✕", t.faint, px(8. * scale)),
        TaskState::Scheduled => ("›", t.faint, px(8. * scale)),
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
        origin.x + (side - line.width).max(px(0.)) / 2.,
        origin.y + (side - size_px * 1.2) / 2.,
    );
    let _ = line.paint(inset, size_px * 1.2, window, cx);
}
