use chrono::{Datelike, Days, Local};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, ElementId, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledText, Window, div, px,
    relative,
};
use gpui_component::h_flex;
use gpui_component::resizable::{h_resizable, resizable_panel};

use kairn_core::{FileKind, Span, SpanKind, file_kind, task_priority};
use crate::sidebar::REVEAL_LABEL;
use crate::theme::KairnTheme;
use crate::workspace::{LayoutMode, PaneView, TaskQuery, Workspace, chord};

impl Workspace {
    pub fn render_main(
        &self,
        t: &KairnTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let container = div().flex_1().min_w(px(0.)).min_h(px(0.));
        match self.layout {
            LayoutMode::TerminalFull => container.child(self.render_terminal_pane(t, cx)),
            LayoutMode::Writing => container.child(self.render_note_pane(t, true, cx)),
            LayoutMode::NotesFull => container.child(self.render_note_pane(t, false, cx)),
            LayoutMode::Split => container.child(
                h_resizable("main-split")
                    .child(
                        resizable_panel()
                            .size(px(560.))
                            .size_range(px(360.)..px(960.))
                            .child(self.render_note_pane(t, false, cx).into_any_element()),
                    )
                    .child(
                        resizable_panel().child(self.render_terminal_pane(t, cx).into_any_element()),
                    ),
            ),
        }
    }

    fn render_terminal_pane(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div().size_full().min_w(px(0.)).min_h(px(0.)).bg(t.term_bg);
        match self.sessions.get(self.active_session) {
            Some(session) => pane.child(session.view.clone()),
            None => pane.child(self.render_start_page(t, cx)),
        }
    }

    /// The start page: with no session open (including on launch, which no
    /// longer auto-starts a shell), the terminal pane lists everything that
    /// can be started — the local shell, then each saved shortcut and SSH
    /// host — as one calm click-to-open column.
    fn render_start_page(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::session::SessionKind;

        let group = |label: SharedString| {
            div()
                .mt(px(14.))
                .mb(px(2.))
                .px(px(12.))
                .text_size(t.ui_px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(t.faint)
                .child(label.to_uppercase())
        };
        let hover_bg = t.hover;
        let hover_text = t.text;
        let row = move |id: ElementId| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(7.))
                .rounded(px(7.))
                .text_color(t.dim)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        };
        let detail = |text: SharedString| {
            div().text_size(t.ui_px(11.)).text_color(t.faint).child(text)
        };

        let shell = std::env::var("SHELL").unwrap_or_default();
        let shell_name: SharedString = std::path::PathBuf::from(&shell)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "shell".into())
            .into();

        let mut col = div()
            .w(px(340.))
            .flex()
            .flex_col()
            .child(group("This machine".into()))
            .child(
                row("start-shell".into())
                    .child(div().flex_1().child("New shell"))
                    .child(detail(shell_name))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.spawn_session(SessionKind::Local, window, cx);
                    })),
            );
        for (i, app) in self.settings.local_apps.iter().enumerate() {
            let kind = SessionKind::App { host: None, app: app.clone() };
            col = col.child(
                row(("start-local-app", i).into())
                    .child(div().flex_1().child(app.display_name()))
                    .child(detail(app.command.clone().into()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.spawn_session(kind.clone(), window, cx);
                    })),
            );
        }
        for (i, host) in self.settings.ssh_hosts.iter().enumerate() {
            col = col.child(group(host.name.clone().into()));
            let kind = SessionKind::Ssh(host.clone());
            col = col.child(
                row(("start-host", i).into())
                    .child(div().flex_1().child("Shell"))
                    .child(detail(host.target.clone().into()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.spawn_session(kind.clone(), window, cx);
                    })),
            );
            for (j, app) in host.apps.iter().enumerate() {
                let kind = SessionKind::App {
                    host: Some(host.clone()),
                    app: app.clone(),
                };
                col = col.child(
                    row(("start-host-app", i * 64 + j).into())
                        .child(div().flex_1().child(app.display_name()))
                        .child(detail(app.command.clone().into()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.spawn_session(kind.clone(), window, cx);
                        })),
                );
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(t.ui_px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child("Start a session"),
            )
            .child(col)
            .child(
                div()
                    .mt(px(18.))
                    .text_size(t.ui_px(11.))
                    .text_color(t.faint)
                    .child(format!(
                        "{} starts a shell · shortcuts live in Settings",
                        chord("N")
                    )),
            )
    }

    fn render_note_pane(
        &self,
        t: &KairnTheme,
        writing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // A configured notes folder that isn't there blocks the pane:
        // rendering an empty vault (and writing into a fresh one at the
        // mount point) would look exactly like data loss.
        if self.root_missing {
            return div()
                .size_full()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(t.bg)
                .child(
                    div()
                        .text_size(t.ui_px(15.))
                        .text_color(t.text)
                        .child("Notes folder unavailable"),
                )
                .child(
                    div()
                        .text_size(t.ui_px(12.))
                        .text_color(t.faint)
                        .child(self.notes_root.display().to_string()),
                )
                .child(
                    div()
                        .text_size(t.ui_px(12.))
                        .text_color(t.dim)
                        .child("Check the drive, or choose a folder in Settings."),
                );
        }

        // The week strip shows per the setting: everywhere, only over daily
        // notes, or never.
        let strip_on = match self.settings.week_strip.as_str() {
            "off" => false,
            "daily" => matches!(self.view, PaneView::Day),
            _ => true,
        };
        let mut pane = div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(t.bg)
            // The notes font: everything in the pane (masthead, editor,
            // mentions) follows it; unset inherits the UI font.
            .when_some(t.editor_font.clone(), |d, f| d.font_family(f));
        if strip_on {
            pane = pane.child(self.render_week_strip(t, writing, cx));
        } else {
            // No strip, no drop targets: stale cell bounds from a previous
            // frame must not catch a task drag released up there.
            self.week_strip_bounds.borrow_mut().clear();
        }

        // The single-buffer editor scrolls this container to keep the caret
        // visible, so it needs the handle the container tracks.
        let editor_scroll = self.note_editor.as_ref().map(|e| e.read(cx).scroll_handle.clone());
        pane.child(
            div()
                .id("note-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .when_some(editor_scroll, |d, h| d.track_scroll(&h))
                .child(self.render_note(t, writing, cx)),
        )
    }

    fn render_week_strip(
        &self,
        t: &KairnTheme,
        writing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let today = Local::now().date_naive();
        let selected = self.selected_day;
        let monday = selected - Days::new(selected.weekday().num_days_from_monday() as u64);

        // Drag-to-a-day: while a line drag is in flight, the day under the
        // pointer rings as the drop target (hit-tested against the cells'
        // last-painted bounds, which a drag can't move).
        let drag_pos = self
            .note_editor
            .as_ref()
            .and_then(|e| e.read(cx).line_drag())
            .map(|(_, _, position)| position);
        let drop_day = drag_pos.and_then(|position| {
            self.week_strip_bounds
                .borrow()
                .iter()
                .find(|(_, bounds)| bounds.contains(&position))
                .map(|(day, _)| *day)
        });
        self.week_strip_bounds.borrow_mut().clear();

        // The strip's content sits inside the note's own side padding (38px,
        // see `note_frame`) and, in the Writing layout, inside the note's
        // centred 720px measure, so the days always start and end where the
        // note text does instead of stretching wall to wall.
        // The day cards' height scales with the UI font, so the strip must
        // too: a fixed 48px let the amber selected pill poke past the strip
        // at larger UI sizes. Fixed padding terms plus ui-scaled text terms,
        // with the cards' line heights pinned below.
        let mut strip = h_flex()
            .w_full()
            .h_full()
            .items_center()
            .gap(px(4.))
            .px(px(38.))
            .when(writing, |d| d.max_w(px(720.)))
            .child(
                week_nav(t, "week-prev", "‹").on_click(cx.listener(move |this, _, _, cx| {
                    this.select_day(this.selected_day - Days::new(7), cx);
                })),
            );

        for i in 0..7u64 {
            let day = monday + Days::new(i);
            let is_today = day == today;
            let is_selected = day == selected;
            let stats = self.week_stats[i as usize];
            let overdue_open = stats.open > 0 && day < today;
            // Same ring/tick indicator as the calendar; on the selected day it
            // sits on the amber pill, so its colours flip to read against amber.
            let base = if is_selected { t.on_amber } else { t.faint };
            let indicator = div()
                .h(px(9.))
                .mt(px(1.))
                .flex()
                .items_center()
                .justify_center()
                .children(crate::ui::day_task_indicator(
                    t,
                    stats,
                    base,
                    overdue_open,
                    is_selected,
                ));

            let hover_bg = t.hover;
            let is_drop = drop_day == Some(day);
            let bounds_store = self.week_strip_bounds.clone();
            strip = strip.child(
                div()
                    .id(("week-day", i as usize))
                    .relative()
                    .flex_1()
                    .py(px(3.))
                    .rounded(px(8.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .when_else(
                        is_selected,
                        |d| d.bg(t.amber).text_color(t.on_amber),
                        |d| d.text_color(t.dim).hover(move |s| s.bg(hover_bg)),
                    )
                    .when(is_drop, |d| {
                        d.bg(t.sel).text_color(t.accent).border_2().border_color(t.accent)
                    })
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| bounds_store.borrow_mut().push((day, bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .text_size(t.ui_px(9.))
                            .line_height(t.ui_px(11.))
                            .text_color(if is_drop {
                                t.accent
                            } else if is_selected {
                                t.on_amber
                            } else {
                                t.faint
                            })
                            .child(day.format("%a").to_string().to_uppercase()),
                    )
                    .child(
                        div()
                            .text_size(t.ui_px(12.5))
                            .line_height(t.ui_px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(is_today && !is_selected, |d| d.text_color(t.amber))
                            .child(day.format("%-d").to_string()),
                    )
                    .child(indicator)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_day(day, cx);
                    })),
            );
        }

        let strip = strip.child(
            week_nav(t, "week-next", "›").on_click(cx.listener(move |this, _, _, cx| {
                this.select_day(this.selected_day + Days::new(7), cx);
            })),
        );

        div()
            .h(px(16.) + t.ui_px(34.))
            .flex_none()
            .flex()
            .justify_center()
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .child(strip)
    }

    fn render_note(
        &self,
        t: &KairnTheme,
        writing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let PaneView::Tasks(query) = self.view {
            return self.render_task_view(t, query, writing, cx);
        }

        let today = Local::now().date_naive();
        let (masthead, subline, empty_text) = match &self.view {
            PaneView::Library(path) => {
                let title = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                (title, self.library_subline(path), "Couldn't read this file.")
            }
            PaneView::Note(path) => {
                let title = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let folder = path
                    .parent()
                    .and_then(|p| p.strip_prefix(&self.notes_root).ok())
                    .map(|p| p.display().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Notes".to_string());
                (title, folder, "Couldn't read this note.")
            }
            PaneView::Week => {
                let monday = self.selected_day
                    - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
                let sunday = monday + Days::new(6);
                let iso = self.selected_day.iso_week();
                let masthead = format!("Week {}, {}", iso.week(), iso.year());
                // "11 – 17 August 2026", or spelled out across a boundary.
                let subline = if monday.month() == sunday.month() {
                    format!(
                        "{} – {} {} {}",
                        monday.format("%-d"),
                        sunday.format("%-d"),
                        sunday.format("%B"),
                        sunday.year()
                    )
                } else {
                    format!(
                        "{} {} – {} {} {}",
                        monday.format("%-d"),
                        monday.format("%B"),
                        sunday.format("%-d"),
                        sunday.format("%B"),
                        sunday.year()
                    )
                };
                (masthead, subline, "No note for this week yet.")
            }
            PaneView::Month => {
                let date = self.selected_day;
                let masthead = format!("{} {}", date.format("%B"), date.year());
                (masthead, "Monthly note".to_string(), "No note for this month yet.")
            }
            _ => {
                let date = self.selected_day;
                let masthead = format!(
                    "{}, {} {}",
                    date.format("%A"),
                    date.format("%-d"),
                    date.format("%B")
                );
                let mut subline = format!("Week {}", date.iso_week().week());
                // Open tasks from earlier days are still carried into this
                // one; the count keeps the masthead honest about the load.
                let carried = self.open_tasks.iter().filter(|task| task.due < date).count();
                if carried > 0 {
                    subline.push_str(&format!(" · {carried} carried over"));
                }
                (masthead, subline, "No note for this day yet.")
            }
        };
        // The relative day sits in the masthead itself, NotePlan-style, so
        // "where am I" reads at a glance.
        let badge = match &self.view {
            PaneView::Day => match (self.selected_day - today).num_days() {
                0 => Some("Today"),
                1 => Some("Tomorrow"),
                -1 => Some("Yesterday"),
                _ => None,
            },
            PaneView::Week => (self.selected_day.iso_week() == today.iso_week())
                .then_some("This week"),
            PaneView::Month => (self.selected_day.year() == today.year()
                && self.selected_day.month() == today.month())
            .then_some("This month"),
            _ => None,
        };

        // Regular notes and library markdown open as just the document: the
        // file's own `# title` line is the title (editing it still renames
        // the file), so a pane masthead on top would read as a double title.
        // Day/week/month views keep their masthead (their files carry no
        // title line), as do non-markdown library files (the metadata card
        // needs a name).
        let bare = match &self.view {
            PaneView::Note(_) => true,
            PaneView::Library(path) => file_kind(path) == FileKind::Markdown,
            _ => false,
        };
        let mut note = if bare {
            note_frame_bare(writing)
        } else {
            note_frame(t, writing, masthead, badge, subline)
        };
        if let Some(banner) = self.render_orphan_banner(t, cx) {
            note = note.child(banner);
        }
        for banner in self.render_conflict_banners(t, cx) {
            note = note.child(banner);
        }
        // Non-markdown library files render by kind instead of through the
        // editor: an inline image, or a metadata card whose open/reveal
        // actions cover everything Kairn doesn't render itself.
        if let PaneView::Library(path) = &self.view
            && file_kind(path) != FileKind::Markdown
        {
            return note
                .child(self.render_library_file(t, path.clone(), cx))
                .into_any_element();
        }
        // The document body is the editor entity; masthead, banners, and
        // mentions stay with the pane. No editor means the note can't be
        // edited here: an unreadable file or a missing notes root.
        if let Some(editor) = &self.note_editor {
            return note
                .child(editor.clone())
                .child(self.render_mentions(t, cx))
                .into_any_element();
        }

        if let Some(err) = &self.doc_error {
            note = note.child(
                div()
                    .mt(px(10.))
                    .text_color(t.faint)
                    .child(format!("Couldn't read this note: {err}")),
            );
        } else {
            note = note.child(div().mt(px(10.)).text_color(t.faint).child(empty_text));
        }

        note.child(self.render_mentions(t, cx)).into_any_element()
    }

    /// Where a library file lives, for the pane subline: the root's name
    /// plus the folder path inside it, falling back to the parent directory
    /// when the file's root has just been removed.
    fn library_subline(&self, path: &std::path::Path) -> String {
        for (root, _) in &self.library_trees {
            if let Ok(rel) = path.strip_prefix(root) {
                let root_name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let parent = rel
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                return if parent.is_empty() {
                    root_name.to_string()
                } else {
                    format!("{root_name} / {parent}")
                };
            }
        }
        path.parent().map(|p| p.display().to_string()).unwrap_or_default()
    }

    /// A non-markdown library file's body, by kind: an inline image with a
    /// sibling gallery strip, a monospace editor for text/code, and a quiet
    /// metadata card for everything else. All carry open/reveal actions.
    fn render_library_file(
        &self,
        t: &KairnTheme,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui::StyledImage as _;
        use gpui_component::button::Button;

        let kind = file_kind(&path);
        let meta = std::fs::metadata(&path).ok();
        let mut facts: Vec<String> = Vec::new();
        facts.push(match kind {
            FileKind::Image => "Image".to_string(),
            FileKind::Text => "Text file".to_string(),
            FileKind::Markdown => "Markdown".to_string(),
            FileKind::Other => match path.extension().and_then(|x| x.to_str()) {
                Some(ext) => format!("{} file", ext.to_uppercase()),
                None => "File".to_string(),
            },
        });
        if let Some(meta) = &meta {
            facts.push(human_size(meta.len()));
            if let Ok(mtime) = meta.modified() {
                let dt: chrono::DateTime<Local> = mtime.into();
                facts.push(format!("modified {}", dt.format("%-d %b %Y, %H:%M")));
            }
        }

        let open_with = path.clone();
        let reveal = path.clone();
        let mut open_label = "Open in default app";
        if kind == FileKind::Text
            && matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("html") | Some("htm")
            )
        {
            // The stack has no webview by design; the browser is the preview.
            open_label = "Open in browser";
        }
        let actions = h_flex()
            .gap(px(8.))
            .child(
                Button::new("lib-open-default")
                    .outline()
                    .label(open_label)
                    .on_click(move |_, _, cx| cx.open_with_system(&open_with)),
            )
            .child(
                Button::new("lib-reveal")
                    .outline()
                    .label(REVEAL_LABEL)
                    .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
            );
        let facts_line = div()
            .text_size(t.ui_px(12.5))
            .text_color(t.dim)
            .child(facts.join(" · "));

        let mut body = div().mt(px(14.)).flex().flex_col().gap(px(12.));

        match kind {
            FileKind::Image => {
                body = body.child(
                    div()
                        .max_w_full()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.border)
                        .overflow_hidden()
                        .child(
                            gpui::img(path.clone())
                                .max_w_full()
                                .max_h(px(520.))
                                .object_fit(gpui::ObjectFit::ScaleDown),
                        ),
                );
                // The folder gallery: every sibling image as a thumbnail,
                // the open one ringed. This is the "pick image 1, 2 or 3"
                // flow — click through, compare, tell the agent.
                if self.library_siblings.len() > 1 {
                    let mut strip = div().flex().flex_wrap().gap(px(8.));
                    for (i, sib) in self.library_siblings.iter().enumerate() {
                        let current = *sib == path;
                        let target = sib.clone();
                        strip = strip.child(
                            div()
                                .id(("lib-thumb", i))
                                .w(px(104.))
                                .h(px(78.))
                                .rounded(px(6.))
                                .overflow_hidden()
                                .border_2()
                                .border_color(if current { t.accent } else { t.border })
                                .cursor_pointer()
                                .child(
                                    gpui::img(sib.clone())
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_library_file(target.clone(), window, cx);
                                })),
                        );
                    }
                    body = body
                        .child(strip)
                        .child(
                            div()
                                .text_size(t.ui_px(11.))
                                .text_color(t.faint)
                                .child(format!(
                                    "{} images in this folder",
                                    self.library_siblings.len()
                                )),
                        );
                }
                body = body.child(facts_line).child(actions);
            }
            FileKind::Text
                if self
                    .library_text
                    .as_ref()
                    .is_some_and(|ed| ed.path == path) =>
            {
                let ed = self.library_text.as_ref().expect("guarded above");
                body = body
                    .child(
                        div()
                            .font_family(t.mono_font.clone())
                            .text_size(t.ui_px(12.))
                            .child(gpui_component::input::Input::new(&ed.input).appearance(false)),
                    )
                    .child(facts_line)
                    .child(actions);
            }
            _ => {
                body = body.child(facts_line).child(actions);
            }
        }

        body.into_any_element()
    }

    /// A line edit whose target line vanished from the file before saving:
    /// the typed text is shown until the user restores or dismisses it,
    /// never silently dropped.
    fn render_orphan_banner(
        &self,
        t: &KairnTheme,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let (_, text) = self.orphaned.as_ref()?;
        let action = |label: &'static str, id: &'static str| {
            let hover = t.text;
            div()
                .id(id)
                .text_color(t.dim)
                .cursor_pointer()
                .hover(move |s| s.text_color(hover))
                .child(label)
        };
        Some(
            div()
                .mb(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(t.amber.opacity(0.5))
                .bg(t.amber.opacity(0.08))
                .text_size(t.ui_px(12.))
                .child(
                    div()
                        .text_color(t.dim)
                        .child("This line changed on disk before your edit could be saved:"),
                )
                .child(div().my(px(4.)).text_color(t.text).child(text.clone()))
                .child(
                    div()
                        .flex()
                        .gap(px(14.))
                        .text_size(t.ui_px(11.5))
                        .child(action("Put it back at the end of its note", "orphan-append").on_click(
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.resolve_orphan(true, cx);
                            }),
                        ))
                        .child(action("Discard", "orphan-discard").on_click(cx.listener(
                            |this, _, _, cx| {
                                cx.stop_propagation();
                                this.resolve_orphan(false, cx);
                            },
                        ))),
                ),
        )
    }

    /// One banner per Syncthing conflict copy of the current note: without
    /// this the conflicted version of a day is unreachable from every
    /// surface, which is silent data loss after a sync clash.
    fn render_conflict_banners(
        &self,
        t: &KairnTheme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.conflicts
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                let open = path.clone();
                let keep_mine = path.clone();
                let keep_copy = path.clone();
                let full_path = path.to_string_lossy().into_owned();
                let hover = t.text;
                let action = |label: &'static str, id: (&'static str, usize)| {
                    div()
                        .id(id)
                        .text_color(t.dim)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(hover))
                        .child(label)
                };
                div()
                    .mb(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.amber.opacity(0.5))
                    .bg(t.amber.opacity(0.08))
                    .text_size(t.ui_px(12.))
                    .child(div().text_color(t.dim).child(
                        "A sync conflict copy of this note exists; it may hold changes this version is missing. Resolving moves the losing file to the vault trash.",
                    ))
                    .child(div().mt(px(2.)).text_color(t.faint).child(name.clone()))
                    .child(
                        h_flex()
                            .mt(px(4.))
                            .gap(px(14.))
                            .text_size(t.ui_px(11.5))
                            .child(action("Open the copy", ("conflict-open", i)).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_note(open.clone(), cx);
                                }),
                            ))
                            .child(
                                action("Keep this version", ("conflict-keep-mine", i))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.resolve_conflict(&keep_mine, false, cx);
                                    })),
                            )
                            .child(
                                action("Use the copy instead", ("conflict-keep-copy", i))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.resolve_conflict(&keep_copy, true, cx);
                                    })),
                            )
                            .child(action("Copy path", ("conflict-copy-path", i)).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        full_path.clone(),
                                    ));
                                }),
                            )),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// Lines elsewhere that link here, at the foot of the note. Empty when
    /// nothing links in.
    fn render_mentions(&self, t: &KairnTheme, cx: &mut Context<Self>) -> AnyElement {
        if self.mentions.is_empty() {
            return div().into_any_element();
        }
        let mut section = div()
            .mb(px(10.))
            .child(
                div()
                    .mb(px(8.))
                    .text_size(t.ui_px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child(format!("LINKED MENTIONS · {}", self.mentions.len())),
            )
            .child(div().mb(px(8.)).h(px(1.)).bg(t.border));
        let sel = t.sel;
        for (i, mention) in self.mentions.iter().enumerate() {
            let name = mention.name.clone();
            let spans = spans_el(t, &mention.spans, t.dim);
            let mention = mention.clone();
            section = section.child(
                div()
                    .id(("mention", i))
                    .flex()
                    .items_start()
                    .gap(px(9.))
                    .py(px(2.5))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(sel))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_mention(&mention, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(1.))
                            .text_size(t.ui_px(11.))
                            .text_color(t.faint)
                            .child(name),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(spans)),
            );
        }
        section.into_any_element()
    }

    fn render_task_view(
        &self,
        t: &KairnTheme,
        query: TaskQuery,
        writing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.task_count(query);
        let subline = format!("{count} open");
        let mut note = note_frame(t, writing, query.title().to_string(), None, subline);

        if count == 0 {
            note = note.child(div().mt(px(10.)).text_color(t.faint).child("Nothing here."));
        }

        let sel = t.sel;
        let dim = t.dim;
        for (i, task) in self.tasks_for(query).enumerate() {
            let due = task.due;
            let date_label = format!("{} {}", due.format("%-d"), due.format("%b"));
            // A task from a regular note names its home and opens that
            // note on click; a daily task opens its day.
            let source_note = task
                .file_date
                .is_none()
                .then(|| task.path.clone())
                .map(|path| {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    (path, stem)
                });
            let nav_note = source_note.as_ref().map(|(path, _)| path.clone());
            // `!`-priority tasks run hot in the views too.
            let base = if task_priority(&task.spans) > 0 { t.red } else { t.text };
            let spans = spans_el(t, &task.spans, base);
            let task = task.clone();
            let checkbox = div()
                .id(("task-view-box", i))
                .w(px(13.))
                .h(px(13.))
                .flex_none()
                .mt(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(t.faint)
                .cursor_pointer()
                .hover(move |s| s.border_color(dim))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_task_ref(&task, cx);
                }));
            note = note.child(
                div()
                    .id(("task-view-row", i))
                    .flex()
                    .items_start()
                    .gap(px(9.))
                    .py(px(2.5))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(sel))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match &nav_note {
                            Some(path) => this.open_note(path.clone(), cx),
                            None => this.select_day(due, cx),
                        }
                    }))
                    .child(checkbox)
                    .child(div().flex_1().min_w(px(0.)).child(spans))
                    .when_some(source_note, |d, (_, stem)| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(t.ui_px(11.))
                                .text_color(t.faint)
                                .child(stem),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(t.ui_px(11.))
                            .text_color(t.faint)
                            .child(date_label),
                    ),
            );
        }

        note.into_any_element()
    }
}

/// The shared pane scaffold: serif masthead (with an optional relative-day
/// badge beside it), faint subline, rule.
fn note_frame(
    t: &KairnTheme,
    writing: bool,
    masthead: String,
    badge: Option<&'static str>,
    subline: String,
) -> gpui::Div {
    let mut head = h_flex()
        .items_center()
        .gap(px(10.))
        .mb(px(2.))
        .child(
            div()
                .text_size(t.ui_px(21.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(t.heading)
                .child(masthead),
        );
    if let Some(badge) = badge {
        head = head.child(
            div()
                .px(px(7.))
                .py(px(1.))
                .rounded(px(5.))
                .bg(t.amber.opacity(0.16))
                .text_size(t.ui_px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(t.amber)
                .child(badge.to_uppercase()),
        );
    }
    div()
        .px(px(38.))
        .pt(px(18.))
        .pb(px(26.))
        .line_height(relative(1.58))
        .when(writing, |d| d.max_w(px(720.)).mx_auto().pt(px(44.)))
        .child(head)
        .child(
            div()
                .text_size(t.ui_px(12.))
                .text_color(t.faint)
                .mb(px(4.))
                .child(subline),
        )
        .child(div().my(px(8.)).h(px(1.)).bg(t.border))
}

/// The pane scaffold without the masthead: padding and measure only, for
/// documents whose own `# title` line is the title.
fn note_frame_bare(writing: bool) -> gpui::Div {
    div()
        .px(px(38.))
        .pt(px(18.))
        .pb(px(26.))
        .line_height(relative(1.58))
        .when(writing, |d| d.max_w(px(720.)).mx_auto().pt(px(44.)))
}

/// Bytes as a short human figure: whole KB/MB below ten, one decimal above.
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.;
    const MB: f64 = 1024. * 1024.;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn week_nav(t: &KairnTheme, id: &'static str, glyph: &'static str) -> gpui::Stateful<gpui::Div> {
    let hover_text = t.text;
    div()
        .id(id)
        .px(px(3.))
        .text_size(t.ui_px(16.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_text))
        .child(glyph)
}

/// Inline fragments as one wrapping styled-text element, tinted per span
/// kind.
fn spans_text(t: &KairnTheme, spans: &[Span]) -> StyledText {
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for (kind, s) in spans {
        // Hidden spans are raw bytes the styled line does not render.
        if matches!(kind, SpanKind::Hidden) {
            continue;
        }
        let start = text.len();
        text.push_str(s);
        let style = match kind {
            SpanKind::Text => None,
            SpanKind::WikiLink | SpanKind::Link | SpanKind::Url | SpanKind::Time => {
                Some(HighlightStyle {
                    color: Some(t.accent),
                    ..Default::default()
                })
            }
            SpanKind::Tag | SpanKind::DateRef => Some(HighlightStyle {
                color: Some(t.amber),
                ..Default::default()
            }),
            SpanKind::Mention => Some(HighlightStyle {
                color: Some(t.faint),
                ..Default::default()
            }),
            SpanKind::Highlight => Some(HighlightStyle {
                color: Some(t.text),
                background_color: Some(t.amber.opacity(0.28)),
                ..Default::default()
            }),
            SpanKind::Bold => Some(HighlightStyle {
                font_weight: Some(gpui::FontWeight::BOLD),
                ..Default::default()
            }),
            SpanKind::Italic => Some(HighlightStyle {
                font_style: Some(gpui::FontStyle::Italic),
                ..Default::default()
            }),
            SpanKind::Hidden => Some(HighlightStyle {
                color: Some(t.faint),
                ..Default::default()
            }),
        };
        if let Some(style) = style {
            highlights.push((start..text.len(), style));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}

fn spans_el(t: &KairnTheme, spans: &[Span], base_color: gpui::Hsla) -> gpui::Div {
    div().text_color(base_color).child(spans_text(t, spans))
}


