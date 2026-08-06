use chrono::{Datelike, Days, Local, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, px, relative,
};
use gpui_component::resizable::{h_resizable, resizable_panel};

use crate::theme::{self, KairnTheme};
use crate::workspace::{LayoutMode, Workspace, chord, kbd, mod_symbol};

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
        div()
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(self.render_week_strip(t, cx))
            .child(
                div()
                    .id("note-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(render_note(t, writing)),
            )
    }

    fn render_week_strip(&self, t: &KairnTheme, _cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let monday = today - Days::new(today.weekday().num_days_from_monday() as u64);
        // Per-day task dots are stubbed until notes land (Phase C).
        let stub_dots: [usize; 7] = [2, 1, 3, 3, 1, 0, 0];

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
            .child(div().px(px(3.)).text_size(px(13.)).text_color(t.faint).child("‹"));

        for i in 0..7u64 {
            let day = monday + Days::new(i);
            let is_today = day == today;
            let dots = stub_dots[i as usize];

            let mut dots_row = div().h(px(5.)).mt(px(1.)).flex().gap(px(2.)).justify_center();
            for _ in 0..dots {
                dots_row = dots_row.child(
                    div()
                        .w(px(3.5))
                        .h(px(3.5))
                        .rounded_full()
                        .bg(if is_today { t.on_amber.opacity(0.6) } else { t.faint }),
                );
            }

            strip = strip.child(
                div()
                    .flex_1()
                    .py(px(5.))
                    .rounded(px(9.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .when_else(
                        is_today,
                        |d| d.bg(t.amber).text_color(t.on_amber),
                        |d| d.text_color(t.dim),
                    )
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(if is_today { t.on_amber } else { t.faint })
                            .child(day.format("%a").to_string().to_uppercase()),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(day.format("%-d").to_string()),
                    )
                    .child(dots_row),
            );
        }

        strip.child(div().px(px(3.)).text_size(px(13.)).text_color(t.faint).child("›"))
    }
}

fn section_heading(t: &KairnTheme, label: &str) -> impl IntoElement {
    div()
        .mt(px(18.))
        .mb(px(8.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(label.to_uppercase())
}

fn task_row(t: &KairnTheme, done: bool, label: &'static str) -> gpui::Div {
    let box_el = if done {
        div()
            .w(px(13.))
            .h(px(13.))
            .flex_none()
            .rounded(px(4.))
            .bg(t.accent)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(px(9.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(t.bg)
                    .child("✓"),
            )
    } else {
        div()
            .w(px(13.))
            .h(px(13.))
            .flex_none()
            .rounded(px(4.))
            .border_1()
            .border_color(t.faint)
    };

    let text = div()
        .when_else(
            done,
            |d| d.text_color(t.faint).line_through(),
            |d| d.text_color(t.text),
        )
        .child(label);

    div()
        .flex()
        .items_center()
        .gap(px(9.))
        .py(px(2.5))
        .rounded(px(6.))
        .hover(|s| s.bg(t.sel))
        .child(box_el)
        .child(text)
}

fn task_row_tagged(
    t: &KairnTheme,
    label: &'static str,
    tag: &'static str,
    tag_color: gpui::Hsla,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(9.))
        .py(px(2.5))
        .rounded(px(6.))
        .hover(|s| s.bg(t.sel))
        .child(
            div()
                .w(px(13.))
                .h(px(13.))
                .flex_none()
                .rounded(px(4.))
                .border_1()
                .border_color(t.faint),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().text_color(t.text).child(label))
                .child(div().text_size(px(12.)).text_color(tag_color).child(tag)),
        )
}

fn bullet_row(t: &KairnTheme, children: Vec<gpui::AnyElement>) -> impl IntoElement {
    let mut content = div().flex().items_center().gap(px(6.)).text_color(t.dim);
    for child in children {
        content = content.child(child);
    }
    div()
        .flex()
        .gap(px(9.))
        .py(px(2.5))
        .child(div().text_color(t.faint).child("–"))
        .child(content)
}

fn render_note(t: &KairnTheme, writing: bool) -> impl IntoElement {
    let today = Local::now();
    let date: NaiveDate = today.date_naive();
    let masthead = format!(
        "{}, {} {}",
        date.format("%A"),
        date.format("%-d"),
        date.format("%B")
    );
    let subline = format!("Week {} · daily notes land in the next phase", date.iso_week().week());

    let timeline: [(&str, &str, bool); 4] = [
        ("09:00", "mail sweep", false),
        ("10:30", "deep work", true),
        ("14:00", "review", false),
        ("16:00", "walk", false),
    ];
    let mut pills = div().flex().flex_wrap().gap(px(5.)).mb(px(6.));
    for (time, label, now) in timeline {
        pills = pills.child(
            div()
                .flex()
                .gap(px(5.))
                .items_center()
                .px(px(10.))
                .py(px(2.))
                .rounded(px(20.))
                .border_1()
                .bg(t.panel)
                .font_family(theme::mono_font())
                .text_size(px(10.5))
                .when_else(
                    now,
                    |d| d.border_color(t.accent).text_color(t.text),
                    |d| d.border_color(t.border).text_color(t.dim),
                )
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(t.accent)
                        .child(time),
                )
                .child(label),
        );
    }

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
        .child(pills)
        .child(div().my(px(14.)).h(px(1.)).bg(t.border))
        .child(section_heading(t, "Routines"))
        .child(task_row(t, true, "Morning review"))
        .child(task_row(t, false, "Weekly plan"))
        .child(section_heading(t, "Today"))
        .child(task_row_tagged(
            t,
            "Run the app shell against the mockup",
            "#kairn",
            t.amber,
        ))
        .child(task_row(t, false, "Wire the week strip to real dates"))
        .child(task_row_tagged(t, "Book the dentist", "›2026-08-12", t.amber))
        .child(section_heading(t, "Notes"))
        .child(bullet_row(
            t,
            vec![
                div()
                    .child("three layout states: split, terminal full, writing")
                    .into_any_element(),
            ],
        ))
        .child(bullet_row(
            t,
            vec![
                div().child("shell phase merges into").into_any_element(),
                div()
                    .text_color(t.accent)
                    .child("[[kairn-prd]]")
                    .into_any_element(),
                div().child("once it matches the mockup").into_any_element(),
            ],
        ))
        .child(bullet_row(
            t,
            vec![
                div()
                    .child(format!(
                        "TUIs get the full pane: ⇧{}⏎ toggles terminal full",
                        mod_symbol()
                    ))
                    .into_any_element(),
            ],
        ))
        .child(
            div()
                .mt(px(22.))
                .flex()
                .gap(px(6.))
                .items_center()
                .text_size(px(11.5))
                .text_color(t.faint)
                .child(kbd(t, format!("⌥{}⏎", mod_symbol())))
                .child("writing mode · markdown editing lands in the notes phase"),
        )
}
