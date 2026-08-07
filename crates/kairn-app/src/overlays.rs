//! The one-at-a-time overlays: the session picker, the jump switcher, and
//! quick capture, plus their shared backdrop.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, StatefulInteractiveElement, Styled, Task, Window, div, px,
};
use gpui_component::{
    WindowExt, h_flex,
    input::{Input, InputEvent, InputState},
};
use chrono::Local;
use kairn_core as notes;

use crate::keymap::{Capture, CloseOverlay, InputDown, InputUp, OpenSettings, ToggleSwitcher};
use crate::session::SessionKind;
use crate::theme::KairnTheme;
use crate::ui::{picker_item, picker_rule, switcher_item, switcher_section};
use crate::workspace::{LayoutMode, Workspace};

/// The single open overlay. One field instead of nine booleans and inputs:
/// mutual exclusion is structural, and closing (or replacing) an overlay
/// drops its input and subscription with it.
pub enum Overlay {
    /// The new-session menu, anchored near the click.
    Picker { pos: Point<Pixels> },
    /// The jump/search switcher.
    Switcher {
        input: gpui::Entity<InputState>,
        _sub: gpui::Subscription,
        hits: Vec<notes::SearchHit>,
        selected: usize,
        /// Bumped per keystroke; a finished search only lands if it is
        /// still the latest, so slow results never overwrite fresh ones.
        generation: u64,
        _search: Option<Task<()>>,
    },
    /// Quick capture into today's note.
    Capture {
        input: gpui::Entity<InputState>,
        _sub: gpui::Subscription,
    },
}

impl Workspace {
    /// Move the switcher's keyboard selection through the results. With no
    /// results the arrows fall through to the input's cursor movement.
    pub fn switcher_move(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(Overlay::Switcher { hits, selected, .. }) = &mut self.overlay else {
            cx.propagate();
            return;
        };
        if hits.is_empty() {
            cx.propagate();
            return;
        }
        let last = (hits.len() - 1) as i64;
        *selected = (*selected as i64 + delta).clamp(0, last) as usize;
        cx.notify();
    }

    /// Open a switcher search hit. The note pane must end up visible, so a
    /// full-screen terminal drops back to the split.
    pub fn open_search_hit(
        &mut self,
        hit: &notes::SearchHit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match hit.date {
            Some(date) => self.select_day(date, cx),
            None => self.open_note(hit.path.clone(), cx),
        }
        if self.layout == LayoutMode::TerminalFull {
            self.layout = LayoutMode::Split;
        }
        self.close_overlays(window, cx);
    }

    pub fn open_picker(&mut self, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Picker { pos });
        // The picker has no input of its own to take focus, so focus the
        // overlay layer itself: Escape then dispatches through the
        // backdrop's key context.
        self.overlay_focus.focus(window);
        cx.notify();
    }

    /// Close whichever overlay is open. Dropping the enum drops the
    /// overlay's input and subscription with it.
    pub fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.take().is_some() {
            self.focus_active_terminal(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn on_capture(&mut self, _: &Capture, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Some(Overlay::Capture { .. })) {
            self.close_overlays(window, cx);
            return;
        }
        // A fresh input each open: empty value, no stale state.
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Capture to today's note…")
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.submit_capture(window, cx);
                }
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.overlay = Some(Overlay::Capture { input, _sub: sub });
        cx.notify();
    }

    fn submit_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.overlay {
            Some(Overlay::Capture { input, .. }) => input.read(cx).value().to_string(),
            _ => return,
        };
        if self.root_missing {
            window.push_notification("Notes folder unavailable; nothing was captured.", cx);
            self.close_overlays(window, cx);
            return;
        }
        let today = Local::now().date_naive();
        match notes::capture(&self.notes_root, today, &text) {
            Ok(Some(path)) => {
                self.note_self_write(&path);
                self.reload_notes()
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("kairn: capture failed: {e}");
                window.push_notification("Could not write today's note, see stderr.", cx);
            }
        }
        self.close_overlays(window, cx);
    }

    pub(crate) fn render_capture(&self, t: &KairnTheme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Some(Overlay::Capture { input, .. }) = &self.overlay else {
            return None;
        };
        let input = input.clone();

        let card = div()
            .w(px(600.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .rounded(px(12.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel2)
            .shadow_lg()
            .overflow_hidden()
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(13.))
                    .gap(px(10.))
                    .text_size(px(15.))
                    .text_color(t.faint)
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(px(2.)).h(px(16.)).bg(t.amber))
                    .child("Capture"),
            )
            .child(div().p(px(12.)).child(Input::new(&input)))
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(8.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(px(11.))
                    .text_color(t.faint)
                    .child("⏎ add to today")
                    .child("esc close"),
            );

        Some(
            div()
                .id("capture-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(100.))
                .bg(gpui::rgba(0x0a0b0873))
                .key_context("Overlay")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_overlays(window, cx);
                    }),
                )
                .child(card),
        )
    }

    pub(crate) fn on_toggle_switcher(
        &mut self,
        _: &ToggleSwitcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.overlay, Some(Overlay::Switcher { .. })) {
            self.close_overlays(window, cx);
        } else {
            // A fresh search input each open: empty query, no stale results.
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Search notes and days…")
            });
            let sub = cx.subscribe_in(
                &input,
                window,
                |this, state, ev: &InputEvent, window, cx| {
                    match ev {
                        InputEvent::Change => {
                            let query = state.read(cx).value().to_string();
                            let root = this.notes_root.clone();
                            let Some(Overlay::Switcher { generation, _search, .. }) =
                                &mut this.overlay
                            else {
                                return;
                            };
                            *generation += 1;
                            let generation = *generation;
                            // Debounced and off the UI thread: the search
                            // reads the vault, which must not run per
                            // keystroke in the input handler.
                            *_search = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_millis(120))
                                    .await;
                                let results = cx
                                    .background_executor()
                                    .spawn(async move {
                                        notes::search_notes(&root, &query, 12)
                                    })
                                    .await;
                                let _ = this.update(cx, |ws, cx| {
                                    if let Some(Overlay::Switcher {
                                        hits,
                                        selected,
                                        generation: current,
                                        ..
                                    }) = &mut ws.overlay
                                        && *current == generation
                                    {
                                        *hits = results;
                                        *selected = 0;
                                        cx.notify();
                                    }
                                });
                            }));
                        }
                        InputEvent::PressEnter { .. } => {
                            let hit = match &this.overlay {
                                Some(Overlay::Switcher { hits, selected, .. }) => {
                                    hits.get(*selected).or_else(|| hits.first()).cloned()
                                }
                                _ => None,
                            };
                            if let Some(hit) = hit {
                                this.open_search_hit(&hit, window, cx);
                            }
                        }
                        _ => {}
                    }
                },
            );
            input.update(cx, |state, cx| state.focus(window, cx));
            self.overlay = Some(Overlay::Switcher {
                input,
                _sub: sub,
                hits: Vec::new(),
                selected: 0,
                generation: 0,
                _search: None,
            });
            cx.notify();
        }
    }

    pub(crate) fn on_close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
    }

    pub(crate) fn render_picker(
        &self,
        t: &KairnTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let Some(Overlay::Picker { pos }) = &self.overlay else {
            return None;
        };
        let pos = *pos;

        // Keep the menu inside the window when the anchor row sits near the
        // bottom edge.
        let item_count = 3 + self.settings.ssh_hosts.len().max(1);
        let est_height = px(item_count as f32 * 30.0 + 32.0);
        let viewport = window.viewport_size();
        let top = pos
            .y
            .min(viewport.height - est_height - px(8.))
            .max(px(0.));

        let shell_name = shell_name();

        let mut menu = div()
            .absolute()
            .left(pos.x)
            .top(top)
            // Without this, the mouse-down bubbles to the click-away backdrop
            // (and through it to the row underneath) and the menu dismisses
            // before its items can receive the click.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .min_w(px(232.))
            .p(px(5.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel2)
            .shadow_lg()
            .text_size(px(12.5))
            .child(
                picker_item(t, "picker-shell", cx)
                    .child(div().flex_1().child("New shell on this machine"))
                    .child(div().text_size(px(11.)).text_color(t.faint).child(shell_name))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.spawn_session(SessionKind::Local, window, cx);
                    })),
            )
            .child(picker_rule(t));

        if self.settings.ssh_hosts.is_empty() {
            menu = menu.child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_color(t.faint)
                    .child("No saved SSH hosts"),
            );
        } else {
            for (i, host) in self.settings.ssh_hosts.iter().enumerate() {
                let kind = SessionKind::Ssh(host.clone());
                menu = menu.child(
                    picker_item(t, ("picker-host", i), cx)
                        .child(div().flex_1().child(host.name.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.faint)
                                .child(host.target.clone()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.spawn_session(kind.clone(), window, cx);
                        })),
                );
            }
        }

        menu = menu.child(picker_rule(t)).child(
            picker_item(t, "picker-settings", cx)
                .text_color(t.dim)
                .child("Settings…")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_open_settings(&OpenSettings, window, cx);
                })),
        );

        Some(
            div()
                .id("picker-backdrop")
                .absolute()
                .inset_0()
                // Escape closes: the same key context as every other
                // overlay backdrop, with the overlay focus tracked so the
                // binding has a dispatch path.
                .track_focus(&self.overlay_focus)
                .key_context("Overlay")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        this.close_overlays(window, cx);
                    }),
                )
                .child(menu),
        )
    }

    pub(crate) fn render_switcher(&self, t: &KairnTheme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Some(Overlay::Switcher { input, hits, selected, .. }) = &self.overlay else {
            return None;
        };
        let selected = *selected;

        let today = chrono::Local::now();
        let day_label = format!(
            "{}, {} {}",
            today.format("%A"),
            today.format("%-d"),
            today.format("%B")
        );

        let mut card = div()
            .w(px(600.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Arrow keys from the focused search input land here and move the
            // selection; with no results they fall through to the input.
            .on_action(cx.listener(|this, _: &InputUp, _, cx| this.switcher_move(-1, cx)))
            .on_action(cx.listener(|this, _: &InputDown, _, cx| this.switcher_move(1, cx)))
            .rounded(px(12.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel2)
            .shadow_lg()
            .overflow_hidden()
            .text_size(px(12.5))
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(13.))
                    .gap(px(10.))
                    .text_size(px(15.))
                    .text_color(t.faint)
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(px(2.)).h(px(16.)).bg(t.accent))
                    .child("Jump to session, day, or note"),
            );

        let query = input.read(cx).value().trim().to_string();
        card = card.child(
            div()
                .px(px(10.))
                .py(px(4.))
                .border_b_1()
                .border_color(t.border)
                .child(Input::new(input).appearance(false)),
        );

        // A live query swaps the jump lists for search results.
        if !query.is_empty() {
            card = card.child(switcher_section(t, "Notes & days"));
            if hits.is_empty() {
                card = card.child(
                    h_flex()
                        .px(px(16.))
                        .py(px(6.))
                        .text_color(t.faint)
                        .child("Nothing found"),
                );
            }
            for (i, hit) in hits.iter().enumerate() {
                let hit = hit.clone();
                let icon = if hit.date.is_some() { "◷" } else { "≡" };
                let snippet: Option<String> = hit.snippet.as_ref().map(|s| {
                    let mut short: String = s.chars().take(48).collect();
                    if short.len() < s.len() {
                        short.push('…');
                    }
                    short
                });
                card = card.child(
                    switcher_item(t, ("switcher-hit", i), cx)
                        .when(i == selected, |d| d.bg(t.sel))
                        .child(div().w(px(14.)).text_color(t.faint).child(icon))
                        .child(div().flex_none().text_color(t.text).child(hit.name.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_size(px(11.))
                                .text_color(t.faint)
                                .children(snippet),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_search_hit(&hit, window, cx);
                        })),
                );
            }
            let card = card.child(
                h_flex()
                    .px(px(16.))
                    .py(px(8.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(px(11.))
                    .text_color(t.faint)
                    .child("↑↓ select")
                    .child("⏎ open")
                    .child("esc close"),
            );
            return Some(switcher_backdrop(self, t, cx).child(card));
        }

        card = card.child(switcher_section(t, "Sessions"));

        for (i, session) in self.sessions.iter().enumerate() {
            let busy = session.busy;
            let meta = match &session.kind {
                SessionKind::Local => {
                    if busy {
                        "local · running"
                    } else {
                        "local · idle"
                    }
                }
                SessionKind::Ssh(_) => "SSH · connected",
            };
            card = card.child(
                switcher_item(t, ("switcher-session", i), cx)
                    .child(
                        div()
                            .w(px(7.))
                            .h(px(7.))
                            .rounded_full()
                            .when_else(
                                busy,
                                |d| d.bg(t.accent),
                                |d| d.border_1().border_color(t.faint),
                            ),
                    )
                    .child(div().flex_1().child(session.label()))
                    .child(div().text_size(px(11.)).text_color(t.faint).child(meta))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_session(i, window, cx);
                        this.close_overlays(window, cx);
                    })),
            );
        }

        let today_day = today.date_naive();
        card = card
            .child(switcher_section(t, "Days"))
            .child(
                switcher_item(t, "switcher-today", cx)
                    .child(div().w(px(14.)).text_color(t.faint).child("◷"))
                    .child(div().flex_1().child(day_label))
                    .child(div().text_size(px(11.)).text_color(t.faint).child("today"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_day(today_day, cx);
                        this.close_overlays(window, cx);
                    })),
            )
            .child(
                h_flex()
                    .px(px(16.))
                    .py(px(8.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(px(11.))
                    .text_color(t.faint)
                    .child("type to search notes")
                    .child("⏎ open")
                    .child("esc close"),
            );

        Some(switcher_backdrop(self, t, cx).child(card))
    }
}

/// The click-away backdrop shared by both switcher states.
fn switcher_backdrop(
    ws: &Workspace,
    _t: &KairnTheme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id("switcher-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(px(100.))
        .bg(gpui::rgba(0x0a0b0873))
        .track_focus(&ws.overlay_focus)
        .key_context("Overlay")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_overlays(window, cx);
            }),
        )
}

/// `$SHELL`'s basename, resolved once per process: the picker renders every
/// frame while open and the environment cannot change under us.
fn shell_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::PathBuf::from(s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "shell".into())
    })
}
