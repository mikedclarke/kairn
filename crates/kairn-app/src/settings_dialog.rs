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

use kairn_core::settings::SshHost;
use crate::keymap::keybind_list;
use crate::theme::{KairnThemeExt as _, Mode};
use crate::ui::kbd;
use crate::workspace::Workspace;

/// Settings sections, one tab each; more arrive as the page grows
/// (templates, theming…).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Ssh,
    Keybinds,
}

impl Tab {
    const ALL: [(Tab, &'static str); 3] = [
        (Tab::General, "General"),
        (Tab::Ssh, "SSH hosts"),
        (Tab::Keybinds, "Keybinds"),
    ];
}

pub struct SettingsEditor {
    workspace: WeakEntity<Workspace>,
    tab: Tab,
    notes_root: Entity<InputState>,
    rows: Vec<HostRow>,
}

struct HostRow {
    name: Entity<InputState>,
    target: Entity<InputState>,
    port: Entity<InputState>,
}

impl HostRow {
    fn new(host: Option<&SshHost>, window: &mut Window, cx: &mut Context<SettingsEditor>) -> Self {
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

impl SettingsEditor {
    fn new(
        workspace: WeakEntity<Workspace>,
        notes_root_raw: Option<String>,
        hosts: &[SshHost],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let notes_root = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("~/kairn");
            match notes_root_raw {
                Some(raw) if !raw.is_empty() => state.default_value(raw),
                _ => state,
            }
        });
        let mut rows: Vec<HostRow> = hosts
            .iter()
            .map(|h| HostRow::new(Some(h), window, cx))
            .collect();
        if rows.is_empty() {
            rows.push(HostRow::new(None, window, cx));
        }
        Self { workspace, tab: Tab::General, notes_root, rows }
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
        let raw = self.notes_root.read(cx).value().trim().to_string();
        let notes_root = (!raw.is_empty()).then_some(raw);
        let hosts = self.collect_hosts(cx);
        let _ = self.workspace.update(cx, |ws, cx| {
            ws.apply_settings(notes_root, hosts, window, cx);
        });
        window.close_dialog(cx);
    }

    fn section(label: &'static str) -> gpui::Div {
        div()
            .mt(px(6.))
            .text_size(px(10.5))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .opacity(0.55)
            .child(label.to_uppercase())
    }

    fn render_general(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mode = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).mode())
            .unwrap_or(Mode::Dark);
        let daily_forward = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).settings.daily_forward)
            .unwrap_or(true);
        let resolved = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).notes_root.display().to_string())
            .unwrap_or_default();

        let theme_button = |id: &'static str, label: &'static str, m: Mode| {
            let btn = Button::new(id).label(label);
            let btn = if m == mode { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, window, cx| {
                let _ = this.workspace.update(cx, |ws, cx| {
                    ws.set_theme(m, window, cx);
                });
                cx.notify();
            }))
        };
        let daily_button = |id: &'static str, label: &'static str, forward: bool| {
            let btn = Button::new(id).label(label);
            let btn = if forward == daily_forward { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                let _ = this.workspace.update(cx, |ws, cx| {
                    ws.set_daily_forward(forward, cx);
                });
                cx.notify();
            }))
        };

        v_flex()
            .gap_2()
            .w_full()
            .child(Self::section("Notes folder"))
            .child(Input::new(&self.notes_root))
            .child(div().text_size(px(11.)).opacity(0.55).child(format!(
                "Currently {resolved}. A NotePlan-style folder works as-is; Calendar/, \
                 Notes/ and .kairn/ are created if missing."
            )))
            .child(Self::section("Theme"))
            .child(
                h_flex()
                    .gap_2()
                    .child(theme_button("theme-dark", "Dark", Mode::Dark))
                    .child(theme_button("theme-light", "Light", Mode::Light)),
            )
            .child(Self::section("Sidebar daily list"))
            .child(
                h_flex()
                    .gap_2()
                    .child(daily_button("daily-forward", "Today + next 2 days", true))
                    .child(daily_button("daily-back", "Today + previous 2 days", false)),
            )
    }

    fn render_ssh(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut root = v_flex().gap_2().w_full().child(Self::section("SSH hosts"));
        for (i, row) in self.rows.iter().enumerate() {
            root = root.child(
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
        root.child(
            h_flex().child(
                Button::new("host-add")
                    .outline()
                    .label("Add host")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.rows.push(HostRow::new(None, window, cx));
                        cx.notify();
                    })),
            ),
        )
    }

    fn render_keybinds(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = cx.kairn().clone();
        let mut root = v_flex().gap_1().w_full();
        for (group, binds) in keybind_list() {
            root = root.child(Self::section(group));
            for (chord, what) in binds {
                root = root.child(
                    h_flex()
                        .items_center()
                        .py(px(2.))
                        .child(div().flex_1().text_size(px(12.5)).child(what))
                        .child(kbd(&t, chord)),
                );
            }
        }
        root
    }
}

impl Render for SettingsEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = h_flex().gap_1().children(Tab::ALL.map(|(tab, label)| {
            let btn = Button::new(label).label(label);
            let btn = if tab == self.tab { btn.primary() } else { btn.ghost() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
        }));

        let content = match self.tab {
            Tab::General => self.render_general(cx),
            Tab::Ssh => self.render_ssh(cx),
            Tab::Keybinds => self.render_keybinds(cx),
        };

        v_flex()
            .gap_2()
            .w_full()
            .child(tabs)
            .child(content)
            .child(
                h_flex()
                    .gap_2()
                    .mt_2()
                    .child(div().flex_1())
                    .child(
                        Button::new("settings-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("settings-save")
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
    let notes_root_raw = workspace.settings.notes_root.clone();
    let hosts = workspace.settings.ssh_hosts.clone();
    let editor = cx.new(|cx| SettingsEditor::new(weak, notes_root_raw, &hosts, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(600.)).title("Settings").child(editor.clone())
    });
}
