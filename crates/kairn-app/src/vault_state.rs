//! Everything the workspace derives from the notes root: navigation,
//! the reload pipeline, task toggling, the file watcher, and applying
//! settings changes.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Days, Local, NaiveDate, Timelike};
use gpui::{AppContext as _, Context, Task, Window};
use gpui_component::WindowExt;
use kairn_core as notes;
use kairn_core::TaskQuery;

use crate::workspace::{LayoutMode, PaneView, Workspace};

/// Paths this instance just wrote, with when and a hash of what was
/// written: the file watcher uses it to skip reload storms caused by our
/// own atomic-write renames, without going blind to real external edits.
pub(crate) type SelfWrites = Arc<parking_lot::Mutex<HashMap<PathBuf, (Instant, u64)>>>;

/// Day drop targets' window bounds, captured at paint time by the surface
/// that rendered them (week strip, mini calendar, Daily rows) and cleared by
/// that surface when it doesn't render.
pub(crate) type DayBounds =
    std::rc::Rc<std::cell::RefCell<Vec<(NaiveDate, gpui::Bounds<gpui::Pixels>)>>>;

/// The line index holding `expected`: `line_idx` when it still matches,
/// else the unique content match; `None` when it is gone or ambiguous (the
/// same relocation contract as kairn-core's disk edits).
fn relocate_line(text: &str, line_idx: usize, expected: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.get(line_idx).is_some_and(|l| *l == expected) {
        return Some(line_idx);
    }
    let mut matches =
        lines.iter().enumerate().filter(|(_, l)| **l == expected).map(|(i, _)| i);
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

/// Byte offset where line `line_idx` starts (the text length when past the
/// end, which `move_block` treats as "the end of the note").
fn line_start_offset(text: &str, line_idx: usize) -> usize {
    text.split('\n').take(line_idx).map(|l| l.len() + 1).sum::<usize>().min(text.len())
}

fn minutes_of(t: chrono::NaiveTime) -> i32 {
    (t.hour() * 60 + t.minute()) as i32
}

/// The clock time `min` minutes into the day, clamped to the day.
fn time_of(min: i32) -> chrono::NaiveTime {
    let min = min.clamp(0, 23 * 60 + 59) as u32;
    chrono::NaiveTime::from_hms_opt(min / 60, min % 60, 0).expect("clamped to a valid time")
}

/// `min` rounded to the nearest 5-minute mark.
fn snap5(min: i32) -> i32 {
    (min + 2).div_euclid(5) * 5
}

fn file_hash(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

/// Whether an event for `path` matches a write this instance made moments
/// ago and the file still holds exactly what we wrote. Any mismatch means
/// an external writer got there too, and the event must reload.
fn is_recent_self_write(self_writes: &SelfWrites, path: &Path) -> bool {
    let mut map = self_writes.lock();
    map.retain(|_, (when, _)| when.elapsed() < Duration::from_secs(10));
    let Some((when, hash)) = map.get(path) else {
        return false;
    };
    when.elapsed() < Duration::from_secs(2) && file_hash(path) == Some(*hash)
}

/// Whether a vault watcher event can change what the UI shows. Access
/// events (reads) never can — and on Linux, inotify reports the reload's
/// own file reads and directory walks back as Access events, so reacting
/// to them makes every reload schedule the next one, a self-sustaining
/// loop that pegs a core. macOS never surfaces reads, which is why only
/// Linux showed it.
fn notes_event_relevant(event: &notify::Event, self_writes: &SelfWrites) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|p| {
            // The activity log is the one `.kairn/` file the UI
            // shows live: CLI writes must surface without a restart.
            let watched_in_kairn = p.file_name().is_some_and(|n| n == "activity.jsonl");
            (watched_in_kairn || !p.components().any(|c| c.as_os_str() == ".kairn"))
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".kairn-tmp"))
                && !is_recent_self_write(self_writes, p)
        })
}

/// Library counterpart of `notes_event_relevant`: the same Access guard,
/// with events under ignored subtrees (VCS, builds, dotfiles) dropped at
/// the source — a `.git` churn in a big root must not cost reloads.
fn library_event_relevant(event: &notify::Event, self_writes: &SelfWrites) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|p| {
            !p.components().any(|c| {
                c.as_os_str().to_str().is_some_and(notes::library_ignored_name)
            }) && !is_recent_self_write(self_writes, p)
        })
}

/// OS-trash shim for library deletes. macOS must use the NSFileManager
/// method: the crate's Finder (AppleScript) default would raise an
/// Automation permission prompt. Linux follows the freedesktop trash spec.
fn os_trash(path: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path)
    }
}

impl Workspace {
    /// Record a write this instance just made, so the watcher event it
    /// triggers doesn't cost a second full reload.
    pub(crate) fn note_self_write(&self, path: &Path) {
        if let Some(hash) = file_hash(path) {
            self.self_writes
                .lock()
                .insert(path.to_path_buf(), (Instant::now(), hash));
        }
    }

    /// Anything that shows a note must actually show it: a full-screen
    /// terminal drops back to the split. Every opener funnels through the
    /// three methods below, so this is the one demotion point.
    fn show_note_pane(&mut self) {
        if self.layout == LayoutMode::TerminalFull {
            self.layout = LayoutMode::Split;
        }
    }

    pub fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.select_period(PaneView::Day, day, cx);
    }

    /// The weekly note of the week containing `day`.
    pub fn select_week(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.select_period(PaneView::Week, day, cx);
    }

    /// The monthly note of the month containing `day`.
    pub fn select_month(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.select_period(PaneView::Month, day, cx);
    }

    fn select_period(&mut self, view: PaneView, day: NaiveDate, cx: &mut Context<Self>) {
        self.flush_note_editor(cx);
        // The mini calendar reads cal_offset in the shown view's unit
        // (months over days and weeks, years over months), so a stale
        // offset from another view kind would land somewhere surprising.
        if self.view != view {
            self.cal_offset = 0;
        }
        self.selected_day = day;
        self.view = view;
        self.show_note_pane();
        self.reload_notes(cx);
        cx.notify();
    }

    pub fn open_note(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let was_open = self.view == PaneView::Note(path.clone());
        self.flush_note_editor(cx);
        // The flush may have just renamed this very note to its typed title
        // (a click on its not-yet-refreshed tree row): follow the rename
        // rather than reopening the stale path as a fresh empty note.
        let path = match (&self.view, was_open) {
            (PaneView::Note(renamed), true) => renamed.clone(),
            _ => path,
        };
        self.view = PaneView::Note(path);
        self.show_note_pane();
        self.reload_notes(cx);
        cx.notify();
    }

    pub fn open_task_view(&mut self, query: TaskQuery, cx: &mut Context<Self>) {
        self.flush_note_editor(cx);
        self.view = PaneView::Tasks(query);
        self.show_note_pane();
        self.reload_notes(cx);
        cx.notify();
    }

    /// The flush point (navigation, save shortcut, quit, window close):
    /// write the single-buffer editor's pending edits now, then let a
    /// freshly created untitled note's filename catch up with its typed
    /// title. A no-op when the editor is clean or absent.
    pub(crate) fn flush_note_editor(&mut self, cx: &mut Context<Self>) {
        self.save_note_editor(cx);
        self.save_library_text(cx);
        if let Some(path) = self.note_editor.as_ref().map(|e| e.read(cx).path.clone()) {
            self.rename_note_to_title(path, cx);
        }
    }

    /// The save half of a flush, without the title rename: for callers about
    /// to trash or rename the file at a path they are holding on to.
    pub(crate) fn save_note_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.note_editor {
            editor.update(cx, |ed, cx| ed.save_now(cx));
        }
    }

    /// Keep the single-buffer editor entity in step with the pane's document
    /// after a reload: same file merges the fresh disk state into any
    /// in-flight edits; a different file (or view) swaps the editor out.
    /// `disk_text` is what the file actually holds (the editor's merge
    /// baseline); `seed` is the daily template rendered over a blank day,
    /// which must never masquerade as disk state.
    fn sync_note_editor(
        &mut self,
        disk_text: Option<&str>,
        seed: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        use crate::note_editor::{NoteEditor, NoteEditorEvent};

        if self.doc_error.is_some() || self.root_missing {
            self.note_editor = None;
            self._note_editor_sub = None;
            return;
        }
        // The editor needs a save target even before the file exists: a day
        // with no note yet saves to its daily path on first edit.
        let path = self.doc_path.clone().or_else(|| match &self.view {
            PaneView::Day => Some(notes::daily_path(&self.notes_root, self.selected_day)),
            PaneView::Week => Some(notes::weekly_path(&self.notes_root, self.selected_day)),
            PaneView::Month => Some(notes::monthly_path(&self.notes_root, self.selected_day)),
            _ => None,
        });
        let Some(path) = path else {
            self.note_editor = None;
            self._note_editor_sub = None;
            return;
        };
        let text = disk_text.unwrap_or_default();
        if let Some(editor) = &self.note_editor
            && editor.read(cx).path == path
        {
            editor.update(cx, |ed, cx| ed.reconcile_from_disk(text, cx));
            return;
        }
        let editor = cx.new(|cx| NoteEditor::new(path, text, seed, cx));
        self._note_editor_sub = Some(cx.subscribe(
            &editor,
            |this, _editor, event: &NoteEditorEvent, cx| match event {
                NoteEditorEvent::Saved(path) => {
                    this.note_self_write(path);
                    this.reload_notes(cx);
                    cx.notify();
                }
                NoteEditorEvent::Conflicts(path, conflicts) => {
                    this.orphaned = Some((path.clone(), conflicts.join("\n")));
                    cx.notify();
                }
                NoteEditorEvent::OpenWikiLink(title) => {
                    let title = title.clone();
                    this.open_wiki_link_quiet(&title, cx);
                }
                NoteEditorEvent::OpenDate(date) => this.select_day(*date, cx),
                NoteEditorEvent::OpenUrl(url) => cx.open_url(url),
                NoteEditorEvent::BlockDropped { ranges, position } => {
                    this.on_block_dropped(ranges.clone(), *position, cx);
                }
                NoteEditorEvent::DragMoved { position } => {
                    this.on_drag_moved(*position, cx);
                }
            },
        ));
        self.note_editor = Some(editor);
    }

    /// The drop half of drag-to-a-day: a line drag was released outside the
    /// editor. An open hold menu's rows are checked first (a release on a
    /// heading drops at that section's end; on the menu's body, the day's
    /// top); otherwise a day drop target under the pointer takes the block
    /// at its top, and anywhere else the drag just ends.
    pub(crate) fn on_block_dropped(
        &mut self,
        ranges: Vec<std::ops::Range<usize>>,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::{HoldState, TOP_OF_NOTE};

        let menu_choice = if let HoldState::Open(menu) = &self.hold {
            let item = menu
                .item_bounds
                .borrow()
                .iter()
                .find(|(_, bounds)| bounds.contains(&position))
                .map(|(idx, _)| *idx);
            match item {
                Some(TOP_OF_NOTE) => Some((menu.day, None)),
                Some(idx) => Some((
                    menu.day,
                    menu.items
                        .iter()
                        .find(|it| it.line_idx == idx)
                        .map(|it| (it.line_idx, it.raw.clone())),
                )),
                None if menu
                    .menu_bounds
                    .borrow()
                    .is_some_and(|b| b.contains(&position)) =>
                {
                    Some((menu.day, None))
                }
                None => None,
            }
        } else {
            None
        };
        self.hold = HoldState::Idle;

        if let Some((day, heading)) = menu_choice {
            self.move_blocks_to_day(&ranges, day, heading, cx);
        } else if let Some(day) = self.resolve_day_drop(position) {
            self.move_blocks_to_day(&ranges, day, None, cx);
        }
        cx.notify();
    }

    /// Tick the hold-for-heading state machine on every drag move: dwell on
    /// a day arms a timer, drifting re-arms it, leaving folds the menu.
    pub(crate) fn on_drag_moved(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::HoldState;

        let over = self.resolve_day_drop(position);
        match &self.hold {
            HoldState::Idle => {
                if let Some(day) = over {
                    self.arm_hold(day, position, cx);
                }
            }
            HoldState::Arming { day, anchor, .. } => {
                let drifted = (position.x - anchor.x).abs() > gpui::px(4.)
                    || (position.y - anchor.y).abs() > gpui::px(4.);
                if over != Some(*day) {
                    self.hold = HoldState::Idle;
                    if let Some(day) = over {
                        self.arm_hold(day, position, cx);
                    }
                } else if drifted {
                    // Still on the day but moving: the dwell restarts from
                    // here (dropping the old timer cancels it).
                    let day = *day;
                    self.arm_hold(day, position, cx);
                }
            }
            HoldState::Open(menu) => {
                let near_menu = menu.menu_bounds.borrow().is_some_and(|b| {
                    gpui::Bounds::new(
                        b.origin - gpui::point(gpui::px(8.), gpui::px(8.)),
                        b.size + gpui::size(gpui::px(16.), gpui::px(16.)),
                    )
                    .contains(&position)
                });
                if !near_menu && over != Some(menu.day) {
                    self.hold = HoldState::Idle;
                    if let Some(day) = over {
                        self.arm_hold(day, position, cx);
                    }
                    cx.notify();
                }
            }
        }
    }

    fn arm_hold(
        &mut self,
        day: NaiveDate,
        anchor: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::HoldState;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1000))
                .await;
            let _ = this.update(cx, |ws, cx| ws.open_hold_menu(cx));
        });
        self.hold = HoldState::Arming { day, anchor, _timer: timer };
    }

    /// The dwell timer fired: list the target day's headings and open the
    /// menu at the pointer. No file or no headings means no menu.
    fn open_hold_menu(&mut self, cx: &mut Context<Self>) {
        use crate::workspace::{HoldItem, HoldMenu, HoldState};

        let HoldState::Arming { day, .. } = self.hold else { return };
        let Some(editor) = &self.note_editor else {
            self.hold = HoldState::Idle;
            return;
        };
        let Some((_, _, position)) = editor.read(cx).line_drag() else {
            self.hold = HoldState::Idle;
            return;
        };
        // Headings come from the live buffer when the target day is the
        // open note (disk may lag the autosave), else from the file.
        let target = notes::daily_file(&self.notes_root, day)
            .unwrap_or_else(|| notes::daily_path(&self.notes_root, day));
        let text = if editor.read(cx).path == target {
            Some(editor.read(cx).doc().to_string())
        } else {
            std::fs::read_to_string(&target).ok()
        };
        let Some(text) = text else {
            self.hold = HoldState::Idle;
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        let items: Vec<HoldItem> = notes::note_headings(&text)
            .into_iter()
            .map(|h| HoldItem {
                line_idx: h.line_idx,
                raw: lines.get(h.line_idx).copied().unwrap_or_default().to_string(),
                display: h.text,
                level: h.level,
            })
            .collect();
        if items.is_empty() {
            self.hold = HoldState::Idle;
            return;
        }
        self.hold = HoldState::Open(HoldMenu {
            day,
            items,
            origin: position + gpui::point(gpui::px(12.), gpui::px(-6.)),
            item_bounds: Default::default(),
            menu_bounds: Default::default(),
        });
        cx.notify();
    }

    /// The day whose drop target contains `position`, if any: the week strip
    /// first, then the sidebar surfaces (mini calendar, Daily rows), whose
    /// hits only count inside the sidebar's own bounds — cells scrolled out
    /// of its clip still capture bounds but must never catch a drop.
    pub(crate) fn resolve_day_drop(
        &self,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<NaiveDate> {
        let hit = |store: &DayBounds| {
            store
                .borrow()
                .iter()
                .find(|(_, bounds)| bounds.contains(&position))
                .map(|(day, _)| *day)
        };
        if let Some(day) = hit(&self.week_strip_bounds) {
            return Some(day);
        }
        let inside_sidebar =
            self.sidebar_bounds.borrow().is_some_and(|b| b.contains(&position));
        if !inside_sidebar {
            return None;
        }
        hit(&self.calendar_drop_bounds).or_else(|| hit(&self.daily_drop_bounds))
    }

    // --- Sidebar day timeline ---

    /// One timeline hour in pixels, scaled with the UI font size the way
    /// `KairnTheme::ui_px` scales chrome.
    pub(crate) fn timeline_hour_px(&self) -> f32 {
        const HOUR: f32 = 52.;
        let base = crate::theme::UI_BASE_SIZE;
        HOUR * self.settings.ui_font_size.unwrap_or(base) / base
    }

    /// Open or close the sidebar timeline (the period strip's clock tab).
    pub(crate) fn toggle_timeline(&mut self, cx: &mut Context<Self>) {
        if self.timeline_open {
            self.close_timeline(cx);
            return;
        }
        self.timeline_open = true;
        // The timeline reads the selected day's daily note, so it forces the
        // day view; the reload fills `day_timeline` now that the gate is on.
        if !matches!(self.view, PaneView::Day) {
            self.select_period(PaneView::Day, self.selected_day, cx);
        } else {
            self.reload_notes(cx);
        }
        // Land the useful part on screen: an hour before now on today, an
        // hour before the first block otherwise, else the working morning.
        let start_min = if self.selected_day == Local::now().date_naive() {
            (Local::now().time().hour() as i32 * 60 - 60).max(0)
        } else if let Some(first) = self.day_timeline.first() {
            (minutes_of(first.start) - 60).max(0)
        } else {
            7 * 60
        };
        let y = -(start_min as f32 / 60. * self.timeline_hour_px());
        self.sidebar_scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(y)));
        cx.notify();
    }

    pub(crate) fn close_timeline(&mut self, cx: &mut Context<Self>) {
        if !self.timeline_open {
            return;
        }
        self.timeline_open = false;
        self.timeline_drag = None;
        self.sidebar_scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        cx.notify();
    }

    /// Pointer height as minutes from midnight on the timeline's 24-hour
    /// canvas; `None` before the first paint.
    fn timeline_pointer_minutes(&self, y: gpui::Pixels) -> Option<i32> {
        let bounds = (*self.timeline_bounds.borrow())?;
        Some((f32::from(y - bounds.top()) / self.timeline_hour_px() * 60.) as i32)
    }

    /// Start dragging a timeline block: its body to move it, its bottom
    /// edge (`resize`) to change how long it runs.
    pub(crate) fn timeline_grab(
        &mut self,
        block_ix: usize,
        resize: bool,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(block) = self.day_timeline.get(block_ix) else { return };
        let Some(pointer_min) = self.timeline_pointer_minutes(position.y) else { return };
        self.timeline_drag = Some(crate::workspace::TimelineDrag {
            line_idx: block.line_idx,
            expected: block.line.clone(),
            start: block.start,
            end: block.end,
            resize,
            grab_offset_min: pointer_min - minutes_of(block.start),
            origin: position,
            position,
            moved: false,
        });
        cx.notify();
    }

    /// The drag's provisional times at its current pointer position,
    /// snapped to 5 minutes. A move keeps the block's length (and an
    /// endless block stays endless); a resize keeps the start and drags
    /// the end, never shorter than 15 minutes.
    pub(crate) fn timeline_drag_times(
        &self,
        drag: &crate::workspace::TimelineDrag,
    ) -> (chrono::NaiveTime, Option<chrono::NaiveTime>) {
        const LAST: i32 = 23 * 60 + 55;
        let Some(pointer_min) = self.timeline_pointer_minutes(drag.position.y) else {
            return (drag.start, drag.end);
        };
        let start_min = minutes_of(drag.start);
        if drag.resize {
            let end = snap5(pointer_min).clamp(start_min + 15, LAST);
            (drag.start, Some(time_of(end)))
        } else {
            let dur = drag.end.map(|e| (minutes_of(e) - start_min).max(5));
            let start = snap5(pointer_min - drag.grab_offset_min)
                .clamp(0, LAST - dur.unwrap_or(0));
            (time_of(start), dur.map(|d| time_of(start + d)))
        }
    }

    pub(crate) fn on_timeline_drag_move(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.timeline_drag else { return };
        drag.position = position;
        if !drag.moved {
            let delta = position - drag.origin;
            if f32::from(delta.x).abs() > 4. || f32::from(delta.y).abs() > 4. {
                drag.moved = true;
            }
        }
        if drag.moved {
            cx.notify();
        }
    }

    pub(crate) fn on_timeline_drag_release(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.timeline_drag.take() else { return };
        cx.notify();
        if !drag.moved {
            return;
        }
        drag.position = position;
        // Carried onto a calendar day: the whole block moves to that day's
        // note, same as dragging a line out of the editor.
        if !drag.resize
            && let Some(day) = self.resolve_day_drop(position)
            && day != self.selected_day
        {
            let Some(text) = self.doc_text.clone() else { return };
            let matches_grab =
                text.lines().nth(drag.line_idx).is_some_and(|l| l == drag.expected);
            if matches_grab {
                let offset = line_start_offset(&text, drag.line_idx);
                let range = notes::block_range(&text, offset);
                self.move_blocks_to_day(&[range], day, None, cx);
            }
            return;
        }
        let (start, end) = self.timeline_drag_times(&drag);
        if (start, end) == (drag.start, drag.end) {
            return;
        }
        let Some(new_line) = notes::retime_line(&drag.expected, start, end) else { return };
        let Some(path) = self.doc_path.clone() else { return };
        // Pending editor keystrokes reach the file first so the rewrite
        // lands on what's actually on screen.
        self.flush_note_editor(cx);
        match notes::replace_line_on_disk(&path, drag.line_idx, &drag.expected, &new_line) {
            Ok(Some(idx)) => {
                self.note_self_write(&path);
                self.vault_history.push(crate::history::VaultOp::Retime {
                    ms: crate::note_editor::now_ms(),
                    path,
                    line_idx: idx,
                    before: drag.expected.clone(),
                    after: new_line,
                });
            }
            Ok(None) => {}
            Err(e) => eprintln!("kairn: could not update {}: {e}", path.display()),
        }
        self.reload_notes(cx);
    }

    /// Move a block out of the open note into `day`'s note: to the top, or
    /// to the end of the section named by `heading` (a hold-menu choice,
    /// `(line_idx, raw line)`), falling back to the top when the heading has
    /// vanished. The target's own day is an in-buffer move (one undo step);
    /// anything else inserts on disk first and removes from the buffer
    /// second, so a crash duplicates rather than loses.
    fn move_blocks_to_day(
        &mut self,
        ranges: &[std::ops::Range<usize>],
        day: NaiveDate,
        heading: Option<(usize, String)>,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.note_editor.clone() else { return };
        let target = notes::daily_file(&self.notes_root, day)
            .unwrap_or_else(|| notes::daily_path(&self.notes_root, day));

        if editor.read(cx).path == target {
            let offset = heading
                .and_then(|(idx, raw)| {
                    let text = editor.read(cx).doc();
                    let idx = relocate_line(text, idx, &raw)?;
                    let line = notes::section_insert_line(text, idx)?;
                    Some(line_start_offset(text, line))
                })
                .unwrap_or(0);
            editor.update(cx, |ed, cx| ed.move_blocks_to(ranges, offset, cx));
            return;
        }

        let block = editor.read(cx).blocks_text(ranges);
        if block.trim().is_empty() {
            return;
        }
        let from_line_idx = editor.read(cx).first_block_line_idx(ranges);
        let source = editor.read(cx).path.clone();
        let mut to_line_idx = 0usize;
        let written = match heading {
            Some((idx, raw)) => {
                match notes::insert_block_under_heading(&target, idx, &raw, &block) {
                    Ok(Some(landed)) => {
                        to_line_idx = landed;
                        Ok(Some(target.clone()))
                    }
                    // The heading (or the whole file) vanished or went
                    // ambiguous underneath the menu: land at the top rather
                    // than guessing.
                    Ok(None) => Ok(None),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e),
                }
            }
            None => Ok(None),
        };
        let written = match written {
            Ok(Some(path)) => Ok(path),
            Ok(None) => notes::insert_block_at_top(
                &self.notes_root,
                day,
                &block,
                &self.settings.daily_template_rule,
            ),
            Err(e) => Err(e),
        };
        match written {
            Ok(path) => {
                self.note_self_write(&path);
                editor.update(cx, |ed, cx| ed.remove_blocks_unrecorded(ranges, cx));
                // Flush the removal now so the move history's disk-level
                // inverses always see the source as it really is.
                self.save_note_editor(cx);
                self.vault_history.push(crate::history::VaultOp::Transfer {
                    ms: crate::note_editor::now_ms(),
                    from: source,
                    to: path,
                    block,
                    from_line_idx,
                    to_line_idx,
                });
                self.reload_notes(cx);
            }
            Err(e) => eprintln!("kairn: could not move block to {day}: {e}"),
        }
    }

    /// Take back the newest cross-note move or retime: both halves are disk
    /// edits verified by content, so a vault that changed underneath (sync,
    /// an agent, the other machine) makes the undo refuse quietly rather
    /// than guess.
    pub(crate) fn vault_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::history::VaultOp;
        let Some(op) = self.vault_history.pop_undo() else { return };
        self.flush_note_editor(cx);
        match op {
            VaultOp::Transfer { ms, from, to, block, from_line_idx, to_line_idx } => {
                match notes::remove_block_lines(&to, &block) {
                    Ok(Some(_)) => {
                        self.note_self_write(&to);
                        match notes::insert_block_at_line(&from, from_line_idx, &block) {
                            Ok(idx) => {
                                self.note_self_write(&from);
                                self.vault_history.push_undone(VaultOp::Transfer {
                                    ms,
                                    from,
                                    to,
                                    block,
                                    from_line_idx: idx,
                                    to_line_idx,
                                });
                            }
                            Err(e) => {
                                // Out of the target but not back in the
                                // source: return it to the target rather
                                // than losing text.
                                eprintln!(
                                    "kairn: undo could not restore into {}: {e}",
                                    from.display()
                                );
                                let _ = notes::insert_block_at_line(&to, to_line_idx, &block);
                            }
                        }
                    }
                    Ok(None) => {
                        eprintln!("kairn: undo skipped, the moved block changed on disk")
                    }
                    Err(e) => eprintln!("kairn: undo failed reading {}: {e}", to.display()),
                }
            }
            VaultOp::Retime { ms, path, line_idx, before, after } => {
                match notes::replace_line_on_disk(&path, line_idx, &after, &before) {
                    Ok(Some(idx)) => {
                        self.note_self_write(&path);
                        self.vault_history.push_undone(VaultOp::Retime {
                            ms,
                            path,
                            line_idx: idx,
                            before,
                            after,
                        });
                    }
                    Ok(None) => {
                        eprintln!("kairn: undo skipped, the line changed on disk")
                    }
                    Err(e) => eprintln!("kairn: undo failed on {}: {e}", path.display()),
                }
            }
            VaultOp::PathMove { ms, src, dest, library } => {
                // Return the item from where it landed to where it started;
                // both ends verified so an undo after a further move refuses
                // rather than clobbering. Tree moves finalize their own
                // tree/view fix-up, not the note-oriented finish below.
                if dest.exists() && !src.exists() {
                    match std::fs::rename(&dest, &src) {
                        Ok(()) => {
                            self.vault_history.push_undone(VaultOp::PathMove {
                                ms,
                                src: src.clone(),
                                dest: dest.clone(),
                                library,
                            });
                            if library {
                                self.relocate_library_path(&dest, &src, window, cx);
                            } else {
                                self.relocate_note_path(&dest, &src, cx);
                            }
                        }
                        Err(e) => {
                            eprintln!("kairn: undo failed moving {}: {e}", dest.display())
                        }
                    }
                } else {
                    eprintln!("kairn: undo skipped, the moved item changed on disk");
                }
                return;
            }
        }
        self.finish_vault_op(cx);
    }

    /// Re-apply the most recently undone vault op, with the same
    /// verification contract as [`Self::vault_undo`].
    pub(crate) fn vault_redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::history::VaultOp;
        let Some(op) = self.vault_history.pop_redo() else { return };
        self.flush_note_editor(cx);
        match op {
            VaultOp::Transfer { ms, from, to, block, from_line_idx, to_line_idx } => {
                match notes::remove_block_lines(&from, &block) {
                    Ok(Some(idx)) => {
                        self.note_self_write(&from);
                        match notes::insert_block_at_line(&to, to_line_idx, &block) {
                            Ok(landed) => {
                                self.note_self_write(&to);
                                self.vault_history.push_redone(VaultOp::Transfer {
                                    ms,
                                    from,
                                    to,
                                    block,
                                    from_line_idx: idx,
                                    to_line_idx: landed,
                                });
                            }
                            Err(e) => {
                                eprintln!(
                                    "kairn: redo could not insert into {}: {e}",
                                    to.display()
                                );
                                let _ =
                                    notes::insert_block_at_line(&from, from_line_idx, &block);
                            }
                        }
                    }
                    Ok(None) => {
                        eprintln!("kairn: redo skipped, the moved block changed on disk")
                    }
                    Err(e) => eprintln!("kairn: redo failed reading {}: {e}", from.display()),
                }
            }
            VaultOp::Retime { ms, path, line_idx, before, after } => {
                match notes::replace_line_on_disk(&path, line_idx, &before, &after) {
                    Ok(Some(idx)) => {
                        self.note_self_write(&path);
                        self.vault_history.push_redone(VaultOp::Retime {
                            ms,
                            path,
                            line_idx: idx,
                            before,
                            after,
                        });
                    }
                    Ok(None) => {
                        eprintln!("kairn: redo skipped, the line changed on disk")
                    }
                    Err(e) => eprintln!("kairn: redo failed on {}: {e}", path.display()),
                }
            }
            VaultOp::PathMove { ms, src, dest, library } => {
                // Re-apply the move src -> dest, same verification contract as
                // the undo above; finalizes its own tree/view fix-up.
                if src.exists() && !dest.exists() {
                    match std::fs::rename(&src, &dest) {
                        Ok(()) => {
                            self.vault_history.push_redone(VaultOp::PathMove {
                                ms,
                                src: src.clone(),
                                dest: dest.clone(),
                                library,
                            });
                            if library {
                                self.relocate_library_path(&src, &dest, window, cx);
                            } else {
                                self.relocate_note_path(&src, &dest, cx);
                            }
                        }
                        Err(e) => {
                            eprintln!("kairn: redo failed moving {}: {e}", src.display())
                        }
                    }
                } else {
                    eprintln!("kairn: redo skipped, the moved item changed on disk");
                }
                return;
            }
        }
        self.finish_vault_op(cx);
    }

    /// Shared tail of a vault-level undo/redo: fold the disk changes back
    /// into the open editor, then drop the merge-absorption record the
    /// reconcile just pushed; the vault history owns that change, and a
    /// buffer undo re-reverting the merge would strand the moved text.
    fn finish_vault_op(&mut self, cx: &mut Context<Self>) {
        self.reload_notes(cx);
        if let Some(editor) = &self.note_editor {
            editor.update(cx, |ed, _| ed.drop_merge_undo());
        }
        cx.notify();
    }

    pub fn notes_expanded_contains(&self, path: &std::path::Path) -> bool {
        self.notes_expanded.contains(path)
    }

    pub fn toggle_notes_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.notes_expanded.remove(&path) {
            self.notes_expanded.insert(path);
        }
        self.notes_tree = notes::notes_tree(&self.notes_root, &self.notes_expanded);
        cx.notify();
    }

    // ----- library folders -----

    /// Open a library file in the pane. Every kind funnels through the same
    /// view; the pane renders it by kind (markdown editor, plain-text
    /// editor, inline image, or a metadata card with open/reveal actions).
    pub fn open_library_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_note_editor(cx);
        self.view = PaneView::Library(path.clone());
        self.show_note_pane();
        // Text files get their editor here, where a Window exists to build
        // the input; the reload below sees the matching path and keeps it.
        if notes::file_kind(&path) == notes::FileKind::Text {
            self.open_library_text_editor(path, window, cx);
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Build (or keep) the plain-text editor for a library text file. An
    /// unreadable file (permissions, not UTF-8 despite its extension) gets
    /// no editor and falls back to the metadata card.
    fn open_library_text_editor(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::input::{InputEvent, InputState};

        if self.library_text.as_ref().is_some_and(|ed| ed.path == path) {
            return;
        }
        // Whatever the previous text file still holds gets written first.
        self.save_library_text(cx);
        let Ok(disk) = std::fs::read_to_string(&path) else {
            self.library_text = None;
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(10, 100_000)
                .default_value(disk.clone())
        });
        let sub = cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change)
                && let Some(ed) = &mut this.library_text
            {
                // Debounced autosave, one pending write at a time.
                ed._save = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(600))
                        .await;
                    let _ = this.update(cx, |ws, cx| ws.save_library_text(cx));
                }));
            }
        });
        self.library_text = Some(LibraryTextEditor {
            path,
            input,
            baseline: disk,
            _sub: sub,
            _save: None,
        });
    }

    /// Write the text editor's buffer if it differs from its baseline.
    /// Atomic like every other write; the watcher skips it as a self-write.
    pub(crate) fn save_library_text(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = &self.library_text else { return };
        let value = ed.input.read(cx).value().to_string();
        if value == ed.baseline {
            return;
        }
        let path = ed.path.clone();
        match notes::write_note(&path, &value) {
            Ok(()) => {
                self.note_self_write(&path);
                if let Some(ed) = &mut self.library_text {
                    ed.baseline = value;
                }
            }
            Err(e) => eprintln!("kairn: could not save {}: {e}", path.display()),
        }
    }

    /// Keep the text editor in step after a reload: drop it (saving first)
    /// when the view moved elsewhere, and while it is clean, fold in
    /// external edits so agent writes appear live. A dirty buffer keeps the
    /// user's text; the next save wins.
    fn sync_library_text(&mut self, cx: &mut Context<Self>) {
        let keep = matches!(
            &self.view,
            PaneView::Library(p)
                if self.library_text.as_ref().is_some_and(|ed| &ed.path == p)
        );
        if !keep {
            self.save_library_text(cx);
            self.library_text = None;
            return;
        }
        let Some(ed) = &self.library_text else { return };
        let Ok(disk) = std::fs::read_to_string(&ed.path) else { return };
        let current = ed.input.read(cx).value().to_string();
        if current != ed.baseline || disk == ed.baseline {
            return;
        }
        // set_value needs a Window; reloads run without one, so the write
        // into the input defers onto the active window.
        let input = ed.input.clone();
        let Some(win) = cx.active_window() else { return };
        if let Some(ed) = &mut self.library_text {
            ed.baseline = disk.clone();
        }
        cx.defer(move |cx| {
            let _ = win.update(cx, |_, window, cx| {
                input.update(cx, |state, cx| state.set_value(disk, window, cx));
            });
        });
    }

    /// Expand or collapse a library root or one of its folders.
    pub fn toggle_library_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.library_expanded.remove(&path) {
            self.library_expanded.insert(path);
        }
        self.reload_library_trees();
        cx.notify();
    }

    /// Rebuild every library root's visible rows. A collapsed root costs
    /// nothing; expanded ones read only their expanded directories.
    pub(crate) fn reload_library_trees(&mut self) {
        let sort = if self.settings.library_sort == "name" {
            notes::LibrarySort::Name
        } else {
            notes::LibrarySort::Modified
        };
        self.library_trees = self
            .settings
            .library_roots()
            .into_iter()
            .map(|root| {
                let rows = if self.library_expanded.contains(&root) {
                    notes::library_tree(&root, &self.library_expanded, sort)
                } else {
                    Vec::new()
                };
                (root, rows)
            })
            .collect();
    }

    /// Refresh only what a library event can touch: the sidebar trees, the
    /// open library document, and an image view's sibling strip. Vault
    /// scans stay out — an agent writing busily under a library root must
    /// not cost a full notes re-parse per burst.
    pub(crate) fn reload_library(&mut self, cx: &mut Context<Self>) {
        self.reload_library_trees();
        let PaneView::Library(path) = &self.view else { return };
        let path = path.clone();
        match notes::file_kind(&path) {
            notes::FileKind::Markdown => {
                let (disk_text, doc_error) = match std::fs::read_to_string(&path) {
                    Ok(t) => (Some(t), None),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
                    Err(e) => (None, Some(e.to_string())),
                };
                self.doc_error = doc_error;
                self.doc_text = disk_text.clone();
                self.sync_note_editor(disk_text.as_deref(), None, cx);
            }
            notes::FileKind::Image => {
                self.library_siblings =
                    path.parent().map(notes::library_images).unwrap_or_default();
            }
            notes::FileKind::Text | notes::FileKind::Other => {}
        }
        self.sync_library_text(cx);
    }

    /// The sidebar +: pick a directory with the native folder dialog and add
    /// it as a library root.
    pub fn pick_library_root(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add to Library".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                let _ = this.update(cx, |ws, cx| ws.add_library_root(path, cx));
            }
        })
        .detach();
    }

    /// Add a directory as a library root (sidebar +, via the folder picker),
    /// persisted to this machine's settings. Already-listed roots are not
    /// duplicated; the new root opens expanded so the add visibly landed.
    pub fn add_library_root(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let raw = crate::settings_dialog::home_relative(&path);
        if self.settings.library_roots.iter().any(|r| *r == raw) {
            return;
        }
        self.settings.library_roots.push(raw);
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        self.library_expanded.insert(path);
        self.rearm_library_watchers(cx);
        self.reload_notes(cx);
        cx.notify();
    }

    /// Remove a library root from the sidebar. Only the listing goes; the
    /// files on disk are untouched. A library file on screen drops back to
    /// the day view rather than showing a path the sidebar no longer owns.
    pub fn remove_library_root(&mut self, root: &Path, cx: &mut Context<Self>) {
        let resolved = self.settings.library_roots();
        let Some(idx) = resolved.iter().position(|r| r == root) else { return };
        self.settings.library_roots.remove(idx);
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        self.library_expanded.retain(|p| !p.starts_with(root));
        if matches!(&self.view, PaneView::Library(p) if p.starts_with(root)) {
            self.flush_note_editor(cx);
            self.view = PaneView::Day;
        }
        self.rearm_library_watchers(cx);
        self.reload_notes(cx);
        cx.notify();
    }

    /// New-file prompt for a library root or folder: an empty file
    /// (markdown when the name carries no extension), opened once created
    /// so typing can start immediately.
    pub fn prompt_new_library_file(
        &mut self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::name_dialog::open(
            "New file",
            "Create",
            None,
            window,
            cx,
            move |ws, name, window, cx| match notes::create_library_file(&dir, name) {
                Ok(path) => {
                    ws.note_self_write(&path);
                    ws.library_expanded.insert(dir.clone());
                    ws.open_library_file(path, window, cx);
                }
                Err(e) => {
                    window.push_notification(format!("Could not create file: {e}"), cx)
                }
            },
        );
    }

    /// New-folder prompt for a library root or folder; the parent and the
    /// new folder open expanded so the create visibly landed.
    pub fn prompt_new_library_folder(
        &mut self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::name_dialog::open(
            "New folder",
            "Create",
            None,
            window,
            cx,
            move |ws, name, window, cx| match notes::create_folder_in(&dir, name) {
                Ok(path) => {
                    ws.library_expanded.insert(dir.clone());
                    ws.library_expanded.insert(path);
                    ws.reload_library_trees();
                    cx.notify();
                }
                Err(e) => {
                    window.push_notification(format!("Could not create folder: {e}"), cx)
                }
            },
        );
    }

    /// Rename prompt for a library file or folder: a one-field dialog
    /// prefilled with the current name, extension included — library names
    /// keep it (a bare new name keeps the old extension).
    pub fn prompt_rename_library_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        crate::name_dialog::open(
            "Rename",
            "Rename",
            Some(initial),
            window,
            cx,
            move |ws, name, window, cx| ws.rename_library_path(&path, name, window, cx),
        );
    }

    /// Rename a library file or folder on disk. Pending edits are flushed
    /// first so they travel with the file; the open document and any
    /// expanded folders under the old path follow the rename.
    pub fn rename_library_path(
        &mut self,
        path: &Path,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_note_editor(cx);
        self.save_library_text(cx);
        match notes::rename_library_path(path, name) {
            Ok(new_path) => self.relocate_library_path(path, &new_path, window, cx),
            Err(e) => {
                window.push_notification(format!("Could not rename: {e}"), cx);
                self.reload_notes(cx);
                cx.notify();
            }
        }
    }

    /// Whether a tree drag (Notes or Library) may land in `dest_dir`: the
    /// source must exist, the target must be a real folder, a folder can't
    /// drop into itself or a descendant, a move into the item's own folder is
    /// a no-op, and a name already taken at the target is refused. Drives both
    /// the drop-target highlight (only valid folders light up) and the drop.
    pub(crate) fn can_move_into(src: &Path, dest_dir: &Path) -> bool {
        if !src.exists() || !dest_dir.is_dir() {
            return false;
        }
        let Some(name) = src.file_name() else { return false };
        if src.parent() == Some(dest_dir) {
            return false;
        }
        if src.is_dir() && dest_dir.starts_with(src) {
            return false;
        }
        !dest_dir.join(name).exists()
    }

    /// Move a library file or folder into `dest_dir` (the sidebar's
    /// drag-to-a-folder), recording it on the vault history so Cmd/Super+Z
    /// takes it back. Pending edits flush first so they travel with the file;
    /// the open document and any expanded folders follow the move.
    pub fn move_library_path(
        &mut self,
        src: PathBuf,
        dest_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::can_move_into(&src, &dest_dir) {
            return;
        }
        self.flush_note_editor(cx);
        self.save_library_text(cx);
        match notes::move_library_path(&src, &dest_dir) {
            Ok(new_path) => {
                self.vault_history.push(crate::history::VaultOp::PathMove {
                    ms: crate::note_editor::now_ms(),
                    src: src.clone(),
                    dest: new_path.clone(),
                    library: true,
                });
                self.relocate_library_path(&src, &new_path, window, cx);
            }
            Err(e) => {
                window.push_notification(format!("Could not move: {e}"), cx);
                self.reload_notes(cx);
                cx.notify();
            }
        }
    }

    /// Shared fix-up after a library file or folder changed path (rename,
    /// move, or an undo/redo of either): carry expanded folders and the open
    /// document over to the new path, reveal the destination folder, then
    /// reload the trees.
    fn relocate_library_path(
        &mut self,
        old: &Path,
        new_path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moved: Vec<PathBuf> = self
            .library_expanded
            .iter()
            .filter(|p| p.starts_with(old))
            .cloned()
            .collect();
        for prev in moved {
            self.library_expanded.remove(&prev);
            if let Ok(rel) = prev.strip_prefix(old) {
                self.library_expanded.insert(new_path.join(rel));
            }
        }
        // Reveal where it landed so the move is visibly confirmed.
        if let Some(parent) = new_path.parent() {
            self.library_expanded.insert(parent.to_path_buf());
        }
        // Dropped, not synced out: its edits are already saved and its old
        // path no longer exists; reopening rebuilds it.
        if self.library_text.as_ref().is_some_and(|ed| ed.path.starts_with(old)) {
            self.library_text = None;
        }
        let followed = match &self.view {
            PaneView::Library(p) => p.strip_prefix(old).ok().map(|rel| {
                if rel.as_os_str().is_empty() {
                    new_path.to_path_buf()
                } else {
                    new_path.join(rel)
                }
            }),
            _ => None,
        };
        if let Some(view_path) = followed {
            self.open_library_file(view_path, window, cx);
            return;
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Move a library file or folder to the OS trash — recoverable, never a
    /// hard delete (libraries live outside the vault, so `Notes/@Trash`
    /// doesn't apply). Pending edits are flushed first so they travel with
    /// the file; a document that lived under the deleted path drops back to
    /// the day view.
    pub fn trash_library_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_note_editor(cx);
        self.save_library_text(cx);
        match os_trash(path) {
            Ok(()) => {
                // Dropped, not synced out: a save after the delete would
                // resurrect the file.
                if self.library_text.as_ref().is_some_and(|ed| ed.path.starts_with(path)) {
                    self.library_text = None;
                }
                self.library_expanded.retain(|p| !p.starts_with(path));
                if matches!(&self.view, PaneView::Library(p) if p.starts_with(path)) {
                    self.view = PaneView::Day;
                }
            }
            Err(e) => window.push_notification(format!("Could not delete: {e}"), cx),
        }
        self.reload_notes(cx);
        cx.notify();
    }

    pub(crate) fn rearm_library_watchers(&mut self, cx: &mut Context<Self>) {
        let (watchers, task) = Self::watch_library(
            self.settings.library_roots(),
            self.self_writes.clone(),
            cx,
        );
        self._library_watchers = watchers;
        self._library_watch_task = task;
    }

    pub fn tasks_for(&self, query: TaskQuery) -> impl Iterator<Item = &notes::TaskRef> {
        let today = Local::now().date_naive();
        self.open_tasks.iter().filter(move |t| query.matches(t.due, today))
    }

    /// Open-task count for a view, computed once per reload rather than
    /// re-scanned every frame.
    pub fn task_count(&self, query: TaskQuery) -> usize {
        let idx = match query {
            TaskQuery::Today => 0,
            TaskQuery::Open => 1,
            TaskQuery::Overdue => 2,
        };
        self.task_counts[idx]
    }

    /// Re-read the pane's document and everything the sidebar derives from
    /// the notes: calendar indicators, open-task counts, the Notes tree.
    pub fn reload_notes(&mut self, cx: &mut Context<Self>) {
        self.root_missing = self.settings.notes_root.as_deref().is_some_and(|r| !r.is_empty())
            && !self.notes_root.exists();
        // One walk of Calendar/ and Notes/ and one read of each daily,
        // shared by the task scan and the mention scan below.
        let scan = notes::VaultScan::new(&self.notes_root);
        let dailies = scan.read_dailies_cached(&mut self.daily_cache);
        self.dailies_skipped = scan.days.len() - dailies.len();
        let note_texts = scan.read_notes_cached(&mut self.note_cache);
        let task_scan = notes::scan_tasks(&dailies, &note_texts);
        self.open_tasks = task_scan.open;
        self.day_stats = task_scan.day_stats;
        self.notes_tree = notes::notes_tree(&self.notes_root, &self.notes_expanded);
        self.reload_library_trees();
        self.agent_activity = notes::recent_activity(&self.notes_root, 6);
        let today = Local::now().date_naive();
        self.task_counts = [TaskQuery::Today, TaskQuery::Open, TaskQuery::Overdue].map(|q| {
            self.open_tasks.iter().filter(|t| q.matches(t.due, today)).count()
        });
        let monday = self.selected_day
            - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
        for (i, stats) in self.week_stats.iter_mut().enumerate() {
            let day = monday + Days::new(i as u64);
            *stats = self.day_stats.get(&day).copied().unwrap_or_default();
        }
        let path = match &self.view {
            PaneView::Day => scan.days.get(&self.selected_day).cloned(),
            PaneView::Week => {
                notes::period_file(&self.notes_root, &notes::weekly_stem(self.selected_day))
            }
            PaneView::Month => {
                notes::period_file(&self.notes_root, &notes::monthly_stem(self.selected_day))
            }
            PaneView::Note(p) => Some(p.clone()),
            // Only markdown library files are documents here; other kinds
            // (images, binaries) render from the view's path without a text
            // read, which would fail on them anyway.
            PaneView::Library(p) => {
                (notes::file_kind(p) == notes::FileKind::Markdown).then(|| p.clone())
            }
            PaneView::Tasks(_) => None,
        };
        let (disk_text, doc_error) = match path.as_deref() {
            None => (None, None),
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => (Some(t), None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
                // The file is there but unreadable: say so instead of
                // rendering a convincing "no note for this day yet".
                Err(e) => (None, Some(e.to_string())),
            },
        };
        self.doc_error = doc_error;
        // A day from today onward with no content yet renders seeded from the
        // daily template (Notes/@Templates/Daily.md). The seed is display
        // plus intent, never disk state: the editor keeps its baseline at
        // what the file actually holds — an empty file already on disk counts
        // (NotePlan pre-creates these, and a stray visit can too) — and
        // writes nothing until a real edit lands, so the first save merges
        // cleanly instead of reading the template as externally deleted.
        // Past days stay blank — a template there would dress up history that
        // never happened. The settings rule can narrow this to weekdays or
        // turn it off.
        let day_is_blank = disk_text.as_deref().is_none_or(|s| s.trim().is_empty());
        let seed = if day_is_blank
            && matches!(self.view, PaneView::Day)
            && self.doc_error.is_none()
            && !self.root_missing
            && self.selected_day >= today
            && notes::template_applies(&self.settings.daily_template_rule, self.selected_day)
        {
            // Drop any leading `# title` from the template: the day's masthead
            // already titles it, so a heading here would just duplicate it.
            notes::daily_template(&self.notes_root)
                .map(|body| notes::strip_daily_title(&body).to_string())
        } else {
            None
        };
        self.doc_text = match &seed {
            Some(s) => Some(s.clone()),
            None => disk_text.clone(),
        };
        self.day_timeline = match (&self.view, &self.doc_text) {
            (PaneView::Day, Some(text)) if self.timeline_open => notes::time_blocks(text),
            _ => Vec::new(),
        };
        // Linked mentions for the pane's document: a day is referenced by its
        // ISO date ([[2026-08-07]] and >2026-08-07 alike), a note by its stem.
        let title = match &self.view {
            PaneView::Day => Some(self.selected_day.format("%Y-%m-%d").to_string()),
            PaneView::Week => Some(notes::weekly_stem(self.selected_day)),
            PaneView::Month => Some(notes::monthly_stem(self.selected_day)),
            PaneView::Note(p) => p
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string),
            // Library files are documents, not notes: no linked mentions.
            PaneView::Library(_) | PaneView::Tasks(_) => None,
        };
        self.mentions = match title {
            Some(t) => notes::mentions_in(&scan, &dailies, &t, path.as_deref()),
            None => Vec::new(),
        };
        // For a day with no file yet, check where the file would be: the
        // conflicted copy of a note that vanished is exactly the case that
        // needs surfacing. Library files stay out: resolving a conflict
        // trashes into the vault, which must never touch external trees.
        let conflict_probe = match &self.view {
            PaneView::Library(_) => None,
            PaneView::Day => path
                .clone()
                .or_else(|| Some(notes::daily_path(&self.notes_root, self.selected_day))),
            PaneView::Week => path
                .clone()
                .or_else(|| Some(notes::weekly_path(&self.notes_root, self.selected_day))),
            PaneView::Month => path
                .clone()
                .or_else(|| Some(notes::monthly_path(&self.notes_root, self.selected_day))),
            _ => path.clone(),
        };
        self.conflicts = conflict_probe
            .as_deref()
            .map(notes::conflict_copies)
            .unwrap_or_default();
        self.vault_conflicts = notes::vault_conflicts(&self.notes_root);
        self.doc_path = path;
        self.note_days = scan.days;
        // The image view's sibling strip: the other images in its folder.
        self.library_siblings = match &self.view {
            PaneView::Library(p) if notes::file_kind(p) == notes::FileKind::Image => {
                p.parent().map(notes::library_images).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        self.sync_library_text(cx);
        self.sync_note_editor(disk_text.as_deref(), seed.as_deref(), cx);
    }

    /// Open whatever a wiki link points at: a day, an existing note, or a
    /// brand-new note created wiki-style on first click. Creation never
    /// overwrites: if the file exists by the time we write, it is opened
    /// as-is (links arrive in synced files and race external writers).
    /// Callers are entity event handlers without a window, so failures log
    /// to stderr instead of raising a notification.
    pub fn open_wiki_link_quiet(&mut self, title: &str, cx: &mut Context<Self>) {
        match notes::resolve_wiki_target(&self.notes_root, title) {
            notes::WikiTarget::Day(date) => self.select_day(date, cx),
            notes::WikiTarget::Note(path) => self.open_note(path, cx),
            notes::WikiTarget::Missing(path) => {
                match notes::create_note_if_absent(&path, &format!("# {title}\n")) {
                    Ok(_) => {
                        self.note_self_write(&path);
                        self.open_note(path, cx)
                    }
                    Err(e) => eprintln!("kairn: could not create {}: {e}", path.display()),
                }
            }
            notes::WikiTarget::Invalid => {
                eprintln!("kairn: link can't name a note inside the notes folder: {title}");
            }
        }
    }

    /// Jump to a mention's source note.
    pub fn open_mention(&mut self, mention: &notes::Mention, cx: &mut Context<Self>) {
        match mention.date {
            Some(date) => self.select_day(date, cx),
            None => self.open_note(mention.path.clone(), cx),
        }
    }

    /// Toggle a task from a task view, addressed at whichever daily note it
    /// was scanned from.
    pub fn toggle_task_ref(&mut self, task: &notes::TaskRef, cx: &mut Context<Self>) {
        match notes::toggle_task_on_disk(&task.path, task.line_idx, &task.line) {
            Ok(true) => self.note_self_write(&task.path),
            Ok(false) => {}
            Err(e) => eprintln!("kairn: could not update {}: {e}", task.path.display()),
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Move a note to `Notes/@Trash/` (NotePlan's soft delete; nothing is
    /// ever hard-deleted). Pending edits are flushed first so they travel
    /// with the file. If the trashed note is on screen, the pane drops back
    /// to the day view.
    pub fn trash_note_at(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        // Save without the title rename: `path` must still name the file.
        self.save_note_editor(cx);
        match notes::trash_note(&self.notes_root, path) {
            Ok(_) => {
                if self.view == PaneView::Note(path.to_path_buf()) {
                    self.view = PaneView::Day;
                }
            }
            Err(e) => window.push_notification(format!("Could not delete note: {e}"), cx),
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Rename a note in place (extension preserved, never overwrites). An
    /// open note stays open under its new name.
    pub fn rename_note_at(
        &mut self,
        path: &Path,
        new_stem: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Save without the title rename: `path` must still name the file.
        self.save_note_editor(cx);
        match notes::rename_note(path, new_stem) {
            Ok(new_path) => {
                if self.view == PaneView::Note(path.to_path_buf()) {
                    self.view = PaneView::Note(new_path);
                }
            }
            Err(e) => window.push_notification(format!("Could not rename note: {e}"), cx),
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Move a note or notes-folder into `dest_dir` (the Notes tree's
    /// drag-to-a-folder), recorded on the vault history for Cmd/Super+Z. A
    /// note keeps its filename, so its title and every wiki link to it stay
    /// valid; only its folder changes. The open note follows the move.
    pub fn move_note_to(
        &mut self,
        src: PathBuf,
        dest_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::can_move_into(&src, &dest_dir) {
            return;
        }
        // Save without the title rename: `src` must still name the file.
        self.save_note_editor(cx);
        match notes::move_note(&src, &dest_dir) {
            Ok(new_path) => {
                self.vault_history.push(crate::history::VaultOp::PathMove {
                    ms: crate::note_editor::now_ms(),
                    src: src.clone(),
                    dest: new_path.clone(),
                    library: false,
                });
                self.relocate_note_path(&src, &new_path, cx);
            }
            Err(e) => {
                window.push_notification(format!("Could not move note: {e}"), cx);
                self.reload_notes(cx);
                cx.notify();
            }
        }
    }

    /// Shared fix-up after a note or notes-folder changed path (a move, or an
    /// undo/redo of one): carry expanded folders and the open note over to
    /// the new path, reveal the destination folder, then reload the tree.
    fn relocate_note_path(&mut self, old: &Path, new_path: &Path, cx: &mut Context<Self>) {
        let moved: Vec<PathBuf> = self
            .notes_expanded
            .iter()
            .filter(|p| p.starts_with(old))
            .cloned()
            .collect();
        for prev in moved {
            self.notes_expanded.remove(&prev);
            if let Ok(rel) = prev.strip_prefix(old) {
                self.notes_expanded.insert(new_path.join(rel));
            }
        }
        // Reveal where it landed (the Notes root is never in the expanded set).
        if let Some(parent) = new_path.parent()
            && parent != self.notes_root.join("Notes")
        {
            self.notes_expanded.insert(parent.to_path_buf());
        }
        // Follow the open note (or a note under a moved folder) to its new path.
        if let PaneView::Note(p) = &self.view
            && let Ok(rel) = p.strip_prefix(old)
        {
            let np = if rel.as_os_str().is_empty() {
                new_path.to_path_buf()
            } else {
                new_path.join(rel)
            };
            self.view = PaneView::Note(np);
        }
        self.reload_notes(cx);
        cx.notify();
    }

    /// Create a fresh untitled note in a folder of the Notes tree and open it,
    /// the caret sitting after the seeded `# ` so the user just types the
    /// title — the file takes that name at the next flush (see
    /// [`Self::rename_note_to_title`]). No name prompt: NotePlan-style.
    pub fn create_new_note(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match notes::new_untitled_note_in(&dir) {
            Ok(path) => {
                self.note_self_write(&path);
                // Expand the folder so the new note is visible in the tree.
                if dir != self.notes_root.join("Notes") {
                    self.notes_expanded.insert(dir);
                }
                self.open_note(path, cx);
                if let Some(editor) = self.note_editor.clone() {
                    editor.update(cx, |ed, cx| ed.focus_title(window, cx));
                }
            }
            Err(e) => window.push_notification(format!("Could not create note: {e}"), cx),
        }
    }

    /// At a flush point, let a freshly created note's filename catch up with
    /// its typed title: derive a stem from the first heading and rename the
    /// file to match. Only notes still carrying an "Untitled" name are ever
    /// renamed — retitling an existing note must not silently move its file
    /// (wiki links to it would dangle, and clicking one would grow a fresh
    /// empty note at the old name). Daily notes are date-named and never
    /// touched. A name collision leaves the file where it is and says so.
    fn rename_note_to_title(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Only the note actually on screen, and only regular notes — dailies
        // are PaneView::Day and keep their date name.
        if self.view != PaneView::Note(path.clone()) {
            return;
        }
        let current = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if !notes::is_untitled_stem(current) {
            return;
        }
        let editor = match &self.note_editor {
            Some(e) if e.read(cx).path == path => e.clone(),
            _ => return,
        };
        let Some(stem) = editor.read(cx).title_stem() else {
            return;
        };
        if stem == current {
            return;
        }
        match notes::rename_note(&path, &stem) {
            Ok(new_path) if new_path != path => {
                self.note_self_write(&new_path);
                editor.update(cx, |ed, _| ed.set_path(new_path.clone()));
                self.view = PaneView::Note(new_path);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Flush points don't carry a Window; reach the active one for
                // the notice, after the current update settles.
                let msg = format!(
                    "Note kept as \"{current}\": a note named \"{stem}\" already exists here."
                );
                match cx.active_window() {
                    Some(win) => cx.defer(move |cx| {
                        let _ = win.update(cx, |_, window, cx| {
                            window.push_notification(msg, cx);
                        });
                    }),
                    None => eprintln!("kairn: {msg}"),
                }
            }
            Err(e) => eprintln!("kairn: could not rename {}: {e}", path.display()),
        }
    }

    /// Rename prompt for a note row: a one-field dialog prefilled with the
    /// current name.
    pub fn prompt_rename_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let initial = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        crate::name_dialog::open(
            "Rename note",
            "Rename",
            Some(initial),
            window,
            cx,
            move |ws, name, window, cx| ws.rename_note_at(&path, name, window, cx),
        );
    }

    /// New-note action for a folder row (or the Notes section header, which
    /// creates at the top level): kept as a thin alias so call sites read as
    /// intent, not mechanism.
    pub fn prompt_new_note(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.create_new_note(dir, window, cx);
    }

    /// New-folder prompt: a one-field dialog, then a plain directory under
    /// `dir`, expanded so the empty folder is visible in the tree.
    pub fn prompt_new_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        crate::name_dialog::open(
            "New folder",
            "Create",
            None,
            window,
            cx,
            move |ws, name, window, cx| match notes::create_folder_in(&dir, name) {
                Ok(path) => {
                    ws.notes_expanded.insert(path);
                    ws.reload_notes(cx);
                    cx.notify();
                }
                Err(e) => window.push_notification(format!("Could not create folder: {e}"), cx),
            },
        );
    }

    /// Flush pending editor changes now instead of waiting for the autosave.
    pub(crate) fn on_save_note(
        &mut self,
        _: &crate::keymap::SaveNote,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_note_editor(cx);
    }

    /// Dismiss the orphaned-line banner, optionally appending its text to
    /// the note it was bound for so nothing typed is lost.
    /// Resolve a sync conflict, keeping either the current note or the
    /// conflict copy. The losing file moves to `Notes/@Trash/` (nothing is
    /// destroyed); adopting the copy renames it into the note's place, and
    /// the file watcher plus reload pick the new content up everywhere.
    pub fn resolve_conflict(&mut self, copy: &Path, keep_copy: bool, cx: &mut Context<Self>) {
        let result = if keep_copy {
            match notes::conflict_owner(copy) {
                Some(owner) => notes::adopt_conflict_copy(&self.notes_root, &owner, copy),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "not a sync-conflict file name",
                )),
            }
        } else {
            notes::trash_note(&self.notes_root, copy).map(|_| ())
        };
        match result {
            Ok(()) => self.reload_notes(cx),
            Err(e) => eprintln!("kairn: could not resolve conflict {}: {e}", copy.display()),
        }
        cx.notify();
    }

    /// Jump to the note a conflict copy shadows, where the banner offers the
    /// resolution actions: a day opens on the calendar, anything else as a
    /// note.
    pub fn open_conflict_owner(&mut self, owner: &Path, cx: &mut Context<Self>) {
        let day = owner
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| chrono::NaiveDate::parse_from_str(stem, "%Y%m%d").ok());
        match day {
            Some(date) => self.select_day(date, cx),
            None => self.open_note(owner.to_path_buf(), cx),
        }
    }

    pub fn resolve_orphan(&mut self, append: bool, cx: &mut Context<Self>) {
        let Some((path, text)) = self.orphaned.take() else { return };
        if append {
            if let Err(e) = notes::append_line(&path, &text) {
                eprintln!("kairn: could not save {}: {e}", path.display());
                self.orphaned = Some((path, text));
                return;
            }
            self.note_self_write(&path);
            self.reload_notes(cx);
        }
        cx.notify();
    }

    /// Watch the notes root recursively; any change outside `.kairn/` reloads
    /// the pane. Events are debounced briefly so an editor's save dance (or
    /// our own temp-file + rename write) causes one reload, not several.
    pub(crate) fn watch_notes(
        root: PathBuf,
        self_writes: SelfWrites,
        cx: &mut Context<Self>,
    ) -> (Option<notify::RecommendedWatcher>, Task<()>) {
        use futures::StreamExt as _;
        use notify::Watcher as _;

        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if notes_event_relevant(&event, &self_writes) {
                let _ = tx.unbounded_send(());
            }
        };
        // Never follow symlinks: one link into a big tree exhausts inotify
        // watches on Linux and the watcher dies.
        let watcher = notify::RecommendedWatcher::new(
            handler,
            notify::Config::default().with_follow_symlinks(false),
        )
        .and_then(|mut w| {
            w.watch(&root, notify::RecursiveMode::Recursive)?;
            Ok(w)
        });
        let watcher = match watcher {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("kairn: notes watching unavailable: {e}");
                None
            }
        };

        let task = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                while rx.try_recv().is_ok() {}
                let ok = this.update(cx, |ws, cx| {
                    ws.reload_notes(cx);
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        (watcher, task)
    }

    /// Watch every library root recursively, one watcher per root feeding a
    /// shared debounced library-scoped reload: agent writes and Syncthing
    /// syncs must appear in the tree and any open library file without a
    /// restart, but library churn never re-scans the vault. Events under
    /// ignored subtrees (VCS, builds, dotfiles) are dropped at the source —
    /// a `.git` churn in a big root must not cost reloads.
    pub(crate) fn watch_library(
        roots: Vec<PathBuf>,
        self_writes: SelfWrites,
        cx: &mut Context<Self>,
    ) -> (Vec<notify::RecommendedWatcher>, Option<Task<()>>) {
        use futures::StreamExt as _;
        use notify::Watcher as _;

        if roots.is_empty() {
            return (Vec::new(), None);
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let mut watchers = Vec::new();
        for root in roots {
            let tx = tx.clone();
            let self_writes = self_writes.clone();
            let handler = move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                if library_event_relevant(&event, &self_writes) {
                    let _ = tx.unbounded_send(());
                }
            };
            // Never follow symlinks, same as the vault watcher: one link
            // into a big tree exhausts inotify watches on Linux.
            let watcher = notify::RecommendedWatcher::new(
                handler,
                notify::Config::default().with_follow_symlinks(false),
            )
            .and_then(|mut w| {
                w.watch(&root, notify::RecursiveMode::Recursive)?;
                Ok(w)
            });
            match watcher {
                Ok(w) => watchers.push(w),
                Err(e) => {
                    eprintln!("kairn: library watching unavailable for {}: {e}", root.display())
                }
            }
        }
        let task = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                while rx.try_recv().is_ok() {}
                let ok = this.update(cx, |ws, cx| {
                    ws.reload_library(cx);
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        (watchers, Some(task))
    }

    /// Apply and persist edits from the settings dialog. A changed notes
    /// folder re-bootstraps the layout, re-points the file watcher, and
    /// reloads the pane and calendar.
    pub fn apply_settings(
        &mut self,
        patch: SettingsPatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.ssh_hosts = patch.hosts;
        self.settings.local_apps = patch.local_apps;
        self.settings.notes_root = patch.notes_root;
        self.settings.daily_template_rule = patch.daily_template_rule;
        self.settings.theme = patch.theme;
        self.settings.ui_font = patch.ui_font;
        self.settings.editor_font = patch.editor_font;
        self.settings.mono_font = patch.mono_font;
        self.settings.editor_font_size = patch.editor_font_size;
        self.settings.ui_font_size = patch.ui_font_size;
        // Applying settings is the explicit user action that ends the
        // degraded no-save state after a corrupt settings.json.
        self.settings.degraded = false;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
            window.push_notification("Could not write settings.json, see stderr.", cx);
        }
        let root = self.settings.notes_root();
        self.root_missing = self.settings.notes_root.as_deref().is_some_and(|r| !r.is_empty())
            && !root.exists();
        self.notes_root = root;
        if !self.root_missing {
            notes::ensure_layout(&self.notes_root);
        }
        // Re-arm the watcher on every apply, root change or not: it may
        // have died (root renamed or deleted, inotify exhaustion) and this
        // is the user's retry lever.
        let (watcher, task) =
            Self::watch_notes(self.notes_root.clone(), self.self_writes.clone(), cx);
        self._notes_watcher = watcher;
        self._notes_watch_task = task;
        // An edited template body writes through to the same file NotePlan
        // reads. Never into a missing root: that would materialise a fresh
        // tree at an unmounted path.
        if let Some(body) = patch.template_body
            && !self.root_missing
        {
            match notes::save_daily_template(&self.notes_root, &body) {
                Ok(()) => self.note_self_write(&notes::daily_template_path(&self.notes_root)),
                Err(e) => {
                    eprintln!("kairn: could not save the daily template: {e}");
                    window.push_notification("Could not save the daily template, see stderr.", cx);
                }
            }
        }
        // Re-resolve the theme every apply: the choice, the fonts, or the
        // notes root (where theme files live) may all have changed.
        crate::theme::apply(&self.settings, &self.notes_root, Some(window), cx);
        self.retheme_sessions(cx);
        self.reload_notes(cx);
        cx.notify();
    }
}

/// The plain-text editor over a library code/text file: a multi-line mono
/// input bound to its file, saving on debounce and at every flush point.
pub(crate) struct LibraryTextEditor {
    pub path: PathBuf,
    pub input: gpui::Entity<gpui_component::input::InputState>,
    /// The file's content at load or last save: saves write only when the
    /// buffer differs, and external reloads land only while it is clean.
    pub baseline: String,
    pub(crate) _sub: gpui::Subscription,
    pub(crate) _save: Option<Task<()>>,
}

/// Edits collected by the settings dialog, applied in one Save.
pub struct SettingsPatch {
    pub notes_root: Option<String>,
    pub hosts: Vec<kairn_core::settings::SshHost>,
    pub local_apps: Vec<kairn_core::settings::HostApp>,
    pub daily_template_rule: String,
    /// The daily template body, only when the dialog changed it.
    pub template_body: Option<String>,
    /// Theme id: "dark", "light", or a `.kairn/themes` file stem.
    pub theme: String,
    pub ui_font: Option<String>,
    pub editor_font: Option<String>,
    pub mono_font: Option<String>,
    pub editor_font_size: Option<f32>,
    pub ui_font_size: Option<f32>,
}

#[cfg(test)]
mod watcher_event_tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind};

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    /// Linux inotify reports the reload's own reads and directory walks
    /// back as Access events (IN_OPEN and friends). Treating those as
    /// relevant makes every reload schedule the next one: a self-sustaining
    /// loop that pegs a core. Reads never change content.
    #[test]
    fn reads_never_trigger_reloads() {
        let writes = SelfWrites::default();
        let open = EventKind::Access(AccessKind::Open(AccessMode::Any));
        let close = EventKind::Access(AccessKind::Close(AccessMode::Read));
        assert!(!notes_event_relevant(&event(open, "/vault/Notes/a.md"), &writes));
        assert!(!notes_event_relevant(&event(close, "/vault/Calendar"), &writes));
        assert!(!library_event_relevant(&event(open, "/lib/doc.md"), &writes));
        assert!(!library_event_relevant(&event(close, "/lib/sub"), &writes));
    }

    #[test]
    fn content_changes_still_reload() {
        let writes = SelfWrites::default();
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        let create = EventKind::Create(CreateKind::File);
        assert!(notes_event_relevant(&event(modify, "/vault/Notes/a.md"), &writes));
        assert!(notes_event_relevant(&event(create, "/vault/Calendar/2026-08-13.md"), &writes));
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(library_event_relevant(&event(modify, "/lib/doc.md"), &writes));
    }

    #[test]
    fn ignored_subtrees_stay_dropped() {
        let writes = SelfWrites::default();
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(!notes_event_relevant(&event(modify, "/vault/.kairn/state.json"), &writes));
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(notes_event_relevant(&event(modify, "/vault/.kairn/activity.jsonl"), &writes));
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(!library_event_relevant(&event(modify, "/lib/node_modules/x.js"), &writes));
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(!library_event_relevant(&event(modify, "/lib/.git/index"), &writes));
    }
}
