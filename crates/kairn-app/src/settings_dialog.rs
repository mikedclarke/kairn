use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, PathPromptOptions, Render, Styled,
    WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    select::{SearchableVec, Select, SelectState},
    v_flex,
};

use gpui::SharedString;
use kairn_core::settings::{HostApp, SshHost};
use kairn_core::themes::ThemeEntry;
use crate::cli_install;
use crate::keymap::keybind_list;
use crate::theme::KairnThemeExt as _;
use crate::ui::kbd_key;
use crate::workspace::Workspace;

/// Settings sections, one tab each; more arrive as the page grows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Theme,
    Templates,
    Ssh,
    Keybinds,
}

impl Tab {
    const ALL: [(Tab, &'static str); 5] = [
        (Tab::General, "General"),
        (Tab::Theme, "Theme"),
        (Tab::Templates, "Templates"),
        (Tab::Ssh, "SSH hosts"),
        (Tab::Keybinds, "Keybinds"),
    ];
}

type FontSelect = Entity<SelectState<SearchableVec<String>>>;

/// The sentinel first entry of every font picker: no override stored, the
/// built-in choice applies.
const DEFAULT_FONT: &str = "Default";

/// Curated proportional/serif families for the interface and notes pickers,
/// spanning macOS and Linux. Only the ones actually installed are offered, so
/// the list stays short and every option renders — rather than dumping every
/// font on the machine. A family already configured but not in this set is
/// still added so it never silently drops.
const TEXT_FONTS: &[&str] = &[
    "Inter",
    "SF Pro Text",
    "Helvetica Neue",
    "Avenir Next",
    "Optima",
    "Charter",
    "Iowan Old Style",
    "New York",
    "Georgia",
    "Cantarell",
    "Noto Sans",
    "Adwaita Sans",
    "DejaVu Sans",
    "Ubuntu",
];

/// Curated monospace families for the terminal & mono picker, same rule.
const MONO_FONTS: &[&str] = &[
    "SF Mono",
    "Menlo",
    "JetBrains Mono",
    "Fira Code",
    "Hack",
    "IBM Plex Mono",
    "Cascadia Code",
    "Source Code Pro",
    "Adwaita Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
];

pub struct SettingsEditor {
    workspace: WeakEntity<Workspace>,
    tab: Tab,
    /// Pending notes folder, in the form settings.json stores (`~/...` when
    /// under home). None means the default `~/kairn`. Set via the native
    /// folder picker; lands with Save.
    notes_root_choice: Option<String>,
    rows: Vec<HostRow>,
    local_apps: Vec<AppRow>,
    /// The daily template body as loaded at open, to skip a rewrite (and a
    /// watcher round-trip) when Save changed nothing.
    template_loaded: String,
    template_body: Entity<InputState>,
    /// Pending apply rule; lands with Save like the body.
    template_rule: String,
    /// Pending theme id ("dark"/"light" or a theme-file stem); lands with
    /// Save, so browsing choices never repaints the app behind the dialog.
    theme_choice: String,
    /// Theme files found in `.kairn/themes/` at open.
    themes: Vec<ThemeEntry>,
    ui_font: FontSelect,
    editor_font: FontSelect,
    mono_font: FontSelect,
    editor_size: Entity<InputState>,
    ui_size: Entity<InputState>,
    /// Font settings as loaded, kept so a font that isn't installed on this
    /// machine (empty picker selection) survives a Save untouched, plus the
    /// loaded sizes so nonsense input falls back rather than resetting.
    fonts_loaded: (Option<String>, Option<String>, Option<String>, Option<f32>),
    ui_size_loaded: Option<f32>,
    /// Result line under the "Install kairn command" button, set on click.
    cli_status: Option<String>,
}

struct HostRow {
    name: Entity<InputState>,
    target: Entity<InputState>,
    port: Entity<InputState>,
    apps: Vec<AppRow>,
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
        let apps = host
            .map(|h| h.apps.iter().map(|a| AppRow::new(Some(a), window, cx)).collect())
            .unwrap_or_default();
        Self { name, target, port, apps }
    }
}

/// One editable shortcut: a name (optional) and the command it runs.
struct AppRow {
    name: Entity<InputState>,
    command: Entity<InputState>,
}

impl AppRow {
    fn new(app: Option<&HostApp>, window: &mut Window, cx: &mut Context<SettingsEditor>) -> Self {
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("name (e.g. herdr)");
            match app {
                Some(a) => state.default_value(a.name.clone()),
                None => state,
            }
        });
        let command = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("command (e.g. herdr)");
            match app {
                Some(a) => state.default_value(a.command.clone()),
                None => state,
            }
        });
        Self { name, command }
    }
}

/// Collect a list of app rows into settings values, dropping rows whose
/// command is empty (the same posture as hosts without a target).
fn collect_apps(rows: &[AppRow], cx: &Context<SettingsEditor>) -> Vec<HostApp> {
    rows.iter()
        .filter_map(|row| {
            let command = row.command.read(cx).value().trim().to_string();
            if command.is_empty() {
                return None;
            }
            let name = row.name.read(cx).value().trim().to_string();
            Some(HostApp { name, command })
        })
        .collect()
}

impl SettingsEditor {
    fn new(
        workspace: WeakEntity<Workspace>,
        ws: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let notes_root_choice = ws.settings.notes_root.clone().filter(|r| !r.is_empty());
        let mut rows: Vec<HostRow> = ws
            .settings
            .ssh_hosts
            .iter()
            .map(|h| HostRow::new(Some(h), window, cx))
            .collect();
        if rows.is_empty() {
            rows.push(HostRow::new(None, window, cx));
        }
        let local_apps = ws
            .settings
            .local_apps
            .iter()
            .map(|a| AppRow::new(Some(a), window, cx))
            .collect();
        let template_loaded = kairn_core::template::daily_template_body(&ws.notes_root);
        let template_body = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(10)
                // Single line only: a newline inside a placeholder panics
                // gpui's Mac text layout (docs/vendor NOTES).
                .placeholder("Markdown seeded into each new daily note")
                .default_value(template_loaded.clone())
        });

        let themes = kairn_core::themes::list_themes(&ws.notes_root);
        // Installed families, macOS dot-prefixed system internals excluded, as
        // a set to filter the curated candidate lists against.
        let installed: std::collections::HashSet<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|f| !f.starts_with('.'))
            .collect();
        let curate = |candidates: &[&str]| -> Vec<String> {
            candidates
                .iter()
                .filter(|c| installed.contains(**c))
                .map(|c| c.to_string())
                .collect()
        };
        let text_fonts = curate(TEXT_FONTS);
        let mono_fonts = curate(MONO_FONTS);
        let mut font_select =
            |candidates: &[String], current: &Option<String>, cx: &mut Context<Self>| -> FontSelect {
                let mut items = Vec::with_capacity(candidates.len() + 2);
                items.push(DEFAULT_FONT.to_string());
                // A configured family outside the curated set stays selectable.
                if let Some(cur) = current
                    && !candidates.iter().any(|c| c == cur)
                {
                    items.push(cur.clone());
                }
                items.extend(candidates.iter().cloned());
                let selected = current.clone().unwrap_or_else(|| DEFAULT_FONT.to_string());
                cx.new(|cx| {
                    let mut state = SelectState::new(SearchableVec::new(items), None, window, cx)
                        .searchable(true);
                    state.set_selected_value(&selected, window, cx);
                    state
                })
            };
        let ui_font = font_select(&text_fonts, &ws.settings.ui_font, cx);
        let editor_font = font_select(&text_fonts, &ws.settings.editor_font, cx);
        let mono_font = font_select(&mono_fonts, &ws.settings.mono_font, cx);
        let editor_size = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("13");
            match ws.settings.editor_font_size {
                Some(s) => state.default_value(fmt_size(s)),
                None => state,
            }
        });
        let ui_size = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("13");
            match ws.settings.ui_font_size {
                Some(s) => state.default_value(fmt_size(s)),
                None => state,
            }
        });

        Self {
            workspace,
            tab: Tab::General,
            notes_root_choice,
            rows,
            local_apps,
            template_loaded,
            template_body,
            template_rule: ws.settings.daily_template_rule.clone(),
            theme_choice: ws.settings.theme.clone(),
            themes,
            ui_font,
            editor_font,
            mono_font,
            editor_size,
            ui_size,
            cli_status: None,
            fonts_loaded: (
                ws.settings.ui_font.clone(),
                ws.settings.editor_font.clone(),
                ws.settings.mono_font.clone(),
                ws.settings.editor_font_size,
            ),
            ui_size_loaded: ws.settings.ui_font_size,
        }
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
                let apps = collect_apps(&row.apps, cx);
                Some(SshHost { name, target, port, apps })
            })
            .collect()
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.template_body.read(cx).value().to_string();
        let font_of = |sel: &FontSelect, loaded: &Option<String>| match sel
            .read(cx)
            .selected_value()
        {
            Some(v) if v == DEFAULT_FONT => None,
            Some(v) => Some(v.clone()),
            // A configured family that isn't installed here selects nothing;
            // keep it rather than silently dropping the other machine's font.
            None => loaded.clone(),
        };
        // Parse a size box: empty clears the override, a valid 9–32 sets it,
        // and nonsense falls back to what was loaded rather than resetting.
        let parse_size = |state: &Entity<InputState>, loaded: Option<f32>| {
            let raw = state.read(cx).value().trim().to_string();
            if raw.is_empty() {
                return None;
            }
            match raw.parse::<f32>() {
                Ok(s) if (9.0..=32.0).contains(&s) => Some(s),
                _ => loaded,
            }
        };
        let editor_font_size = parse_size(&self.editor_size, self.fonts_loaded.3);
        let ui_font_size = parse_size(&self.ui_size, self.ui_size_loaded);
        let patch = crate::vault_state::SettingsPatch {
            notes_root: self.notes_root_choice.clone(),
            hosts: self.collect_hosts(cx),
            local_apps: collect_apps(&self.local_apps, cx),
            daily_template_rule: self.template_rule.clone(),
            template_body: (body != self.template_loaded).then_some(body),
            theme: self.theme_choice.clone(),
            ui_font: font_of(&self.ui_font, &self.fonts_loaded.0),
            editor_font: font_of(&self.editor_font, &self.fonts_loaded.1),
            mono_font: font_of(&self.mono_font, &self.fonts_loaded.2),
            editor_font_size,
            ui_font_size,
        };
        let _ = self.workspace.update(cx, |ws, cx| {
            ws.apply_settings(patch, window, cx);
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
        let daily_forward = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).settings.daily_forward)
            .unwrap_or(true);
        let week_strip = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).settings.week_strip.clone())
            .unwrap_or_else(|| "always".to_string());
        let day_timeline = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).settings.day_timeline)
            .unwrap_or(true);
        let show_agents = self
            .workspace
            .upgrade()
            .map(|ws| ws.read(cx).settings.show_agents)
            .unwrap_or(true);
        let resolved = self
            .workspace
            .upgrade()
            .map(|ws| home_relative(&ws.read(cx).notes_root))
            .unwrap_or_default();

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
        let strip_button = |id: &'static str, label: &'static str, mode: &'static str| {
            let btn = Button::new(id).label(label);
            let btn = if mode == week_strip { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                let _ = this.workspace.update(cx, |ws, cx| {
                    ws.set_week_strip(mode, cx);
                });
                cx.notify();
            }))
        };
        let timeline_button = |id: &'static str, label: &'static str, on: bool| {
            let btn = Button::new(id).label(label);
            let btn = if on == day_timeline { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                let _ = this.workspace.update(cx, |ws, cx| {
                    ws.set_day_timeline(on, cx);
                });
                cx.notify();
            }))
        };
        let agents_button = |id: &'static str, label: &'static str, on: bool| {
            let btn = Button::new(id).label(label);
            let btn = if on == show_agents { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                let _ = this.workspace.update(cx, |ws, cx| {
                    ws.set_show_agents(on, cx);
                });
                cx.notify();
            }))
        };

        v_flex()
            .gap_2()
            .w_full()
            .child(Self::section("Notes folder"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .w_full()
                    .child(
                        // min_w_0: a long unbreakable path must not drive the
                        // column's min-content width past the dialog (that
                        // collapses sibling layouts to zero width).
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.5))
                            .child(
                                self.notes_root_choice
                                    .clone()
                                    .unwrap_or_else(|| "~/kairn (default)".to_string()),
                            ),
                    )
                    .child(
                        Button::new("notes-root-choose")
                            .outline()
                            .label("Choose folder…")
                            .on_click(cx.listener(|_, _, _, cx| {
                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: false,
                                    prompt: Some("Use this folder".into()),
                                });
                                cx.spawn(async move |this, cx| {
                                    if let Ok(Ok(Some(mut paths))) = rx.await
                                        && let Some(path) = paths.pop()
                                    {
                                        let _ = this.update(cx, |this, cx| {
                                            this.notes_root_choice =
                                                Some(home_relative(&path));
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            })),
                    )
                    .when(self.notes_root_choice.is_some(), |this| {
                        this.child(
                            Button::new("notes-root-default")
                                .ghost()
                                .label("Use default")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.notes_root_choice = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(div().min_w_0().text_size(px(11.)).opacity(0.55).child(format!(
                "Currently {resolved}; a change lands on Save. A NotePlan-style folder \
                 works as-is; Calendar/, Notes/ and .kairn/ are created if missing."
            )))
            .child(Self::section("Sidebar daily list"))
            .child(
                h_flex()
                    .gap_2()
                    .child(daily_button("daily-forward", "Today + next 2 days", true))
                    .child(daily_button("daily-back", "Today + previous 2 days", false)),
            )
            .child(Self::section("Week strip above notes"))
            .child(
                h_flex()
                    .gap_2()
                    .child(strip_button("strip-always", "Always", "always"))
                    .child(strip_button("strip-daily", "Daily notes only", "daily"))
                    .child(strip_button("strip-off", "Hidden", "off")),
            )
            .child(Self::section("Day timeline"))
            .child(
                h_flex()
                    .gap_2()
                    .child(timeline_button("timeline-on", "Shown", true))
                    .child(timeline_button("timeline-off", "Hidden", false)),
            )
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "The pill row of timed lines (09:00 standup) at the top of a daily note.",
            ))
            .child(Self::section("Agents section"))
            .child(
                h_flex()
                    .gap_2()
                    .child(agents_button("agents-on", "Shown", true))
                    .child(agents_button("agents-off", "Hidden", false)),
            )
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "The sidebar feed of agent CLI activity. Hide it if agents only run on \
                 other machines over SSH.",
            ))
            .child(Self::section("Command line tool"))
            .child(self.render_cli(cx))
            .child(Self::section("About"))
            .child(
                div()
                    .text_size(px(11.))
                    .opacity(0.55)
                    .child(concat!("Kairn ", env!("CARGO_PKG_VERSION"))),
            )
    }

    fn render_cli(&self, cx: &mut Context<Self>) -> gpui::Div {
        if cli_install::already_installed() {
            return div().text_size(px(11.)).opacity(0.55).child(
                "The kairn command is on your PATH. Terminals and agents can run it directly.",
            );
        }
        v_flex()
            .gap_2()
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "Add the kairn command to your PATH so terminals and agents can read notes, \
                 list tasks, and capture from the command line.",
            ))
            .child(
                Button::new("cli-install")
                    .outline()
                    .label("Install kairn command")
                    // Run off the UI thread: the install may raise a native
                    // admin-auth prompt, which must not block rendering.
                    .on_click(cx.listener(|_, _, _, cx| {
                        let install =
                            cx.background_executor().spawn(async { cli_install::install() });
                        cx.spawn(async move |this, cx| {
                            let msg = match install.await {
                                cli_install::Outcome::Linked(p) => {
                                    format!("Installed to {}.", p.display())
                                }
                                cli_install::Outcome::Manual { reason, command }
                                    if command.is_empty() =>
                                {
                                    reason
                                }
                                cli_install::Outcome::Manual { reason, command } => {
                                    format!("{reason} Run this in a terminal: {command}")
                                }
                            };
                            let _ = this.update(cx, |this, cx| {
                                this.cli_status = Some(msg);
                                cx.notify();
                            });
                        })
                        .detach();
                    })),
            )
            .when_some(self.cli_status.clone(), |this, s| {
                this.child(div().text_size(px(11.)).opacity(0.7).child(s))
            })
    }

    fn render_theme(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut choices: Vec<(String, String)> = vec![
            ("dark".to_string(), "Dark".to_string()),
            ("light".to_string(), "Light".to_string()),
        ];
        choices.extend(
            crate::theme::BUILTIN_PRESETS
                .iter()
                .map(|(id, name)| (id.to_string(), name.to_string())),
        );
        choices.extend(self.themes.iter().map(|t| (t.id.clone(), t.name.clone())));
        let mut theme_row = h_flex().gap_2().flex_wrap();
        for (i, (id, name)) in choices.into_iter().enumerate() {
            let btn = Button::new(("theme-choice", i)).label(name);
            let btn = if id == self.theme_choice { btn.primary() } else { btn.outline() };
            theme_row = theme_row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                this.theme_choice = id.clone();
                cx.notify();
            })));
        }

        let label = |text: &'static str| {
            div().w(px(120.)).flex_none().text_size(px(12.5)).child(text)
        };
        let font_row = |text: &'static str, sel: &FontSelect| {
            h_flex()
                .gap_2()
                .items_center()
                .child(label(text))
                .child(div().flex_1().child(Select::new(sel)))
        };

        v_flex()
            .gap_2()
            .w_full()
            .child(Self::section("Theme"))
            .child(theme_row)
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "Dark, Light, and the coloured presets are built in. For full \
                 control, custom themes are JSON files in .kairn/themes/ inside \
                 your notes folder; any colours, fonts, and terminal shades they \
                 leave out fall back to the built-ins.",
            ))
            .child(Self::section("Fonts"))
            .child(font_row("Interface", &self.ui_font))
            .child(font_row("Notes editor", &self.editor_font))
            .child(font_row("Terminal & mono", &self.mono_font))
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "A curated set of the families installed on this machine. Default \
                 keeps the built-in choice: the system font for the interface, the \
                 interface font for notes, and an auto-detected mono for the \
                 terminal. Any other installed font can be set in a theme file.",
            ))
            .child(Self::section("Interface text size"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(70.)).flex_none().child(Input::new(&self.ui_size)))
                    .child(div().text_size(px(11.)).opacity(0.55).child(
                        "In pixels; the whole app chrome (sidebar, calendar, panes) \
                         scales from it. Default 13.",
                    )),
            )
            .child(Self::section("Editor text size"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(70.)).flex_none().child(Input::new(&self.editor_size)))
                    .child(div().text_size(px(11.)).opacity(0.55).child(
                        "In pixels; headings scale with it. Default 13.",
                    )),
            )
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "Changes apply when you save.",
            ))
    }

    fn render_templates(&self, cx: &mut Context<Self>) -> gpui::Div {
        let rule_button = |id: &'static str, label: &'static str, rule: &'static str| {
            let btn = Button::new(id).label(label);
            let btn = if rule == self.template_rule { btn.primary() } else { btn.outline() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                this.template_rule = rule.to_string();
                cx.notify();
            }))
        };

        v_flex()
            .gap_2()
            .w_full()
            .child(Self::section("Daily template"))
            .child(
                div()
                    .font_family(cx.kairn().mono_font.clone())
                    .text_size(px(12.))
                    .child(Input::new(&self.template_body).h(px(230.))),
            )
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "Seeds a new daily note the first time you edit it; past days are never \
                 templated. Saved to Notes/@Templates/Daily.md, the same file NotePlan \
                 reads, keeping any frontmatter the file already has.",
            ))
            .child(Self::section("Applies to"))
            .child(
                h_flex()
                    .gap_2()
                    .child(rule_button("tpl-always", "Every day", "always"))
                    .child(rule_button("tpl-weekdays", "Weekdays only", "weekdays"))
                    .child(rule_button("tpl-off", "No days (off)", "off")),
            )
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "Changes apply when you save.",
            ))
    }

    /// A shortcut's editable row: name, command, remove. `host` indexes
    /// `rows`; None edits the local list.
    fn app_row_ui(&self, host: Option<usize>, j: usize, cx: &mut Context<Self>) -> gpui::Div {
        let row = match host {
            Some(i) => &self.rows[i].apps[j],
            None => &self.local_apps[j],
        };
        let rm_id = SharedString::from(match host {
            Some(i) => format!("app-remove-{i}-{j}"),
            None => format!("local-app-remove-{j}"),
        });
        h_flex()
            .gap_2()
            .items_center()
            .when(host.is_some(), |d| d.pl(px(18.)))
            .child(div().w(px(140.)).child(Input::new(&row.name)))
            .child(div().flex_1().child(Input::new(&row.command)))
            .child(
                Button::new(rm_id)
                    .ghost()
                    .label("✕")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match host {
                            Some(i) => {
                                this.rows[i].apps.remove(j);
                            }
                            None => {
                                this.local_apps.remove(j);
                            }
                        }
                        cx.notify();
                    })),
            )
    }

    fn render_ssh(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut root = v_flex()
            .gap_2()
            .w_full()
            .child(Self::section("Shortcuts on this machine"))
            .child(div().text_size(px(11.)).opacity(0.55).child(
                "A shortcut opens a command in its own session, from the Sessions + menu \
                 and the start page. When the command exits, the session drops to a shell.",
            ));
        for j in 0..self.local_apps.len() {
            root = root.child(self.app_row_ui(None, j, cx));
        }
        root = root.child(
            h_flex().child(
                Button::new("local-app-add")
                    .outline()
                    .label("Add shortcut")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.local_apps.push(AppRow::new(None, window, cx));
                        cx.notify();
                    })),
            ),
        );

        root = root.child(Self::section("SSH hosts"));
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
            for j in 0..self.rows[i].apps.len() {
                root = root.child(self.app_row_ui(Some(i), j, cx));
            }
            root = root.child(
                h_flex().pl(px(18.)).child(
                    Button::new(SharedString::from(format!("host-app-add-{i}")))
                        .ghost()
                        .label("Add shortcut")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let app = AppRow::new(None, window, cx);
                            this.rows[i].apps.push(app);
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
                        .gap_2()
                        .py(px(3.))
                        .child(div().flex_1().text_size(px(13.)).child(what))
                        .child(kbd_key(&t, chord)),
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
            Tab::Theme => self.render_theme(cx),
            Tab::Templates => self.render_templates(cx),
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

/// A path for display: home-relative (`~/...`) when it is under $HOME.
fn home_relative(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

/// A size for the input field: "13", not "13.0", but "14.5" stays exact.
fn fmt_size(s: f32) -> String {
    if s.fract() == 0.0 {
        format!("{}", s as i32)
    } else {
        format!("{s}")
    }
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let weak = cx.weak_entity();
    let editor = cx.new(|cx| SettingsEditor::new(weak, workspace, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(600.)).title("Settings").child(editor.clone())
    });
}
