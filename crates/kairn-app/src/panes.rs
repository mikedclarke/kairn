use chrono::{Datelike, Days, Local};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, HighlightStyle, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, StyledText, Window, div, px, relative,
};
use gpui_component::h_flex;
use gpui_component::resizable::{h_resizable, resizable_panel};

use kairn_core::{Span, SpanKind};
use crate::theme::{self, KairnTheme};
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

    fn render_terminal_pane(&self, t: &KairnTheme, _cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div().size_full().min_w(px(0.)).min_h(px(0.)).bg(t.term_bg);
        match self.sessions.get(self.active_session) {
            Some(session) => pane.child(session.view.clone()),
            None => pane
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_color(t.faint)
                .child("No session")
                .child(
                    div()
                        .text_size(px(11.5))
                        .child(format!("{} starts a new shell", chord("N"))),
                ),
        }
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
                        .text_size(px(15.))
                        .text_color(t.text)
                        .child("Notes folder unavailable"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.faint)
                        .child(self.notes_root.display().to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.dim)
                        .child("Check the drive, or choose a folder in Settings."),
                );
        }

        let pane = div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(self.render_week_strip(t, cx));

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

    fn render_week_strip(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let selected = self.selected_day;
        let monday = selected - Days::new(selected.weekday().num_days_from_monday() as u64);

        let mut strip = div()
            .h(px(62.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(16.))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .child(
                week_nav(t, "week-prev", "‹").on_click(cx.listener(move |this, _, _, cx| {
                    this.select_day(this.selected_day - Days::new(7), cx);
                })),
            );

        for i in 0..7u64 {
            let day = monday + Days::new(i);
            let is_today = day == today;
            let is_selected = day == selected;
            let dots = self.week_open_counts[i as usize].min(4);
            // Open tasks on a past day are overdue: those dots run hot.
            let dot_color = if is_selected {
                t.on_amber.opacity(0.6)
            } else if day < today {
                t.amber
            } else {
                t.faint
            };

            let mut dots_row = div().h(px(5.)).mt(px(1.)).flex().gap(px(2.)).justify_center();
            for _ in 0..dots {
                dots_row = dots_row.child(
                    div().w(px(3.5)).h(px(3.5)).rounded_full().bg(dot_color),
                );
            }

            let hover_bg = t.hover;
            strip = strip.child(
                div()
                    .id(("week-day", i as usize))
                    .flex_1()
                    .py(px(5.))
                    .rounded(px(9.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .when_else(
                        is_selected,
                        |d| d.bg(t.amber).text_color(t.on_amber),
                        |d| d.text_color(t.dim).hover(move |s| s.bg(hover_bg)),
                    )
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(if is_selected { t.on_amber } else { t.faint })
                            .child(day.format("%a").to_string().to_uppercase()),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(is_today && !is_selected, |d| d.text_color(t.amber))
                            .child(day.format("%-d").to_string()),
                    )
                    .child(dots_row)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_day(day, cx);
                    })),
            );
        }

        strip.child(
            week_nav(t, "week-next", "›").on_click(cx.listener(move |this, _, _, cx| {
                this.select_day(this.selected_day + Days::new(7), cx);
            })),
        )
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
            _ => {
                let date = self.selected_day;
                let masthead = format!(
                    "{}, {} {}",
                    date.format("%A"),
                    date.format("%-d"),
                    date.format("%B")
                );
                let relative_label = match (date - today).num_days() {
                    0 => Some("today"),
                    1 => Some("tomorrow"),
                    -1 => Some("yesterday"),
                    _ => None,
                };
                let mut subline = match relative_label {
                    Some(label) => format!("Week {} · {}", date.iso_week().week(), label),
                    None => format!("Week {}", date.iso_week().week()),
                };
                // Open tasks from earlier days are still carried into this
                // one; the count keeps the masthead honest about the load.
                let carried = self.open_tasks.iter().filter(|task| task.due < date).count();
                if carried > 0 {
                    subline.push_str(&format!(" · {carried} carried over"));
                }
                (masthead, subline, "No note for this day yet.")
            }
        };

        let mut note = note_frame(t, writing, masthead, subline);
        if let Some(banner) = self.render_orphan_banner(t, cx) {
            note = note.child(banner);
        }
        for banner in self.render_conflict_banners(t, cx) {
            note = note.child(banner);
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
                .text_size(px(12.))
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
                        .text_size(px(11.5))
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
                let hover = t.text;
                div()
                    .mb(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.amber.opacity(0.5))
                    .bg(t.amber.opacity(0.08))
                    .text_size(px(12.))
                    .child(div().text_color(t.dim).child(
                        "A sync conflict copy of this note exists; it may hold changes this version is missing.",
                    ))
                    .child(
                        h_flex()
                            .mt(px(4.))
                            .gap(px(14.))
                            .text_size(px(11.5))
                            .child(
                                div()
                                    .id(("conflict-open", i))
                                    .text_color(t.dim)
                                    .cursor_pointer()
                                    .hover(move |s| s.text_color(hover))
                                    .child("Open the copy")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_note(open.clone(), cx);
                                    })),
                            )
                            .child(div().text_color(t.faint).child(name)),
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
                    .text_size(px(11.))
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
                            .text_size(px(11.))
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
        let mut note = note_frame(t, writing, query.title().to_string(), subline);

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
            let spans = spans_el(t, &task.spans, t.text);
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
                                .text_size(px(11.))
                                .text_color(t.faint)
                                .child(stem),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(date_label),
                    ),
            );
        }

        note.into_any_element()
    }
}

/// The shared pane scaffold: serif masthead, faint subline, rule.
fn note_frame(t: &KairnTheme, writing: bool, masthead: String, subline: String) -> gpui::Div {
    div()
        .px(px(38.))
        .py(px(26.))
        .line_height(relative(1.58))
        .when(writing, |d| d.max_w(px(720.)).mx_auto().pt(px(44.)))
        .child(
            div()
                .font_family(theme::serif_font())
                .text_size(px(27.))
                .font_weight(gpui::FontWeight::BOLD)
                .mb(px(3.))
                .child(masthead),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.faint)
                .mb(px(12.))
                .child(subline),
        )
        .child(div().my(px(14.)).h(px(1.)).bg(t.border))
}

fn week_nav(t: &KairnTheme, id: &'static str, glyph: &'static str) -> gpui::Stateful<gpui::Div> {
    let hover_text = t.text;
    div()
        .id(id)
        .px(px(3.))
        .text_size(px(13.))
        .text_color(t.faint)
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
            SpanKind::WikiLink | SpanKind::Link | SpanKind::Url => Some(HighlightStyle {
                color: Some(t.accent),
                ..Default::default()
            }),
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
            SpanKind::Marker | SpanKind::Hidden => Some(HighlightStyle {
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


