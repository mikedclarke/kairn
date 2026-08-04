use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{
    AppContext, Context, Edges, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, div, px,
};
use gpui_terminal::{ColorPalette, TerminalConfig, TerminalView};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const FONT_SIZE: f32 = 14.0;
const SCROLLBACK_LINES: usize = 10_000;

// Font resolution goes through the platform text system, so the default family
// has to exist per-OS; everything else is shared.
fn default_font() -> String {
    if cfg!(target_os = "macos") {
        "Menlo".into()
    } else {
        "DejaVu Sans Mono".into()
    }
}

fn dark_palette() -> ColorPalette {
    ColorPalette::builder()
        .background(0x14, 0x14, 0x16)
        .foreground(0xC9, 0xC7, 0xCD)
        .cursor(0xC9, 0xC7, 0xCD)
        .black(0x10, 0x10, 0x10)
        .red(0xEF, 0xA6, 0xA2)
        .green(0x80, 0xC9, 0x90)
        .yellow(0xC8, 0xB0, 0x60)
        .blue(0xA3, 0xB8, 0xEF)
        .magenta(0xE6, 0xA3, 0xDC)
        .cyan(0x50, 0xCA, 0xCD)
        .white(0xB0, 0xB0, 0xB0)
        .bright_black(0x5C, 0x63, 0x70)
        .bright_red(0xF2, 0xB4, 0xB0)
        .bright_green(0x9A, 0xD8, 0xA8)
        .bright_yellow(0xD8, 0xC8, 0x84)
        .bright_blue(0xB8, 0xC8, 0xF4)
        .bright_magenta(0xF2, 0xB8, 0xE8)
        .bright_cyan(0x74, 0xD8, 0xDC)
        .bright_white(0xE0, 0xE0, 0xE0)
        .build()
}

// `kairn` runs $SHELL as a login shell; `kairn <cmd> [args...]` runs that
// command in the PTY instead (e.g. `kairn ssh somehost`).
fn shell_command() -> CommandBuilder {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = if args.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg("-l");
        cmd
    } else {
        let mut cmd = CommandBuilder::new(&args[0]);
        cmd.args(&args[1..]);
        cmd
    };
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(home) = std::env::var_os("HOME") {
        cmd.cwd(home);
    }
    cmd
}

struct TerminalApp {
    terminal: Entity<TerminalView>,
}

impl TerminalApp {
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        if !(ks.modifiers.platform || ks.modifiers.control) {
            return;
        }
        let delta = match ks.key.as_str() {
            "+" | "=" => 1.0,
            "-" => -1.0,
            _ => return,
        };
        self.terminal.update(cx, |terminal, cx| {
            let mut config = terminal.config().clone();
            let new_size = config.font_size + px(delta);
            if new_size >= px(6.0) {
                config.font_size = new_size;
                terminal.update_config(config, cx);
            }
        });
        cx.stop_propagation();
    }
}

impl Render for TerminalApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.terminal.clone())
    }
}

fn main() -> Result<()> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to open PTY")?;

    let child = pair
        .slave
        .spawn_command(shell_command())
        .context("failed to spawn command in PTY")?;
    drop(pair.slave);

    let writer = pair.master.take_writer().context("PTY writer")?;
    let reader = pair.master.try_clone_reader().context("PTY reader")?;
    let pty_master = Arc::new(Mutex::new(pair.master));

    // Mutex because the exit callback is an Fn; reaping there keeps the dead
    // shell from lingering as a zombie until the app quits.
    let child = Arc::new(Mutex::new(child));

    gpui::Application::new().run(move |cx| {
        let config = TerminalConfig {
            font_family: default_font(),
            font_size: px(FONT_SIZE),
            cols: 80,
            rows: 24,
            scrollback: SCROLLBACK_LINES,
            line_height_multiplier: 1.0,
            padding: Edges::all(px(8.0)),
            colors: dark_palette(),
        };

        let resize_pty = pty_master.clone();
        let resize_callback = move |cols: usize, rows: usize| {
            if let Err(e) = resize_pty.lock().resize(PtySize {
                cols: cols as u16,
                rows: rows as u16,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                eprintln!("PTY resize failed: {e}");
            }
        };

        let window = cx.open_window(
            gpui::WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Kairn".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let terminal = cx.new(|cx| {
                    TerminalView::new(writer, reader, config, cx)
                        .with_resize_callback(resize_callback)
                        .with_exit_callback(move |_window, cx| {
                            let _ = child.lock().wait();
                            cx.quit();
                        })
                });
                terminal.read(cx).focus_handle().focus(window);
                cx.new(|_cx| TerminalApp { terminal })
            },
        );

        if let Err(e) = window {
            eprintln!("failed to open window: {e}");
            cx.quit();
        } else {
            cx.activate(true);
        }
    });

    Ok(())
}
