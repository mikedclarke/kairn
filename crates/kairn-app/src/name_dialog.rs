//! A one-field prompt dialog (rename note, new note): a name input that
//! submits on Enter or the confirm button.

use std::rc::Rc;

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, WeakEntity, Window, px,
};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::workspace::Workspace;

type Submit = Rc<dyn Fn(&mut Workspace, &str, &mut Window, &mut Context<Workspace>)>;

pub struct NamePrompt {
    workspace: WeakEntity<Workspace>,
    input: Entity<InputState>,
    confirm: SharedString,
    on_submit: Submit,
    _sub: Subscription,
}

impl NamePrompt {
    fn new(
        workspace: WeakEntity<Workspace>,
        initial: Option<String>,
        confirm: SharedString,
        on_submit: Submit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("Note name");
            match initial {
                Some(text) if !text.is_empty() => state.default_value(text),
                _ => state,
            }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        });
        Self { workspace, input, confirm, on_submit, _sub: sub }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let on_submit = self.on_submit.clone();
        let _ = self.workspace.update(cx, |ws, cx| {
            on_submit(ws, &name, window, cx);
        });
        window.close_dialog(cx);
    }
}

impl Render for NamePrompt {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .w_full()
            .child(Input::new(&self.input))
            .child(
                h_flex()
                    .gap_2()
                    .mt_1()
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("name-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("name-confirm")
                            .primary()
                            .label(self.confirm.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(window, cx);
                            })),
                    ),
            )
    }
}

/// Open the prompt over the workspace. `on_submit` runs with the trimmed,
/// non-empty name when the user confirms.
pub fn open(
    title: &'static str,
    confirm: &'static str,
    initial: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    on_submit: impl Fn(&mut Workspace, &str, &mut Window, &mut Context<Workspace>) + 'static,
) {
    let weak = cx.weak_entity();
    let on_submit: Submit = Rc::new(on_submit);
    let prompt =
        cx.new(|cx| NamePrompt::new(weak, initial, confirm.into(), on_submit, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(420.)).title(title).child(prompt.clone())
    });
}
