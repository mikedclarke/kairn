//! Window chrome: the custom titlebar and the statusbar.

use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_component::{TitleBar, h_flex};

use crate::keymap::{Capture, ToggleSidebar, ToggleSwitcher, ToggleThemeMode, chord, mod_symbol};
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
            .text_size(px(12.))
            .text_color(t.faint)
            .cursor_pointer()
            .hover(|s| s.border_color(t.faint))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_switcher(&ToggleSwitcher, window, cx);
            }))
            .child(div().flex_1().child("Jump to session, day, or note"))
            .child(kbd(t, chord("J")));

        let capture_btn = titlebar_button(t, "capture-btn", cx).child(
            h_flex()
                .gap(px(6.))
                .child("Capture")
                .child(kbd(t, format!("{}⇧K", mod_symbol()))),
        );
        let capture_btn = capture_btn.on_click(cx.listener(|this, _, window, cx| {
            this.on_capture(&Capture, window, cx);
        }));

        let theme_btn = titlebar_button(t, "theme-btn", cx)
            .child("◐")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_theme(&ToggleThemeMode, window, cx);
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
                            .text_size(px(13.))
                            .child(cairn_mark(t))
                            .child("Kairn"),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(8.))
                    .child(jump_hint)
                    .child(capture_btn)
                    .child(theme_btn),
            )
    }

    pub(crate) fn render_statusbar(&self, t: &KairnTheme, cx: &App) -> impl IntoElement {
        let running = self.sessions.iter().filter(|s| s.busy).count();
        let m = mod_symbol();
        let hints = [
            format!("{m}\\ sidebar"),
            format!("{m}1–9 sessions"),
            format!("{} jump", chord("J")),
            format!("⇧{m}⏎ terminal"),
            format!("⌥{m}⏎ writing"),
        ];
        let _ = cx;
        let mut bar = h_flex()
            .h(px(26.))
            .flex_none()
            .px(px(14.))
            .gap(px(18.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border)
            .text_size(px(11.5))
            .text_color(t.dim)
            .child(
                h_flex()
                    .gap(px(5.))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(if running > 0 { t.accent } else { t.faint }),
                    )
                    .child(format!(
                        "{} session{}",
                        self.sessions.len(),
                        if self.sessions.len() == 1 { "" } else { "s" }
                    )),
            )
            .child(format!("{running} running"));
        // Quiet unless something is genuinely wrong; each of these is a
        // rare state the user must know about, not chrome churn.
        if self._notes_watcher.is_none() && !self.root_missing {
            bar = bar.child(div().text_color(t.amber).child("file watching off"));
        }
        if self.dailies_skipped > 0 {
            bar = bar.child(
                div()
                    .text_color(t.amber)
                    .child(format!("{} unreadable day notes", self.dailies_skipped)),
            );
        }
        if self.settings.degraded {
            bar = bar.child(
                div()
                    .text_color(t.amber)
                    .child("settings on defaults after a corrupt file; apply Settings to fix"),
            );
        }
        bar.child(
                h_flex()
                    .flex_1()
                    .justify_end()
                    .gap(px(18.))
                    .text_color(t.faint)
                    .children(hints),
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
        .text_size(px(12.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}
