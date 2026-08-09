use chrono::{Datelike, Days, Local, Months, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};

use crate::session::SessionKind;
use crate::theme::KairnTheme;
use crate::workspace::{PaneView, TaskQuery, Workspace, chord, kbd};

/// The file manager by its platform name, for context-menu labels.
const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Show in file manager"
};

impl Workspace {
    pub fn render_sidebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let session_count = self.sessions.len();
        let today = Local::now().date_naive();

        let mut side = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(self.render_calendar(t, cx));

        // Daily: today plus the next (or previous, per settings) two days.
        let collapsed = self.section_collapsed("Daily");
        side = side.child(
            sechead(t, "sec-daily", "Daily", None, collapsed).on_click(cx.listener(
                |this, _, _, cx| this.toggle_section("Daily", cx),
            )),
        );
        if !collapsed {
            let forward = self.settings.daily_forward;
            for i in 0..3u64 {
                let day = if forward { today + Days::new(i) } else { today - Days::new(i) };
                let selected = day == self.selected_day;
                let has_note = self.note_days.contains_key(&day);
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
        }

        // Tasks: real counts from the daily-note scan; each row opens a view.
        let collapsed = self.section_collapsed("Tasks");
        side = side.child(
            sechead(t, "sec-tasks", "Tasks", None, collapsed).on_click(cx.listener(
                |this, _, _, cx| this.toggle_section("Tasks", cx),
            )),
        );
        if !collapsed {
            let queries = [
                ("tasks-today", "Today", TaskQuery::Today),
                ("tasks-open", "Open", TaskQuery::Open),
                ("tasks-overdue", "Overdue", TaskQuery::Overdue),
            ];
            for (id, label, query) in queries {
                let count = self.task_count(query);
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
        }

        // Notes: the real tree of the Notes/ folder. The section header's
        // menu creates at the top level.
        let collapsed = self.section_collapsed("Notes");
        let notes_dir = self.notes_root.join("Notes");
        let ws = cx.weak_entity();
        side = side.child(
            sechead(t, "sec-notes", "Notes", None, collapsed)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_section("Notes", cx)))
                .context_menu(move |menu, _, _| {
                    let ws = ws.clone();
                    let dir = notes_dir.clone();
                    menu.item(PopupMenuItem::new("New note…").on_click(move |_, window, cx| {
                        let _ = ws.update(cx, |this, cx| {
                            this.prompt_new_note(dir.clone(), window, cx);
                        });
                    }))
                }),
        );
        if !collapsed {
            if self.notes_tree.is_empty() {
                side = side.child(
                    nav_item(t, "notes-empty")
                        .text_color(t.faint)
                        .child("No notes yet"),
                );
            }
            for (i, entry) in self.notes_tree.iter().enumerate() {
                let path = entry.path.clone();
                let indent = px(14. + entry.depth as f32 * 12.);
                let mut row = nav_item(t, ("note-row", i)).pl(indent).py(px(3.)).gap(px(6.));
                if entry.special {
                    row = row.text_color(t.faint);
                }
                let ws = cx.weak_entity();
                let menu_path = entry.path.clone();
                if entry.is_dir {
                    let open = self.notes_expanded_contains(&entry.path);
                    row = row
                        .child(
                            div()
                                .w(px(11.))
                                .flex_none()
                                .text_size(t.ui_px(11.))
                                .text_color(t.dim)
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(folder_icon(t))
                        .child(div().flex_1().child(entry.name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_notes_folder(path.clone(), cx);
                        }));
                    side = side.child(row.context_menu(move |menu, _, _| {
                        let dir = menu_path.clone();
                        let reveal = menu_path.clone();
                        let ws = ws.clone();
                        menu.item(PopupMenuItem::new("New note…").on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |this, cx| {
                                this.prompt_new_note(dir.clone(), window, cx);
                            });
                        }))
                        .separator()
                        .item(PopupMenuItem::new(REVEAL_LABEL).on_click(move |_, _, cx| {
                            cx.reveal_path(&reveal);
                        }))
                    }));
                } else {
                    let selected = self.view == PaneView::Note(entry.path.clone());
                    row = row
                        .when(selected, |d| d.bg(t.sel).text_color(t.text))
                        .child(div().w(px(11.)).flex_none())
                        .child(note_icon(t))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(entry.name.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_note(path.clone(), cx);
                        }));
                    side = side.child(row.context_menu(move |menu, _, _| {
                        let rename = menu_path.clone();
                        let trash = menu_path.clone();
                        let reveal = menu_path.clone();
                        let ws_rename = ws.clone();
                        let ws_trash = ws.clone();
                        menu.item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                            let _ = ws_rename.update(cx, |this, cx| {
                                this.prompt_rename_note(rename.clone(), window, cx);
                            });
                        }))
                        .item(PopupMenuItem::new("Delete note").on_click(move |_, window, cx| {
                            let _ = ws_trash.update(cx, |this, cx| {
                                this.trash_note_at(&trash, window, cx);
                            });
                        }))
                        .separator()
                        .item(PopupMenuItem::new(REVEAL_LABEL).on_click(move |_, _, cx| {
                            cx.reveal_path(&reveal);
                        }))
                    }));
                }
            }
        }

        // Sessions: the real list.
        let collapsed = self.section_collapsed("Sessions");
        side = side.child(
            sechead(t, "sec-sessions", "Sessions", Some(session_count.to_string()), collapsed)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_section("Sessions", cx))),
        );
        if !collapsed {
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
                            .text_size(t.ui_px(9.5))
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
                            .font_family(t.mono_font.clone())
                            .text_size(t.ui_px(10.5))
                            .text_color(t.faint)
                            .child(chord(&(i + 1).to_string())),
                    );
                }
                let ws = cx.weak_entity();
                side = side.child(
                    row.on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_session(i, window, cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        let ws = ws.clone();
                        menu.item(PopupMenuItem::new("Close session").on_click(move |_, _, cx| {
                            let _ = ws.update(cx, |this, _| this.close_session(i));
                        }))
                    }),
                );
            }
            side = side.child(
                nav_item(t, "new-session")
                    .text_color(t.faint)
                    .child("＋ New session…")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            this.open_picker(point(ev.position.x, ev.position.y + px(12.)), window, cx);
                        }),
                    ),
            );
        }

        // Agents: recent CLI writes from the vault's activity log, quiet
        // read-only rows. The empty state stays honest when there are none.
        let collapsed = self.section_collapsed("Agents");
        side = side.child(
            sechead(t, "sec-agents", "Agents", None, collapsed).on_click(cx.listener(
                |this, _, _, cx| this.toggle_section("Agents", cx),
            )),
        );
        if !collapsed {
            if self.agent_activity.is_empty() {
                side = side.child(
                    div()
                        .px(px(14.))
                        .pb(px(16.))
                        .text_size(t.ui_px(11.5))
                        .text_color(t.faint)
                        .child("No agent activity yet"),
                );
            }
            for entry in &self.agent_activity {
                let verb = match entry.action.as_str() {
                    "add" => "added",
                    "done" => "completed",
                    "capture" => "captured",
                    other => other,
                };
                side = side.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(3.))
                        .text_size(t.ui_px(11.5))
                        .child(
                            div()
                                .flex_none()
                                .font_family(t.mono_font.clone())
                                .text_size(t.ui_px(10.5))
                                .text_color(t.faint)
                                .child(activity_time(&entry.ts, today)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(t.text)
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(entry.actor.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(t.dim)
                                        .child(format!("{verb} {:?}", entry.detail)),
                                ),
                        ),
                );
            }
            if !self.agent_activity.is_empty() {
                side = side.child(div().pb(px(12.)));
            }
        }

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
            .child(div().text_size(t.ui_px(13.)).child("⚙"))
            .child(div().flex_1().child("Settings"))
            .child(kbd(t, chord(",")))
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
            .text_size(t.ui_px(12.5))
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
        let today_hover = t.text;
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
                    .text_size(t.ui_px(12.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(
                        // Clicking the month/year is the "take me to today"
                        // shortcut: jump the calendar to the current month and
                        // open today's daily note.
                        div()
                            .id("cal-today")
                            .cursor_pointer()
                            .hover(move |s| s.text_color(today_hover))
                            .child(title)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cal_offset = 0;
                                this.select_day(today, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .text_color(t.dim)
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
                    .text_size(t.ui_px(10.5))
                    .text_color(t.faint)
                    .children(["M", "T", "W", "T", "F", "S", "S"].map(|d| {
                        div().flex_1().py(px(2.)).text_center().child(d)
                    })),
            );

        for week in 0..6u64 {
            let mut row = div().flex().text_size(t.ui_px(11.5));
            for wd in 0..7u64 {
                let day: NaiveDate = grid_start + Days::new(week * 7 + wd);
                let in_month = day.month() == shown_first.month();
                let is_today = day == today;
                let is_selected = day == self.selected_day;
                let has_note = self.note_days.contains_key(&day);
                let stats = self.day_stats.get(&day).copied().unwrap_or_default();
                let overdue_open = stats.open > 0 && day < today;

                let cell = div()
                    .id(("cal-cell", (week * 7 + wd) as usize))
                    .flex_1()
                    .pt(px(2.))
                    .pb(px(1.))
                    .rounded(px(5.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .child(div().child(day.format("%-d").to_string()));
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
                } else if overdue_open {
                    // A past day with unfinished tasks reads slightly hot.
                    cell.bg(t.red.opacity(0.10)).text_color(t.dim)
                } else if has_note {
                    cell.text_color(t.text)
                } else {
                    cell.text_color(t.dim)
                };

                // NotePlan-style day indicator under the number: a tick when
                // the day's tasks are all done, a circle while any are open
                // (red once the day is past), nothing on task-free days. The
                // slot has fixed height so the grid never shifts.
                let base = if is_today {
                    t.on_amber
                } else if !in_month {
                    t.faint.opacity(0.5)
                } else {
                    t.faint
                };
                let slot = div()
                    .h(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(crate::ui::day_task_indicator(
                        t,
                        stats,
                        base,
                        overdue_open,
                        is_today,
                    ));
                let cell = cell.child(slot);

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

/// A collapsible section header: the whole row toggles, the chevron shows
/// the state.
fn sechead(
    t: &KairnTheme,
    id: &'static str,
    label: &'static str,
    count: Option<String>,
    collapsed: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover_text = t.dim;
    let mut head = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .px(px(14.))
        .pt(px(16.))
        .pb(px(6.))
        .text_size(t.ui_px(10.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_text))
        .child(label.to_uppercase())
        .child(div().text_size(t.ui_px(11.)).child(if collapsed { "▸" } else { "▾" }))
        .child(div().flex_1());
    if let Some(count) = count {
        head = head.child(div().child(count));
    }
    head
}

/// A tiny folder mark for the Notes browser, drawn with quads so no asset
/// or glyph font is needed.
fn folder_icon(t: &KairnTheme) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(12.))
        .h(px(10.))
        .flex()
        .flex_col()
        .child(div().w(px(5.)).h(px(2.)).rounded(px(1.)).bg(t.faint.opacity(0.8)))
        .child(div().w(px(12.)).h(px(8.)).rounded(px(2.)).bg(t.faint.opacity(0.55)))
}

/// A tiny document mark for the Notes browser: a bordered page with two
/// text lines.
fn note_icon(t: &KairnTheme) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(10.))
        .h(px(12.))
        .rounded(px(2.))
        .border_1()
        .border_color(t.faint)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.5))
        .child(div().w(px(5.)).h(px(1.)).bg(t.faint))
        .child(div().w(px(5.)).h(px(1.)).bg(t.faint))
}

fn count_label(t: &KairnTheme, label: &str, hot: bool) -> impl IntoElement {
    div()
        .text_size(t.ui_px(11.))
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

/// Activity timestamps (`YYYY-MM-DD HH:MM:SS`, local time) shown the way
/// the feed reads: the clock time for today's entries, the day for older
/// ones, the raw string if it isn't in the log's format.
fn activity_time(ts: &str, today: NaiveDate) -> SharedString {
    let parsed = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S");
    match parsed {
        Ok(dt) if dt.date() == today => dt.format("%H:%M").to_string().into(),
        Ok(dt) => dt.format("%-d %b").to_string().into(),
        Err(_) => ts.to_string().into(),
    }
}

fn cal_nav(t: &KairnTheme, id: &'static str, glyph: &'static str) -> gpui::Stateful<gpui::Div> {
    let hover_text = t.text;
    div()
        .id(id)
        .px(px(4.))
        .text_size(t.ui_px(16.))
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_text))
        .child(glyph)
}
