use chrono::{Datelike, Days, Local, Months, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px,
};

use crate::session::SessionKind;
use crate::theme::{self, KairnTheme};
use crate::workspace::{Workspace, mod_symbol};

impl Workspace {
    pub fn render_sidebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let session_count = self.sessions.len();

        let mut side = div()
            .id("sidebar")
            .w(px(272.))
            .flex_none()
            .h_full()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.border)
            .overflow_y_scroll()
            .text_size(px(12.5))
            .child(self.render_calendar(t, cx));

        // Daily: real dates, stub selection (today).
        let today = Local::now().date_naive();
        side = side
            .child(sechead(t, "Daily", None))
            .child(
                nav_item(t, "daily-0")
                    .bg(t.sel)
                    .text_color(t.text)
                    .child(div().w(px(7.)).h(px(7.)).flex_none().rounded_full().bg(t.amber))
                    .child(div().flex_1().child(day_label(today)))
                    .child(count_label(t, "today", false)),
            )
            .child(
                nav_item(t, "daily-1")
                    .child(div().flex_1().child(day_label(today - Days::new(1)))),
            )
            .child(
                nav_item(t, "daily-2")
                    .child(div().flex_1().child(day_label(today - Days::new(2)))),
            );

        // Tasks and Inbox: stub counts until the notes subsystem exists.
        side = side
            .child(sechead(t, "Tasks", None))
            .child(
                nav_item(t, "tasks-today")
                    .child(div().flex_1().child("Today"))
                    .child(count_label(t, "3", false)),
            )
            .child(
                nav_item(t, "tasks-open")
                    .child(div().flex_1().child("Open"))
                    .child(count_label(t, "7", false)),
            )
            .child(
                nav_item(t, "tasks-overdue")
                    .child(div().flex_1().child("Overdue"))
                    .child(count_label(t, "2", true)),
            )
            .child(sechead(t, "Inbox", Some("2")))
            .child(nav_item(t, "inbox-0").child("Clipped: GPUI layout notes"))
            .child(nav_item(t, "inbox-1").child("Idea: week strip drag animation"))
            .child(sechead(t, "Notes", None))
            .child(nav_item(t, "notes-0").child("▸ knowledge"))
            .child(nav_item(t, "notes-1").child("▾ projects"))
            .child(nav_item(t, "notes-2").pl(px(28.)).child("kairn · prd"))
            .child(nav_item(t, "notes-3").pl(px(28.)).child("kairn · shell notes"))
            .child(nav_item(t, "notes-4").child("▸ archive"));

        // Sessions: the real list.
        side = side.child(sechead(t, "Sessions", Some(&session_count.to_string())));
        for (i, session) in self.sessions.iter().enumerate() {
            let busy = session.is_busy();
            let active = i == self.active_session;
            let dot = div()
                .w(px(7.))
                .h(px(7.))
                .flex_none()
                .rounded_full()
                .when_else(
                    busy,
                    |d| d.bg(t.accent),
                    |d| d.border_1().border_color(t.faint),
                );

            let mut row = nav_item(t, ("session", i))
                .when(active, |d| d.bg(t.sel).text_color(t.text))
                .child(dot)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(session.label()),
                );
            if matches!(session.kind, SessionKind::Ssh(_)) {
                row = row.child(
                    div()
                        .text_size(px(9.5))
                        .border_1()
                        .border_color(t.border)
                        .rounded(px(3.))
                        .px(px(4.))
                        .text_color(t.faint)
                        .child("SSH"),
                );
            }
            if i < 9 {
                row = row.child(
                    div()
                        .font_family(theme::mono_font())
                        .text_size(px(10.5))
                        .text_color(t.faint)
                        .child(format!("{}{}", mod_symbol(), i + 1)),
                );
            }
            side = side.child(row.on_click(cx.listener(move |this, _, window, cx| {
                this.activate_session(i, window, cx);
            })));
        }
        side = side.child(
            nav_item(t, "new-session")
                .text_color(t.faint)
                .child("＋ New session…")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                        this.open_picker(point(ev.position.x, ev.position.y + px(12.)), cx);
                    }),
                ),
        );

        // Agents feed: stub until the agent layer exists (Phase E).
        let feed: [(&str, &str, &str); 3] = [
            ("08:41", "pm", "appended 2 items to Today"),
            ("08:12", "research", "updated knowledge/gpui-notes.md"),
            ("07:55", "build", "linux build green"),
        ];
        side = side.child(sechead(t, "Agents", None));
        let mut feed_box = div().px(px(14.)).pb(px(16.)).flex().flex_col().gap(px(7.));
        for (time, agent, text) in feed {
            feed_box = feed_box.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(11.5))
                    .text_color(t.dim)
                    .child(div().w(px(34.)).flex_none().text_color(t.faint).child(time))
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(t.accent)
                                            .child(agent),
                                    )
                                    .child(div().child(text)),
                            ),
                    ),
            );
        }
        side.child(feed_box)
    }

    fn render_calendar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();
        let current_first = today.with_day(1).expect("valid first of month");
        let shown_first = if self.cal_offset >= 0 {
            current_first
                .checked_add_months(Months::new(self.cal_offset as u32))
                .unwrap_or(current_first)
        } else {
            current_first
                .checked_sub_months(Months::new((-self.cal_offset) as u32))
                .unwrap_or(current_first)
        };

        let title = format!("{} {}", shown_first.format("%B"), shown_first.year());
        let grid_start =
            shown_first - Days::new(shown_first.weekday().num_days_from_monday() as u64);

        let mut cal = div()
            .px(px(14.))
            .pt(px(14.))
            .pb(px(2.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .mb(px(8.))
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title)
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .text_color(t.faint)
                            .child(
                                cal_nav(t, "cal-prev", "‹").on_click(cx.listener(|this, _, _, cx| {
                                    this.cal_offset -= 1;
                                    cx.notify();
                                })),
                            )
                            .child(
                                cal_nav(t, "cal-next", "›").on_click(cx.listener(|this, _, _, cx| {
                                    this.cal_offset += 1;
                                    cx.notify();
                                })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .text_size(px(10.5))
                    .text_color(t.faint)
                    .children(["M", "T", "W", "T", "F", "S", "S"].map(|d| {
                        div().flex_1().py(px(2.)).text_center().child(d)
                    })),
            );

        for week in 0..6u64 {
            let mut row = div().flex().text_size(px(10.5));
            for wd in 0..7u64 {
                let day: NaiveDate = grid_start + Days::new(week * 7 + wd);
                let in_month = day.month() == shown_first.month();
                let is_today = day == today;
                // "Has note" is stubbed as past-days-of-this-month until Phase C.
                let has_note = in_month && day < today;

                let cell = div()
                    .flex_1()
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_center()
                    .child(day.format("%-d").to_string());
                let cell = if is_today {
                    cell.bg(t.amber)
                        .text_color(t.on_amber)
                        .font_weight(gpui::FontWeight::BOLD)
                } else if !in_month {
                    cell.text_color(t.faint.opacity(0.5))
                } else if has_note {
                    cell.text_color(t.text)
                } else {
                    cell.text_color(t.dim)
                };
                row = row.child(cell);
            }
            cal = cal.child(row);
        }
        cal
    }
}

fn day_label(date: NaiveDate) -> SharedString {
    format!(
        "{} {} {}",
        date.format("%A"),
        date.format("%-d"),
        date.format("%b")
    )
    .into()
}

fn sechead(t: &KairnTheme, label: &str, count: Option<&str>) -> impl IntoElement {
    let mut head = div()
        .flex()
        .px(px(14.))
        .pt(px(16.))
        .pb(px(6.))
        .text_size(px(10.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(div().flex_1().child(label.to_uppercase()));
    if let Some(count) = count {
        head = head.child(div().child(count.to_string()));
    }
    head
}

fn count_label(t: &KairnTheme, label: &str, hot: bool) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(if hot { t.amber } else { t.faint })
        .child(label.to_string())
}

fn nav_item(t: &KairnTheme, id: impl Into<ElementId>) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    let hover_text = t.text;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(14.))
        .py(px(4.))
        .text_color(t.dim)
        .cursor_default()
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
}

fn cal_nav(t: &KairnTheme, id: &'static str, glyph: &'static str) -> gpui::Stateful<gpui::Div> {
    let hover_text = t.text;
    div()
        .id(id)
        .px(px(4.))
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_text))
        .child(glyph)
}
