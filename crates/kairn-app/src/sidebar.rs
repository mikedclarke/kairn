use chrono::{Datelike, Days, Local, Months, NaiveDate, Timelike};
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};

use crate::theme::KairnTheme;
use crate::workspace::{PaneView, TaskQuery, Workspace};

/// The file manager by its platform name, for context-menu labels.
pub(crate) const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Show in file manager"
};

impl Workspace {
    pub fn render_sidebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Local::now().date_naive();

        // While a line drag is in flight, the day row or cell under the
        // pointer rings as the drop target (hit-tested against last-painted
        // bounds, like the week strip). A carried timeline block rings the
        // same way.
        let drop_day = self
            .note_editor
            .as_ref()
            .and_then(|e| e.read(cx).line_drag())
            .and_then(|(_, _, position)| self.resolve_day_drop(position))
            .or_else(|| {
                self.timeline_drag
                    .as_ref()
                    .filter(|d| d.moved && !d.resize)
                    .and_then(|d| self.resolve_day_drop(d.position))
            });
        self.sidebar_bounds.borrow_mut().take();

        // Timeline mode: the calendar and its tab strip stay pinned while
        // the scroll container holds only the day timeline, so the sidebar
        // scroll walks the day. The hidden sections' drop stores still
        // clear, or stale bounds would keep catching drops.
        if self.timeline_open && self.settings.show_daily {
            self.daily_drop_bounds.borrow_mut().clear();
            let sidebar_store = self.sidebar_bounds.clone();
            return div()
                .w(px(272.))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .relative()
                .bg(t.panel)
                .border_r_1()
                .border_color(t.border)
                .text_size(t.ui_px(12.5))
                .child(
                    gpui::canvas(
                        move |bounds, _, _| {
                            sidebar_store.borrow_mut().replace(bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(self.render_calendar_component(t, drop_day, cx))
                .child(
                    div()
                        .id("sidebar-scroll")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .track_scroll(&self.sidebar_scroll)
                        .on_scroll_wheel(cx.listener(
                            |this, ev: &gpui::ScrollWheelEvent, _, cx| {
                                if cfg!(target_os = "linux") && ev.delta.precise() {
                                    this.sidebar_flick(
                                        f32::from(ev.delta.pixel_delta(px(20.)).y),
                                        cx,
                                    );
                                }
                            },
                        ))
                        .child(self.render_timeline_view(t, cx)),
                );
        }

        let sidebar_store = self.sidebar_bounds.clone();
        let mut side = div()
            .id("sidebar-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| {
                // Touchpad flicks only: wheel notches (Lines) are discrete
                // by design, and macOS delivers real momentum events, so
                // synthesizing there would double-scroll.
                if cfg!(target_os = "linux") && ev.delta.precise() {
                    this.sidebar_flick(f32::from(ev.delta.pixel_delta(px(20.)).y), cx);
                }
            }))
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        sidebar_store.borrow_mut().replace(bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.render_calendar_component(t, drop_day, cx));

        // Sync conflicts anywhere in the vault: without this list, a
        // conflict copy of a note that isn't open stays invisible forever.
        // A rare blocking state, so the section only exists while conflicts
        // do; each row jumps to the note, whose banner offers resolution.
        if !self.vault_conflicts.is_empty() {
            let mut owners: Vec<(std::path::PathBuf, usize)> = Vec::new();
            for (owner, _) in &self.vault_conflicts {
                match owners.last_mut() {
                    Some((o, n)) if o == owner => *n += 1,
                    _ => owners.push((owner.clone(), 1)),
                }
            }
            let collapsed = self.section_collapsed("Conflicts");
            side = side.child(
                sechead(
                    t,
                    "sec-conflicts",
                    "Sync conflicts",
                    Some(owners.len().to_string()),
                    collapsed,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_section("Conflicts", cx))),
            );
            if !collapsed {
                for (i, (owner, copies)) in owners.into_iter().enumerate() {
                    let stem = owner
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    let label: SharedString = match NaiveDate::parse_from_str(&stem, "%Y%m%d") {
                        Ok(date) => day_label(date),
                        Err(_) => stem.into(),
                    };
                    let row = nav_item(t, ("conflict-row", i))
                        .cursor_pointer()
                        .child(div().w(px(7.)).h(px(7.)).flex_none().rounded_full().bg(t.amber))
                        .child(div().flex_1().child(label))
                        .when(copies > 1, |d| {
                            d.child(count_label(t, &copies.to_string(), true))
                        });
                    side = side.child(row.on_click(cx.listener(move |this, _, _, cx| {
                        this.open_conflict_owner(&owner, cx);
                    })));
                }
            }
        }

        // The old three-day list is gone, but its drop bounds must stay
        // cleared so a stale frame's targets can't catch a drag release.
        self.daily_drop_bounds.borrow_mut().clear();

        // Tasks: real counts from the daily-note scan; each row opens a view.
        if self.settings.show_tasks {
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
                })
                .child(sechead_plus(t, "notes-plus").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_notes_menu(
                            point(ev.position.x, ev.position.y + px(8.)),
                            window,
                            cx,
                        );
                    }),
                )),
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
                                .w(t.ui_px(14.))
                                .flex_none()
                                .text_size(t.ui_px(14.))
                                .text_color(t.dim)
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(folder_icon(t, open))
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
                        .child(div().w(t.ui_px(14.)).flex_none())
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

        // Library: external local folders (per-machine), browsable read/write
        // but never parsed into tasks or links. The + adds a folder via the
        // native picker; each root can be removed from its context menu.
        let collapsed = self.section_collapsed("Library");
        side = side.child(
            sechead(t, "sec-library", "Library", None, collapsed)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_section("Library", cx)))
                .child(sechead_plus(t, "library-plus").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.pick_library_root(cx);
                    }),
                )),
        );
        if !collapsed {
            if self.library_trees.is_empty() {
                side = side.child(
                    nav_item(t, "library-empty")
                        .text_color(t.faint)
                        .child("No folders yet — add one with +"),
                );
            }
            for (ri, (root, rows)) in self.library_trees.iter().enumerate() {
                let root_name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                let open = self.library_expanded.contains(root);
                let toggle = root.clone();
                let menu_root = root.clone();
                let ws = cx.weak_entity();
                side = side.child(
                    nav_item(t, ("lib-root", ri))
                        .py(px(3.))
                        .gap(px(6.))
                        .child(
                            div()
                                .w(t.ui_px(14.))
                                .flex_none()
                                .text_size(t.ui_px(14.))
                                .text_color(t.dim)
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(folder_icon(t, open))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(root_name),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_library_folder(toggle.clone(), cx);
                        }))
                        .context_menu(move |menu, _, _| {
                            let reveal = menu_root.clone();
                            let copy = menu_root.clone();
                            let remove = menu_root.clone();
                            let ws = ws.clone();
                            menu.item(PopupMenuItem::new(REVEAL_LABEL).on_click(
                                move |_, _, cx| {
                                    cx.reveal_path(&reveal);
                                },
                            ))
                            .item(PopupMenuItem::new("Copy path").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copy.display().to_string(),
                                    ));
                                },
                            ))
                            .separator()
                            .item(PopupMenuItem::new("Remove from Library").on_click(
                                move |_, _, cx| {
                                    let _ = ws.update(cx, |this, cx| {
                                        this.remove_library_root(&remove, cx);
                                    });
                                },
                            ))
                        }),
                );
                for (i, entry) in rows.iter().enumerate() {
                    let path = entry.path.clone();
                    let indent = px(14. + (entry.depth + 1) as f32 * 12.);
                    let row = nav_item(t, ("lib-row", ri * 4096 + i))
                        .pl(indent)
                        .py(px(3.))
                        .gap(px(6.));
                    let menu_path = entry.path.clone();
                    if entry.is_dir {
                        let open = self.library_expanded.contains(&entry.path);
                        side = side.child(
                            row.child(
                                div()
                                    .w(t.ui_px(14.))
                                    .flex_none()
                                    .text_size(t.ui_px(14.))
                                    .text_color(t.dim)
                                    .child(if open { "▾" } else { "▸" }),
                            )
                            .child(folder_icon(t, open))
                            .child(div().flex_1().child(entry.name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_library_folder(path.clone(), cx);
                            }))
                            .context_menu(move |menu, _, _| {
                                let reveal = menu_path.clone();
                                let copy = menu_path.clone();
                                menu.item(PopupMenuItem::new(REVEAL_LABEL).on_click(
                                    move |_, _, cx| {
                                        cx.reveal_path(&reveal);
                                    },
                                ))
                                .item(PopupMenuItem::new("Copy path").on_click(
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy.display().to_string(),
                                        ));
                                    },
                                ))
                            }),
                        );
                    } else {
                        let selected = self.view == PaneView::Library(entry.path.clone());
                        side = side.child(
                            row.when(selected, |d| d.bg(t.sel).text_color(t.text))
                                .child(div().w(t.ui_px(14.)).flex_none())
                                .child(file_icon(t, &entry.path))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(entry.name.clone()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_library_file(path.clone(), window, cx);
                                }))
                                .context_menu(move |menu, _, _| {
                                    let open_with = menu_path.clone();
                                    let reveal = menu_path.clone();
                                    let copy = menu_path.clone();
                                    menu.item(
                                        PopupMenuItem::new("Open in default app").on_click(
                                            move |_, _, cx| {
                                                cx.open_with_system(&open_with);
                                            },
                                        ),
                                    )
                                    .item(PopupMenuItem::new(REVEAL_LABEL).on_click(
                                        move |_, _, cx| {
                                            cx.reveal_path(&reveal);
                                        },
                                    ))
                                    .item(PopupMenuItem::new("Copy path").on_click(
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy.display().to_string(),
                                            ));
                                        },
                                    ))
                                }),
                        );
                    }
                }
            }
        }

        // Sessions live in the titlebar (the indicator and its dropdown),
        // not in the sidebar.

        // Agents: recent CLI writes from the vault's activity log, quiet
        // read-only rows. The empty state stays honest when there are none.
        // The whole section can be turned off in Settings (working fully
        // remote leaves it permanently empty).
        if self.settings.show_agents {
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
        }

        // The floating settings gear overlays the sidebar's bottom-left
        // corner; a tail spacer lets the last rows scroll clear of it.
        side = side.child(div().h(px(48.)).flex_none());

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
    }

    /// Synthesized momentum for touchpad flicks in the sidebar. Wayland
    /// delivers finger scrolls as bare pixel deltas with no lift or
    /// momentum phase, so the scroll dies the instant the finger leaves
    /// the pad. Velocity is tracked while events stream in; a short
    /// watchdog fires once they stop, and if the finger left with speed
    /// the offset keeps moving with exponential decay until it runs out,
    /// hits an edge, or fresh input lands (each event re-arms the task,
    /// and assigning it drops — cancels — the old one).
    pub(crate) fn sidebar_flick(&mut self, dy: f32, cx: &mut Context<Self>) {
        use std::time::{Duration, Instant};

        let now = Instant::now();
        self.sidebar_flick_samples.push_back((now, dy));
        while self
            .sidebar_flick_samples
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_millis(100))
        {
            self.sidebar_flick_samples.pop_front();
        }
        self._sidebar_kinetic_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(60)).await;
            let Ok(mut velocity) = this.update(cx, |ws, _| {
                let now = Instant::now();
                let mut sum = 0.0f32;
                let mut oldest = now;
                for (t, dy) in &ws.sidebar_flick_samples {
                    sum += dy;
                    if *t < oldest {
                        oldest = *t;
                    }
                }
                ws.sidebar_flick_samples.clear();
                // The watchdog delay is part of the span: velocity at lift
                // is what the finger left behind, in px/s.
                sum / now.duration_since(oldest).as_secs_f32().max(0.06)
            }) else {
                return;
            };
            if velocity.abs() < 200. {
                return;
            }
            velocity = velocity.clamp(-8000., 8000.);
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                velocity *= 0.95;
                let moved = this.update(cx, |ws, cx| {
                    let max = ws.sidebar_scroll.max_offset().height;
                    let before = ws.sidebar_scroll.offset();
                    let mut offset = before;
                    offset.y = (offset.y + px(velocity * 0.016)).clamp(-max, px(0.));
                    ws.sidebar_scroll.set_offset(offset);
                    cx.notify();
                    offset.y != before.y
                });
                match moved {
                    Ok(true) if velocity.abs() >= 30. => {}
                    _ => break,
                }
            }
        }));
    }

    /// The calendar and its period switcher as one unit: the mini calendar
    /// (day grid, week picker, or month grid) with the Daily / Weekly /
    /// Monthly tabs docked beneath it. The tabs drive the calendar, so they
    /// belong to it rather than floating as a separate sidebar row.
    fn render_calendar_component(
        &self,
        t: &KairnTheme,
        drop_day: Option<NaiveDate>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let calendar = self.render_calendar(t, drop_day, cx);
        if !self.settings.show_daily {
            return calendar;
        }
        div()
            .child(calendar)
            .child(self.render_period_tabs(t, cx))
            .into_any_element()
    }

    /// Period switcher as a mirrored browser-tab strip drawn as one hairline:
    /// the divider runs under the calendar and dips into a chamfered
    /// (45-degree cut) outline around the active mode, an upside-down browser
    /// tab with no fill. Labels are centred with flex, not text alignment:
    /// gpui drops a div's text alignment while a hover text style applies,
    /// which made centred labels jump left on hover in every earlier version
    /// of this strip.
    fn render_period_tabs(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        const PAD: f32 = 14.;
        const GAP: f32 = 3.;
        const HEIGHT: f32 = 21.;
        const CHAMFER: f32 = 6.;
        /// The clock tab's width: an icon-sized stub so the three period
        /// tabs keep most of the strip.
        const CLOCK_W: f32 = 26.;
        let tabs: [(&str, &str, PaneView); 3] = [
            ("period-daily", "Daily", PaneView::Day),
            ("period-weekly", "Weekly", PaneView::Week),
            ("period-monthly", "Monthly", PaneView::Month),
        ];
        // While the timeline hangs open, the outline cups its clock tab; the
        // period tabs read as plain labels even though a day is showing. The
        // clock sits first (index 0), next to Daily, so timeline and daily
        // swap with one small pointer move; the labels follow at 1..3.
        let active_idx = if self.timeline_open {
            Some(0)
        } else {
            tabs.iter()
                .position(|(_, _, view)| self.view == *view)
                .map(|i| i + 1)
        };
        let line = t.border;
        let outline = gpui::canvas(
            |_, _, _| {},
            move |bounds: gpui::Bounds<gpui::Pixels>, _, window, _| {
                let l = f32::from(bounds.left());
                let r = f32::from(bounds.right());
                let top = f32::from(bounds.top()) + 0.5;
                let bottom = f32::from(bounds.bottom()) - 0.5;
                let mut path = gpui::PathBuilder::stroke(px(1.));
                path.move_to(point(px(l), px(top)));
                if let Some(i) = active_idx {
                    let w = (r - l - 2. * PAD - 3. * GAP - CLOCK_W) / 3.;
                    let x0 = if i == 0 {
                        l + PAD
                    } else {
                        l + PAD + CLOCK_W + GAP + ((i - 1) as f32) * (w + GAP)
                    };
                    let x1 = x0 + if i == 0 { CLOCK_W } else { w };
                    path.line_to(point(px(x0), px(top)));
                    path.line_to(point(px(x0), px(bottom - CHAMFER)));
                    path.line_to(point(px(x0 + CHAMFER), px(bottom)));
                    path.line_to(point(px(x1 - CHAMFER), px(bottom)));
                    path.line_to(point(px(x1), px(bottom - CHAMFER)));
                    path.line_to(point(px(x1), px(top)));
                }
                path.line_to(point(px(r), px(top)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, line);
                }
            },
        )
        .absolute()
        .size_full();

        let mut row = div().flex().size_full().px(px(PAD)).gap(px(GAP));
        let hover_text = t.text;
        row = row.child(
            div()
                .id("period-timeline")
                .w(px(CLOCK_W))
                .flex_none()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when_else(
                    self.timeline_open,
                    |d| d.text_color(t.text),
                    |d| d.text_color(t.dim).hover(move |s| s.text_color(hover_text)),
                )
                .child(
                    gpui::svg()
                        .path("kairn-icons/clock.svg")
                        .flex_none()
                        .w(t.ui_px(12.))
                        .h(t.ui_px(12.))
                        .text_color(if self.timeline_open { t.text } else { t.dim }),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_timeline(cx))),
        );
        for (id, label, view) in tabs {
            let active = !self.timeline_open && self.view == view;
            let hover_text = t.text;
            row = row.child(
                div()
                    .id(id)
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(t.ui_px(10.5))
                    .cursor_pointer()
                    .when_else(
                        active,
                        |d| d.text_color(t.text),
                        |d| d.text_color(t.dim).hover(move |s| s.text_color(hover_text)),
                    )
                    .child(label.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_timeline(cx);
                        let today = Local::now().date_naive();
                        match view {
                            PaneView::Week => this.select_week(today, cx),
                            PaneView::Month => this.select_month(today, cx),
                            _ => this.select_day(today, cx),
                        }
                    })),
            );
        }
        div().relative().h(px(HEIGHT)).child(outline).child(row)
    }

    /// The day timeline hanging under the calendar: 24 hour rows, the day's
    /// time-blocked lines laid over them at their clock positions, and a
    /// now line on today. Blocks drag to move, drag by the bottom edge to
    /// resize, and carry onto a calendar day like any other task drag.
    fn render_timeline_view(&self, t: &KairnTheme, cx: &mut Context<Self>) -> gpui::AnyElement {
        const GUTTER: f32 = 46.;
        let min_of = |time: chrono::NaiveTime| (time.hour() * 60 + time.minute()) as i32;
        let hour_h = px(self.timeline_hour_px());
        let today = Local::now().date_naive() == self.selected_day;

        let mut rows = div().relative().h(hour_h * 24.);

        // The 24-hour canvas publishes its bounds (the y-to-time ruler) and,
        // while a drag is live, window-level listeners keep tracking the
        // pointer wherever it goes, the note editor's drag recipe.
        let bounds_store = self.timeline_bounds.clone();
        let dragging = self.timeline_drag.is_some();
        let workspace = cx.entity();
        rows = rows.child(
            gpui::canvas(
                move |bounds, _, _| {
                    bounds_store.borrow_mut().replace(bounds);
                },
                move |_, _, window, _| {
                    if !dragging {
                        return;
                    }
                    window.on_mouse_event({
                        let workspace = workspace.clone();
                        move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            if event.pressed_button == Some(MouseButton::Left) {
                                workspace.update(cx, |ws, cx| {
                                    ws.on_timeline_drag_move(event.position, cx);
                                });
                            }
                        }
                    });
                    window.on_mouse_event({
                        let workspace = workspace.clone();
                        move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble
                                || event.button != MouseButton::Left
                            {
                                return;
                            }
                            workspace.update(cx, |ws, cx| {
                                ws.on_timeline_drag_release(event.position, cx);
                            });
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        );

        for hour in 0..24 {
            rows = rows.child(
                div()
                    .absolute()
                    .top(hour_h * hour as f32)
                    .left(px(0.))
                    .right(px(0.))
                    .h(hour_h)
                    .border_t_1()
                    .border_color(t.border.opacity(if hour == 0 { 0. } else { 0.5 }))
                    .child(
                        div()
                            .absolute()
                            .top(px(-7.))
                            .left(px(10.))
                            .text_size(t.ui_px(9.))
                            .text_color(t.faint)
                            .font_family(t.mono_font.clone())
                            .child(format!("{hour:02}:00")),
                    ),
            );
        }

        if today {
            let now_min = min_of(Local::now().time());
            rows = rows.child(
                div()
                    .absolute()
                    .top(hour_h * (now_min as f32 / 60.) - px(0.75))
                    .left(px(GUTTER - 5.))
                    .right(px(0.))
                    .h(px(1.5))
                    .bg(t.amber)
                    .child(
                        div()
                            .absolute()
                            .top(px(-2.))
                            .left(px(0.))
                            .w(px(5.))
                            .h(px(5.))
                            .rounded_full()
                            .bg(t.amber),
                    ),
            );
        }

        if self.day_timeline.is_empty() {
            rows = rows.child(
                div()
                    .absolute()
                    .top(hour_h * 9.15)
                    .left(px(GUTTER))
                    .right(px(10.))
                    .text_size(t.ui_px(10.))
                    .text_color(t.faint)
                    .child("No timed lines yet. Give a task a time like 09:30 - 10:15 and it lands here."),
            );
        }

        // Greedy two-lane layout so overlapping blocks sit side by side
        // instead of covering each other.
        let spans: Vec<(i32, i32)> = self
            .day_timeline
            .iter()
            .map(|b| {
                let s = min_of(b.start);
                (s, b.end.map(&min_of).unwrap_or(s + 30).max(s + 15))
            })
            .collect();
        let mut lane_busy_until = [i32::MIN, i32::MIN];
        for (ix, block) in self.day_timeline.iter().enumerate() {
            let (mut s_min, mut e_min) = spans[ix];
            let live = match &self.timeline_drag {
                Some(d) if d.line_idx == block.line_idx && d.moved => {
                    let (s, e) = self.timeline_drag_times(d);
                    s_min = min_of(s);
                    e_min = e.map(&min_of).unwrap_or(s_min + 30).max(s_min + 15);
                    true
                }
                _ => false,
            };
            let shared = spans
                .iter()
                .enumerate()
                .any(|(j, &(s, e))| j != ix && s < spans[ix].1 && spans[ix].0 < e);
            let lane = usize::from(spans[ix].0 < lane_busy_until[0]);
            lane_busy_until[lane] = lane_busy_until[lane].max(spans[ix].1);

            let y = hour_h * (s_min as f32 / 60.);
            let h = (hour_h * ((e_min - s_min) as f32 / 60.)).max(px(17.));
            let time_text = match block.end.is_some() || live {
                true => format!(
                    "{:02}:{:02} - {:02}:{:02}",
                    s_min / 60,
                    s_min % 60,
                    e_min / 60,
                    e_min % 60
                ),
                false => format!("{:02}:{:02}", s_min / 60, s_min % 60),
            };
            let tall = h >= px(31.);

            let mut card = div()
                .id(("timeline-block", ix))
                .absolute()
                .top(y)
                .h(h)
                .rounded(px(6.))
                .bg(t.sel)
                .border_1()
                .border_color(if live { t.accent } else { t.border })
                .px(px(7.))
                .py(px(2.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_center()
                .cursor_grab()
                .text_size(t.ui_px(10.5))
                .text_color(t.text)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.timeline_grab(ix, false, ev.position, cx);
                    }),
                );
            card = match (shared, lane) {
                (false, _) => card.left(px(GUTTER)).right(px(10.)),
                (true, 0) => card.left(px(GUTTER)).right(gpui::relative(0.52)),
                (true, _) => card.left(gpui::relative(0.52)).right(px(10.)),
            };
            let label: String = if block.label.is_empty() {
                "(untitled)".to_string()
            } else {
                block.label.clone()
            };
            card = card.child(div().whitespace_nowrap().child(label));
            if tall {
                card = card.child(
                    div()
                        .text_size(t.ui_px(9.))
                        .text_color(if live { t.accent } else { t.dim })
                        .font_family(t.mono_font.clone())
                        .child(time_text),
                );
            }
            // The bottom edge retimes the end instead of moving the block.
            card = card.child(
                div()
                    .id(("timeline-resize", ix))
                    .absolute()
                    .bottom(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .h(px(7.))
                    .cursor_row_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.timeline_grab(ix, true, ev.position, cx);
                        }),
                    ),
            );
            rows = rows.child(card);
        }

        // The strip slides open from under the calendar rather than popping.
        div()
            .pt(px(8.))
            .pb(px(28.))
            .child(rows)
            .with_animation(
                "timeline-slide",
                gpui::Animation::new(std::time::Duration::from_millis(170))
                    .with_easing(gpui::ease_out_quint()),
                |this, delta| this.mt(px((delta - 1.) * 14.)).opacity(delta),
            )
            .into_any_element()
    }

    fn render_calendar(
        &self,
        t: &KairnTheme,
        drop_day: Option<NaiveDate>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.calendar_drop_bounds.borrow_mut().clear();
        let today = Local::now().date_naive();
        // Over a monthly note the calendar is a month picker; over a weekly
        // note the day grid selects whole weeks; everywhere else it is the
        // normal day calendar.
        if self.view == PaneView::Month {
            return self.render_month_picker(t, cx).into_any_element();
        }
        let week_mode = self.view == PaneView::Week;
        let sel_monday = self.selected_day
            - Days::new(self.selected_day.weekday().num_days_from_monday() as u64);
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
            .pt(px(10.))
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
                            .text_size(t.ui_px(15.))
                            .cursor_pointer()
                            .hover(move |s| s.text_color(today_hover))
                            .child(title)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cal_offset = 0;
                                if week_mode {
                                    this.select_week(today, cx);
                                } else {
                                    this.select_day(today, cx);
                                }
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
                    // Explicit line height so the grid's total height is
                    // deterministic; the month picker matches it exactly to
                    // keep the sidebar from jumping between views.
                    .line_height(t.ui_px(15.))
                    .text_color(t.faint)
                    .children(["M", "T", "W", "T", "F", "S", "S"].map(|d| {
                        div().flex_1().py(px(2.)).text_center().child(d)
                    })),
            );

        for week in 0..6u64 {
            let row_monday: NaiveDate = grid_start + Days::new(week * 7);
            let row_selected = week_mode && row_monday == sel_monday;
            let mut cells: Vec<gpui::AnyElement> = Vec::with_capacity(7);
            for wd in 0..7u64 {
                let day: NaiveDate = grid_start + Days::new(week * 7 + wd);
                let in_month = day.month() == shown_first.month();
                let is_today = day == today;
                // Week mode selects whole rows; a single lit day would fight
                // the row highlight.
                let is_selected = !week_mode && day == self.selected_day;
                let has_note = self.note_days.contains_key(&day);
                let stats = self.day_stats.get(&day).copied().unwrap_or_default();
                let overdue_open = stats.open > 0 && day < today;

                let is_drop = drop_day == Some(day);
                let bounds_store = self.calendar_drop_bounds.clone();
                let cell = div()
                    .id(("cal-cell", (week * 7 + wd) as usize))
                    .relative()
                    .flex_1()
                    .pt(px(2.))
                    .pb(px(3.))
                    .rounded(px(5.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| bounds_store.borrow_mut().push((day, bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    // A tight line height keeps the indicator slot snug under
                    // the digits instead of drifting to the cell's bottom edge.
                    .child(
                        div()
                            .line_height(t.ui_px(13.))
                            .child(day.format("%-d").to_string()),
                    );
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
                // The drop ring is applied last so it wins over the today and
                // selected treatments while a drag hovers the cell.
                let cell = if is_drop {
                    cell.bg(t.sel).text_color(t.accent).border_1().border_color(t.accent)
                } else {
                    cell
                };

                let hover_bg = t.hover;
                let cell = cell
                    .when(!week_mode && !is_today && !is_selected && !is_drop, |c| {
                        c.hover(move |s| s.bg(hover_bg))
                    })
                    .when(!week_mode, |c| {
                        c.on_click(cx.listener(move |this, _, _, cx| {
                            this.select_day(day, cx);
                        }))
                    });
                cells.push(cell.into_any_element());
            }
            if week_mode {
                let hover_bg = t.hover;
                cal = cal.child(
                    div()
                        .id(("cal-week", week as usize))
                        .flex()
                        .text_size(t.ui_px(11.5))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when_else(
                            row_selected,
                            |d| d.bg(t.sel),
                            |d| d.hover(move |s| s.bg(hover_bg)),
                        )
                        .children(cells)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_week(row_monday, cx);
                        })),
                );
            } else {
                cal = cal.child(div().flex().text_size(t.ui_px(11.5)).children(cells));
            }
        }
        cal.into_any_element()
    }

    /// The mini calendar's month-picker face, shown over a monthly note: a
    /// year of months, arrows stepping whole years.
    fn render_month_picker(&self, t: &KairnTheme, cx: &mut Context<Self>) -> gpui::Div {
        let today = Local::now().date_naive();
        let shown_year = today.year() + self.cal_offset;
        let today_hover = t.text;
        let cal = div().px(px(14.)).pt(px(10.)).pb(px(2.)).child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .mb(px(8.))
                .text_size(t.ui_px(12.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(
                    // Clicking the year jumps back to the current month's
                    // note, mirroring the day calendar's title shortcut.
                    div()
                        .id("cal-today")
                        .text_size(t.ui_px(15.))
                        .cursor_pointer()
                        .hover(move |s| s.text_color(today_hover))
                        .child(shown_year.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cal_offset = 0;
                            this.select_month(today, cx);
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(2.))
                        .text_color(t.dim)
                        .child(cal_nav(t, "cal-prev", "‹").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cal_offset -= 1;
                                cx.notify();
                            },
                        )))
                        .child(cal_nav(t, "cal-next", "›").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cal_offset += 1;
                                cx.notify();
                            },
                        ))),
                ),
        );
        // The month grid fills exactly the height of the day calendar's
        // weekday header plus six week rows (px terms and ui-scaled terms
        // summed separately), so switching between Daily/Weekly and Monthly
        // never moves the period strip or the sections below it.
        let mut grid = div()
            .h(px(88.) + t.ui_px(93.))
            .flex()
            .flex_col();
        for row_ix in 0..3u32 {
            let mut row = div().flex().flex_1().items_center().text_size(t.ui_px(11.5));
            for col in 0..4u32 {
                let month = row_ix * 4 + col + 1;
                let Some(first) = NaiveDate::from_ymd_opt(shown_year, month, 1) else {
                    continue;
                };
                let is_current = shown_year == today.year() && month == today.month();
                let is_selected = shown_year == self.selected_day.year()
                    && month == self.selected_day.month();
                let hover_bg = t.hover;
                // Flex centring, not text_center: a hover restyle drops the
                // div's text alignment in gpui, so centred text jumps left
                // under the pointer.
                let cell = div()
                    .id(("cal-month", month as usize))
                    .flex_1()
                    .h(t.ui_px(30.))
                    .mx(px(2.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(first.format("%b").to_string());
                let cell = if is_current {
                    cell.bg(t.amber)
                        .text_color(t.on_amber)
                        .font_weight(gpui::FontWeight::BOLD)
                } else if is_selected {
                    cell.bg(t.sel)
                        .text_color(t.text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                } else {
                    cell.text_color(t.dim).hover(move |s| s.bg(hover_bg))
                };
                row = row.child(cell.on_click(cx.listener(move |this, _, _, cx| {
                    this.select_month(first, cx);
                })));
            }
            grid = grid.child(row);
        }
        cal.child(grid)
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
        .text_size(t.ui_px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| s.text_color(hover_text))
        .child(label.to_uppercase())
        .child(div().text_size(t.ui_px(13.)).child(if collapsed { "▸" } else { "▾" }))
        .child(div().flex_1());
    if let Some(count) = count {
        head = head.child(div().child(count));
    }
    head
}

/// The small + at the right edge of a section header. Callers attach the
/// mouse-down behaviour; it must stop propagation so the header's collapse
/// toggle doesn't also fire.
fn sechead_plus(t: &KairnTheme, id: &'static str) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    let hover_text = t.text;
    div()
        .id(id)
        .flex_none()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .text_size(t.ui_px(17.))
        .text_color(t.faint)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).text_color(hover_text))
        .child("+")
}

/// A sidebar icon from the app's embedded SVG set (see `KairnAssets`),
/// scaled with the interface size and tinted per call.
fn svg_icon(t: &KairnTheme, path: &'static str, color: gpui::Hsla) -> gpui::AnyElement {
    gpui::svg()
        .path(path)
        .flex_none()
        .w(t.ui_px(13.))
        .h(t.ui_px(13.))
        .text_color(color)
        .into_any_element()
}

/// The folder mark for the Notes and Library trees; open folders get the
/// open-flap variant so expansion reads from the icon, not just the chevron.
fn folder_icon(t: &KairnTheme, open: bool) -> gpui::AnyElement {
    let path = if open { "kairn-icons/folder-open.svg" } else { "kairn-icons/folder.svg" };
    svg_icon(t, path, t.faint)
}

/// The document mark for Notes rows (always markdown).
fn note_icon(t: &KairnTheme) -> gpui::AnyElement {
    svg_icon(t, "kairn-icons/file-text.svg", t.faint)
}

/// A per-kind file mark for Library rows, shaped and tinted so kinds read
/// at a glance even when long names ellipsize before their extension:
/// text page for markdown, code page for text/code, photo page for
/// images, a red page for PDFs, and a blank page for everything else.
fn file_icon(t: &KairnTheme, path: &std::path::Path) -> gpui::AnyElement {
    use kairn_core::FileKind;

    let is_pdf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return svg_icon(t, "kairn-icons/file.svg", t.red.opacity(0.8));
    }
    match kairn_core::file_kind(path) {
        FileKind::Markdown => svg_icon(t, "kairn-icons/file-text.svg", t.faint),
        FileKind::Text => svg_icon(t, "kairn-icons/file-code.svg", t.accent.opacity(0.9)),
        FileKind::Image => svg_icon(t, "kairn-icons/file-image.svg", t.amber.opacity(0.9)),
        FileKind::Other => svg_icon(t, "kairn-icons/file.svg", t.faint),
    }
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
