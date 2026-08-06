use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Datelike, Days, Local, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, actions, div, point, px,
};
use gpui_component::{Root, TitleBar, WindowExt, h_flex};

use crate::notes;
use crate::session::{Session, SessionKind, spawn};
use crate::settings::Settings;
use crate::theme::{self, KairnTheme, KairnThemeExt, Mode};

actions!(
    kairn,
    [
        ToggleSidebar,
        ToggleTerminalFull,
        ToggleWriting,
        ToggleSwitcher,
        CloseOverlay,
        ToggleThemeMode,
        EditHosts,
        NewLocalSession,
        Quit,
        Session1,
        Session2,
        Session3,
        Session4,
        Session5,
        Session6,
        Session7,
        Session8,
        Session9
    ]
);

pub fn init(cx: &mut App) {
    // Primary chords: Cmd on macOS, Ctrl on Linux. On Linux, plain Ctrl+letter
    // combos are shell control characters (Ctrl+J accept-line, Ctrl+N
    // next-history, Ctrl+Q XON resume) and bindings win over the terminal, so
    // letter chords take Ctrl+Shift instead (the GNOME Terminal / VS Code
    // convention). Digits and punctuation stay plain Ctrl: the terminal emits
    // nothing for them, and shifted punctuation resolves to a different key
    // per layout (Ctrl+Shift+\ arrives as ctrl-|), so it can't be bound
    // reliably.
    let p = |k: &str| {
        if cfg!(target_os = "macos") {
            format!("cmd-{k}")
        } else if k.len() == 1 && k.chars().next().unwrap().is_ascii_alphabetic() {
            format!("ctrl-shift-{k}")
        } else {
            format!("ctrl-{k}")
        }
    };
    cx.bind_keys([
        KeyBinding::new(&p("\\"), ToggleSidebar, None),
        KeyBinding::new(&p("shift-enter"), ToggleTerminalFull, None),
        KeyBinding::new(&p("alt-enter"), ToggleWriting, None),
        KeyBinding::new(&p("j"), ToggleSwitcher, None),
        KeyBinding::new(&p(","), EditHosts, None),
        KeyBinding::new(&p("n"), NewLocalSession, None),
        KeyBinding::new(&p("q"), Quit, None),
        KeyBinding::new(&p("1"), Session1, None),
        KeyBinding::new(&p("2"), Session2, None),
        KeyBinding::new(&p("3"), Session3, None),
        KeyBinding::new(&p("4"), Session4, None),
        KeyBinding::new(&p("5"), Session5, None),
        KeyBinding::new(&p("6"), Session6, None),
        KeyBinding::new(&p("7"), Session7, None),
        KeyBinding::new(&p("8"), Session8, None),
        KeyBinding::new(&p("9"), Session9, None),
        KeyBinding::new("escape", CloseOverlay, Some("Overlay")),
    ]);
}

pub fn mod_symbol() -> &'static str {
    if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" }
}

/// Display label for a primary-modifier letter chord, matching `init`:
/// ⌘ on macOS, Ctrl+⇧ on Linux.
pub fn chord(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+⇧{key}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutMode {
    Split,
    TerminalFull,
    Writing,
}

pub struct Workspace {
    pub settings: Settings,
    focus_handle: FocusHandle,
    overlay_focus: FocusHandle,
    pub layout: LayoutMode,
    sidebar_open: bool,
    switcher_open: bool,
    picker_open: bool,
    picker_pos: Point<Pixels>,
    pub sessions: Vec<Session>,
    pub active_session: usize,
    next_session_id: u64,
    pub cal_offset: i32,
    pub notes_root: PathBuf,
    pub selected_day: NaiveDate,
    /// Parsed note for the selected day; `None` when no file exists.
    pub day_note: Option<Vec<notes::Line>>,
    /// The selected day's note as read from disk, line-aligned with
    /// `day_note`; toggles pass the rendered line back so a file that changed
    /// underneath is never clobbered.
    day_note_text: Option<String>,
    /// The file `day_note_text` was read from (`.md` or NotePlan's `.txt`).
    day_note_path: Option<PathBuf>,
    /// Days that have a daily note, for calendar indicators.
    pub note_days: HashSet<NaiveDate>,
    /// Open-task counts for Monday..Sunday of the selected day's week.
    pub week_open_counts: [usize; 7],
    _activity_timer: Task<()>,
    /// Watches the notes root so outside edits (agents, Syncthing, NotePlan
    /// elsewhere) appear without a restart. Dropped with the workspace.
    _notes_watcher: Option<notify::RecommendedWatcher>,
    _notes_watch_task: Task<()>,
}

impl Workspace {
    pub fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Sidebar status dots poll the PTY foreground process; tick a repaint
        // so they stay honest without any terminal event.
        let activity_timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        let notes_root = settings.notes_root();
        notes::ensure_layout(&notes_root);
        let (notes_watcher, notes_watch_task) = Self::watch_notes(notes_root.clone(), cx);

        let mut this = Self {
            settings,
            focus_handle: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            layout: LayoutMode::Split,
            sidebar_open: true,
            switcher_open: false,
            picker_open: false,
            picker_pos: point(px(0.), px(0.)),
            sessions: Vec::new(),
            active_session: 0,
            next_session_id: 1,
            cal_offset: 0,
            notes_root,
            selected_day: Local::now().date_naive(),
            day_note: None,
            day_note_text: None,
            day_note_path: None,
            note_days: HashSet::new(),
            week_open_counts: [0; 7],
            _activity_timer: activity_timer,
            _notes_watcher: notes_watcher,
            _notes_watch_task: notes_watch_task,
        };
        this.reload_notes();
        this.spawn_session(SessionKind::Local, window, cx);
        this
    }

    // ----- notes -----

    pub fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.selected_day = day;
        self.reload_notes();
        cx.notify();
    }

    /// Re-read the selected day's note and the calendar/week indicators.
    pub fn reload_notes(&mut self) {
        let path = notes::daily_file(&self.notes_root, self.selected_day);
        let text = path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
        self.day_note = text.as_deref().map(notes::parse);
        self.day_note_text = text;
        self.day_note_path = path;
        self.note_days = notes::days_with_notes(&self.notes_root);
        let monday = self.selected_day
            - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
        for (i, count) in self.week_open_counts.iter_mut().enumerate() {
            *count = notes::load_day(&self.notes_root, monday + Days::new(i as u64))
                .map(|t| notes::open_task_count(&t))
                .unwrap_or(0);
        }
    }

    /// Toggle the task on line `line_idx` of the selected day's note between
    /// open and done, writing the change back to the file.
    pub fn toggle_task(&mut self, line_idx: usize, cx: &mut Context<Self>) {
        let (Some(path), Some(text)) = (&self.day_note_path, &self.day_note_text) else {
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

    /// Watch the notes root recursively; any change outside `.kairn/` reloads
    /// the pane. Events are debounced briefly so an editor's save dance (or
    /// our own temp-file + rename write) causes one reload, not several.
    fn watch_notes(
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
                            .is_some_and(|n| n.to_string_lossy().ends_with(".kairn-tmp"))
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
        self.picker_open = false;
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
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
            if session.title != title {
                session.title = title;
                cx.notify();
            }
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

    // ----- overlays -----

    pub fn open_picker(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.picker_pos = pos;
        self.picker_open = true;
        self.switcher_open = false;
        cx.notify();
    }

    pub fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open || self.switcher_open {
            self.picker_open = false;
            self.switcher_open = false;
            self.focus_active_terminal(window, cx);
            cx.notify();
        }
    }

    // ----- action handlers -----

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_toggle_switcher(
        &mut self,
        _: &ToggleSwitcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switcher_open {
            self.close_overlays(window, cx);
        } else {
            self.switcher_open = true;
            self.picker_open = false;
            self.overlay_focus.focus(window);
            cx.notify();
        }
    }

    fn on_close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
    }

    fn on_toggle_theme(&mut self, _: &ToggleThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.mode().toggled();
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

    fn on_edit_hosts(&mut self, _: &EditHosts, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = false;
        crate::hosts_dialog::open(self, window, cx);
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

    // ----- chrome -----

    fn render_titlebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let jump_hint = h_flex()
            .id("jump-hint")
            .w(px(280.))
            .px(px(10.))
            .py(px(3.))
            .gap(px(6.))
            .rounded(px(7.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_size(px(12.))
            .text_color(t.faint)
            .cursor_pointer()
            .hover(|s| s.border_color(t.faint))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_switcher(&ToggleSwitcher, window, cx);
            }))
            .child(div().flex_1().child("Jump to session, day, or note"))
            .child(kbd(t, chord("J")));

        let capture_btn = titlebar_button(t, "capture-btn", cx).child(
            h_flex()
                .gap(px(6.))
                .child("Capture")
                .child(kbd(t, format!("{}⇧K", mod_symbol()))),
        );
        let capture_btn = capture_btn.on_click(cx.listener(|_, _, window, cx| {
            window.push_notification("Quick capture arrives with the notes phase.", cx);
        }));

        let theme_btn = titlebar_button(t, "theme-btn", cx)
            .child("◐")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_theme(&ToggleThemeMode, window, cx);
            }));

        let sidebar_btn = titlebar_button(t, "sidebar-btn", cx)
            .text_color(t.dim)
            .child("◧")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_sidebar(&ToggleSidebar, window, cx);
            }));

        TitleBar::new()
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(sidebar_btn)
                    .child(
                        h_flex()
                            .gap(px(7.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(13.))
                            .child(cairn_mark(t))
                            .child("Kairn"),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(8.))
                    .child(jump_hint)
                    .child(capture_btn)
                    .child(theme_btn),
            )
    }

    fn render_statusbar(&self, t: &KairnTheme, cx: &App) -> impl IntoElement {
        let running = self.sessions.iter().filter(|s| s.is_busy()).count();
        let m = mod_symbol();
        let hints = [
            format!("{m}\\ sidebar"),
            format!("{m}1–9 sessions"),
            format!("{} jump", chord("J")),
            format!("⇧{m}⏎ terminal"),
            format!("⌥{m}⏎ writing"),
        ];
        let _ = cx;
        h_flex()
            .h(px(26.))
            .flex_none()
            .px(px(14.))
            .gap(px(18.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border)
            .text_size(px(11.5))
            .text_color(t.dim)
            .child(
                h_flex()
                    .gap(px(5.))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(if running > 0 { t.accent } else { t.faint }),
                    )
                    .child(format!(
                        "{} session{}",
                        self.sessions.len(),
                        if self.sessions.len() == 1 { "" } else { "s" }
                    )),
            )
            .child(format!("{running} running"))
            .child(
                h_flex()
                    .flex_1()
                    .justify_end()
                    .gap(px(18.))
                    .text_color(t.faint)
                    .children(hints),
            )
    }

    // ----- overlays -----

    fn render_picker(
        &self,
        t: &KairnTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.picker_open {
            return None;
        }

        // Keep the menu inside the window when the anchor row sits near the
        // bottom edge.
        let item_count = 3 + self.settings.ssh_hosts.len().max(1);
        let est_height = px(item_count as f32 * 30.0 + 32.0);
        let viewport = window.viewport_size();
        let top = self
            .picker_pos
            .y
            .min(viewport.height - est_height - px(8.))
            .max(px(0.));

        let shell_name = std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::PathBuf::from(s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "shell".into());

        let mut menu = div()
            .absolute()
            .left(self.picker_pos.x)
            .top(top)
            // Without this, the mouse-down bubbles to the click-away backdrop
            // (and through it to the row underneath) and the menu dismisses
            // before its items can receive the click.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .min_w(px(232.))
            .p(px(5.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel2)
            .shadow_lg()
            .text_size(px(12.5))
            .child(
                picker_item(t, "picker-shell", cx)
                    .child(div().flex_1().child("New shell on this machine"))
                    .child(div().text_size(px(11.)).text_color(t.faint).child(shell_name))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.spawn_session(SessionKind::Local, window, cx);
                    })),
            )
            .child(picker_rule(t));

        if self.settings.ssh_hosts.is_empty() {
            menu = menu.child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_color(t.faint)
                    .child("No saved SSH hosts"),
            );
        } else {
            for (i, host) in self.settings.ssh_hosts.iter().enumerate() {
                let kind = SessionKind::Ssh(host.clone());
                menu = menu.child(
                    picker_item(t, ("picker-host", i), cx)
                        .child(div().flex_1().child(host.name.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.faint)
                                .child(host.target.clone()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.spawn_session(kind.clone(), window, cx);
                        })),
                );
            }
        }

        menu = menu.child(picker_rule(t)).child(
            picker_item(t, "picker-edit", cx)
                .text_color(t.dim)
                .child("Edit hosts…")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_edit_hosts(&EditHosts, window, cx);
                })),
        );

        Some(
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_overlays(window, cx);
                    }),
                )
                .child(menu),
        )
    }

    fn render_switcher(&self, t: &KairnTheme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.switcher_open {
            return None;
        }

        let today = chrono::Local::now();
        let day_label = format!(
            "{}, {} {}",
            today.format("%A"),
            today.format("%-d"),
            today.format("%B")
        );

        let mut card = div()
            .w(px(600.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .rounded(px(12.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel2)
            .shadow_lg()
            .overflow_hidden()
            .text_size(px(12.5))
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(13.))
                    .gap(px(10.))
                    .text_size(px(15.))
                    .text_color(t.faint)
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(px(2.)).h(px(16.)).bg(t.accent))
                    .child("Jump to session, day, or note"),
            )
            .child(switcher_section(t, "Sessions"));

        for (i, session) in self.sessions.iter().enumerate() {
            let busy = session.is_busy();
            let meta = match &session.kind {
                SessionKind::Local => {
                    if busy {
                        "local · running"
                    } else {
                        "local · idle"
                    }
                }
                SessionKind::Ssh(_) => "SSH · connected",
            };
            card = card.child(
                switcher_item(t, ("switcher-session", i), cx)
                    .child(
                        div()
                            .w(px(7.))
                            .h(px(7.))
                            .rounded_full()
                            .when_else(
                                busy,
                                |d| d.bg(t.accent),
                                |d| d.border_1().border_color(t.faint),
                            ),
                    )
                    .child(div().flex_1().child(session.label()))
                    .child(div().text_size(px(11.)).text_color(t.faint).child(meta))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_session(i, window, cx);
                        this.close_overlays(window, cx);
                    })),
            );
        }

        card = card
            .child(switcher_section(t, "Days"))
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(6.))
                    .gap(px(9.))
                    .text_color(t.dim)
                    .child(div().w(px(14.)).text_color(t.faint).child("◷"))
                    .child(div().flex_1().child(day_label))
                    .child(div().text_size(px(11.)).text_color(t.faint).child("today")),
            )
            .child(switcher_section(t, "Notes"))
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(6.))
                    .gap(px(9.))
                    .text_color(t.faint)
                    .child(div().w(px(14.)).child("≡"))
                    .child(div().flex_1().child("Search lands with the notes phase")),
            )
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(8.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(px(11.))
                    .text_color(t.faint)
                    .child("⏎ open")
                    .child("esc close"),
            );

        Some(
            div()
                .id("switcher-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(100.))
                .bg(gpui::rgba(0x0a0b0873))
                .track_focus(&self.overlay_focus)
                .key_context("Overlay")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_overlays(window, cx);
                    }),
                )
                .child(card),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.kairn().clone();

        let mut body = div().flex().flex_1().min_h(px(0.));
        if self.sidebar_open {
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
            .on_action(cx.listener(Self::on_edit_hosts))
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
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

// ----- small shared pieces -----

pub fn kbd(t: &KairnTheme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(theme::mono_font())
        .text_size(px(10.5))
        .text_color(t.faint)
        .border_1()
        .border_color(t.border)
        .rounded(px(4.))
        .px(px(4.))
        .bg(t.bg)
        .child(label.into())
}

fn cairn_mark(t: &KairnTheme) -> impl IntoElement {
    // The stacked-stones mark, drawn as bars so no asset pipeline is needed.
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(1.))
        .child(div().w(px(4.)).h(px(2.)).rounded_full().bg(t.text.opacity(0.35)))
        .child(div().w(px(7.)).h(px(2.5)).rounded_full().bg(t.text.opacity(0.5)))
        .child(div().w(px(10.)).h(px(3.)).rounded_full().bg(t.text.opacity(0.7)))
        .child(div().w(px(13.)).h(px(3.5)).rounded_full().bg(t.text.opacity(0.9)))
}

fn titlebar_button<T: 'static>(
    t: &KairnTheme,
    id: &'static str,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    div()
        .id(id)
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}

fn picker_item<T: 'static>(
    t: &KairnTheme,
    id: impl Into<gpui::ElementId>,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(6.))
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}

fn picker_rule(t: &KairnTheme) -> impl IntoElement {
    div().my(px(5.)).mx(px(4.)).h(px(1.)).bg(t.border)
}

fn switcher_section(t: &KairnTheme, label: &'static str) -> impl IntoElement {
    div()
        .px(px(16.))
        .pt(px(10.))
        .pb(px(3.))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(label.to_uppercase())
}

fn switcher_item<T: 'static>(
    t: &KairnTheme,
    id: impl Into<gpui::ElementId>,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.sel;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(16.))
        .py(px(6.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}

