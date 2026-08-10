use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::Root;
use kairn_core as notes;

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
    /// The note pane at full width with the sidebar: the terminal closed
    /// via the titlebar toggle, not the focused Writing layout.
    NotesFull,
    TerminalFull,
    Writing,
}

impl LayoutMode {
    /// Whether the terminal pane is on screen in this layout.
    pub fn shows_terminal(self) -> bool {
        matches!(self, LayoutMode::Split | LayoutMode::TerminalFull)
    }
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
    /// The single-buffer editor over the pane's document: the only editing
    /// model. The Writing layout is a focused-width view of the same
    /// editor, not a separate one. Absent when nothing here is editable
    /// (task views, unreadable notes, missing root).
    pub(crate) note_editor: Option<gpui::Entity<crate::note_editor::NoteEditor>>,
    pub(crate) _note_editor_sub: Option<gpui::Subscription>,
    /// Editor text whose merge target vanished or conflicted before it could
    /// be saved: (file it was bound for, the user's text). Rendered as a
    /// banner so typed text is never silently dropped.
    pub orphaned: Option<(PathBuf, String)>,
    pub sessions: Vec<Session>,
    pub active_session: usize,
    next_session_id: u64,
    pub cal_offset: i32,
    pub notes_root: PathBuf,
    pub selected_day: NaiveDate,
    pub view: PaneView,
    /// The pane's document as read from disk (or the daily template for a
    /// day with no file yet), seeded into the editor on each reload.
    pub(crate) doc_text: Option<String>,
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
    /// Per-day open/done task tallies (by due date) for the calendar's
    /// NotePlan-style day indicators.
    pub day_stats: HashMap<NaiveDate, notes::DayTaskStats>,
    /// Every open task across the daily notes, newest first.
    pub open_tasks: Vec<notes::TaskRef>,
    /// Visible rows of the sidebar Notes browser.
    pub notes_tree: Vec<notes::NoteEntry>,
    /// Folders currently expanded in the Notes browser.
    pub(crate) notes_expanded: HashSet<PathBuf>,
    /// Open/done task tallies for Monday..Sunday of the selected day's week,
    /// so the week strip can show the same indicators as the calendar.
    pub week_stats: [notes::DayTaskStats; 7],
    /// Time-blocked lines of the selected day's note, for the timeline pill
    /// row; empty for other views. Recomputed on reload, not per frame.
    pub day_timeline: Vec<notes::TimeBlock>,
    /// Open-task counts for the Today/Open/Overdue views, from the last
    /// reload; renders read these instead of re-scanning per frame.
    pub(crate) task_counts: [usize; 3],
    /// Recent entries from `.kairn/activity.jsonl`, newest first: what
    /// agents did to the notes via the CLI, for the sidebar's Agents feed.
    pub agent_activity: Vec<notes::ActivityEntry>,
    /// Recent writes by this instance, for watcher self-event suppression.
    pub(crate) self_writes: crate::vault_state::SelfWrites,
    /// Daily-note text carried across reloads; unchanged files skip the read.
    pub(crate) daily_cache: notes::TextCache,
    /// Non-daily note text (period notes, Notes/ files) for the task scan.
    pub(crate) note_cache: notes::TextCache,
    /// Week-strip day cells' window bounds from the last paint, captured so
    /// a task drag can hit-test its drop day without a second layout pass.
    pub(crate) week_strip_bounds:
        std::rc::Rc<std::cell::RefCell<Vec<(NaiveDate, gpui::Bounds<gpui::Pixels>)>>>,
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

        // KAIRN_ROOT overrides the configured notes folder for this process
        // only (dev, testing, screenshots); it is never written to settings.
        // The CLI honours the same variable via --root.
        let env_root = std::env::var("KAIRN_ROOT").ok().filter(|r| !r.is_empty());
        let notes_root = env_root
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| settings.notes_root());
        // A configured folder that isn't there means an unmounted drive or
        // a moved path: creating a fresh empty vault at that spot would be
        // worse than stopping. The default ~/kairn is always created.
        let root_missing = (env_root.is_some()
            || settings.notes_root.as_deref().is_some_and(|r| !r.is_empty()))
            && !notes_root.exists();
        if !root_missing {
            notes::ensure_layout(&notes_root);
        }
        let self_writes = crate::vault_state::SelfWrites::default();
        let (notes_watcher, notes_watch_task) =
            Self::watch_notes(notes_root.clone(), self_writes.clone(), cx);

        // Closing the window must not drop a pending edit.
        let flush = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            flush
                .update(cx, |ws, cx| ws.flush_note_editor(cx))
                .ok();
            if cfg!(target_os = "macos") {
                // Minimize instead of destroying the window: the workspace
                // (and its live terminal sessions) survives, and a Dock
                // click brings it back. Destroying it leaves a windowless
                // process the Dock can't reopen. Quit remains the way to
                // exit. The minimize must run after this handler returns:
                // AppKit's close-button sequence reverses a miniaturize
                // issued from inside windowShouldClose.
                let handle = window.window_handle();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    handle
                        .update(cx, |_, window, _| window.minimize_window())
                        .ok();
                })
                .detach();
                false
            } else {
                // Linux: a closed window with no Dock to reopen from would
                // strand a headless process, so close means quit.
                cx.quit();
                true
            }
        });

        let mut this = Self {
            settings,
            focus_handle: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            layout: LayoutMode::Split,
            sidebar_open: true,
            overlay: None,
            note_editor: None,
            _note_editor_sub: None,
            orphaned: None,
            sessions: Vec::new(),
            active_session: 0,
            next_session_id: 1,
            cal_offset: 0,
            notes_root,
            selected_day: Local::now().date_naive(),
            view: PaneView::Day,
            doc_text: None,
            doc_path: None,
            mentions: Vec::new(),
            conflicts: Vec::new(),
            doc_error: None,
            dailies_skipped: 0,
            root_missing,
            note_days: HashMap::new(),
            day_stats: HashMap::new(),
            open_tasks: Vec::new(),
            notes_tree: Vec::new(),
            notes_expanded: HashSet::new(),
            week_stats: [notes::DayTaskStats::default(); 7],
            day_timeline: Vec::new(),
            task_counts: [0; 3],
            agent_activity: Vec::new(),
            self_writes,
            daily_cache: notes::TextCache::default(),
            note_cache: notes::TextCache::default(),
            week_strip_bounds: Default::default(),
            _activity_timer: activity_timer,
            _notes_watcher: notes_watcher,
            _notes_watch_task: notes_watch_task,
        };
        this.reload_notes(cx);
        this.spawn_session(SessionKind::Local, window, cx);
        this
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
        match spawn(id, kind, weak, cx) {
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
        // Activating a session must show its terminal.
        if !self.layout.shows_terminal() {
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

    /// Close a session from the sidebar menu: kill its process and let the
    /// exit callback (the same path `exit` takes) remove it and fix focus.
    pub fn close_session(&mut self, idx: usize) {
        if let Some(session) = self.sessions.get(idx) {
            session.terminate();
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

    /// The titlebar segment: set one of the three layouts directly. Split and
    /// TerminalFull hand focus to the terminal; NotesFull leaves it with the
    /// note editor. Also the escape from Writing back into a normal layout.
    pub(crate) fn set_layout(&mut self, mode: LayoutMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout == mode {
            return;
        }
        self.layout = mode;
        if mode.shows_terminal() {
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    /// Whether a sidebar section is collapsed, by its header label.
    pub fn section_collapsed(&self, label: &str) -> bool {
        self.settings.sidebar_collapsed.iter().any(|s| s == label)
    }

    /// Collapse or expand a sidebar section, persisted like every other
    /// setting.
    pub fn toggle_section(&mut self, label: &str, cx: &mut Context<Self>) {
        match self.settings.sidebar_collapsed.iter().position(|s| s == label) {
            Some(i) => {
                self.settings.sidebar_collapsed.remove(i);
            }
            None => self.settings.sidebar_collapsed.push(label.to_string()),
        }
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    /// Set week-strip visibility: "always", "daily", or "off".
    pub fn set_week_strip(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.settings.week_strip == mode {
            return;
        }
        self.settings.week_strip = mode.to_string();
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    /// Show or hide the daily note's timeline pill row.
    pub fn set_day_timeline(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.day_timeline == on {
            return;
        }
        self.settings.day_timeline = on;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        // Turning it on must fill the row for the note already on screen;
        // off clears it (reload skips the parse entirely while disabled).
        self.reload_notes(cx);
        cx.notify();
    }

    /// Point the sidebar Daily section forward (today + next two days) or
    /// back (today + previous two).
    pub fn set_daily_forward(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.settings.daily_forward == forward {
            return;
        }
        self.settings.daily_forward = forward;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    pub(crate) fn on_toggle_theme(&mut self, _: &ToggleThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        // With a custom theme active, the toggle jumps to the built-in of
        // the opposite mode: a predictable escape hatch, not a cycle.
        let name = match cx.kairn().mode {
            Mode::Dark => "light",
            Mode::Light => "dark",
        };
        self.set_theme(name, window, cx);
    }

    pub fn set_theme(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = name.to_string();
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        theme::apply(&self.settings, &self.notes_root, Some(window), cx);
        self.retheme_sessions(cx);
        cx.notify();
    }

    /// Push the active theme's terminal palette and mono font into every
    /// live session.
    pub(crate) fn retheme_sessions(&self, cx: &mut Context<Self>) {
        let (colors, font) = {
            let t = cx.kairn();
            (t.term_colors.clone(), t.mono_font.to_string())
        };
        for session in &self.sessions {
            session.view.update(cx, |view, cx| {
                let mut config = view.config().clone();
                config.colors = colors.clone();
                config.font_family = font.clone();
                view.update_config(config, cx);
            });
        }
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
        self.flush_note_editor(cx);
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
            .text_size(t.ui_px(13.))
            .when_some(t.ui_font.clone(), |d, f| d.font_family(f))
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
            .children(self.render_statusbar(&t, cx))
            .children(self.render_picker(&t, window, cx))
            .children(self.render_switcher(&t, cx))
            .children(self.render_capture(&t, cx))
            .children(self.render_drag_ghost(&t, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

impl Workspace {
    /// The floating ghost that follows a task's drag-to-reschedule: a small
    /// card with the task's text, offset from the pointer so it never sits
    /// under it (which would block the week strip's hit-testing).
    fn render_drag_ghost(
        &self,
        t: &theme::KairnTheme,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let (line, position) = self.note_editor.as_ref()?.read(cx).task_drag()?;
        let text = line.trim_start();
        let text = ["* ", "+ ", "- "]
            .iter()
            .find_map(|m| text.strip_prefix(m))
            .unwrap_or(text)
            .trim_start();
        let text = text.strip_prefix("[ ]").map(str::trim_start).unwrap_or(text);
        let text: String = text.chars().take(48).collect();
        Some(
            div()
                .absolute()
                .left(position.x + px(14.))
                .top(position.y + px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(7.))
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .shadow_md()
                .text_size(t.ui_px(12.))
                .text_color(t.dim)
                .child(
                    div()
                        .w(px(10.))
                        .h(px(10.))
                        .flex_none()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t.faint),
                )
                .child(text),
        )
    }
}
