//! Small shared UI pieces used across the chrome, overlays, and panes.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, px,
};

use crate::theme::{self, KairnTheme};

pub fn kbd(t: &KairnTheme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(theme::mono_font())
        .text_size(px(10.5))
        .text_color(t.faint)
        .border_1()
        .border_color(t.border)
        .rounded(px(4.))
        .px(px(4.))
        .bg(t.bg)
        .child(label.into())
}

pub(crate) fn picker_item<T: 'static>(
    t: &KairnTheme,
    id: impl Into<gpui::ElementId>,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(6.))
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}

pub(crate) fn picker_rule(t: &KairnTheme) -> impl IntoElement {
    div().my(px(5.)).mx(px(4.)).h(px(1.)).bg(t.border)
}

pub(crate) fn switcher_section(t: &KairnTheme, label: &'static str) -> impl IntoElement {
    div()
        .px(px(16.))
        .pt(px(10.))
        .pb(px(3.))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.faint)
        .child(label.to_uppercase())
}

pub(crate) fn switcher_item<T: 'static>(
    t: &KairnTheme,
    id: impl Into<gpui::ElementId>,
    _cx: &mut Context<T>,
) -> gpui::Stateful<gpui::Div> {
    let hover_bg = t.sel;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(16.))
        .py(px(6.))
        .text_color(t.dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
}
