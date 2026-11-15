use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MAX_PENDING_WRITES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    Full,
    Closed,
    Write(String),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => {
                f.write_str("Terminal input buffer is full. The latest input was not sent.")
            }
            Self::Closed => f.write_str("Terminal input is closed. The latest input was not sent."),
            Self::Write(error) => write!(f, "Terminal input failed: {error}"),
        }
    }
}

struct Packet {
    bytes: Vec<u8>,
    pending: Arc<AtomicUsize>,
}

impl Drop for Packet {
    fn drop(&mut self) {
        self.pending.fetch_sub(self.bytes.len(), Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct PtyWriter {
    tx: flume::Sender<Packet>,
    pending: Arc<AtomicUsize>,
    errors: flume::Sender<InputError>,
    reported: Arc<AtomicBool>,
}

impl PtyWriter {
    pub fn new(mut writer: impl Write + Send + 'static) -> (Self, flume::Receiver<InputError>) {
        let (tx, rx) = flume::bounded::<Packet>(MAX_PENDING_WRITES);
        let (errors, error_rx) = flume::bounded(1);
        let pending = Arc::new(AtomicUsize::new(0));
        let reported = Arc::new(AtomicBool::new(false));
        let worker_errors = errors.clone();
        let worker_reported = reported.clone();
        std::thread::spawn(move || {
            while let Ok(packet) = rx.recv() {
                if let Err(error) = writer.write_all(&packet.bytes).and_then(|_| writer.flush()) {
                    let _ = worker_errors.send(InputError::Write(error.to_string()));
                    break;
                }
                worker_reported.store(false, Ordering::Relaxed);
            }
        });
        (
            Self {
                tx,
                pending,
                errors,
                reported,
            },
            error_rx,
        )
    }

    /// Enqueue a complete input operation, including both paste guards.
    /// The byte budget includes the write currently blocked in the worker.
    pub fn send(&self, bytes: &[u8]) -> Result<(), InputError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let result = if self.tx.is_disconnected() {
            Err(InputError::Closed)
        } else if self
            .pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |size| {
                size.checked_add(bytes.len())
                    .filter(|&size| size <= MAX_PENDING_BYTES)
            })
            .is_err()
        {
            Err(InputError::Full)
        } else {
            let packet = Packet {
                bytes: bytes.to_vec(),
                pending: self.pending.clone(),
            };
            self.tx.try_send(packet).map_err(|error| match error {
                flume::TrySendError::Full(_) => InputError::Full,
                flume::TrySendError::Disconnected(_) => InputError::Closed,
            })
        };
        if let Err(error) = &result {
            self.report(error.clone());
        }
        result
    }

    fn report(&self, error: InputError) {
        if !self.reported.swap(true, Ordering::Relaxed) {
            let _ = self.errors.try_send(error);
        }
    }

    pub fn paste(&self, text: &str, bracketed: bool) {
        if text.len() > MAX_PENDING_BYTES - 12 {
            self.report(InputError::Full);
            return;
        }
        let text = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        let _ = self.send(text.as_bytes());
    }

    pub fn repeat(&self, bytes: &[u8], count: usize) {
        if bytes
            .len()
            .checked_mul(count)
            .is_none_or(|size| size > MAX_PENDING_BYTES)
        {
            self.report(InputError::Full);
            return;
        }
        let _ = self.send(&bytes.repeat(count));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct StalledWriter {
        started: mpsc::Sender<()>,
        resume: mpsc::Receiver<()>,
        output: mpsc::Sender<Vec<u8>>,
    }

    impl Write for StalledWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.started.send(()).unwrap();
            self.resume.recv_timeout(Duration::from_secs(5)).unwrap();
            self.output.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stalled_writer_bounds_pending_bytes_without_blocking_or_partial_enqueue() {
        let (started, start_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let (output, output_rx) = mpsc::channel();
        let (writer, errors) = PtyWriter::new(StalledWriter {
            started,
            resume: resume_rx,
            output,
        });
        writer.send(b"first").unwrap();
        start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let rest = vec![b'x'; MAX_PENDING_BYTES - 5];
        writer.send(&rest).unwrap();
        assert_eq!(writer.send(b"rejected"), Err(InputError::Full));
        assert_eq!(
            errors.recv_timeout(Duration::from_secs(2)).unwrap(),
            InputError::Full
        );
        assert_eq!(writer.pending.load(Ordering::Relaxed), MAX_PENDING_BYTES);
        resume.send(()).unwrap();
        assert_eq!(
            output_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            b"first"
        );
        start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        resume.send(()).unwrap();
        assert_eq!(
            output_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            rest
        );
        drop(writer);
        assert!(output_rx.recv_timeout(Duration::from_secs(2)).is_err());
    }

    #[test]
    fn reports_worker_errors_without_terminal_output() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (writer, errors) = PtyWriter::new(BrokenWriter);
        writer.send(b"input").unwrap();
        assert!(matches!(
            errors.recv_timeout(Duration::from_secs(2)),
            Ok(InputError::Write(_))
        ));
    }

    #[test]
    fn paste_guards_survive_nested_escapes_and_queries_cannot_split_a_paste() {
        struct Capture(mpsc::Sender<Vec<u8>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.send(bytes.to_vec()).unwrap();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (tx, rx) = mpsc::channel();
        let (writer, errors) = PtyWriter::new(Capture(tx));
        writer.paste("α\x1b[\x1b[201~201~\x1b[201~\nβ", true);
        writer.send(b"\x1b[1;1R").unwrap();
        writer.paste("α\r\nβ\n", false);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "\x1b[200~α[[201~201~[201~\nβ\x1b[201~".as_bytes()
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            b"\x1b[1;1R"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "α\rβ\r".as_bytes()
        );
        writer.paste(&"x".repeat(MAX_PENDING_BYTES), true);
        assert_eq!(
            errors.recv_timeout(Duration::from_secs(2)).unwrap(),
            InputError::Full
        );
        drop(writer);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
    }
}
