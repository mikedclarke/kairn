//! Everything the workspace derives from the notes root: navigation,
//! the reload pipeline, task toggling, the file watcher, and applying
//! settings changes.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{Datelike, Days, Local, NaiveDate};
use gpui::{Context, Task, Window};
use gpui_component::WindowExt;
use kairn_core as notes;
use kairn_core::TaskQuery;

use crate::workspace::{PaneView, Workspace};

impl Workspace {
    pub fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        self.selected_day = day;
        self.view = PaneView::Day;
        self.reload_notes();
        cx.notify();
    }

    pub fn open_note(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        self.view = PaneView::Note(path);
        self.reload_notes();
        cx.notify();
    }

    pub fn open_task_view(&mut self, query: TaskQuery, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        self.view = PaneView::Tasks(query);
        self.reload_notes();
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
        self.open_tasks.iter().filter(move |t| query.matches(t.date, today))
    }

    /// Re-read the pane's document and everything the sidebar derives from
    /// the notes: calendar indicators, open-task counts, the Notes tree.
    pub fn reload_notes(&mut self) {
        self.note_days = notes::days_with_notes(&self.notes_root);
        self.open_tasks = notes::open_tasks_in_dailies(&self.notes_root);
        self.notes_tree = notes::notes_tree(&self.notes_root, &self.notes_expanded);
        let monday = self.selected_day
            - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
        for (i, count) in self.week_open_counts.iter_mut().enumerate() {
            let day = monday + Days::new(i as u64);
            *count = self.open_tasks.iter().filter(|t| t.date == day).count();
        }
        let path = match &self.view {
            PaneView::Day => notes::daily_file(&self.notes_root, self.selected_day),
            PaneView::Note(p) => Some(p.clone()),
            PaneView::Tasks(_) => None,
        };
        let text = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
        self.doc_lines = text.as_deref().map(notes::parse);
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
            Some(t) => notes::mentions_of(&self.notes_root, &t, path.as_deref()),
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
    }

    /// Open whatever a wiki link points at: a day, an existing note, or a
    /// brand-new note created wiki-style on first click. Creation never
    /// overwrites: if the file exists by the time we write, it is opened
    /// as-is (links arrive in synced files and race external writers).
    pub fn open_wiki_link(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        match notes::resolve_wiki_target(&self.notes_root, title) {
            notes::WikiTarget::Day(date) => self.select_day(date, cx),
            notes::WikiTarget::Note(path) => self.open_note(path, cx),
            notes::WikiTarget::Missing(path) => {
                match notes::create_note_if_absent(&path, &format!("# {title}\n")) {
                    Ok(_) => self.open_note(path, cx),
                    Err(e) => {
                        eprintln!("kairn: could not create {}: {e}", path.display());
                        window.push_notification("Could not create the linked note, see stderr.", cx);
                    }
                }
            }
            notes::WikiTarget::Invalid => {
                window.push_notification("That link can't name a note inside the notes folder.", cx);
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

    /// Toggle the task on line `line_idx` of the pane's document between open
    /// and done, writing the change back to the file.
    pub fn toggle_task(&mut self, line_idx: usize, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        let (Some(path), Some(text)) = (&self.doc_path, &self.doc_text) else {
            return;
        };
        let Some(expected) = text.lines().nth(line_idx) else {
            return;
        };
        match notes::toggle_task_on_disk(path, line_idx, expected) {
            Ok(true) => {}
            // The line changed on disk since render; the reload below picks
            // up whatever is there now.
            Ok(false) => {}
            Err(e) => eprintln!("kairn: could not update {}: {e}", path.display()),
        }
        self.reload_notes();
        cx.notify();
    }

    /// Toggle a task from a task view, addressed at whichever daily note it
    /// was scanned from.
    pub fn toggle_task_ref(&mut self, task: &notes::TaskRef, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        match notes::toggle_task_on_disk(&task.path, task.line_idx, &task.line) {
            Ok(_) => {}
            Err(e) => eprintln!("kairn: could not update {}: {e}", task.path.display()),
        }
        self.reload_notes();
        cx.notify();
    }

    /// Watch the notes root recursively; any change outside `.kairn/` reloads
    /// the pane. Events are debounced briefly so an editor's save dance (or
    /// our own temp-file + rename write) causes one reload, not several.
    pub(crate) fn watch_notes(
        root: PathBuf,
        cx: &mut Context<Self>,
    ) -> (Option<notify::RecommendedWatcher>, Task<()>) {
        use futures::StreamExt as _;
        use notify::Watcher as _;

        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            let relevant = event.paths.is_empty()
                || event.paths.iter().any(|p| {
                    !p.components().any(|c| c.as_os_str() == ".kairn")
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().contains(".kairn-tmp"))
                });
            if relevant {
                let _ = tx.unbounded_send(());
            }
        })
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
                    ws.reload_notes();
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
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
            window.push_notification("Could not write settings.json, see stderr.", cx);
        }
        let root = self.settings.notes_root();
        if root != self.notes_root {
            self.notes_root = root;
            notes::ensure_layout(&self.notes_root);
            let (watcher, task) = Self::watch_notes(self.notes_root.clone(), cx);
            self._notes_watcher = watcher;
            self._notes_watch_task = task;
            self.reload_notes();
        }
        cx.notify();
    }
}
