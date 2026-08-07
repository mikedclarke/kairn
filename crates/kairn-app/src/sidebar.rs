use chrono::{Datelike, Days, Local, Months, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px,
};

use crate::session::SessionKind;
use crate::theme::{self, KairnTheme};
use crate::workspace::{PaneView, TaskQuery, Workspace, kbd, mod_symbol};

impl Workspace {
    pub fn render_sidebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let session_count = self.sessions.len();

        let mut side = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(self.render_calendar(t, cx));

        let today = Local::now().date_naive();
        side = side.child(sechead(t, "Daily", None));
        for i in 0..3u64 {
            let day = today - Days::new(i);
            let selected = day == self.selected_day;
            let has_note = self.note_days.contains(&day);
            let mut row = nav_item(t, ("daily", i as usize))
                .when(selected, |d| d.bg(t.sel).text_color(t.text))
                .when(has_note, |d| {
                    d.child(div().w(px(7.)).h(px(7.)).flex_none().rounded_full().bg(t.amber))
                })
                .child(div().flex_1().child(day_label(day)));
            if day == today {
                row = row.child(count_label(t, "today", false));
            }
            side = side.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.select_day(day, cx);
            })));
        }

        // Tasks: real counts from the daily-note scan; each row opens a view.
        side = side.child(sechead(t, "Tasks", None));
        let queries = [
            ("tasks-today", "Today", TaskQuery::Today),
            ("tasks-open", "Open", TaskQuery::Open),
            ("tasks-overdue", "Overdue", TaskQuery::Overdue),
        ];
        for (id, label, query) in queries {
            let count = self.tasks_for(query).count();
            let active = self.view == PaneView::Tasks(query);
            let hot = query == TaskQuery::Overdue && count > 0;
            side = side.child(
                nav_item(t, id)
                    .when(active, |d| d.bg(t.sel).text_color(t.text))
                    .child(div().flex_1().child(label))
                    .child(count_label(t, &count.to_string(), hot))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_task_view(query, cx);
                    })),
            );
        }

        // Notes: the real tree of the Notes/ folder.
        side = side.child(sechead(t, "Notes", None));
        if self.notes_tree.is_empty() {
            side = side.child(
                nav_item(t, "notes-empty")
                    .text_color(t.faint)
                    .child("No notes yet"),
            );
        }
        for (i, entry) in self.notes_tree.iter().enumerate() {
            let path = entry.path.clone();
            let indent = px(14. + entry.depth as f32 * 14.);
            let mut row = nav_item(t, ("note-row", i)).pl(indent);
            if entry.special {
                row = row.text_color(t.faint);
            }
            if entry.is_dir {
                let open = self.notes_expanded_contains(&entry.path);
                row = row
                    .child(
                        div()
                            .w(px(11.))
                            .flex_none()
                            .text_size(px(9.))
                            .text_color(t.faint)
                            .child(if open { "▾" } else { "▸" }),
                    )
                    .child(div().flex_1().child(entry.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_notes_folder(path.clone(), cx);
                    }));
            } else {
                let selected = self.view == PaneView::Note(entry.path.clone());
                row = row
                    .when(selected, |d| d.bg(t.sel).text_color(t.text))
                    .child(div().w(px(11.)).flex_none())
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(entry.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_note(path.clone(), cx);
                    }));
            }
            side = side.child(row);
        }

        // Sessions: the real list.
        side = side.child(sechead(t, "Sessions", Some(&session_count.to_string())));
        for (i, session) in self.sessions.iter().enumerate() {
            let busy = session.busy;
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
        side = side.child(feed_box);

        // Pinned footer: the always-visible way into Settings.
        let hover_bg = t.hover;
        let hover_text = t.text;
        let settings_row = div()
            .id("sidebar-settings")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(9.))
            .border_t_1()
            .border_color(t.border)
            .text_color(t.dim)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_text))
            .child(div().text_size(px(13.)).child("⚙"))
            .child(div().flex_1().child("Settings"))
            .child(kbd(t, format!("{},", mod_symbol())))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_settings(window, cx);
            }));

        div()
            .w(px(272.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.border)
            .text_size(px(12.5))
            .child(side)
            .child(settings_row)
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
                let is_selected = day == self.selected_day;
                let has_note = self.note_days.contains(&day);

                let cell = div()
                    .id(("cal-cell", (week * 7 + wd) as usize))
                    .flex_1()
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_center()
                    .cursor_pointer()
                    .child(day.format("%-d").to_string());
                let cell = if is_today {
                    cell.bg(t.amber)
                        .text_color(t.on_amber)
                        .font_weight(gpui::FontWeight::BOLD)
                } else if is_selected {
                    cell.bg(t.sel)
                        .text_color(t.text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                } else if !in_month {
                    cell.text_color(t.faint.opacity(0.5))
                } else if has_note {
                    cell.text_color(t.text)
                } else {
                    cell.text_color(t.dim)
                };
                let hover_bg = t.hover;
                row = row.child(
                    cell.when(!is_today && !is_selected, |c| c.hover(move |s| s.bg(hover_bg)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_day(day, cx);
                        })),
                );
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
