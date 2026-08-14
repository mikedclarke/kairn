//! Small shared UI pieces used across the chrome, overlays, and panes.

use gpui::{
    AnyView, App, AppContext as _, Context, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use kairn_core::tasks::DayTaskStats;

use crate::theme::{KairnTheme, KairnThemeExt as _};

/// The NotePlan-style day indicator shared by the calendar grid and the week
/// strip so the two can't drift: a hollow ring while any of the day's tasks
/// are open (red once the day is past), a tick when they're all done, nothing
/// on a task-free day. `base` is the ring colour for this cell's context and
/// `emphasized` marks the day that sits on the amber pill (today in the
/// calendar, the selected day in the strip), which flips the colours to read
/// against amber. Returns `None` when there's nothing to show; wrap the result
/// in the caller's fixed-height slot so the layout never shifts.
pub(crate) fn day_task_indicator(
    t: &KairnTheme,
    stats: DayTaskStats,
    base: Hsla,
    overdue_open: bool,
    emphasized: bool,
) -> Option<gpui::Div> {
    if stats.open > 0 {
        let ring = if overdue_open && !emphasized { t.red } else { base };
        Some(div().w(px(5.)).h(px(5.)).rounded_full().border_1().border_color(ring))
    } else if stats.done > 0 {
        let check = if emphasized { t.on_amber } else { t.accent };
        Some(
            div()
                .text_size(px(7.5))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(check)
                .child("✓"),
        )
    } else {
        None
    }
}

/// The hover hint on chrome controls: the control's name with its keyboard
/// chord beneath in smaller faint type, so the shortcuts are learnable by
/// exploring the UI with the mouse.
struct HoverHint {
    name: SharedString,
    key: Option<SharedString>,
}

impl Render for HoverHint {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.kairn().clone();
        div()
            .m(px(8.))
            .flex()
            .flex_col()
            .items_center()
            .px(px(11.))
            .py(px(5.))
            .rounded(px(7.))
            .bg(t.panel2)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .text_size(t.ui_px(12.))
            .text_color(t.text)
            .child(self.name.clone())
            .when_some(self.key.clone(), |d, key| {
                d.child(
                    div()
                        .font_family(t.mono_font.clone())
                        .text_size(t.ui_px(10.5))
                        .text_color(t.faint)
                        .child(key),
                )
            })
    }
}

/// Tooltip builder for [`gpui::StatefulInteractiveElement::tooltip`]: a
/// [`HoverHint`] with the given name and optional chord label.
pub(crate) fn hover_hint(
    name: impl Into<SharedString>,
    key: Option<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let name = name.into();
    let key: Option<SharedString> = key.map(SharedString::from);
    move |_, cx| {
        cx.new(|_| HoverHint { name: name.clone(), key: key.clone() })
            .into()
    }
}

/// A larger, full-contrast key chip for the settings keybinds list, where the
/// binding is the content and must be easy to read at a glance.
pub fn kbd_key(t: &KairnTheme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(t.mono_font.clone())
        .text_size(px(14.))
        .text_color(t.text)
        .border_1()
        .border_color(t.border)
        .rounded(px(5.))
        .px(px(6.))
        .py(px(1.))
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
