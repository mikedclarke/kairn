use chrono::{Datelike, Days, Local, Months, NaiveDate};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};

use crate::theme::KairnTheme;
use crate::workspace::{PaneView, TaskQuery, Workspace, chord};

/// The file manager by its platform name, for context-menu labels.
pub(crate) const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Show in file manager"
};

impl Workspace {
    pub fn render_sidebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let session_count = self.sessions.len();
        let today = Local::now().date_naive();

        // While a line drag is in flight, the day row or cell under the
        // pointer rings as the drop target (hit-tested against last-painted
        // bounds, like the week strip).
        let drop_day = self
            .note_editor
            .as_ref()
            .and_then(|e| e.read(cx).line_drag())
            .and_then(|(_, _, position)| self.resolve_day_drop(position));
        self.sidebar_bounds.borrow_mut().take();

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

        // Sessions: the real list.
        let collapsed = self.section_collapsed("Sessions");
        side = side.child(
            sechead(t, "sec-sessions", "Sessions", Some(session_count.to_string()), collapsed)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_section("Sessions", cx)))
                .child(sechead_plus(t, "sessions-plus").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_picker(
                            point(ev.position.x, ev.position.y + px(8.)),
                            window,
                            cx,
                        );
                    }),
                )),
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
                if session.kind.is_remote() {
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
        }

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

    /// Period switcher as browser-style tabs: the active mode is a cupped tab
    /// standing on the strip's baseline, the other two are plain floating
    /// labels with no chrome. Every tab reserves the same border and padding
    /// in every state, so selecting or hovering never nudges a label out of
    /// line (the earlier bold-active tab changed width and did).
    fn render_period_tabs(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: [(&str, &str, PaneView); 3] = [
            ("period-daily", "Daily", PaneView::Day),
            ("period-weekly", "Weekly", PaneView::Week),
            ("period-monthly", "Monthly", PaneView::Month),
        ];
        let mut row = div()
            .flex()
            .px(px(14.))
            .pt(px(8.))
            .gap(px(3.))
            .border_b(px(1.))
            .border_color(t.border);
        for (id, label, view) in tabs {
            let active = self.view == view;
            let hover_text = t.text;
            row = row.child(
                div()
                    .id(id)
                    .flex_1()
                    .pt(px(6.))
                    .pb(px(6.))
                    .text_center()
                    .text_size(t.ui_px(11.))
                    .rounded_t(px(7.))
                    .border_t(px(1.))
                    .border_l(px(1.))
                    .border_r(px(1.))
                    .cursor_pointer()
                    .when_else(
                        active,
                        |d| d.bg(t.sel).border_color(t.border).text_color(t.text),
                        |d| {
                            d.border_color(t.border.opacity(0.))
                                .text_color(t.dim)
                                .hover(move |s| s.text_color(hover_text))
                        },
                    )
                    .child(label.to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let today = Local::now().date_naive();
                        match view {
                            PaneView::Week => this.select_week(today, cx),
                            PaneView::Month => this.select_month(today, cx),
                            _ => this.select_day(today, cx),
                        }
                    })),
            );
        }
        row
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
        let mut cal = div().px(px(14.)).pt(px(14.)).pb(px(2.)).child(
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
        for row_ix in 0..3u32 {
            let mut row = div().flex().text_size(t.ui_px(11.5)).mb(px(2.));
            for col in 0..4u32 {
                let month = row_ix * 4 + col + 1;
                let Some(first) = NaiveDate::from_ymd_opt(shown_year, month, 1) else {
                    continue;
                };
                let is_current = shown_year == today.year() && month == today.month();
                let is_selected = shown_year == self.selected_day.year()
                    && month == self.selected_day.month();
                let hover_bg = t.hover;
                let cell = div()
                    .id(("cal-month", month as usize))
                    .flex_1()
                    .py(px(7.))
                    .rounded(px(5.))
                    .text_center()
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
