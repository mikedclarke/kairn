//! Window chrome: the custom titlebar and the statusbar.

use gpui::{
    App, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{TitleBar, h_flex};

use crate::keymap::{ToggleSidebar, ToggleSwitcher, chord};
use crate::theme::KairnTheme;
use crate::ui::kbd;
use crate::workspace::{LayoutMode, Workspace};

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

        let hover_bg = t.hover;
        let sidebar_btn = div()
            .id("sidebar-btn")
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(26.))
            .rounded(px(6.))
            .text_size(t.ui_px(16.))
            .text_color(t.dim)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .child("◧")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_sidebar(&ToggleSidebar, window, cx);
            }));

        // The layout switch: Notes | Split | Term. One click sets the layout,
        // the active state stays lit. Notes-first, so Split is the resting
        // state; Term hands the whole main area to the terminal.
        let layout_seg = h_flex()
            .gap(px(3.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(t.bg)
            .border_1()
            .border_color(t.border)
            .child(self.layout_seg_button(
                t,
                "seg-notes",
                self.layout == LayoutMode::NotesFull,
                LayoutMode::NotesFull,
                cx,
            ))
            .child(self.layout_seg_button(
                t,
                "seg-split",
                self.layout == LayoutMode::Split,
                LayoutMode::Split,
                cx,
            ))
            .child(self.layout_seg_button(
                t,
                "seg-term",
                self.layout == LayoutMode::TerminalFull,
                LayoutMode::TerminalFull,
                cx,
            ));

        TitleBar::new()
            .child(h_flex().h_full().items_center().child(sidebar_btn))
            .child(
                h_flex()
                    .gap(px(8.))
                    .pr(px(8.))
                    .child(jump_hint)
                    .child(layout_seg),
            )
    }

    /// One cell of the layout segment: its glyph (drawn, so it renders the
    /// same on every platform), lit when its layout is active.
    fn layout_seg_button(
        &self,
        t: &KairnTheme,
        id: &'static str,
        active: bool,
        mode: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover_bg = t.hover;
        let sel = t.sel;
        let color = if active { t.accent } else { t.dim };
        let glyph = match mode {
            LayoutMode::NotesFull => seg_icon_notes(color).into_any_element(),
            LayoutMode::TerminalFull => seg_icon_term(t, color).into_any_element(),
            _ => seg_icon_split(color).into_any_element(),
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .py(px(4.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |d| d.bg(sel))
            .when(!active, |d| d.hover(move |s| s.bg(hover_bg)))
            .child(glyph)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_layout(mode, window, cx);
            }))
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

/// Notes-full glyph: a page with a couple of text lines.
fn seg_icon_notes(color: Hsla) -> impl IntoElement {
    div()
        .w(px(13.))
        .h(px(11.))
        .rounded(px(2.))
        .border_1()
        .border_color(color)
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(1.5))
        .px(px(2.5))
        .child(div().h(px(1.)).w_full().bg(color))
        .child(div().h(px(1.)).w(px(5.)).bg(color))
}

/// Split glyph: a frame divided into two panes.
fn seg_icon_split(color: Hsla) -> impl IntoElement {
    div()
        .w(px(13.))
        .h(px(11.))
        .rounded(px(2.))
        .border_1()
        .border_color(color)
        .flex()
        .justify_center()
        .child(div().w(px(1.)).h_full().bg(color))
}

/// Terminal-full glyph: the `>_` prompt, the app's established terminal mark.
fn seg_icon_term(t: &KairnTheme, color: Hsla) -> impl IntoElement {
    div()
        .font_family(t.mono_font.clone())
        .text_size(t.ui_px(11.))
        .text_color(color)
        .child(">_")
}
