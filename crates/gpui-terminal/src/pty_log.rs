//! Opt-in PTY diagnostics. `KAIRN_PTY_LOG` records output; also setting
//! `KAIRN_PTY_LOG_INPUT=1` includes input, including unechoed passwords.
//! Files are private, capped at 4 MiB, with at most ten retained logs.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Instant;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_FILES: usize = 10;
static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

struct LogFile {
    file: File,
    remaining: usize,
}

pub struct PtyLogger {
    file: parking_lot::Mutex<LogFile>,
    start: Instant,
    input: bool,
    stopped: AtomicU8,
}

impl PtyLogger {
    pub fn from_env() -> Option<Arc<Self>> {
        let dir = PathBuf::from(std::env::var_os("KAIRN_PTY_LOG")?);
        let input = std::env::var("KAIRN_PTY_LOG_INPUT").is_ok_and(|value| value == "1");
        match Self::open(&dir, input, MAX_BYTES) {
            Ok(logger) => Some(logger),
            Err(error) => {
                eprintln!("PTY diagnostics could not start: {error}");
                None
            }
        }
    }

    fn open(dir: &Path, input: bool, limit: usize) -> io::Result<Arc<Self>> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let guard = options.open(dir.join(".pty-retention.lock"))?;
        guard.try_lock().map_err(io::Error::other)?;
        make_space(dir)?;
        let mut file = loop {
            let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("pty-{}-{seq}.log", std::process::id()));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(file) => break file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        file.try_lock().map_err(io::Error::other)?;
        let header = format!(
            "# PTY diagnostics: {}; limit {limit} bytes\n",
            if input {
                "input and output (may include passwords)"
            } else {
                "output only"
            }
        );
        file.write_all(header.as_bytes())?;
        Ok(Arc::new(Self {
            file: parking_lot::Mutex::new(LogFile {
                file,
                remaining: limit.saturating_sub(header.len()),
            }),
            start: Instant::now(),
            input,
            stopped: AtomicU8::new(0),
        }))
    }

    pub fn status(&self) -> &'static str {
        match self.stopped.load(Ordering::Relaxed) {
            1 => "PTY logging stopped: 4 MiB limit reached",
            2 => "PTY logging stopped: write failed",
            _ if self.input => {
                "PTY input and output logging active · includes passwords · 4 MiB limit"
            }
            _ => "PTY output logging active · 4 MiB limit",
        }
    }

    fn log(&self, direction: char, bytes: &[u8]) {
        if (direction == '>' && !self.input) || self.stopped.load(Ordering::Relaxed) != 0 {
            return;
        }
        let mut log = self.file.lock();
        let record = if bytes.len() > log.remaining {
            None
        } else {
            Some(format!(
                "[+{:>9.3}s] {} {:4}B {}\n",
                self.start.elapsed().as_secs_f64(),
                direction,
                bytes.len(),
                escape_bytes(bytes)
            ))
        };
        let Some(record) = record.filter(|record| record.len() <= log.remaining) else {
            self.stopped.store(1, Ordering::Relaxed);
            return;
        };
        if log
            .file
            .write_all(record.as_bytes())
            .and_then(|_| log.file.flush())
            .is_err()
        {
            self.stopped.store(2, Ordering::Relaxed);
        } else {
            log.remaining -= record.len();
        }
    }
}

fn make_space(dir: &Path) -> io::Result<()> {
    let mut logs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let owned_name = name
            .strip_prefix("pty-")
            .and_then(|s| s.strip_suffix(".log"))
            .and_then(|s| s.split_once('-'))
            .is_some_and(|(pid, seq)| pid.parse::<u32>().is_ok() && seq.parse::<u64>().is_ok());
        if owned_name && entry.file_type()?.is_file() {
            logs.push((entry.metadata()?.modified()?, entry.path()));
        }
    }
    logs.sort();
    let mut remaining = logs.len();
    for (_, path) in logs {
        if remaining < MAX_FILES {
            break;
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        if file.try_lock().is_ok() {
            std::fs::remove_file(path)?;
            remaining -= 1;
        }
    }
    if remaining >= MAX_FILES {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "ten diagnostic logs are still open",
        ));
    }
    Ok(())
}

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
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.logger.log('>', &buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

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
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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

    fn text(dir: &Path) -> String {
        std::fs::read_to_string(logs(dir).pop().unwrap()).unwrap()
    }

    fn logs(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .collect()
    }

    #[test]
    fn logs_output_only_unless_input_capture_is_explicit() {
        for input in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let logger = PtyLogger::open(dir.path(), input, MAX_BYTES).unwrap();
            LoggingWriter::new(io::sink(), logger.clone())
                .write_all(b"fake-password")
                .unwrap();
            LoggingReader::new(io::Cursor::new(b"output\x1b[6n"), logger)
                .read_to_end(&mut Vec::new())
                .unwrap();
            let log = text(dir.path());
            assert_eq!(log.contains("fake-password"), input);
            assert!(log.contains("output\\x1b[6n"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for path in logs(dir.path()) {
                    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o177, 0);
                }
            }
        }
    }

    #[test]
    fn bounds_each_file_and_preserves_active_logs_during_retention() {
        let dir = tempfile::tempdir().unwrap();
        let logger = PtyLogger::open(dir.path(), false, 256).unwrap();
        for _ in 0..100 {
            logger.log('<', b"fixture output");
        }
        assert!(text(dir.path()).len() <= 256);
        assert_eq!(logger.stopped.load(Ordering::Relaxed), 1);
        drop(logger);
        let active: Vec<_> = (0..MAX_FILES)
            .map(|_| PtyLogger::open(dir.path(), false, MAX_BYTES).unwrap())
            .collect();
        assert!(PtyLogger::open(dir.path(), false, MAX_BYTES).is_err());
        drop(active);
        for _ in 0..20 {
            PtyLogger::open(dir.path(), false, MAX_BYTES).unwrap();
        }
        assert_eq!(logs(dir.path()).len(), MAX_FILES);
    }

    #[test]
    fn escapes_control_sequences_readably() {
        assert_eq!(escape_bytes(b"\x1b[6n"), "\\x1b[6n");
        assert_eq!(escape_bytes(b"a\r\nb\\"), "a\\r\\nb\\\\");
        assert_eq!(escape_bytes(&[0x00, 0x9c]), "\\x00\\x9c");
    }
}
