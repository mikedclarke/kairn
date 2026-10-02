use super::{TerminalConfig, TerminalView};
use gpui::{AppContext, EmptyView, TestAppContext};
use std::{
    cell::RefCell,
    io::Cursor,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

#[gpui::test]
fn hidden_terminal_processes_titles_clipboard_and_eof(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (_, cx) = cx.add_window_view(|_, _| EmptyView);
    let events = Rc::new(RefCell::new(Vec::new()));
    let title_events = events.clone();
    let clipboard_events = events.clone();
    let exit_events = events.clone();
    let terminal = cx.update(|window, cx| {
        cx.new(|cx| {
            TerminalView::new(
                std::io::sink(),
                Cursor::new(b"\x1b]2;first\x07\x1b]2;latest\x07\x1b]52;c;Y29weQ==\x07"),
                TerminalConfig::default(),
                window,
                cx,
            )
            .with_title_callback(move |_, _, title| {
                title_events.borrow_mut().push(format!("title:{title}"));
            })
            .with_clipboard_store_callback(move |_, _, text| {
                clipboard_events
                    .borrow_mut()
                    .push(format!("clipboard:{text}"));
            })
            .with_exit_callback(move |_, _| exit_events.borrow_mut().push("exit".into()))
        })
    });

    let deadline = Instant::now() + Duration::from_secs(3);
    while !events.borrow().iter().any(|event| event == "exit") && Instant::now() < deadline {
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        &*events.borrow(),
        &["clipboard:copy", "title:latest", "exit"]
    );
    terminal.update(cx, |terminal, _| {
        assert!(terminal.focus_subscriptions.is_empty());
        assert!(terminal.event_rx.try_recv().is_err());
    });
}

#[gpui::test]
fn stalled_input_keeps_ui_responsive_and_reports_overflow(cx: &mut TestAppContext) {
    use std::{io::Write, sync::mpsc};

    struct StalledWriter(mpsc::Receiver<()>);
    impl Write for StalledWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.recv_timeout(Duration::from_secs(3));
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    cx.executor().allow_parking();
    let (_, cx) = cx.add_window_view(|_, _| EmptyView);
    let (resume, wait) = mpsc::channel();
    let errors = Rc::new(RefCell::new(Vec::new()));
    let received = errors.clone();
    let terminal = cx.update(|window, cx| {
        cx.new(|cx| {
            TerminalView::new(
                StalledWriter(wait),
                std::io::empty(),
                TerminalConfig::default(),
                window,
                cx,
            )
            .with_input_error_callback(move |_, _, error| {
                received.borrow_mut().push(error.to_string())
            })
        })
    });
    let start = Instant::now();
    terminal.update(cx, |view, _| {
        view.write_to_pty(&vec![b'x'; 1024 * 1024]);
        view.write_to_pty(b"rejected");
    });
    let deadline = start + Duration::from_secs(2);
    while errors.borrow().is_empty() && Instant::now() < deadline {
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(errors.borrow()[0].contains("buffer is full"));
    resume.send(()).unwrap();
}

#[gpui::test]
fn non_web_links_disclose_the_target_and_require_confirmation(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| EmptyView);
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
    for target in ["file:///tmp/example.txt", "example-app:open/item"] {
        terminal.update_in(cx, |view, window, cx| {
            view.state.process_bytes(
                format!("\x1b[2J\x1b[H\x1b]8;;{target}\x1b\\Documentation\x1b]8;;\x1b\\")
                    .as_bytes(),
            );
            *view.last_layout.lock() = Some((
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::px(10.),
                gpui::px(20.),
            ));
            view.update_hovered_link(
                gpui::point(gpui::px(5.), gpui::px(5.)),
                &gpui::Modifiers::secondary_key(),
                cx,
            );
            assert_eq!(view.hovered_link(), Some(target));
            view.open_link(view.hovered_link().unwrap().to_owned(), window, cx);
        });
        assert!(cx.has_pending_prompt());
        assert!(cx.pending_prompt().unwrap().1.contains(target));
        assert_eq!(cx.opened_url(), None);
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(cx.opened_url(), None);
    }
    let target = "example-app:open/item";
    terminal.update_in(cx, |view, window, cx| {
        view.open_link(target.into(), window, cx)
    });
    cx.simulate_prompt_answer("Open link");
    cx.run_until_parked();
    assert_eq!(cx.opened_url().as_deref(), Some(target));
    terminal.update_in(cx, |view, window, cx| {
        view.open_link("https://example.com".into(), window, cx)
    });
    assert!(!cx.has_pending_prompt());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
    terminal.update_in(cx, |view, window, cx| {
        view.open_link("https://example.com\nmalformed".into(), window, cx)
    });
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
}
