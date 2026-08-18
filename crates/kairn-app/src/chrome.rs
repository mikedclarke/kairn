//! Window chrome: the custom titlebar and the statusbar.

use gpui::{
    App, Context, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, PathBuilder, StatefulInteractiveElement, Styled, div, point,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{TitleBar, h_flex};

use crate::keymap::{ToggleSidebar, ToggleSwitcher, chord, chord_alt};
use crate::theme::KairnTheme;
use crate::ui::hover_hint;
use crate::workspace::{LayoutMode, Workspace};

/// Inner height of the search-and-layout capsule; every control in it sits
/// inside this one frame so nothing can render at its own height.
const CAPSULE_H: f32 = 28.;

impl Workspace {
    pub(crate) fn render_titlebar(&self, t: &KairnTheme, cx: &mut Context<Self>) -> impl IntoElement {
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
            .tooltip(hover_hint("Sidebar", Some(chord("\\"))))
            .child("◧")
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_sidebar(&ToggleSidebar, window, cx);
            }));

        // Search and the layout switcher as one capsule: the left half opens
        // the ⌘J overlay (a full search: note titles and bodies, days,
        // sessions, library), the right half is all four layouts. The chord
        // hints live in the hover tooltips, not in the chrome.
        let search = div()
            .id("jump-hint")
            .flex()
            .items_center()
            .w(px(252.))
            .h_full()
            .px(px(12.))
            .rounded_l(px(7.))
            .text_size(t.ui_px(12.))
            .text_color(t.faint)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .tooltip(hover_hint("Search & jump", Some(chord("J"))))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_switcher(&ToggleSwitcher, window, cx);
            }))
            .child("Search notes, days, sessions");

        let capsule = h_flex()
            .h(px(CAPSULE_H))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .overflow_hidden()
            .child(search)
            .child(div().w(px(1.)).h_full().flex_none().bg(t.border))
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .gap(px(2.))
                    .px(px(3.))
                    .child(self.layout_mode_button(t, "mode-writing", LayoutMode::Writing, cx))
                    .child(self.layout_mode_button(t, "mode-notes", LayoutMode::NotesFull, cx))
                    .child(self.layout_mode_button(t, "mode-split", LayoutMode::Split, cx))
                    .child(self.layout_mode_button(t, "mode-term", LayoutMode::TerminalFull, cx)),
            );

        // The sessions indicator: how many are open, lit while any is busy.
        // Clicking drops the sessions menu (switch, close, start new). The
        // sessions list lives here rather than in the sidebar.
        let busy_any = self.sessions.iter().any(|s| s.busy);
        let count = self.sessions.len();
        let sessions_btn = h_flex()
            .id("sessions-btn")
            .h(px(CAPSULE_H))
            .px(px(11.))
            .gap(px(7.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_size(t.ui_px(12.))
            .text_color(t.dim)
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .tooltip(hover_hint("Sessions", None))
            .child(
                div()
                    .w(px(7.))
                    .h(px(7.))
                    .flex_none()
                    .rounded_full()
                    .when_else(
                        busy_any,
                        |d| d.bg(t.accent),
                        |d| d.border_1().border_color(t.faint),
                    ),
            )
            .child(count.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_sessions_menu(
                        point(ev.position.x, ev.position.y + px(10.)),
                        window,
                        cx,
                    );
                }),
            );

        TitleBar::new()
            .child(h_flex().h_full().items_center().child(sidebar_btn))
            .child(h_flex().pr(px(8.)).gap(px(8.)).child(capsule).child(sessions_btn))
    }

    /// One cell of the layout switcher: its glyph (drawn, so every cell keeps
    /// the same box on every platform and UI scale), lit when its layout is
    /// active, named in its tooltip with the layout's chord.
    fn layout_mode_button(
        &self,
        t: &KairnTheme,
        id: &'static str,
        mode: LayoutMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.layout == mode;
        let hover_bg = t.hover;
        let sel = t.sel;
        let color = if active { t.accent } else { t.dim };
        let (name, key) = match mode {
            LayoutMode::Writing => ("Writing", chord_alt("1")),
            LayoutMode::NotesFull => ("Notes", chord_alt("2")),
            LayoutMode::Split => ("Notes + Terminal", chord_alt("3")),
            LayoutMode::TerminalFull => ("Terminal", chord_alt("4")),
        };
        let glyph = match mode {
            LayoutMode::NotesFull => seg_icon_notes(color).into_any_element(),
            LayoutMode::Split => seg_icon_split(color).into_any_element(),
            LayoutMode::TerminalFull => seg_icon_term(color).into_any_element(),
            LayoutMode::Writing => seg_icon_writing(color).into_any_element(),
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(22.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |d| d.bg(sel))
            .when(!active, |d| d.hover(move |s| s.bg(hover_bg)))
            .tooltip(hover_hint(name, Some(key)))
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

/// Terminal-full glyph: the `>_` prompt drawn as strokes, so its cell keeps
/// the same box as the other drawn glyphs (as text it carried a full line
/// box that made this cell taller than its neighbours).
fn seg_icon_term(color: Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let mut chevron = PathBuilder::stroke(px(1.3));
            chevron.move_to(point(o.x + px(1.4), o.y + px(1.6)));
            chevron.line_to(point(o.x + px(5.2), o.y + px(5.5)));
            chevron.line_to(point(o.x + px(1.4), o.y + px(9.4)));
            if let Ok(path) = chevron.build() {
                window.paint_path(path, color);
            }
            let mut underscore = PathBuilder::stroke(px(1.3));
            underscore.move_to(point(o.x + px(7.6), o.y + px(9.4)));
            underscore.line_to(point(o.x + px(12.6), o.y + px(9.4)));
            if let Ok(path) = underscore.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(14.))
    .h(px(11.))
}

/// Writing glyph: ragged text lines with no frame, the chrome stripped away.
fn seg_icon_writing(color: Hsla) -> impl IntoElement {
    div()
        .w(px(13.))
        .h(px(11.))
        .flex()
        .flex_col()
        .justify_between()
        .py(px(0.5))
        .child(div().h(px(1.)).w_full().bg(color))
        .child(div().h(px(1.)).w_full().bg(color))
        .child(div().h(px(1.)).w(px(8.)).bg(color))
        .child(div().h(px(1.)).w(px(10.)).bg(color))
}
