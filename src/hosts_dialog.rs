use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, WeakEntity, Window,
    div, px,
};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::settings::SshHost;
use crate::workspace::Workspace;

pub struct HostsEditor {
    workspace: WeakEntity<Workspace>,
    rows: Vec<HostRow>,
}

struct HostRow {
    name: Entity<InputState>,
    target: Entity<InputState>,
    port: Entity<InputState>,
}

impl HostRow {
    fn new(host: Option<&SshHost>, window: &mut Window, cx: &mut Context<HostsEditor>) -> Self {
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("name");
            match host {
                Some(h) => state.default_value(h.name.clone()),
                None => state,
            }
        });
        let target = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("user@host");
            match host {
                Some(h) => state.default_value(h.target.clone()),
                None => state,
            }
        });
        let port = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("22");
            match host.and_then(|h| h.port) {
                Some(p) => state.default_value(p.to_string()),
                None => state,
            }
        });
        Self { name, target, port }
    }
}

impl HostsEditor {
    fn new(
        workspace: WeakEntity<Workspace>,
        hosts: &[SshHost],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut rows: Vec<HostRow> = hosts
            .iter()
            .map(|h| HostRow::new(Some(h), window, cx))
            .collect();
        if rows.is_empty() {
            rows.push(HostRow::new(None, window, cx));
        }
        Self { workspace, rows }
    }

    fn collect_hosts(&self, cx: &Context<Self>) -> Vec<SshHost> {
        self.rows
            .iter()
            .filter_map(|row| {
                let target = row.target.read(cx).value().trim().to_string();
                if target.is_empty() {
                    return None;
                }
                let name = row.name.read(cx).value().trim().to_string();
                let name = if name.is_empty() {
                    target.split('@').next_back().unwrap_or(&target).to_string()
                } else {
                    name
                };
                let port = row.port.read(cx).value().trim().parse::<u16>().ok();
                Some(SshHost { name, target, port })
            })
            .collect()
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosts = self.collect_hosts(cx);
        let _ = self.workspace.update(cx, |ws, cx| {
            ws.settings.ssh_hosts = hosts;
            if let Err(e) = ws.settings.save() {
                eprintln!("kairn: failed to save settings: {e}");
                window.push_notification("Could not write settings.json, see stderr.", cx);
            }
            cx.notify();
        });
        window.close_dialog(cx);
    }
}

impl Render for HostsEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex().gap_2().w_full();

        for (i, row) in self.rows.iter().enumerate() {
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(140.)).child(Input::new(&row.name)))
                    .child(div().flex_1().child(Input::new(&row.target)))
                    .child(div().w(px(70.)).child(Input::new(&row.port)))
                    .child(
                        Button::new(("host-remove", i))
                            .ghost()
                            .label("✕")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rows.remove(i);
                                cx.notify();
                            })),
                    ),
            );
        }

        list.child(
            h_flex()
                .gap_2()
                .mt_2()
                .child(
                    Button::new("host-add")
                        .outline()
                        .label("Add host")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.rows.push(HostRow::new(None, window, cx));
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("hosts-cancel")
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|_, _, window, cx| {
                            window.close_dialog(cx);
                        })),
                )
                .child(
                    Button::new("hosts-save")
                        .primary()
                        .label("Save")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save(window, cx);
                        })),
                ),
        )
    }
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let weak = cx.weak_entity();
    let hosts = workspace.settings.ssh_hosts.clone();
    let editor = cx.new(|cx| HostsEditor::new(weak, &hosts, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(600.)).title("SSH hosts").child(editor.clone())
    });
}
