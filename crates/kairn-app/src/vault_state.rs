//! Everything the workspace derives from the notes root: navigation,
//! the reload pipeline, task toggling, the file watcher, and applying
//! settings changes.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Days, Local, NaiveDate};
use gpui::{AppContext as _, Context, Task, Window};
use gpui_component::WindowExt;
use kairn_core as notes;
use kairn_core::TaskQuery;

use crate::workspace::{LayoutMode, PaneView, Workspace};

/// Paths this instance just wrote, with when and a hash of what was
/// written: the file watcher uses it to skip reload storms caused by our
/// own atomic-write renames, without going blind to real external edits.
pub(crate) type SelfWrites = Arc<parking_lot::Mutex<HashMap<PathBuf, (Instant, u64)>>>;

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
        self.flush_note_editor(cx);
        self.selected_day = day;
        self.view = PaneView::Day;
        self.show_note_pane();
        self.reload_notes(cx);
        cx.notify();
    }

    pub fn open_note(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.flush_note_editor(cx);
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

    /// Write the single-buffer editor's pending edits now (navigation, save
    /// shortcut, window close). A no-op when the editor is clean or absent.
    pub(crate) fn flush_note_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.note_editor {
            editor.update(cx, |ed, cx| ed.save_now(cx));
        }
    }

    /// Keep the single-buffer editor entity in step with the pane's document
    /// after a reload: same file merges the fresh disk state into any
    /// in-flight edits; a different file (or view) swaps the editor out.
    fn sync_note_editor(&mut self, cx: &mut Context<Self>) {
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
            _ => None,
        });
        let Some(path) = path else {
            self.note_editor = None;
            self._note_editor_sub = None;
            return;
        };
        let text = self.doc_text.clone().unwrap_or_default();
        if let Some(editor) = &self.note_editor
            && editor.read(cx).path == path
        {
            editor.update(cx, |ed, cx| ed.reconcile_from_disk(&text, cx));
            return;
        }
        let editor = cx.new(|cx| NoteEditor::new(path, &text, cx));
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
                NoteEditorEvent::TaskDropped { line_start, position } => {
                    this.on_task_dropped(*line_start, *position, cx);
                }
            },
        ));
        self.note_editor = Some(editor);
    }

    /// The drop half of drag-to-reschedule: an open task's glyph drag was
    /// released outside the editor. If the pointer sat on a week-strip day,
    /// rewrite the task's `>date` through the editor buffer (undoable,
    /// autosaved); anywhere else the drag just ends.
    pub(crate) fn on_task_dropped(
        &mut self,
        line_start: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let day = self
            .week_strip_bounds
            .borrow()
            .iter()
            .find(|(_, bounds)| bounds.contains(&position))
            .map(|(day, _)| *day);
        if let Some(day) = day
            && let Some(editor) = &self.note_editor
        {
            editor.update(cx, |ed, cx| ed.reschedule_line_at(line_start, day, cx));
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
        self.agent_activity = notes::recent_activity(&self.notes_root, 6);
        let today = Local::now().date_naive();
        self.task_counts = [TaskQuery::Today, TaskQuery::Open, TaskQuery::Overdue].map(|q| {
            self.open_tasks.iter().filter(|t| q.matches(t.due, today)).count()
        });
        let monday = self.selected_day
            - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
        for (i, count) in self.week_open_counts.iter_mut().enumerate() {
            let day = monday + Days::new(i as u64);
            *count = self.open_tasks.iter().filter(|t| t.due == day).count();
        }
        let path = match &self.view {
            PaneView::Day => scan.days.get(&self.selected_day).cloned(),
            PaneView::Note(p) => Some(p.clone()),
            PaneView::Tasks(_) => None,
        };
        let (text, doc_error) = match path.as_deref() {
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
        // A day from today onward with no file yet starts from the daily
        // template (Notes/@Templates/Daily.md): rendered immediately, written
        // to disk only when the first edit lands. Past days stay blank — a
        // template there would dress up history that never happened.
        let text = if text.is_none()
            && matches!(self.view, PaneView::Day)
            && self.doc_error.is_none()
            && !self.root_missing
            && self.selected_day >= today
        {
            notes::daily_template(&self.notes_root)
        } else {
            text
        };
        self.doc_text = text;
        // Linked mentions for the pane's document: a day is referenced by its
        // ISO date ([[2026-08-07]] and >2026-08-07 alike), a note by its stem.
        let title = match &self.view {
            PaneView::Day => Some(self.selected_day.format("%Y-%m-%d").to_string()),
            PaneView::Note(p) => p
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string),
            PaneView::Tasks(_) => None,
        };
        self.mentions = match title {
            Some(t) => notes::mentions_in(&scan, &dailies, &t, path.as_deref()),
            None => Vec::new(),
        };
        // For a day with no file yet, check where the file would be: the
        // conflicted copy of a note that vanished is exactly the case that
        // needs surfacing.
        let conflict_probe = path.clone().or_else(|| match &self.view {
            PaneView::Day => Some(notes::daily_path(&self.notes_root, self.selected_day)),
            _ => None,
        });
        self.conflicts = conflict_probe
            .as_deref()
            .map(notes::conflict_copies)
            .unwrap_or_default();
        self.doc_path = path;
        self.note_days = scan.days;
        self.sync_note_editor(cx);
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
            let relevant = event.paths.is_empty()
                || event.paths.iter().any(|p| {
                    // The activity log is the one `.kairn/` file the UI
                    // shows live: CLI writes must surface without a restart.
                    let watched_in_kairn = p.file_name().is_some_and(|n| n == "activity.jsonl");
                    (watched_in_kairn
                        || !p.components().any(|c| c.as_os_str() == ".kairn"))
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().contains(".kairn-tmp"))
                        && !is_recent_self_write(&self_writes, p)
                });
            if relevant {
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

    /// Apply and persist edits from the settings dialog. A changed notes
    /// folder re-bootstraps the layout, re-points the file watcher, and
    /// reloads the pane and calendar.
    pub fn apply_settings(
        &mut self,
        notes_root: Option<String>,
        hosts: Vec<kairn_core::settings::SshHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.ssh_hosts = hosts;
        self.settings.notes_root = notes_root;
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
        self.reload_notes(cx);
        cx.notify();
    }
}
