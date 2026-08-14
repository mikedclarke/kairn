use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::Root;
use kairn_core as notes;

use crate::overlays::Overlay;
use crate::session::{Session, SessionKind, spawn};
use crate::theme::{self, KairnTheme, KairnThemeExt, Mode};

// The keymap (actions, chord labels) and small UI helpers are re-exported
// here so the render modules keep one import surface for workspace types.
pub use crate::keymap::*;
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
    /// The weekly note of the week containing the selected day.
    Week,
    /// The monthly note of the month containing the selected day.
    Month,
    /// A note from the `Notes/` tree.
    Note(PathBuf),
    /// A file from a sidebar Library root: rendered by kind (markdown
    /// editor, image, or a metadata card), never fed into tasks or links.
    Library(PathBuf),
    /// A generated list of open tasks from the daily notes.
    Tasks(TaskQuery),
}

/// The hold-for-heading gesture over a day drop target: dwelling an
/// in-flight line drag on a day for a moment opens a menu of that day's
/// headings; sliding onto one and releasing drops at that section's end.
pub(crate) enum HoldState {
    Idle,
    /// The pointer is dwelling on a day; the timer opens the menu unless
    /// the pointer moves away first (dropping the task cancels it).
    Arming {
        day: NaiveDate,
        anchor: gpui::Point<gpui::Pixels>,
        _timer: Task<()>,
    },
    Open(HoldMenu),
}

/// A drag on a sidebar timeline block. Times are recomputed from the
/// pointer each frame (the block renders at the provisional slot) and the
/// note line is rewritten only on release.
pub(crate) struct TimelineDrag {
    pub line_idx: usize,
    /// The raw line at grab time, the write-back verification token.
    pub expected: String,
    pub start: chrono::NaiveTime,
    pub end: Option<chrono::NaiveTime>,
    /// Dragging the bottom edge (retime the end) rather than the body
    /// (move the whole block).
    pub resize: bool,
    /// Pointer minutes-from-midnight minus block-start minutes at grab
    /// time, so the block doesn't snap its top edge to the pointer.
    pub grab_offset_min: i32,
    pub origin: gpui::Point<gpui::Pixels>,
    pub position: gpui::Point<gpui::Pixels>,
    /// True once the pointer has travelled past the drag threshold;
    /// releases before that are clicks, not edits.
    pub moved: bool,
}

pub(crate) struct HoldMenu {
    pub day: NaiveDate,
    pub items: Vec<HoldItem>,
    /// Anchor for the popup, near the pointer at open time.
    pub origin: gpui::Point<gpui::Pixels>,
    /// Rows' painted bounds, keyed by heading line index (`TOP_OF_NOTE` for
    /// the synthetic first row): the release hit-tests these. The mouse
    /// button is down throughout, so rows carry no click handlers.
    pub item_bounds: HoldItemBounds,
    pub menu_bounds: std::rc::Rc<std::cell::RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

pub(crate) type HoldItemBounds =
    std::rc::Rc<std::cell::RefCell<Vec<(usize, gpui::Bounds<gpui::Pixels>)>>>;

pub(crate) struct HoldItem {
    pub line_idx: usize,
    /// The raw heading line, for verify-by-content at drop time.
    pub raw: String,
    pub display: String,
    pub level: u8,
}

/// Sentinel item key for the menu's "Top of note" row.
pub(crate) const TOP_OF_NOTE: usize = usize::MAX;
/// Most headings the hold menu lists: it can't scroll mid-drag, so the tail
/// of a very long note is summarised instead.
pub(crate) const HOLD_MENU_CAP: usize = 16;

pub struct Workspace {
    pub settings: Settings,
    focus_handle: FocusHandle,
    pub(crate) overlay_focus: FocusHandle,
    pub layout: LayoutMode,
    sidebar_open: bool,
    /// The one open overlay (picker, switcher, or capture), if any.
    pub(crate) overlay: Option<Overlay>,
    /// The settings page, replacing the whole area below the titlebar while
    /// open. Its batch edits apply when it closes.
    pub(crate) settings_view: Option<gpui::Entity<crate::settings_dialog::SettingsEditor>>,
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
    /// Every Syncthing conflict in the vault as (owner note, conflict copy),
    /// for the sidebar's conflict list: conflicts on notes that aren't open
    /// would otherwise stay invisible.
    pub vault_conflicts: Vec<(PathBuf, PathBuf)>,
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
    /// Each configured library root with its visible tree rows, in the
    /// settings order. Rebuilt on reload; empty when no roots are set.
    pub library_trees: Vec<(PathBuf, Vec<notes::LibraryEntry>)>,
    /// Library roots and folders currently expanded in the sidebar.
    pub(crate) library_expanded: HashSet<PathBuf>,
    /// Images sharing the open library image's folder, for the sibling
    /// strip under the image view. Computed on reload, not per frame.
    pub(crate) library_siblings: Vec<PathBuf>,
    /// The plain-text editor over a library code/text file. Present only
    /// while a Text-kind library file is on screen.
    pub(crate) library_text: Option<crate::vault_state::LibraryTextEditor>,
    /// Open/done task tallies for Monday..Sunday of the selected day's week,
    /// so the week strip can show the same indicators as the calendar.
    pub week_stats: [notes::DayTaskStats; 7],
    /// Time-blocked lines of the selected day's note, for the sidebar's
    /// timeline view; empty for other views. Recomputed on reload, not per
    /// frame.
    pub day_timeline: Vec<notes::TimeBlock>,
    /// Whether the sidebar's day timeline hangs open under the calendar
    /// (the clock tab); while open the other sidebar sections make way and
    /// the sidebar scroll scrolls the timeline. Session state, not a
    /// setting.
    pub(crate) timeline_open: bool,
    /// An in-flight drag of a timeline block: moving it to another time,
    /// resizing its end, or carrying it onto a calendar day.
    pub(crate) timeline_drag: Option<TimelineDrag>,
    /// The timeline's 24-hour canvas bounds from the last paint, the ruler
    /// that converts drag positions to clock times.
    pub(crate) timeline_bounds:
        std::rc::Rc<std::cell::RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
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
    pub(crate) week_strip_bounds: crate::vault_state::DayBounds,
    /// Mini-calendar day cells' window bounds, same contract as the strip.
    pub(crate) calendar_drop_bounds: crate::vault_state::DayBounds,
    /// Sidebar Daily rows' window bounds, same contract as the strip.
    pub(crate) daily_drop_bounds: crate::vault_state::DayBounds,
    /// The sidebar scroll container's window bounds this frame. Cells
    /// scrolled out of its clip still prepaint their capture canvases, so a
    /// sidebar drop must also fall inside this to count.
    pub(crate) sidebar_bounds:
        std::rc::Rc<std::cell::RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Hold-for-heading state: dwelling a drag on a day target for a moment
    /// opens a menu of that day's headings to drop under.
    pub(crate) hold: HoldState,
    /// The sidebar scroll position, tracked so synthesized momentum can
    /// keep it moving after a touchpad flick (see `sidebar_flick`).
    pub(crate) sidebar_scroll: gpui::ScrollHandle,
    /// Timestamped vertical deltas from the last ~100ms of touchpad
    /// scrolling, the velocity source for synthesized momentum.
    pub(crate) sidebar_flick_samples: std::collections::VecDeque<(std::time::Instant, f32)>,
    /// The pending momentum watchdog or animation; replacing it cancels
    /// the old one, so fresh finger input always wins.
    pub(crate) _sidebar_kinetic_task: Option<Task<()>>,
    _activity_timer: Task<()>,
    /// Watches the notes root so outside edits (agents, Syncthing, NotePlan
    /// elsewhere) appear without a restart. Dropped with the workspace.
    pub(crate) _notes_watcher: Option<notify::RecommendedWatcher>,
    pub(crate) _notes_watch_task: Task<()>,
    /// One watcher per library root (agent writes and Syncthing must appear
    /// live there too), plus the shared debounce task draining their events.
    pub(crate) _library_watchers: Vec<notify::RecommendedWatcher>,
    pub(crate) _library_watch_task: Option<Task<()>>,
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
        let (library_watchers, library_watch_task) =
            Self::watch_library(settings.library_roots(), self_writes.clone(), cx);

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
            settings_view: None,
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
            vault_conflicts: Vec::new(),
            doc_error: None,
            dailies_skipped: 0,
            root_missing,
            note_days: HashMap::new(),
            day_stats: HashMap::new(),
            open_tasks: Vec::new(),
            notes_tree: Vec::new(),
            notes_expanded: HashSet::new(),
            library_trees: Vec::new(),
            library_expanded: HashSet::new(),
            library_siblings: Vec::new(),
            library_text: None,
            week_stats: [notes::DayTaskStats::default(); 7],
            day_timeline: Vec::new(),
            task_counts: [0; 3],
            agent_activity: Vec::new(),
            self_writes,
            daily_cache: notes::TextCache::default(),
            note_cache: notes::TextCache::default(),
            week_strip_bounds: Default::default(),
            calendar_drop_bounds: Default::default(),
            daily_drop_bounds: Default::default(),
            sidebar_bounds: Default::default(),
            timeline_open: false,
            timeline_drag: None,
            timeline_bounds: Default::default(),
            sidebar_scroll: gpui::ScrollHandle::new(),
            sidebar_flick_samples: std::collections::VecDeque::new(),
            _sidebar_kinetic_task: None,
            hold: HoldState::Idle,
            _activity_timer: activity_timer,
            _notes_watcher: notes_watcher,
            _notes_watch_task: notes_watch_task,
            _library_watchers: library_watchers,
            _library_watch_task: library_watch_task,
        };
        this.reload_notes(cx);
        // No session on launch: the terminal pane opens on the start page
        // and the first session is whatever the user picks there.
        let _ = window;
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

    /// Show or hide the sidebar's Agents activity section.
    pub fn set_show_agents(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.show_agents == on {
            return;
        }
        self.settings.show_agents = on;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    /// Show or hide the sidebar's Daily section.
    pub fn set_show_daily(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.show_daily == on {
            return;
        }
        self.settings.show_daily = on;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    /// Show or hide the sidebar's Tasks section.
    pub fn set_show_tasks(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.settings.show_tasks == on {
            return;
        }
        self.settings.show_tasks = on;
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        cx.notify();
    }

    /// Order library files by "modified" (newest first) or "name".
    pub fn set_library_sort(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.settings.library_sort == mode {
            return;
        }
        self.settings.library_sort = mode.to_string();
        if let Err(e) = self.settings.save() {
            eprintln!("kairn: failed to save settings: {e}");
        }
        self.reload_library_trees();
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

    /// Open the settings page, or close it (applying its edits) when it is
    /// already up, so the settings chord toggles.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_view.is_some() {
            self.close_settings(window, cx);
            return;
        }
        self.overlay = None;
        let editor = crate::settings_dialog::open(self, window, cx);
        window.focus(&editor.read(cx).focus_handle());
        self.settings_view = Some(editor);
        cx.notify();
    }

    /// Close the settings page, applying its batch edits. The patch is
    /// collected in the editor's context and applied here, so neither
    /// entity re-enters the other.
    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_view.take() {
            let patch = editor.update(cx, |editor, cx| editor.collect_patch(cx));
            self.apply_settings(patch, window, cx);
            window.focus(&self.focus_handle);
            cx.notify();
        }
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

        // The hold menu lives only as long as its drag: mouse-up and Escape
        // end the drag in the editor, and this render-side sweep folds the
        // menu (and any armed timer) the frame after.
        if !matches!(self.hold, HoldState::Idle) {
            let drag_live = self
                .note_editor
                .as_ref()
                .is_some_and(|e| e.read(cx).line_drag().is_some());
            if !drag_live {
                self.hold = HoldState::Idle;
            }
        }

        let mut body = div().flex().flex_1().min_h(px(0.));
        if let Some(editor) = &self.settings_view {
            // Settings take over everything below the titlebar. The hidden
            // panes' drop stores must not linger as invisible hit zones.
            self.sidebar_bounds.borrow_mut().take();
            self.calendar_drop_bounds.borrow_mut().clear();
            self.daily_drop_bounds.borrow_mut().clear();
            body = body.child(editor.clone());
        } else {
            // Writing is the focused layout: the note at a comfortable
            // measure, no sidebar.
            if self.sidebar_open && self.layout != LayoutMode::Writing {
                body = body.child(self.render_sidebar(&t, cx));
            } else {
                // No sidebar this frame: its drop targets must not linger as
                // invisible hit zones for an in-flight drag.
                self.sidebar_bounds.borrow_mut().take();
                self.calendar_drop_bounds.borrow_mut().clear();
                self.daily_drop_bounds.borrow_mut().clear();
            }
            body = body.child(self.render_main(&t, window, cx));
        }

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
            .on_action(cx.listener(|this, _: &LayoutNotes, w, cx| {
                this.set_layout(LayoutMode::NotesFull, w, cx)
            }))
            .on_action(cx.listener(|this, _: &LayoutSplit, w, cx| {
                this.set_layout(LayoutMode::Split, w, cx)
            }))
            .on_action(cx.listener(|this, _: &LayoutTerminal, w, cx| {
                this.set_layout(LayoutMode::TerminalFull, w, cx)
            }))
            .on_action(cx.listener(|this, _: &LayoutWriting, w, cx| {
                this.set_layout(LayoutMode::Writing, w, cx)
            }))
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
            .children(
                self.settings_view
                    .is_none()
                    .then(|| self.render_settings_fab(&t, cx)),
            )
            .children(self.render_statusbar(&t, cx))
            .children(self.render_picker(&t, window, cx))
            .children(self.render_notes_menu(&t, window, cx))
            .children(self.render_switcher(&t, cx))
            .children(self.render_capture(&t, cx))
            .children(self.render_drag_ghost(&t, cx))
            .children(self.render_hold_menu(&t, window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

impl Workspace {
    /// The way into Settings: a floating gear pinned to the window's
    /// bottom-left corner, above the pane content but under every overlay.
    fn render_settings_fab(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let hover_bg = t.hover;
        let hover_text = t.text;
        div()
            .id("settings-fab")
            .absolute()
            .left(px(10.))
            .bottom(px(10.))
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(t.panel2)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .text_size(t.ui_px(14.))
            .text_color(t.dim)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_text))
            .child("⚙")
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_settings(window, cx);
            }))
    }

    /// The hold-for-heading menu: a floating card of the target day's
    /// headings while a drag dwells on its day cell. Rows carry no click
    /// handlers (the mouse button is down for the whole gesture) — their
    /// painted bounds are captured and the release hit-tests them.
    fn render_hold_menu(
        &self,
        t: &theme::KairnTheme,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let HoldState::Open(menu) = &self.hold else { return None };
        let (_, _, position) = self.note_editor.as_ref()?.read(cx).line_drag()?;
        let hovered = menu
            .item_bounds
            .borrow()
            .iter()
            .find(|(_, b)| b.contains(&position))
            .map(|(idx, _)| *idx);
        menu.item_bounds.borrow_mut().clear();

        const MENU_W: f32 = 230.;
        let row_h = 26.;
        let shown = menu.items.len().min(HOLD_MENU_CAP);
        let truncated = menu.items.len() - shown;
        let approx_h = px(12. + row_h * (shown + 1 + usize::from(truncated > 0)) as f32);
        let viewport = window.viewport_size();
        let x = menu.origin.x.min(viewport.width - px(MENU_W + 8.)).max(px(8.));
        let y = menu.origin.y.min(viewport.height - approx_h - px(8.)).max(px(8.));

        let menu_store = menu.menu_bounds.clone();
        let mut card = div()
            .absolute()
            .left(x)
            .top(y)
            .w(px(MENU_W))
            .py(px(6.))
            .rounded(px(10.))
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .text_size(t.ui_px(12.))
            .text_color(t.dim)
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        menu_store.borrow_mut().replace(bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        let row = |key: usize, indent: f32, label: String, faint: bool| {
            let bounds_store = menu.item_bounds.clone();
            let is_hover = hovered == Some(key);
            div()
                .relative()
                .h(px(row_h))
                .flex()
                .items_center()
                .mx(px(5.))
                .pl(px(9. + indent))
                .pr(px(9.))
                .rounded(px(6.))
                .when(is_hover, |d| d.bg(t.sel).text_color(t.accent))
                .when(faint, |d| d.text_color(t.faint))
                .child(
                    gpui::canvas(
                        move |bounds, _, _| bounds_store.borrow_mut().push((key, bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(label)
        };

        card = card.child(row(TOP_OF_NOTE, 0., "↑  Top of note".into(), false));
        let min_level = menu.items.iter().map(|i| i.level).min().unwrap_or(1);
        for item in menu.items.iter().take(HOLD_MENU_CAP) {
            let indent = f32::from(item.level.saturating_sub(min_level)) * 10.;
            card = card.child(row(item.line_idx, indent, item.display.clone(), false));
        }
        if truncated > 0 {
            card = card.child(
                div()
                    .h(px(row_h))
                    .flex()
                    .items_center()
                    .pl(px(14.))
                    .text_color(t.faint)
                    .child(format!("… {truncated} more")),
            );
        }
        Some(card)
    }

    /// The floating ghost that follows a line drag: a small card with the
    /// block's first line, offset from the pointer so it never sits under it
    /// (which would block the drop targets' hit-testing).
    fn render_drag_ghost(
        &self,
        t: &theme::KairnTheme,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let (line, extra, position) = self.note_editor.as_ref()?.read(cx).line_drag()?;
        let text = line.trim_start();
        let is_task = ["* ", "+ "].iter().any(|m| text.starts_with(m));
        let text = ["* ", "+ ", "- ", "> "]
            .iter()
            .find_map(|m| text.strip_prefix(m))
            .unwrap_or(text)
            .trim_start();
        let text = text.strip_prefix("[ ]").map(str::trim_start).unwrap_or(text);
        let text = text.trim_start_matches('#').trim_start();
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
                .when(is_task, |d| {
                    d.child(
                        div()
                            .w(px(10.))
                            .h(px(10.))
                            .flex_none()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(t.faint),
                    )
                })
                .child(text)
                .when(extra > 0, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(4.))
                            .bg(t.sel)
                            .text_size(t.ui_px(10.))
                            .text_color(t.faint)
                            .child(format!("+{extra}")),
                    )
                }),
        )
    }
}
