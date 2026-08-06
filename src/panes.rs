use chrono::{Datelike, Days, Local};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, HighlightStyle, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, StyledText, Window, div, px, relative,
};
use gpui_component::input::Input;
use gpui_component::resizable::{h_resizable, resizable_panel};

use crate::notes::{self, Line, Span, SpanKind, TaskState};
use crate::theme::{self, KairnTheme};
use crate::workspace::{LayoutMode, PaneView, TaskQuery, Workspace, chord, kbd, mod_symbol};

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
        let pane = div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(self.render_week_strip(t, cx));

        // Writing mode with a live editor: the raw markdown, autosaved.
        if writing && let Some(editor) = self.editor.clone() {
            return pane.child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .px(px(24.))
                    .py(px(20.))
                    .child(Input::new(&editor).h_full().appearance(false)),
            );
        }

        pane.child(
            div()
                .id("note-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
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

            let mut dots_row = div().h(px(5.)).mt(px(1.)).flex().gap(px(2.)).justify_center();
            for _ in 0..dots {
                dots_row = dots_row.child(
                    div()
                        .w(px(3.5))
                        .h(px(3.5))
                        .rounded_full()
                        .bg(if is_selected { t.on_amber.opacity(0.6) } else { t.faint }),
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
                let subline = match relative_label {
                    Some(label) => format!("Week {} · {}", date.iso_week().week(), label),
                    None => format!("Week {}", date.iso_week().week()),
                };
                (masthead, subline, "No note for this day yet.")
            }
        };

        let mut note = note_frame(t, writing, masthead, subline);
        let editing_idx = self.line_edit.as_ref().map(|le| le.line_idx);

        match &self.doc_lines {
            None => {
                if editing_idx == Some(0) {
                    note = note.child(self.render_line_editor());
                } else {
                    note = note.child(
                        div()
                            .id("empty-note")
                            .mt(px(10.))
                            .text_color(t.faint)
                            .cursor_text()
                            .child(empty_text)
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.edit_line(0, window, cx);
                            })),
                    );
                }
            }
            Some(lines) => {
                for (idx, line) in lines.iter().enumerate() {
                    if editing_idx == Some(idx) {
                        note = note.child(self.render_line_editor());
                    } else {
                        note = note.child(clickable_line(idx, render_line(t, idx, line, cx), cx));
                    }
                }
                if editing_idx == Some(lines.len()) {
                    note = note.child(self.render_line_editor());
                }
            }
        }

        // Clicking the space under the note starts a new line at the end.
        note = note.child(div().id("note-append").h(px(140.)).on_click(cx.listener(
            |this, _, window, cx| {
                cx.stop_propagation();
                this.edit_line(usize::MAX, window, cx);
            },
        )));

        note.child(
            div()
                .mt(px(4.))
                .flex()
                .gap(px(6.))
                .items_center()
                .text_size(px(11.5))
                .text_color(t.faint)
                .child(kbd(t, format!("⌥{}⏎", mod_symbol())))
                .child("writing mode · click any line to edit in place"),
        )
        .into_any_element()
    }

    /// The single-line input standing in for the line being edited.
    fn render_line_editor(&self) -> AnyElement {
        let Some(le) = &self.line_edit else {
            return div().into_any_element();
        };
        div()
            .py(px(1.))
            .child(Input::new(&le.input).appearance(false).w_full())
            .into_any_element()
    }

    fn render_task_view(
        &self,
        t: &KairnTheme,
        query: TaskQuery,
        writing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tasks: Vec<notes::TaskRef> = self.tasks_for(query).cloned().collect();
        let subline = format!("{} open", tasks.len());
        let mut note = note_frame(t, writing, query.title().to_string(), subline);

        if tasks.is_empty() {
            note = note.child(div().mt(px(10.)).text_color(t.faint).child("Nothing here."));
        }

        let sel = t.sel;
        let dim = t.dim;
        for (i, task) in tasks.into_iter().enumerate() {
            let date = task.date;
            let date_label = format!("{} {}", date.format("%-d"), date.format("%b"));
            let spans = task.spans.clone();
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
                        this.select_day(date, cx);
                    }))
                    .child(checkbox)
                    .child(div().flex_1().min_w(px(0.)).child(spans_el(t, &spans, t.text)))
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

fn section_heading(t: &KairnTheme, label: String) -> impl IntoElement {
    div()
        .mt(px(18.))
        .mb(px(8.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(label.to_uppercase())
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

/// Wrap a rendered line so clicking it starts editing it in place.
fn clickable_line(idx: usize, inner: AnyElement, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id(("line", idx))
        .cursor_text()
        .child(inner)
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.edit_line(idx, window, cx);
        }))
        .into_any_element()
}

fn render_line(
    t: &KairnTheme,
    idx: usize,
    line: &Line,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match line {
        Line::Heading { level, spans } => {
            if *level == 1 {
                div()
                    .mt(px(18.))
                    .mb(px(6.))
                    .font_family(theme::serif_font())
                    .text_size(px(19.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(spans_el(t, spans, t.text))
                    .into_any_element()
            } else {
                section_heading_spans(t, spans).into_any_element()
            }
        }
        Line::Task { state, spans } => task_row(t, idx, *state, spans, cx).into_any_element(),
        Line::Bullet { spans } => div()
            .flex()
            .gap(px(9.))
            .py(px(2.5))
            .child(div().text_color(t.faint).child("–"))
            .child(div().flex_1().min_w(px(0.)).child(spans_el(t, spans, t.dim)))
            .into_any_element(),
        Line::Quote { spans } => div()
            .my(px(4.))
            .pl(px(12.))
            .border_l_2()
            .border_color(t.border)
            .child(spans_el(t, spans, t.dim))
            .into_any_element(),
        Line::Rule => div().my(px(14.)).h(px(1.)).bg(t.border).into_any_element(),
        Line::Blank => div().h(px(8.)).into_any_element(),
        Line::Text { spans } => div()
            .py(px(1.))
            .child(spans_el(t, spans, t.dim))
            .into_any_element(),
    }
}

fn section_heading_spans(t: &KairnTheme, spans: &[Span]) -> impl IntoElement {
    let label: String = spans.iter().map(|(_, s)| s.as_str()).collect();
    section_heading(t, label)
}

fn task_row(
    t: &KairnTheme,
    idx: usize,
    state: TaskState,
    spans: &[Span],
    cx: &mut Context<Workspace>,
) -> gpui::Div {
    let box_base = div()
        .id(("task-box", idx))
        .w(px(13.))
        .h(px(13.))
        .flex_none()
        .rounded(px(4.));
    // Open and done boxes toggle on click; scheduled and cancelled stay
    // inert until full editing.
    let box_base = if matches!(state, TaskState::Open | TaskState::Done) {
        box_base
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_task(idx, cx);
            }))
    } else {
        box_base
    };
    let dim = t.dim;
    let box_el = match state {
        TaskState::Done => box_base.bg(t.accent).flex().items_center().justify_center().child(
            div()
                .text_size(px(9.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(t.bg)
                .child("✓"),
        ),
        TaskState::Cancelled => box_base
            .border_1()
            .border_color(t.faint)
            .flex()
            .items_center()
            .justify_center()
            .child(div().text_size(px(8.)).text_color(t.faint).child("✕")),
        TaskState::Scheduled => box_base
            .border_1()
            .border_color(t.faint)
            .flex()
            .items_center()
            .justify_center()
            .child(div().text_size(px(8.)).text_color(t.faint).child("›")),
        TaskState::Open => box_base
            .border_1()
            .border_color(t.faint)
            .hover(move |s| s.border_color(dim)),
    };

    let struck = matches!(state, TaskState::Done | TaskState::Cancelled);
    let base_color = match state {
        TaskState::Open => t.text,
        TaskState::Scheduled => t.dim,
        TaskState::Done | TaskState::Cancelled => t.faint,
    };
    let text = div()
        .flex_1()
        .min_w(px(0.))
        .when(struck, |d| d.line_through())
        .child(spans_el(t, spans, base_color));

    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .py(px(2.5))
        .rounded(px(6.))
        .hover(|s| s.bg(t.sel))
        .child(box_el.mt(px(4.)))
        .child(text)
}

/// Inline fragments as one wrapping text element, tinted per span kind.
fn spans_el(t: &KairnTheme, spans: &[Span], base_color: gpui::Hsla) -> gpui::Div {
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for (kind, s) in spans {
        let start = text.len();
        text.push_str(s);
        let style = match kind {
            SpanKind::Text => None,
            SpanKind::WikiLink => Some(HighlightStyle {
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
        };
        if let Some(style) = style {
            highlights.push((start..text.len(), style));
        }
    }
    div()
        .text_color(base_color)
        .child(StyledText::new(text).with_highlights(highlights))
}

