use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{AppContext, Context, Edges, Entity, SharedString, WeakEntity, px};
use gpui_terminal::{TerminalConfig, TerminalView};
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use kairn_core::settings::{HostApp, SshHost};
use crate::theme::KairnThemeExt as _;
use crate::workspace::Workspace;

pub const TERM_FONT_SIZE: f32 = 13.0;
const SCROLLBACK_LINES: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub enum SessionKind {
    Local,
    Ssh(SshHost),
    /// A saved launch shortcut: a command in its own session, on this
    /// machine (`host: None`) or on an SSH host.
    App { host: Option<SshHost>, app: HostApp },
}

impl SessionKind {
    /// Whether the session runs on another machine (shows the SSH chip).
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            SessionKind::Ssh(_) | SessionKind::App { host: Some(_), .. }
        )
    }
}

pub struct Session {
    pub id: u64,
    pub kind: SessionKind,
    pub display_name: SharedString,
    /// Latest OSC title from the shell (cwd/command on most setups).
    pub title: SharedString,
    /// Cached busy state, polled by the workspace activity timer; renders read
    /// this field rather than probing the PTY per frame.
    pub busy: bool,
    pub view: Entity<TerminalView>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    shell_pid: Option<u32>,
}

impl Session {
    /// A local session is busy when the PTY's foreground process group is not
    /// the shell itself (i.e. a command is running). SSH sessions count as
    /// busy while connected: the remote foreground process isn't visible here.
    pub fn is_busy(&self) -> bool {
        match &self.kind {
            SessionKind::Ssh(_) | SessionKind::App { host: Some(_), .. } => true,
            SessionKind::Local | SessionKind::App { host: None, .. } => {
                let leader = self.master.lock().process_group_leader();
                match (leader, self.shell_pid) {
                    (Some(leader), Some(pid)) => leader > 0 && leader as u32 != pid,
                    _ => false,
                }
            }
        }
    }

    pub fn label(&self) -> SharedString {
        if self.title.is_empty() {
            self.display_name.clone()
        } else {
            format!("{} · {}", self.display_name, self.title).into()
        }
    }

    /// Force-quit the session's process. The PTY reader then sees EOF and the
    /// exit callback removes the session from the workspace, the same path an
    /// ordinary `exit` takes.
    pub fn terminate(&self) {
        let mut child = self.child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn shell_command(kind: &SessionKind) -> (CommandBuilder, SharedString) {
    match kind {
        SessionKind::Local => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let name: SharedString = PathBuf::from(&shell)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "shell".into())
                .into();
            let mut cmd = CommandBuilder::new(shell);
            cmd.arg("-l");
            (cmd, name)
        }
        SessionKind::Ssh(host) => {
            let args = host.command_args();
            let mut cmd = CommandBuilder::new(&args[0]);
            cmd.args(&args[1..]);
            (cmd, host.name.clone().into())
        }
        SessionKind::App { host: None, app } => {
            // An interactive login shell (`-i -l`) so the command sees the
            // same PATH a typed command would (rc files, not just profiles),
            // with an exec tail so the pane drops to a shell when the app
            // exits instead of dying.
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            let mut cmd = CommandBuilder::new(&shell);
            cmd.args([
                "-i",
                "-l",
                "-c",
                &format!("{}; exec {} -il", app.command, sq(&shell)),
            ]);
            (cmd, app.display_name().into())
        }
        SessionKind::App { host: Some(host), app } => {
            // `-t` allocates the remote TTY an interactive app needs; the
            // remote command re-wraps in the user's login shell for the same
            // PATH and exec-tail reasons as the local arm.
            let mut args = vec!["ssh".to_string(), "-t".to_string()];
            if let Some(port) = host.port {
                args.push("-p".to_string());
                args.push(port.to_string());
            }
            args.push(host.target.clone());
            args.push(format!(
                "exec $SHELL -il -c {}",
                sq(&format!("{}; exec $SHELL -il", app.command))
            ));
            let mut cmd = CommandBuilder::new(&args[0]);
            cmd.args(&args[1..]);
            let name = format!("{} · {}", host.name, app.display_name());
            (cmd, name.into())
        }
    }
}

/// Single-quote a string for a POSIX shell (`'` becomes `'\''`), so a saved
/// command travels into the wrapping shell verbatim.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn spawn(
    id: u64,
    kind: SessionKind,
    workspace: WeakEntity<Workspace>,
    cx: &mut Context<Workspace>,
) -> Result<Session> {
    let (term_colors, mono_font) = {
        let t = cx.kairn();
        (t.term_colors.clone(), t.mono_font.to_string())
    };
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to open PTY")?;

    let (mut cmd, display_name) = shell_command(&kind);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(home) = std::env::var_os("HOME") {
        cmd.cwd(home);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn command in PTY")?;
    let shell_pid = child.process_id();
    drop(pair.slave);

    let writer = pair.master.take_writer().context("PTY writer")?;
    let reader = pair.master.try_clone_reader().context("PTY reader")?;
    let master = Arc::new(Mutex::new(pair.master));
    let child = Arc::new(Mutex::new(child));

    let config = TerminalConfig {
        font_family: mono_font,
        font_size: px(TERM_FONT_SIZE),
        cols: 80,
        rows: 24,
        scrollback: SCROLLBACK_LINES,
        line_height_multiplier: 1.0,
        padding: Edges {
            top: px(12.0),
            right: px(14.0),
            bottom: px(12.0),
            left: px(14.0),
        },
        colors: term_colors,
    };

    let resize_master = master.clone();
    let exit_child = child.clone();
    let exit_workspace = workspace.clone();
    let title_workspace = workspace;

    let view = cx.new(|cx| {
        TerminalView::new(writer, reader, config, cx)
            .with_resize_callback(move |cols, rows| {
                if let Err(e) = resize_master.lock().resize(PtySize {
                    cols: cols as u16,
                    rows: rows as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                }) {
                    eprintln!("PTY resize failed: {e}");
                }
            })
            .with_title_callback(move |_window, cx, title| {
                let title = title.to_string();
                let _ = title_workspace.update(cx, |ws, cx| {
                    ws.set_session_title(id, title.into(), cx);
                });
            })
            .with_exit_callback(move |window, cx| {
                // Reap here so the dead shell doesn't linger as a zombie.
                let _ = exit_child.lock().wait();
                let _ = exit_workspace.update(cx, |ws, cx| {
                    ws.handle_session_exit(id, window, cx);
                });
            })
    });

    Ok(Session {
        busy: kind.is_remote(),
        id,
        kind,
        display_name,
        title: SharedString::default(),
        view,
        master,
        child,
        shell_pid,
    })
}
