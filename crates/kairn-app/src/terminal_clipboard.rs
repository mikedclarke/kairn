use std::{cell::Cell, rc::Rc};

use gpui::{App, ClipboardItem, Context, PromptButton, PromptLevel, Window};
use gpui_component::{WindowExt as _, notification::Notification};
use gpui_terminal::TerminalView;

#[derive(Clone, Copy)]
enum Permission {
    Ask,
    Pending,
    Allowed,
    Denied,
}

type FocusCheck = dyn Fn(&Window, &App) -> bool;

#[derive(Clone)]
pub struct TerminalClipboard {
    permission: Rc<Cell<Permission>>,
    focused: Rc<FocusCheck>,
}

impl TerminalClipboard {
    pub fn new(focused: impl Fn(&Window, &App) -> bool + 'static) -> Self {
        Self {
            permission: Rc::new(Cell::new(Permission::Ask)),
            focused: Rc::new(focused),
        }
    }

    pub fn store(&self, text: &str, window: &mut Window, cx: &mut Context<TerminalView>) {
        if !(self.focused)(window, cx) {
            return;
        }
        match self.permission.get() {
            Permission::Denied | Permission::Pending => (),
            Permission::Allowed => Self::copy(text, window, cx),
            Permission::Ask => {
                self.permission.set(Permission::Pending);
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "Allow this terminal to replace the clipboard?",
                    Some("Programs in this session will be allowed to copy text while this terminal is focused."),
                    &[PromptButton::cancel("Don't allow"), PromptButton::new("Allow this session")],
                    cx,
                );
                let access = self.clone();
                let text = text.to_owned();
                cx.spawn_in(window, async move |terminal, cx| {
                    let allowed = matches!(answer.await, Ok(1));
                    access.permission.set(if allowed {
                        Permission::Allowed
                    } else {
                        Permission::Denied
                    });
                    let _ = terminal.update_in(cx, |_, window, cx| {
                        if allowed && (access.focused)(window, cx) {
                            Self::copy(&text, window, cx);
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn copy(text: &str, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        window.push_notification(
            Notification::new()
                .id::<Self>()
                .message("Terminal copied text to the clipboard."),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalClipboard;
    use gpui::{AppContext, ClipboardItem, EmptyView, TestAppContext};
    use gpui_component::Root;
    use gpui_terminal::{TerminalConfig, TerminalView};
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn clipboard_consent_is_per_session_and_focus_is_rechecked(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| EmptyView);
            Root::new(empty, window, cx)
        });
        let focused = Rc::new(Cell::new(false));
        let check = focused.clone();
        let access = TerminalClipboard::new(move |_, _| check.get());
        let terminal = cx.update(|window, cx| {
            cx.new(|cx| {
                TerminalView::new(
                    std::io::sink(),
                    std::io::empty(),
                    TerminalConfig::default(),
                    window,
                    cx,
                )
            })
        });
        cx.write_to_clipboard(ClipboardItem::new_string("original".into()));
        let request = |text: &str, cx: &mut gpui::VisualTestContext| {
            terminal.update_in(cx, |_, window, cx| access.store(text, window, cx));
        };
        request("hidden", cx);
        assert!(!cx.has_pending_prompt());
        focused.set(true);
        request("requested", cx);
        assert!(cx.has_pending_prompt());
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "original"
        );
        request("while pending", cx);
        focused.set(false);
        cx.simulate_prompt_answer("Allow this session");
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "original"
        );
        focused.set(true);
        request("permitted", cx);
        assert!(!cx.has_pending_prompt());
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "permitted"
        );

        let denied = TerminalClipboard::new(|_, _| true);
        terminal.update_in(cx, |_, window, cx| denied.store("new session", window, cx));
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Don't allow");
        cx.run_until_parked();
        terminal.update_in(cx, |_, window, cx| denied.store("still denied", window, cx));
        assert!(!cx.has_pending_prompt());
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "permitted"
        );
    }
}
