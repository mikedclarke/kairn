//! Reading and writing the vault on disk. Every write is atomic (temp file +
//! rename, permissions preserved), matching `kairn_core`'s `write.rs` discipline
//! so a crash never leaves a half-written note (invariant §15.1). Just before a
//! rename lands, a hook fires (spec §7 echo suppression) so the desktop app can
//! feed the path into its own self-write suppression. The engine does not filter
//! its own writes out of the watcher stream; instead a self-write triggers at
//! most a cheap no-op cycle, because the file already matches its baseline
//! (compared by hash), so nothing loops.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Local};

use crate::hash::hash_bytes;
use crate::ignore::is_ignored;
use crate::types::{ContentHash, VaultPath};

/// One file found by a scan: its vault path, content hash, and byte size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: VaultPath,
    pub hash: ContentHash,
    pub size: u64,
}

/// Fired with the vault path about to be written, just before the rename lands.
pub type BeforeWrite<'a> = dyn Fn(&VaultPath) + 'a;

pub struct VaultIo {
    root: PathBuf,
    /// The device label stamped into conflict-copy names, e.g. `IPHONE`.
    device_label: String,
}

impl VaultIo {
    pub fn new(root: impl Into<PathBuf>, device_label: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            device_label: device_label.into(),
        }
    }

    fn abs(&self, path: &VaultPath) -> PathBuf {
        self.root.join(&path.0)
    }

    pub fn exists(&self, path: &VaultPath) -> bool {
        self.abs(path).is_file()
    }

    pub fn read(&self, path: &VaultPath) -> io::Result<Vec<u8>> {
        std::fs::read(self.abs(path))
    }

    /// Read a file if it exists, returning `None` for a missing file (a clean
    /// "not there" rather than an error the caller must special-case).
    pub fn read_opt(&self, path: &VaultPath) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.abs(path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Atomically write `content` to `path`, creating parent folders as needed.
    /// `before` is called with the path immediately before the rename (spec §7).
    pub fn write(&self, path: &VaultPath, content: &[u8], before: &BeforeWrite) -> io::Result<()> {
        let dest = self.abs(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write(&dest, content, || before(path))
    }

    /// Delete a file, announcing the change first (spec §7). A file that is
    /// already gone is success — deletes are idempotent (invariant §15.3).
    pub fn delete(&self, path: &VaultPath, before: &BeforeWrite) -> io::Result<()> {
        before(path);
        match std::fs::remove_file(self.abs(path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The conflict-copy path for `original` at `now`, in the Syncthing pattern
    /// the app already detects and banners: `{stem}.sync-conflict-YYYYMMDD-
    /// HHMMSS-{DEVICE}.{ext}` (spec §8, §17.2).
    pub fn conflict_copy_path(&self, original: &VaultPath, now: DateTime<Local>) -> VaultPath {
        let name = original.file_name();
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s, Some(e)),
            None => (name, None),
        };
        let ts = now.format("%Y%m%d-%H%M%S");
        let copy_name = match ext {
            Some(ext) => format!("{stem}.sync-conflict-{ts}-{}.{ext}", self.device_label),
            None => format!("{stem}.sync-conflict-{ts}-{}", self.device_label),
        };
        match original.0.rsplit_once('/') {
            Some((dir, _)) => VaultPath(format!("{dir}/{copy_name}")),
            None => VaultPath(copy_name),
        }
    }

    /// Write `content` as a conflict copy beside `original`, returning its path
    /// (spec §8). Conflict copies sync like any other file, so the same
    /// resolution shows up on every device.
    pub fn write_conflict_copy(
        &self,
        original: &VaultPath,
        content: &[u8],
        before: &BeforeWrite,
    ) -> io::Result<VaultPath> {
        let copy = self.conflict_copy_path(original, Local::now());
        self.write(&copy, content, before)?;
        Ok(copy)
    }

    /// Walk the vault, returning every syncable file with its hash and size
    /// (spec §4 exclusions applied). The startup and 30-minute safety scans and
    /// the push scan all run through this.
    pub fn scan(&self) -> io::Result<Vec<ScannedFile>> {
        let mut out = Vec::new();
        if self.root.is_dir() {
            self.scan_dir(&self.root, &mut out)?;
        }
        out.sort_by(|a, b| a.path.0.cmp(&b.path.0));
        Ok(out)
    }

    fn scan_dir(&self, dir: &Path, out: &mut Vec<ScannedFile>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let abs = entry.path();
            let Some(rel) = self.rel_of(&abs) else {
                continue;
            };
            // Prune ignored directories and files up front (spec §4), which also
            // skips our own `.kairn-tmp.` files mid-rename.
            if is_ignored(&rel) {
                continue;
            }
            let ft = entry.file_type()?;
            if ft.is_dir() {
                self.scan_dir(&abs, out)?;
            } else if ft.is_file() {
                let bytes = std::fs::read(&abs)?;
                out.push(ScannedFile {
                    path: rel,
                    hash: hash_bytes(&bytes),
                    size: bytes.len() as u64,
                });
            }
            // Symlinks are neither followed nor synced in v1.
        }
        Ok(())
    }

    /// The vault-relative, forward-slashed path of an absolute path under the
    /// root, or `None` if it is outside the root.
    fn rel_of(&self, abs: &Path) -> Option<VaultPath> {
        let rel = abs.strip_prefix(&self.root).ok()?;
        let mut parts = Vec::new();
        for comp in rel.components() {
            match comp {
                std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
                _ => return None,
            }
        }
        Some(VaultPath(parts.join("/")))
    }
}

/// Atomic write shared with `kairn_core`'s pattern: a hidden temp file carrying
/// pid + counter (so two writers never collide), original permissions carried
/// over, then rename. `before_rename` fires in the gap between the temp write
/// and the rename that makes the change visible.
fn atomic_write(path: &Path, content: &[u8], before_rename: impl FnOnce()) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = path.with_file_name(format!(
        ".{}.kairn-tmp.{}.{}",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let write = (|| {
        std::fs::write(&tmp, content)?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        before_rename();
        std::fs::rename(&tmp, path)
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kairn-sync-vio-{tag}-{}-{}",
                std::process::id(),
                COUNTER_TEST.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    static COUNTER_TEST: AtomicU64 = AtomicU64::new(0);

    fn noop() -> Box<BeforeWrite<'static>> {
        Box::new(|_: &VaultPath| {})
    }

    #[test]
    fn write_then_read_round_trips_and_creates_folders() {
        let s = Scratch::new("rt");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/deep/a.md");
        io.write(&p, b"hello", &*noop()).unwrap();
        assert_eq!(io.read(&p).unwrap(), b"hello");
        assert!(io.exists(&p));
    }

    #[test]
    fn before_hook_fires_and_no_temp_files_survive() {
        let s = Scratch::new("hook");
        let io = VaultIo::new(&s.0, "MAC");
        let seen = std::cell::RefCell::new(Vec::new());
        let hook = |p: &VaultPath| seen.borrow_mut().push(p.clone());
        io.write(&VaultPath::new("Notes/a.md"), b"x", &hook)
            .unwrap();
        assert_eq!(seen.into_inner(), vec![VaultPath::new("Notes/a.md")]);
        // A scan sees the note and none of the atomic temp files (spec §4).
        let scanned: Vec<_> = io.scan().unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(scanned, vec![VaultPath::new("Notes/a.md")]);
    }

    #[test]
    fn delete_is_idempotent() {
        let s = Scratch::new("del");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/a.md");
        io.write(&p, b"x", &*noop()).unwrap();
        io.delete(&p, &*noop()).unwrap();
        assert!(!io.exists(&p));
        // Deleting again is still success.
        io.delete(&p, &*noop()).unwrap();
    }

    #[test]
    fn conflict_copy_uses_the_syncthing_pattern() {
        let io = VaultIo::new("/tmp/whatever", "IPHONE");
        let now = Local.with_ymd_and_hms(2026, 8, 8, 10, 11, 12).unwrap();
        let copy = io.conflict_copy_path(&VaultPath::new("Calendar/20260808.md"), now);
        assert_eq!(
            copy,
            VaultPath::new("Calendar/20260808.sync-conflict-20260808-101112-IPHONE.md")
        );
        // kairn-core detects it: same folder, `{stem}.sync-conflict-` prefix.
        assert!(copy.file_name().starts_with("20260808.sync-conflict-"));
    }

    #[test]
    fn scan_skips_ignored_and_hashes_content() {
        let s = Scratch::new("scan");
        let io = VaultIo::new(&s.0, "MAC");
        io.write(&VaultPath::new("Notes/a.md"), b"a", &*noop())
            .unwrap();
        io.write(
            &VaultPath::new(".kairn/local/dev.json"),
            b"secret",
            &*noop(),
        )
        .unwrap();
        std::fs::write(s.0.join(".DS_Store"), b"junk").unwrap();
        let files = io.scan().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, VaultPath::new("Notes/a.md"));
        assert_eq!(files[0].hash, hash_bytes(b"a"));
    }
}
