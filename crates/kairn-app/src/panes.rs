use std::cell::RefCell;
use std::collections::HashMap;

use chrono::{Datelike, Days, Local};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, HighlightStyle, InteractiveElement,
    IntoElement, ParentElement, StatefulInteractiveElement, Styled, StyledText, TextLayout,
    Window, div, px, relative,
};
use gpui_component::h_flex;
use gpui_component::input::Input;
use gpui_component::resizable::{h_resizable, resizable_panel};

use kairn_core::{self as notes, Line, Span, SpanKind, TaskState};
use crate::theme::{self, KairnTheme, KairnThemeExt as _};
use crate::workspace::{
    InputDown, InputUp, LayoutMode, PaneView, TaskQuery, Workspace, chord,
};

/// Per-render stash of each note line's text layout, for click hit-testing.
type LineLayouts = RefCell<HashMap<usize, TextLayout>>;

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
                let carried = self.open_tasks.iter().filter(|task| task.date < date).count();
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
        // Single-buffer editor (dev flag): the document body is the editor
        // entity; masthead, banners, and mentions stay with the pane.
        if let Some(editor) = &self.note_editor {
            return note
                .child(editor.clone())
                .child(self.render_mentions(t, cx))
                .into_any_element();
        }

        let editing_idx = self.line_edit.as_ref().map(|le| le.line_idx);
        self.line_layouts.borrow_mut().clear();

        match &self.doc_lines {
            None => {
                if let Some(err) = &self.doc_error {
                    note = note.child(
                        div()
                            .mt(px(10.))
                            .text_color(t.faint)
                            .child(format!("Couldn't read this note: {err}")),
                    );
                } else if editing_idx == Some(0) {
                    note = note.child(self.render_line_editor(cx));
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
                let raw: Vec<String> = self
                    .doc_text
                    .as_deref()
                    .map(|text| text.lines().map(str::to_string).collect())
                    .unwrap_or_default();
                for (idx, line) in lines.iter().enumerate() {
                    if editing_idx == Some(idx) {
                        note = note.child(self.render_line_editor(cx));
                    } else {
                        let inner = clickable_line(
                            idx,
                            render_line(t, idx, line, &self.line_layouts, cx),
                            cx,
                        );
                        let draggable = !matches!(line, Line::Blank | Line::Rule);
                        note = note.child(draggable_row(
                            t,
                            idx,
                            raw.get(idx).cloned().unwrap_or_default(),
                            draggable,
                            inner,
                            cx,
                        ));
                    }
                }
                if editing_idx == Some(lines.len()) {
                    note = note.child(self.render_line_editor(cx));
                }
            }
        }

        // Clicking the space under the note starts a new line at the end;
        // dropping a dragged line here moves it to the end.
        let accent = t.accent;
        note = note.child(
            div()
                .id("note-append")
                .h(px(140.))
                .border_t_2()
                .border_color(gpui::transparent_black())
                .drag_over::<DragLine>(move |style, _, _, _| style.border_color(accent))
                .on_drop::<DragLine>(cx.listener(|this, drag: &DragLine, _, cx| {
                    this.drop_line(drag.idx, &drag.text, usize::MAX, cx);
                }))
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.edit_line(usize::MAX, window, cx);
                })),
        );

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

    /// The single-line input standing in for the line being edited. The
    /// wrapper owns the cross-line movement actions: the LineEdit* bindings
    /// match any focused input, but only here do they find a handler, so
    /// everywhere else they fall through to the input's normal behaviour.
    fn render_line_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(le) = &self.line_edit else {
            return div().into_any_element();
        };
        div()
            .py(px(1.))
            .on_action(cx.listener(|this, _: &InputUp, window, cx| {
                this.line_edit_vertical(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &InputDown, window, cx| {
                this.line_edit_vertical(1, window, cx);
            }))
            .on_action(cx.listener(Self::on_line_edit_left))
            .on_action(cx.listener(Self::on_line_edit_right))
            .on_action(cx.listener(Self::on_line_edit_backspace))
            .on_action(cx.listener(Self::on_line_edit_delete))
            // Strip the input's own metrics so the raw line sits exactly
            // where the rendered line was: same size, leading, and left
            // edge. Height stays automatic — the input grows with wrapped
            // paragraphs.
            .child(
                Input::new(&le.input)
                    .appearance(false)
                    .w_full()
                    .p_0()
                    .text_size(px(13.))
                    .line_height(relative(1.58)),
            )
            .into_any_element()
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
            let date = task.date;
            let date_label = format!("{} {}", date.format("%-d"), date.format("%b"));
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
                        this.select_day(date, cx);
                    }))
                    .child(checkbox)
                    .child(div().flex_1().min_w(px(0.)).child(spans))
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

/// A dragged note line: the source index plus the raw markdown, so the drop
/// applies a verified move that can never clobber a file that shifted.
#[derive(Clone)]
struct DragLine {
    idx: usize,
    text: String,
}

/// The floating preview while a line is dragged.
struct DragPreview(String);

impl gpui::Render for DragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.kairn().clone();
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(t.panel2)
            .border_1()
            .border_color(t.border)
            .text_size(px(12.))
            .text_color(t.dim)
            .max_w(px(420.))
            .overflow_hidden()
            .child(self.0.clone())
    }
}

/// Wrap a rendered line with its reorder affordances: a grab handle in the
/// left margin, visible while the row is hovered, and a drop target that
/// moves the dragged line to sit above this one.
fn draggable_row(
    t: &KairnTheme,
    idx: usize,
    raw: String,
    draggable: bool,
    inner: AnyElement,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let group = gpui::SharedString::from(format!("note-line-{idx}"));
    let accent = t.accent;
    let grip_hover = t.dim;
    let mut row = div()
        .id(("line-row", idx))
        .group(group.clone())
        .flex()
        .items_start()
        .border_t_2()
        .border_color(gpui::transparent_black())
        .drag_over::<DragLine>(move |style, _, _, _| style.border_color(accent))
        .on_drop::<DragLine>(cx.listener(move |this, drag: &DragLine, _, cx| {
            this.drop_line(drag.idx, &drag.text, idx, cx);
        }));
    if draggable {
        row = row.child(
            div()
                .id(("line-grip", idx))
                .flex_none()
                .w(px(16.))
                .ml(px(-16.))
                .pt(px(5.))
                .text_size(px(10.))
                .text_color(t.faint)
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .hover(move |s| s.text_color(grip_hover))
                .cursor_grab()
                .on_drag(DragLine { idx, text: raw }, |drag, _, _, cx| {
                    cx.new(|_| DragPreview(drag.text.clone()))
                })
                .child("⠿"),
        );
    }
    row.child(div().flex_1().min_w(px(0.)).child(inner))
        .into_any_element()
}

/// Wrap a rendered line so clicking it starts editing it in place, cursor
/// under the pointer.
fn clickable_line(idx: usize, inner: AnyElement, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id(("line", idx))
        .cursor_text()
        .child(inner)
        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            cx.stop_propagation();
            let pos = ev.mouse_position();
            // A click landing on a link navigates; anywhere else edits.
            if let Some((kind, text)) = pos.and_then(|p| this.line_click_link(idx, p)) {
                match kind {
                    SpanKind::WikiLink => {
                        let title = notes::wiki_link_title(&text).to_string();
                        this.open_wiki_link(&title, window, cx);
                        return;
                    }
                    SpanKind::DateRef => {
                        if let Some(date) = text
                            .strip_prefix('>')
                            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                        {
                            this.select_day(date, cx);
                            return;
                        }
                    }
                    _ => {}
                }
            }
            let col = pos.and_then(|p| this.line_click_col(idx, p));
            this.edit_line_at(idx, col, window, cx);
        }))
        .into_any_element()
}

fn render_line(
    t: &KairnTheme,
    idx: usize,
    line: &Line,
    layouts: &LineLayouts,
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
                    .child(spans_line(t, spans, t.text, idx, layouts))
                    .into_any_element()
            } else {
                section_heading_spans(t, spans, idx, layouts).into_any_element()
            }
        }
        Line::Task { state, spans } => {
            task_row(t, idx, *state, spans, layouts, cx).into_any_element()
        }
        Line::Bullet { spans } => div()
            .flex()
            .gap(px(9.))
            .py(px(2.5))
            .child(div().text_color(t.faint).child("–"))
            .child(div().flex_1().min_w(px(0.)).child(spans_line(t, spans, t.text, idx, layouts)))
            .into_any_element(),
        Line::Quote { spans } => div()
            .my(px(4.))
            .pl(px(12.))
            .border_l_2()
            .border_color(t.border)
            .child(spans_line(t, spans, t.dim, idx, layouts))
            .into_any_element(),
        Line::Rule => div().my(px(14.)).h(px(1.)).bg(t.border).into_any_element(),
        Line::Blank => div().h(px(8.)).into_any_element(),
        // Body text renders in the same color and size the editor uses, so
        // leaving an edit never dims or restyles what was just typed.
        Line::Text { spans } => div()
            .py(px(1.))
            .child(spans_line(t, spans, t.text, idx, layouts))
            .into_any_element(),
    }
}

fn section_heading_spans(
    t: &KairnTheme,
    spans: &[Span],
    idx: usize,
    layouts: &LineLayouts,
) -> impl IntoElement {
    let label: String = spans
        .iter()
        .filter(|(kind, _)| !matches!(kind, SpanKind::Hidden))
        .map(|(_, s)| s.as_str())
        .collect();
    let styled = StyledText::new(label.to_uppercase());
    layouts.borrow_mut().insert(idx, styled.layout().clone());
    div()
        .mt(px(18.))
        .mb(px(8.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(styled)
}

fn task_row(
    t: &KairnTheme,
    idx: usize,
    state: TaskState,
    spans: &[Span],
    layouts: &LineLayouts,
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
        .child(spans_line(t, spans, base_color, idx, layouts));

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

/// [`spans_el`] for a note line, recording its text layout for click
/// hit-testing.
fn spans_line(
    t: &KairnTheme,
    spans: &[Span],
    base_color: gpui::Hsla,
    idx: usize,
    layouts: &LineLayouts,
) -> gpui::Div {
    let styled = spans_text(t, spans);
    layouts.borrow_mut().insert(idx, styled.layout().clone());
    div().text_color(base_color).child(styled)
}

