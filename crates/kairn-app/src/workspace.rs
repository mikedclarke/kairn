use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Task, TextLayout, Window, div, px,
};
use gpui_component::Root;
use kairn_core as notes;

use crate::editing::LineEdit;
use crate::overlays::Overlay;
use crate::session::{Session, SessionKind, spawn};
use crate::theme::{self, KairnThemeExt, Mode};

// The keymap (actions, chord labels) and small UI helpers are re-exported
// here so the render modules keep one import surface for workspace types.
pub use crate::keymap::*;
pub use crate::ui::kbd;
pub use kairn_core::TaskQuery;
use kairn_core::settings::Settings;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    Split,
    TerminalFull,
    Writing,
}

/// What the note pane is showing.
#[derive(Clone, PartialEq, Debug)]
pub enum PaneView {
    /// The selected day's daily note.
    Day,
    /// A note from the `Notes/` tree.
    Note(PathBuf),
    /// A generated list of open tasks from the daily notes.
    Tasks(TaskQuery),
}

pub struct Workspace {
    pub settings: Settings,
    focus_handle: FocusHandle,
    pub(crate) overlay_focus: FocusHandle,
    pub layout: LayoutMode,
    sidebar_open: bool,
    /// The one open overlay (picker, switcher, or capture), if any.
    pub(crate) overlay: Option<Overlay>,
    pub(crate) _autosave: Option<Task<()>>,
    /// In-place edit of one line of the pane document: the only editing
    /// model. The Writing layout is a focused-width view of the same
    /// line-rendered note, not a separate editor.
    pub line_edit: Option<LineEdit>,
    pub(crate) _line_edit_sub: Option<gpui::Subscription>,
    /// A line edit whose target vanished from the file before it could be
    /// saved: (file it was bound for, the user's text). Rendered as a banner
    /// so typed text is never silently dropped.
    pub orphaned: Option<(PathBuf, String)>,
    /// Text layout of each rendered note line from the latest render, for
    /// mapping a click position to a character. Interior-mutable because it
    /// is filled in while rendering.
    pub line_layouts: RefCell<HashMap<usize, TextLayout>>,
    pub sessions: Vec<Session>,
    pub active_session: usize,
    next_session_id: u64,
    pub cal_offset: i32,
    pub notes_root: PathBuf,
    pub selected_day: NaiveDate,
    pub view: PaneView,
    /// Parsed document the pane is showing; `None` when no file exists.
    pub doc_lines: Option<Vec<notes::Line>>,
    /// The document as read from disk, line-aligned with `doc_lines`; toggles
    /// pass the rendered line back so a file that changed underneath is never
    /// clobbered.
    pub(crate) doc_text: Option<String>,
    /// The pane document is the daily template rendered for a day with no
    /// file yet; the first mutation writes it to disk.
    pub(crate) doc_seeded: bool,
    /// The file `doc_text` was read from (`.md` or NotePlan's `.txt`).
    pub(crate) doc_path: Option<PathBuf>,
    /// Lines elsewhere that link to the pane's document.
    pub mentions: Vec<notes::Mention>,
    /// Syncthing conflict copies sitting next to the pane's document.
    pub conflicts: Vec<PathBuf>,
    /// Read error for the pane's document: the file exists but couldn't be
    /// read (permissions, invalid UTF-8). Rendered instead of pretending
    /// there is no note.
    pub doc_error: Option<String>,
    /// Daily notes that exist but couldn't be read this reload.
    pub dailies_skipped: usize,
    /// The configured notes folder doesn't exist (unmounted drive, moved
    /// path): the note pane blocks instead of showing a convincing empty
    /// vault, and nothing writes into the void.
    pub root_missing: bool,
    /// Daily-note file per date, for calendar indicators and lookups.
    pub note_days: HashMap<NaiveDate, PathBuf>,
    /// Every open task across the daily notes, newest first.
    pub open_tasks: Vec<notes::TaskRef>,
    /// Visible rows of the sidebar Notes browser.
    pub notes_tree: Vec<notes::NoteEntry>,
    /// Folders currently expanded in the Notes browser.
    pub(crate) notes_expanded: HashSet<PathBuf>,
    /// Open-task counts for Monday..Sunday of the selected day's week.
    pub week_open_counts: [usize; 7],
    /// Open-task counts for the Today/Open/Overdue views, from the last
    /// reload; renders read these instead of re-scanning per frame.
    pub(crate) task_counts: [usize; 3],
    /// Recent writes by this instance, for watcher self-event suppression.
    pub(crate) self_writes: crate::vault_state::SelfWrites,
    _activity_timer: Task<()>,
    /// Watches the notes root so outside edits (agents, Syncthing, NotePlan
    /// elsewhere) appear without a restart. Dropped with the workspace.
    pub(crate) _notes_watcher: Option<notify::RecommendedWatcher>,
    pub(crate) _notes_watch_task: Task<()>,
}

impl Workspace {
    pub fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Sidebar status dots reflect the PTY foreground process. Poll it here
        // and repaint only when a session's busy state actually changed: a
        // repaint is a full-window rebuild, far too expensive for a blind tick.
        let activity_timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let tick = this.update(cx, |ws, cx| {
                    let mut changed = false;
                    for session in &mut ws.sessions {
                        let busy = session.is_busy();
                        if busy != session.busy {
                            session.busy = busy;
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
                if tick.is_err() {
                    break;
                }
            }
        });

        let notes_root = settings.notes_root();
        // A configured folder that isn't there means an unmounted drive or
        // a moved path: creating a fresh empty vault at that spot would be
        // worse than stopping. The default ~/kairn is always created.
        let root_missing = settings.notes_root.as_deref().is_some_and(|r| !r.is_empty())
            && !notes_root.exists();
        if !root_missing {
            notes::ensure_layout(&notes_root);
        }
        let self_writes = crate::vault_state::SelfWrites::default();
        let (notes_watcher, notes_watch_task) =
            Self::watch_notes(notes_root.clone(), self_writes.clone(), cx);

        // Closing the window must not drop a pending line edit.
        let flush = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            flush.update(cx, |ws, cx| ws.commit_line_edit(true, cx)).ok();
            true
        });

        let mut this = Self {
            settings,
            focus_handle: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            layout: LayoutMode::Split,
            sidebar_open: true,
            overlay: None,
            _autosave: None,
            line_edit: None,
            _line_edit_sub: None,
            orphaned: None,
            line_layouts: RefCell::new(HashMap::new()),
            sessions: Vec::new(),
            active_session: 0,
            next_session_id: 1,
            cal_offset: 0,
            notes_root,
            selected_day: Local::now().date_naive(),
            view: PaneView::Day,
            doc_lines: None,
            doc_text: None,
            doc_seeded: false,
            doc_path: None,
            mentions: Vec::new(),
            conflicts: Vec::new(),
            doc_error: None,
            dailies_skipped: 0,
            root_missing,
            note_days: HashMap::new(),
            open_tasks: Vec::new(),
            notes_tree: Vec::new(),
            notes_expanded: HashSet::new(),
            week_open_counts: [0; 7],
            task_counts: [0; 3],
            self_writes,
            _activity_timer: activity_timer,
            _notes_watcher: notes_watcher,
            _notes_watch_task: notes_watch_task,
        };
        this.reload_notes();
        this.spawn_session(SessionKind::Local, window, cx);
        this
    }

    pub fn mode(&self) -> Mode {
        Mode::from_str(&self.settings.theme)
    }

    // ----- sessions -----

    pub fn spawn_session(
        &mut self,
        kind: SessionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_session_id;
        self.next_session_id += 1;
        let weak = cx.weak_entity();
        match spawn(id, kind, self.mode(), weak, cx) {
            Ok(session) => {
                self.sessions.push(session);
                self.activate_session(self.sessions.len() - 1, window, cx);
            }
            Err(e) => eprintln!("kairn: failed to start session: {e}"),
        }
        self.overlay = None;
        cx.notify();
    }

    pub fn activate_session(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.sessions.len() {
            return;
        }
        self.active_session = idx;
        if self.layout == LayoutMode::Writing {
            self.layout = LayoutMode::Split;
        }
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    pub fn focus_active_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.get(self.active_session) {
            session.view.read(cx).focus_handle().clone().focus(window);
        }
    }

    pub fn set_session_title(&mut self, id: u64, title: SharedString, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id)
            && session.title != title {
                session.title = title;
                cx.notify();
            }
    }

    pub fn handle_session_exit(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.sessions.retain(|s| s.id != id);
        if self.active_session >= self.sessions.len() {
            self.active_session = self.sessions.len().saturating_sub(1);
        }
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    // ----- action handlers -----

    pub(crate) fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn on_toggle_terminal_full(
        &mut self,
        _: &ToggleTerminalFull,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.layout = if self.layout == LayoutMode::TerminalFull {
            LayoutMode::Split
        } else {
            LayoutMode::TerminalFull
        };
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn on_toggle_writing(&mut self, _: &ToggleWriting, _: &mut Window, cx: &mut Context<Self>) {
        self.layout = if self.layout == LayoutMode::Writing {
            LayoutMode::Split
        } else {
            LayoutMode::Writing
        };
        cx.notify();
    }

    pub(crate) fn on_toggle_theme(&mut self, _: &ToggleThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.set_theme(self.mode().toggled(), window, cx);
    }

    pub fn set_theme(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = mode.as_str().to_string();
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        theme::apply(mode, Some(window), cx);
        for session in &self.sessions {
            session.view.update(cx, |view, cx| {
                let mut config = view.config().clone();
                config.colors = theme::terminal_palette(mode);
                view.update_config(config, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
    }

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        crate::settings_dialog::open(self, window, cx);
    }

    fn on_new_local_session(
        &mut self,
        _: &NewLocalSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spawn_session(SessionKind::Local, window, cx);
    }

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_line_edit(true, cx);
        cx.quit();
    }

    fn on_activate_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_session(n, window, cx);
    }

    // Terminal font zoom, carried over from the spike (cmd/ctrl +/-).
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        if !(ks.modifiers.platform || ks.modifiers.control) {
            return;
        }
        let delta = match ks.key.as_str() {
            "+" | "=" => 1.0,
            "-" => -1.0,
            _ => return,
        };
        if let Some(session) = self.sessions.get(self.active_session) {
            session.view.update(cx, |terminal, cx| {
                let mut config = terminal.config().clone();
                let new_size = config.font_size + px(delta);
                if new_size >= px(6.0) {
                    config.font_size = new_size;
                    terminal.update_config(config, cx);
                }
            });
            cx.stop_propagation();
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.kairn().clone();

        let mut body = div().flex().flex_1().min_h(px(0.));
        // Writing is the focused layout: the note at a comfortable measure,
        // no sidebar.
        if self.sidebar_open && self.layout != LayoutMode::Writing {
            body = body.child(self.render_sidebar(&t, cx));
        }
        body = body.child(self.render_main(&t, window, cx));

        div()
            .id("kairn-root")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(13.))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_toggle_terminal_full))
            .on_action(cx.listener(Self::on_toggle_writing))
            .on_action(cx.listener(Self::on_toggle_switcher))
            .on_action(cx.listener(Self::on_close_overlay))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_capture))
            .on_action(cx.listener(Self::on_save_note))
            .on_action(cx.listener(Self::on_new_local_session))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(|this, _: &Session1, w, cx| this.on_activate_nth(0, w, cx)))
            .on_action(cx.listener(|this, _: &Session2, w, cx| this.on_activate_nth(1, w, cx)))
            .on_action(cx.listener(|this, _: &Session3, w, cx| this.on_activate_nth(2, w, cx)))
            .on_action(cx.listener(|this, _: &Session4, w, cx| this.on_activate_nth(3, w, cx)))
            .on_action(cx.listener(|this, _: &Session5, w, cx| this.on_activate_nth(4, w, cx)))
            .on_action(cx.listener(|this, _: &Session6, w, cx| this.on_activate_nth(5, w, cx)))
            .on_action(cx.listener(|this, _: &Session7, w, cx| this.on_activate_nth(6, w, cx)))
            .on_action(cx.listener(|this, _: &Session8, w, cx| this.on_activate_nth(7, w, cx)))
            .on_action(cx.listener(|this, _: &Session9, w, cx| this.on_activate_nth(8, w, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_titlebar(&t, cx))
            .child(body)
            .child(self.render_statusbar(&t, cx))
            .children(self.render_picker(&t, window, cx))
            .children(self.render_switcher(&t, cx))
            .children(self.render_capture(&t, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}
