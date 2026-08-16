//! Env-gated PTY byte logging for diagnosing terminal handshake issues.
//!
//! Set `KAIRN_PTY_LOG` to a directory path and every terminal session
//! appends to its own `pty-<pid>-<seq>.log` file in that directory,
//! recording each chunk of bytes crossing the PTY with a millisecond
//! timestamp relative to session start:
//!
//! - `>` host-ward: what the app writes to the child (keys, pastes,
//!   resizes' side effects, and query replies such as cursor position
//!   reports and device attributes)
//! - `<` child output: what the child process writes (screen content and
//!   the escape queries that trigger the replies above)
//!
//! Both directions share one file so the query/reply ordering is visible.
//! Bytes are logged escaped (`\x1b[6n` style) so control sequences stay
//! readable. The tap is inert unless the variable is set.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Per-process counter so concurrent terminal sessions get distinct files.
static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

/// A shared log file for one terminal session, timestamped from creation.
pub struct PtyLogger {
    file: parking_lot::Mutex<File>,
    start: Instant,
}

impl PtyLogger {
    /// Build a logger if `KAIRN_PTY_LOG` names a usable directory, else None.
    /// Creation failures are swallowed: diagnostics must never break the
    /// terminal.
    pub fn from_env() -> Option<Arc<Self>> {
        let dir = std::env::var_os("KAIRN_PTY_LOG")?;
        let dir = PathBuf::from(dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("pty-{}-{}.log", std::process::id(), seq));
        let mut file = File::create(&path).ok()?;
        let _ = writeln!(
            file,
            "# kairn pty log, pid {} session {}; '>' host-ward (keys + query replies), '<' child output",
            std::process::id(),
            seq
        );
        Some(Arc::new(Self {
            file: parking_lot::Mutex::new(file),
            start: Instant::now(),
        }))
    }

    /// Append one direction-tagged, escaped chunk.
    fn log(&self, direction: char, bytes: &[u8]) {
        let elapsed = self.start.elapsed();
        let mut file = self.file.lock();
        let _ = writeln!(
            file,
            "[+{:>9.3}s] {} {:4}B {}",
            elapsed.as_secs_f64(),
            direction,
            bytes.len(),
            escape_bytes(bytes)
        );
        let _ = file.flush();
    }
}

/// Escape a byte chunk into printable ASCII (`\x1b[6n` style).
fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

/// Tap on the host-ward writer: logs, then forwards to the real PTY writer.
pub struct LoggingWriter<W> {
    inner: W,
    logger: Arc<PtyLogger>,
}

impl<W: Write> LoggingWriter<W> {
    pub fn new(inner: W, logger: Arc<PtyLogger>) -> Self {
        Self { inner, logger }
    }
}

impl<W: Write> Write for LoggingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.logger.log('>', &buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Tap on the child-output reader: logs whatever the child produced.
pub struct LoggingReader<R> {
    inner: R,
    logger: Arc<PtyLogger>,
}

impl<R: Read> LoggingReader<R> {
    pub fn new(inner: R, logger: Arc<PtyLogger>) -> Self {
        Self { inner, logger }
    }
}

impl<R: Read> Read for LoggingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.logger.log('<', &buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_control_sequences_readably() {
        assert_eq!(escape_bytes(b"\x1b[6n"), "\\x1b[6n");
        assert_eq!(escape_bytes(b"a\r\nb\\"), "a\\r\\nb\\\\");
        assert_eq!(escape_bytes(&[0x00, 0x9c]), "\\x00\\x9c");
    }

    #[test]
    fn absent_env_var_disables_logging() {
        // The test runner does not set KAIRN_PTY_LOG.
        assert!(std::env::var_os("KAIRN_PTY_LOG").is_none());
        assert!(PtyLogger::from_env().is_none());
    }
}
