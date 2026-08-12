//! Event handling for the terminal emulator.
//!
//! This module bridges alacritty's event system with GPUI by providing
//! [`GpuiEventProxy`], which implements alacritty's [`EventListener`] trait
//! and forwards relevant events through a channel.
//!
//! # Event Flow
//!
//! ```text
//! alacritty Term → GpuiEventProxy → mpsc channel → TerminalView
//!                        │
//!                        └─ Translates Event enum to TerminalEvent
//! ```
//!
//! # Supported Events
//!
//! | Alacritty Event | TerminalEvent | Description |
//! |-----------------|---------------|-------------|
//! | `Event::Wakeup` | `Wakeup` | Terminal has new content |
//! | `Event::Bell` | `Bell` | BEL character received |
//! | `Event::Title(_)` | `Title(String)` | Title escape sequence (OSC 0/2) |
//! | `Event::ClipboardStore(_, _)` | `ClipboardStore(String)` | Copy request (OSC 52) |
//! | `Event::ClipboardLoad(_, _)` | `ClipboardLoad` | Paste request |
//! | `Event::Exit` | `Exit` | Terminal exited |
//! | `Event::ChildExit(_)` | `Exit` | Child process exited |
//! | `Event::ResetTitle` | `Title("")` | Reset to empty title |
//!
//! `Event::PtyWrite` (query responses: cursor position reports, device
//! attributes, kitty keyboard queries) and `Event::ColorRequest` (OSC
//! 4/10/11/12 color queries) are answered directly on the PTY when the
//! proxy was built with [`GpuiEventProxy::with_pty_responder`]; applications
//! block waiting for these replies, so they must not be dropped.
//! `MouseCursorDirty` and `CursorBlinkingChange` are ignored as they're not
//! needed for GPUI integration.
//!
//! # Example
//!
//! ```
//! use std::sync::mpsc::channel;
//! use gpui_terminal::event::{GpuiEventProxy, TerminalEvent};
//!
//! let (tx, rx) = channel();
//! let proxy = GpuiEventProxy::new(tx);
//!
//! // The proxy is passed to alacritty's Term and will forward events
//! // Events can be received on the other end of the channel
//! ```
//!
//! [`EventListener`]: alacritty_terminal::event::EventListener

use crate::colors::ColorPalette;
use alacritty_terminal::event::{Event, EventListener};
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::Sender;

/// Shared handle to the PTY input, for writing query responses.
pub type PtyWriter = Arc<parking_lot::Mutex<Box<dyn Write + Send>>>;

/// Shared handle to the palette, for answering color queries after runtime
/// config updates.
pub type SharedPalette = Arc<parking_lot::Mutex<ColorPalette>>;

/// Events emitted by the terminal that the GPUI application cares about.
///
/// This enum represents a subset of alacritty's events that are relevant
/// for the GPUI terminal emulator implementation.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// The terminal has new content to display and needs a redraw.
    Wakeup,

    /// The terminal bell was triggered (visual or audible alert).
    Bell,

    /// The terminal title has changed.
    Title(String),

    /// The terminal wants to store data to the clipboard.
    ClipboardStore(String),

    /// The terminal wants to load data from the clipboard.
    ClipboardLoad,

    /// The terminal process has exited.
    Exit,
}

/// An event proxy that implements alacritty's EventListener trait.
///
/// This struct forwards relevant terminal events to a channel that can be
/// consumed by the GPUI application on the main thread.
pub struct GpuiEventProxy {
    /// Channel sender for forwarding events to the GPUI application.
    tx: Sender<TerminalEvent>,

    /// PTY writer and palette for answering terminal queries (cursor
    /// position, device attributes, color queries) directly. Responses
    /// bypass the event channel: applications block waiting for them, and
    /// the channel is only drained on render.
    responder: Option<(PtyWriter, SharedPalette)>,
}

impl GpuiEventProxy {
    /// Creates a new event proxy with the given channel sender.
    ///
    /// # Arguments
    ///
    /// * `tx` - The channel sender to forward events through
    ///
    /// # Returns
    ///
    /// A new GpuiEventProxy instance
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::mpsc::channel;
    /// use gpui_terminal::event::GpuiEventProxy;
    ///
    /// let (tx, rx) = channel();
    /// let proxy = GpuiEventProxy::new(tx);
    /// ```
    pub fn new(tx: Sender<TerminalEvent>) -> Self {
        Self {
            tx,
            responder: None,
        }
    }

    /// Attach a PTY writer and palette so the proxy can answer terminal
    /// queries (`PtyWrite`, `ColorRequest`) directly. Without this, query
    /// responses are dropped and applications waiting on them stall.
    pub fn with_pty_responder(mut self, writer: PtyWriter, palette: SharedPalette) -> Self {
        self.responder = Some((writer, palette));
        self
    }

    /// Write a query response to the PTY, if a responder is attached.
    fn respond(&self, bytes: &[u8]) {
        if let Some((writer, _)) = &self.responder {
            let mut writer = writer.lock();
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    /// Sends a terminal event through the channel.
    ///
    /// If the channel is disconnected, this method will silently drop the event.
    /// This can happen if the GPUI application has been shut down.
    fn send(&self, event: TerminalEvent) {
        // Ignore send errors - they just mean the receiver has been dropped
        let _ = self.tx.send(event);
    }
}

impl EventListener for GpuiEventProxy {
    /// Handles events from the alacritty terminal.
    ///
    /// This method is called by alacritty when terminal events occur.
    /// It translates alacritty's Event enum to our TerminalEvent enum
    /// and forwards relevant events through the channel.
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => {
                self.send(TerminalEvent::Wakeup);
            }
            Event::Bell => {
                self.send(TerminalEvent::Bell);
            }
            Event::Title(title) => {
                self.send(TerminalEvent::Title(title));
            }
            Event::ClipboardStore(_clipboard_type, data) => {
                // For simplicity, we ignore the clipboard type and just store the data
                self.send(TerminalEvent::ClipboardStore(data));
            }
            Event::ClipboardLoad(_clipboard_type, _format) => {
                // For simplicity, we ignore the clipboard type and format
                self.send(TerminalEvent::ClipboardLoad);
            }
            Event::Exit => {
                self.send(TerminalEvent::Exit);
            }
            Event::PtyWrite(data) => {
                // Query responses the application is blocked waiting for:
                // cursor position reports (CSI 6n), device attributes,
                // kitty keyboard queries.
                self.respond(data.as_bytes());
            }
            Event::ColorRequest(index, format) => {
                // OSC 4/10/11/12 color queries; applications use OSC 11 to
                // detect light/dark background.
                if let Some((_, palette)) = &self.responder {
                    let rgb = palette.lock().query_color(index);
                    if let Some(rgb) = rgb {
                        self.respond(format(rgb).as_bytes());
                    }
                }
            }
            // Ignore events we don't care about
            Event::MouseCursorDirty => {}
            Event::TextAreaSizeRequest(ref _format) => {
                // Answering CSI 14t needs the terminal's dimensions, but the
                // terminal is locked while this event fires; left unanswered.
            }
            Event::CursorBlinkingChange => {
                // Cursor blinking changes could be handled if needed
            }
            Event::ResetTitle => {
                // Reset title to default - we can treat this as an empty title
                self.send(TerminalEvent::Title(String::new()));
            }
            Event::ChildExit(_exit_code) => {
                // Child process exited - treat this as a terminal exit
                self.send(TerminalEvent::Exit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn test_event_proxy_creation() {
        let (tx, _rx) = channel();
        let _proxy = GpuiEventProxy::new(tx);
    }

    /// A writer that appends into a shared buffer, standing in for the PTY.
    struct SinkWriter(Arc<parking_lot::Mutex<Vec<u8>>>);

    impl Write for SinkWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn responder_proxy() -> (GpuiEventProxy, Arc<parking_lot::Mutex<Vec<u8>>>) {
        let (tx, _rx) = channel();
        let sink = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let writer: PtyWriter = Arc::new(parking_lot::Mutex::new(
            Box::new(SinkWriter(sink.clone())) as Box<dyn Write + Send>,
        ));
        let palette: SharedPalette = Arc::new(parking_lot::Mutex::new(ColorPalette::default()));
        let proxy = GpuiEventProxy::new(tx).with_pty_responder(writer, palette);
        (proxy, sink)
    }

    #[test]
    fn test_cursor_position_query_is_answered() {
        let (proxy, sink) = responder_proxy();
        let mut state = crate::terminal::TerminalState::new(80, 24, proxy);

        // DSR: the application asks where the cursor is and blocks on the
        // reply.
        state.process_bytes(b"\x1b[6n");

        assert_eq!(&*sink.lock(), b"\x1b[1;1R");
    }

    #[test]
    fn test_background_color_query_is_answered() {
        let (proxy, sink) = responder_proxy();
        let mut state = crate::terminal::TerminalState::new(80, 24, proxy);

        // OSC 11: query the background color (light/dark detection).
        state.process_bytes(b"\x1b]11;?\x07");

        let response = sink.lock().clone();
        assert!(
            response.starts_with(b"\x1b]11;rgb:"),
            "unexpected response: {response:?}"
        );
    }

    #[test]
    fn test_queries_are_dropped_without_responder() {
        let (tx, _rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        // No responder attached: must not panic.
        proxy.send_event(Event::PtyWrite("\x1b[1;1R".into()));
    }

    #[test]
    fn test_wakeup_event() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::Wakeup);

        let event = rx.recv().unwrap();
        assert!(matches!(event, TerminalEvent::Wakeup));
    }

    #[test]
    fn test_bell_event() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::Bell);

        let event = rx.recv().unwrap();
        assert!(matches!(event, TerminalEvent::Bell));
    }

    #[test]
    fn test_title_event() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::Title("Test Title".to_string()));

        let event = rx.recv().unwrap();
        match event {
            TerminalEvent::Title(title) => assert_eq!(title, "Test Title"),
            _ => panic!("Expected Title event"),
        }
    }

    #[test]
    fn test_clipboard_store_event() {
        use alacritty_terminal::term::ClipboardType;

        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::ClipboardStore(
            ClipboardType::Clipboard,
            "clipboard data".to_string(),
        ));

        let event = rx.recv().unwrap();
        match event {
            TerminalEvent::ClipboardStore(data) => assert_eq!(data, "clipboard data"),
            _ => panic!("Expected ClipboardStore event"),
        }
    }

    #[test]
    fn test_clipboard_load_event() {
        use alacritty_terminal::term::ClipboardType;
        use std::sync::Arc;

        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        // ClipboardLoad requires a callback function
        let callback = Arc::new(|s: &str| s.to_string());
        proxy.send_event(Event::ClipboardLoad(ClipboardType::Clipboard, callback));

        let event = rx.recv().unwrap();
        assert!(matches!(event, TerminalEvent::ClipboardLoad));
    }

    #[test]
    fn test_exit_event() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::Exit);

        let event = rx.recv().unwrap();
        assert!(matches!(event, TerminalEvent::Exit));
    }

    #[test]
    fn test_reset_title_event() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        proxy.send_event(Event::ResetTitle);

        let event = rx.recv().unwrap();
        match event {
            TerminalEvent::Title(title) => assert!(title.is_empty()),
            _ => panic!("Expected Title event"),
        }
    }

    #[test]
    fn test_ignored_events() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        // These events should be ignored and not sent through the channel
        proxy.send_event(Event::MouseCursorDirty);
        proxy.send_event(Event::CursorBlinkingChange);

        // The channel should be empty
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_disconnected_channel() {
        let (tx, rx) = channel();
        let proxy = GpuiEventProxy::new(tx);

        // Drop the receiver to disconnect the channel
        drop(rx);

        // Sending should not panic even though the channel is disconnected
        proxy.send_event(Event::Wakeup);
    }
}
