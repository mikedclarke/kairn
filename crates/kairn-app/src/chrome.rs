//! Window chrome: the custom titlebar and the statusbar.

use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_component::{TitleBar, h_flex};

use crate::keymap::{ToggleSidebar, ToggleSwitcher, chord};
use crate::theme::KairnTheme;
use crate::ui::kbd;
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn render_titlebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let jump_hint = h_flex()
            .id("jump-hint")
            .w(px(280.))
            .px(px(10.))
            .py(px(3.))
            .gap(px(6.))
            .rounded(px(7.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_size(t.ui_px(12.))
            .text_color(t.faint)
            .cursor_pointer()
            .hover(|s| s.border_color(t.faint))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_switcher(&ToggleSwitcher, window, cx);
            }))
            .child(div().flex_1().child("Jump to session, day, or note"))
            .child(kbd(t, chord("J")));

        // Open/close the terminal pane, so it isn't always stuck on screen.
        let terminal_open = self.layout.shows_terminal();
        let terminal_btn = titlebar_button(t, "terminal-btn", cx)
            .font_family(t.mono_font.clone())
            .text_color(if terminal_open { t.accent } else { t.dim })
            .child(">_")
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_terminal_pane(window, cx);
            }));

        let sidebar_btn = titlebar_button(t, "sidebar-btn", cx)
            .text_color(t.dim)
            .child("◧")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_sidebar(&ToggleSidebar, window, cx);
            }));

        TitleBar::new()
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(sidebar_btn)
                    .child(
                        h_flex()
                            .gap(px(7.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(t.ui_px(13.))
                            .child(cairn_mark(t))
                            .child("Kairn"),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(8.))
                    .child(jump_hint)
                    .child(terminal_btn),
            )
    }

    /// Warnings only: each of these is a rare state the user must know
    /// about. With nothing wrong there is no bar at all — the space belongs
    /// to the app.
    pub(crate) fn render_statusbar(&self, t: &KairnTheme, cx: &App) -> Option<impl IntoElement> {
        let _ = cx;
        let mut warnings: Vec<String> = Vec::new();
        if self._notes_watcher.is_none() && !self.root_missing {
            warnings.push("file watching off".to_string());
        }
        if self.dailies_skipped > 0 {
            warnings.push(format!("{} unreadable day notes", self.dailies_skipped));
        }
        if self.settings.degraded {
            warnings
                .push("settings on defaults after a corrupt file; apply Settings to fix".to_string());
        }
        if warnings.is_empty() {
            return None;
        }
        Some(
            h_flex()
                .h(px(26.))
                .flex_none()
                .px(px(14.))
                .gap(px(18.))
                .bg(t.panel)
                .border_t_1()
                .border_color(t.border)
                .text_size(t.ui_px(11.5))
                .text_color(t.amber)
                .children(warnings),
        )
    }
}

pub(crate) fn cairn_mark(t: &KairnTheme) -> impl IntoElement {
    // The stacked-stones mark, drawn as bars so no asset pipeline is needed.
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(1.))
        .child(div().w(px(4.)).h(px(2.)).rounded_full().bg(t.text.opacity(0.35)))
        .child(div().w(px(7.)).h(px(2.5)).rounded_full().bg(t.text.opacity(0.5)))
        .child(div().w(px(10.)).h(px(3.)).rounded_full().bg(t.text.opacity(0.7)))
        .child(div().w(px(13.)).h(px(3.5)).rounded_full().bg(t.text.opacity(0.9)))
}

pub(crate) fn titlebar_button<T: 'static>(
    t: &KairnTheme,
    id: &'static str,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    div()
        .id(id)
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(t.ui_px(12.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}
