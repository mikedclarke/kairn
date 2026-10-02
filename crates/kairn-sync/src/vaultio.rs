//! Reading and writing the vault on disk. Every write is atomic (temp file +
//! rename, permissions preserved), matching `kairn_core`'s `write.rs` discipline
//! so a crash never leaves a half-written note (invariant §15.1). Just before a
//! rename lands, a hook fires (spec §7 echo suppression) so the desktop app can
//! feed the path into its own self-write suppression. The engine does not filter
//! its own writes out of the watcher stream; instead a self-write triggers at
//! most a cheap no-op cycle, because the file already matches its baseline
//! (compared by hash), so nothing loops.
//!
//! Two things here exist purely to keep the never-clobber invariant honest
//! against writers the engine does not control (the app, Syncthing on a bridged
//! folder): every write carries a [`Precondition`] re-checked in the instant
//! before the rename, and a scan refuses to report an empty vault when the root
//! is simply not there.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use crate::vault_fs;

use crate::hash::hash_bytes;
use crate::ignore::is_ignored;
use crate::types::{ContentHash, VaultPath};

/// A memoised hash is trusted only for files whose mtime is at least this old.
/// Timestamp resolution is finite, so a write landing in the same tick as the
/// one we memoised would otherwise be invisible: same size, same mtime, new
/// content. Anything written in the last couple of seconds is re-hashed.
const MTIME_SETTLE: Duration = Duration::from_secs(2);

/// One file found by a scan: its vault path, content hash, and byte size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: VaultPath,
    pub hash: ContentHash,
    pub size: u64,
}

/// Fired with the vault path about to be written, just before the rename lands.
pub type BeforeWrite<'a> = dyn Fn(&VaultPath) + 'a;

/// What the destination must still hold for a guarded write (or delete) to
/// land. The engine decides what to write from a pre-image it read *before* a
/// network round trip; re-checking that pre-image immediately before the rename
/// is what stops a write that arrived in the meantime — from the app, or from
/// Syncthing on a bridged folder — being clobbered (invariant §15.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition {
    /// Land it whatever is there. Used where the destination is ours by
    /// construction (a freshly uniquified conflict-copy name).
    Any,
    /// The file must still be absent.
    Absent,
    /// The file must still hash to this.
    Unchanged(ContentHash),
}

impl Precondition {
    /// The precondition matching what the caller last read at that path.
    pub fn from_local(local: Option<&[u8]>) -> Self {
        match local {
            None => Self::Absent,
            Some(bytes) => Self::Unchanged(hash_bytes(bytes)),
        }
    }

    fn holds(&self, current: Option<&[u8]>) -> bool {
        match (self, current) {
            (Self::Any, _) => true,
            (Self::Absent, current) => current.is_none(),
            (Self::Unchanged(h), Some(bytes)) => &hash_bytes(bytes) == h,
            (Self::Unchanged(_), None) => false,
        }
    }
}

/// A `(path, size, mtime) -> hash` memo so a scan re-hashes only the files whose
/// metadata moved. The bridge polls every few seconds; without a memo every
/// cycle re-reads and re-hashes the whole vault, which the spec's "a cycle with
/// an empty pull and empty push must be cheap" (§7) does not survive.
pub trait HashCache {
    fn lookup(&self, path: &VaultPath, size: u64, mtime_ns: i64) -> Option<ContentHash>;
    fn remember(&self, path: &VaultPath, size: u64, mtime_ns: i64, hash: &ContentHash);
}

/// No memo: every file is read and hashed on every scan.
pub struct NoHashCache;

impl HashCache for NoHashCache {
    fn lookup(&self, _path: &VaultPath, _size: u64, _mtime_ns: i64) -> Option<ContentHash> {
        None
    }
    fn remember(&self, _path: &VaultPath, _size: u64, _mtime_ns: i64, _hash: &ContentHash) {}
}

pub struct VaultIo {
    root: PathBuf,
    root_handle: Option<Dir>,
    /// The device label stamped into conflict-copy names, e.g. `IPHONE`.
    device_label: String,
}

impl VaultIo {
    pub fn new(root: impl Into<PathBuf>, device_label: impl Into<String>) -> Self {
        let root = root.into();
        let root_handle = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).ok();
        Self {
            root,
            root_handle,
            device_label: device_label.into(),
        }
    }

    pub fn validate_root(&self) -> io::Result<()> {
        let current = Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
            .map_err(|error| io::Error::new(error.kind(),
                format!("vault root is missing or unreadable: {}", self.root.display())))?;
        let Some(original) = &self.root_handle else {
            return Err(io::Error::new(io::ErrorKind::NotFound,
                "vault root was unavailable when sync started; restart sync after checking the folder"));
        };
        if !vault_fs::same_directory(original, &current)? {
            return Err(io::Error::other("vault root changed; restart sync after checking the folder"));
        }
        Ok(())
    }

    fn parent(&self, path: &VaultPath, create: bool) -> io::Result<(Dir, String)> {
        self.validate_root()?;
        vault_fs::parent(self.root_handle.as_ref().unwrap(), path, create)
    }

    fn validate_parent(&self, path: &VaultPath, original: &Dir) -> io::Result<()> {
        let (current, _) = self.parent(path, false)?;
        if !vault_fs::same_directory(original, &current)? {
            return Err(io::Error::other("vault directory changed during sync; retry the operation"));
        }
        Ok(())
    }

    #[cfg(test)]
    fn abs(&self, path: &VaultPath) -> PathBuf {
        self.root.join(&path.0)
    }

    pub fn exists(&self, path: &VaultPath) -> bool {
        self.parent(path, false).and_then(|(dir, name)| vault_fs::open(&dir, &name)).is_ok()
    }

    pub fn read(&self, path: &VaultPath) -> io::Result<Vec<u8>> {
        self.read_opt(path)?.ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    /// Read an existing file, refusing symlinks in any component.
    pub fn read_opt(&self, path: &VaultPath) -> io::Result<Option<Vec<u8>>> {
        match self.parent(path, false) {
            Ok((dir, name)) => vault_fs::read_opt(&dir, &name),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.validate_root()?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Atomically write `content` to `path`, creating parent folders as needed,
    /// but only while `pre` still holds. Returns whether the write landed;
    /// `false` means the file changed underneath us between the caller reading
    /// it and the rename, so the caller must re-resolve rather than clobber.
    /// `before` is called with the path immediately before the rename (spec §7).
    pub fn write(
        &self,
        path: &VaultPath,
        content: &[u8],
        pre: &Precondition,
        before: &BeforeWrite,
    ) -> io::Result<bool> {
        let (dir, name) = self.parent(path, true)?;
        let _lock = vault_fs::lock(&dir, &name)?;
        vault_fs::atomic_write(&dir, &name, content, || {
            self.validate_parent(path, &dir)?;
            if !pre.holds(vault_fs::read_opt(&dir, &name)?.as_deref()) {
                return Ok(false);
            }
            before(path);
            self.validate_parent(path, &dir)?;
            Ok(true)
        })
    }

    /// Delete a file while `pre` still holds, announcing the change first (spec
    /// §7). A file that is already gone is success — deletes are idempotent
    /// (invariant §15.3). Returns `false` when the file changed underneath us.
    pub fn delete(
        &self,
        path: &VaultPath,
        pre: &Precondition,
        before: &BeforeWrite,
    ) -> io::Result<bool> {
        let (dir, name) = match self.parent(path, false) {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.validate_root()?;
                return Ok(pre.holds(None));
            }
            Err(error) => return Err(error),
        };
        let _lock = vault_fs::lock(&dir, &name)?;
        if !pre.holds(vault_fs::read_opt(&dir, &name)?.as_deref()) {
            return Ok(false);
        }
        before(path);
        self.validate_parent(path, &dir)?;
        match dir.remove_file(&name) {
            Ok(()) => { vault_fs::sync_directory(&dir)?; Ok(true) }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// The conflict-copy path for `original` at `now`, in the Syncthing pattern
    /// the app already detects and banners: `{stem}.sync-conflict-YYYYMMDD-
    /// HHMMSS-{DEVICE}.{ext}` (spec §8, §17.2). `nth` past the first appends a
    /// counter, because the stamp is only second-resolution.
    pub fn conflict_copy_path(
        &self,
        original: &VaultPath,
        now: DateTime<Local>,
        nth: u32,
    ) -> VaultPath {
        let name = original.file_name();
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s, Some(e)),
            None => (name, None),
        };
        let ts = now.format("%Y%m%d-%H%M%S");
        let label = &self.device_label;
        let tail = if nth > 1 {
            format!("{label}-{nth}")
        } else {
            label.clone()
        };
        let copy_name = match ext {
            Some(ext) => format!("{stem}.sync-conflict-{ts}-{tail}.{ext}"),
            None => format!("{stem}.sync-conflict-{ts}-{tail}"),
        };
        match original.0.rsplit_once('/') {
            Some((dir, _)) => VaultPath(format!("{dir}/{copy_name}")),
            None => VaultPath(copy_name),
        }
    }

    /// Write `content` as a conflict copy beside `original`, returning its path
    /// (spec §8). Conflict copies sync like any other file, so the same
    /// resolution shows up on every device.
    ///
    /// The name is uniquified against what is already on disk: the push retry
    /// loop can produce two conflict copies for one file inside the same second,
    /// and the second overwriting the first would destroy the only copy of the
    /// losing text (invariant §15.2). Writing with [`Precondition::Absent`] and
    /// stepping the counter makes that race-free rather than merely unlikely.
    pub fn write_conflict_copy(
        &self,
        original: &VaultPath,
        content: &[u8],
        before: &BeforeWrite,
    ) -> io::Result<VaultPath> {
        let (dir, name) = self.parent(original, true)?;
        let original_name = Path::new(&name);
        let prefix = format!("{}.sync-conflict-", original_name.file_stem().unwrap().to_string_lossy());
        let mut copies = Vec::new();
        for entry in dir.entries()? {
            let entry = entry?;
            let file_name = entry.file_name();
            if entry.file_type()?.is_file()
                && file_name.to_str().is_some_and(|name| name.starts_with(&prefix))
                && Path::new(&file_name).extension() == original_name.extension()
            {
                copies.push(file_name.to_str().unwrap().to_owned());
            }
        }
        copies.sort();
        for name in copies {
            let _lock = vault_fs::lock(&dir, &name)?;
            if vault_fs::read_opt(&dir, &name)?.as_deref() == Some(content) {
                self.validate_parent(original, &dir)?;
                vault_fs::open(&dir, &name)?.sync_all()?;
                vault_fs::sync_directory(&dir)?;
                return Ok(match original.0.rsplit_once('/') {
                    Some((parent, _)) => VaultPath(format!("{parent}/{name}")),
                    None => VaultPath(name),
                });
            }
        }
        let now = Local::now();
        for nth in 1..=1000 {
            let copy = self.conflict_copy_path(original, now, nth);
            if self.write(&copy, content, &Precondition::Absent, before)? {
                self.sync_copy(&copy)?;
                return Ok(copy);
            }
            // A copy already sitting there with exactly this text *is* this
            // copy: a retried resolution should not leave the same losing text
            // twice over.
            if self.read_opt(&copy)?.as_deref() == Some(content) {
                self.sync_copy(&copy)?;
                return Ok(copy);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("no free conflict-copy name beside {original}"),
        ))
    }

    fn sync_copy(&self, path: &VaultPath) -> io::Result<()> {
        let (dir, name) = self.parent(path, false)?;
        vault_fs::open(&dir, &name)?.sync_all()?;
        vault_fs::sync_directory(&dir)
    }

    /// Walk the vault, returning every syncable file with its hash and size
    /// (spec §4 exclusions applied), re-hashing only files whose size or mtime
    /// moved since `cache` last saw them. The startup and 30-minute safety scans
    /// and the push scan all run through this.
    pub fn scan_cached(&self, cache: &dyn HashCache) -> io::Result<Vec<ScannedFile>> {
        self.validate_root()?;
        let mut out = Vec::new();
        let now_ns = now_ns();
        self.scan_dir(self.root_handle.as_ref().unwrap(), "", cache, now_ns, &mut out)?;
        self.validate_root()?;
        out.sort_by(|a, b| a.path.0.cmp(&b.path.0));
        Ok(out)
    }

    /// A scan that hashes every file (no memo).
    pub fn scan(&self) -> io::Result<Vec<ScannedFile>> {
        self.scan_cached(&NoHashCache)
    }

    fn scan_dir(
        &self,
        dir: &Dir,
        prefix: &str,
        cache: &dyn HashCache,
        now_ns: i64,
        out: &mut Vec<ScannedFile>,
    ) -> io::Result<()> {
        for entry in dir.entries()? {
            let entry = entry?;
            let name = entry.file_name().into_string()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "vault file name is not UTF-8"))?;
            let rel = VaultPath(if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") });
            if is_ignored(&rel) { continue; }
            if !rel.is_safe() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "vault file name cannot be synced"));
            }
            let ft = entry.file_type()?;
            if ft.is_dir() {
                let child = dir.open_dir_nofollow(&name)?;
                self.scan_dir(&child, &rel.0, cache, now_ns, out)?;
                self.validate_root()?;
                let current = vault_fs::directory(self.root_handle.as_ref().unwrap(), &rel, false)?;
                if !vault_fs::same_directory(&child, &current)? {
                    return Err(io::Error::other("vault directory changed during scan"));
                }
            } else if ft.is_file() {
                out.push(self.scan_file(vault_fs::open(dir, &name)?, rel, cache, now_ns)?);
            }
        }
        Ok(())
    }

    fn scan_file(
        &self,
        mut file: std::fs::File,
        rel: VaultPath,
        cache: &dyn HashCache,
        now_ns: i64,
    ) -> io::Result<ScannedFile> {
        let meta = file.metadata()?;
        let size = meta.len();
        let mtime = mtime_ns(&meta);
        if let Some(mtime) = mtime
            && now_ns.saturating_sub(mtime) >= MTIME_SETTLE.as_nanos() as i64
            && let Some(hash) = cache.lookup(&rel, size, mtime)
        {
            return Ok(ScannedFile {
                path: rel,
                hash,
                size,
            });
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let hash = hash_bytes(&bytes);
        if let Some(mtime) = mtime {
            cache.remember(&rel, bytes.len() as u64, mtime, &hash);
        }
        Ok(ScannedFile {
            path: rel,
            hash,
            size: bytes.len() as u64,
        })
    }

}

/// Nanoseconds since the epoch, negative before it. `i64` runs to the year 2262
/// and is what SQLite stores.
fn to_ns(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).ok(),
        Err(e) => i64::try_from(e.duration().as_nanos()).ok().map(|n| -n),
    }
}

fn now_ns() -> i64 {
    to_ns(SystemTime::now()).unwrap_or(i64::MAX)
}

fn mtime_ns(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified().ok().and_then(to_ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

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

    /// A memo that counts how many files a scan actually re-hashed.
    #[derive(Default)]
    struct CountingCache {
        entries: RefCell<HashMap<(String, u64, i64), ContentHash>>,
        hashed: Cell<u32>,
    }

    impl HashCache for CountingCache {
        fn lookup(&self, path: &VaultPath, size: u64, mtime_ns: i64) -> Option<ContentHash> {
            self.entries
                .borrow()
                .get(&(path.0.clone(), size, mtime_ns))
                .cloned()
        }
        fn remember(&self, path: &VaultPath, size: u64, mtime_ns: i64, hash: &ContentHash) {
            self.hashed.set(self.hashed.get() + 1);
            self.entries
                .borrow_mut()
                .insert((path.0.clone(), size, mtime_ns), hash.clone());
        }
    }

    #[test]
    fn write_then_read_round_trips_and_creates_folders() {
        let s = Scratch::new("rt");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/deep/a.md");
        assert!(
            io.write(&p, b"hello", &Precondition::Absent, &*noop())
                .unwrap()
        );
        assert_eq!(io.read(&p).unwrap(), b"hello");
        assert!(io.exists(&p));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parents_refuse_reads_writes_deletes_and_conflict_copies() {
        let vault = Scratch::new("contained");
        let outside = Scratch::new("outside");
        std::fs::write(outside.0.join("a.md"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside.0, vault.0.join("Notes")).unwrap();
        let io = VaultIo::new(&vault.0, "DEVICE");
        let existing = VaultPath::new("Notes/a.md");
        assert!(io.read(&existing).is_err());
        assert!(io.read_opt(&existing).is_err());
        assert!(!io.exists(&existing));
        assert!(io.write(&existing, b"replacement", &Precondition::Any, &*noop()).is_err());
        assert!(io.write(&VaultPath::new("Notes/new/sub.md"), b"new", &Precondition::Absent, &*noop()).is_err());
        assert!(io.delete(&existing, &Precondition::Any, &*noop()).is_err());
        assert!(io.write_conflict_copy(&existing, b"conflict", &*noop()).is_err());
        assert!(io.scan().unwrap().is_empty());
        assert_eq!(std::fs::read(outside.0.join("a.md")).unwrap(), b"outside");
        assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_and_lock_sidecars_are_never_followed() {
        let vault = Scratch::new("leaf-links");
        let outside = Scratch::new("leaf-target");
        let target = outside.0.join("a.md");
        std::fs::write(&target, b"outside").unwrap();
        std::os::unix::fs::symlink(&target, vault.0.join("a.md")).unwrap();
        let io = VaultIo::new(&vault.0, "DEVICE");
        let path = VaultPath::new("a.md");
        assert!(io.read(&path).is_err());
        assert!(io.write(&path, b"replacement", &Precondition::Any, &*noop()).is_err());
        assert!(io.delete(&path, &Precondition::Any, &*noop()).is_err());
        std::os::unix::fs::symlink(&target, vault.0.join(".b.md.kairn-lock")).unwrap();
        assert!(io.write(&VaultPath::new("b.md"), b"new", &Precondition::Absent, &*noop()).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"outside");
        assert!(!vault.0.join("b.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn parent_swapped_for_a_symlink_before_commit_aborts_without_touching_the_target() {
        for delete in [false, true] {
            let vault = Scratch::new("parent-swap");
            let outside = Scratch::new("swap-target");
            std::fs::create_dir(vault.0.join("Notes")).unwrap();
            std::fs::write(vault.0.join("Notes/a.md"), b"inside").unwrap();
            std::fs::write(outside.0.join("a.md"), b"outside").unwrap();
            let io = VaultIo::new(&vault.0, "DEVICE");
            let hook = |_: &VaultPath| {
                std::fs::rename(vault.0.join("Notes"), vault.0.join("old")).unwrap();
                std::os::unix::fs::symlink(&outside.0, vault.0.join("Notes")).unwrap();
            };
            let path = VaultPath::new("Notes/a.md");
            let result = if delete {
                io.delete(&path, &Precondition::Any, &hook)
            } else {
                io.write(&path, b"replacement", &Precondition::Any, &hook)
            };
            assert!(result.is_err());
            assert_eq!(std::fs::read(outside.0.join("a.md")).unwrap(), b"outside");
            assert_eq!(std::fs::read(vault.0.join("old/a.md")).unwrap(), b"inside");
            assert!(std::fs::read_dir(vault.0.join("old")).unwrap().all(|entry|
                !entry.unwrap().file_name().to_string_lossy().contains(".kairn-tmp.")));
            assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn filesystem_operations_reject_unsafe_paths_even_without_a_cycle() {
        let vault = Scratch::new("unsafe-path");
        let io = VaultIo::new(&vault.0, "DEVICE");
        for path in ["../outside.md", "/outside.md", "Notes/../outside.md", "Notes\\outside.md"] {
            let path = VaultPath::new(path);
            assert!(io.read_opt(&path).is_err());
            assert!(io.write(&path, b"new", &Precondition::Any, &*noop()).is_err());
            assert!(io.delete(&path, &Precondition::Any, &*noop()).is_err());
            assert!(io.write_conflict_copy(&path, b"copy", &*noop()).is_err());
        }
        assert_eq!(std::fs::read_dir(&vault.0).unwrap().count(), 0);
    }

    #[test]
    fn before_hook_fires_and_no_temp_files_survive() {
        let s = Scratch::new("hook");
        let io = VaultIo::new(&s.0, "MAC");
        let seen = std::cell::RefCell::new(Vec::new());
        let hook = |p: &VaultPath| seen.borrow_mut().push(p.clone());
        io.write(
            &VaultPath::new("Notes/a.md"),
            b"x",
            &Precondition::Any,
            &hook,
        )
        .unwrap();
        assert_eq!(seen.into_inner(), vec![VaultPath::new("Notes/a.md")]);
        // A scan sees the note and none of the atomic temp files (spec §4).
        let scanned: Vec<_> = io.scan().unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(scanned, vec![VaultPath::new("Notes/a.md")]);
    }

    #[test]
    fn a_stale_precondition_abandons_the_write() {
        // The never-clobber check: content resolved against bytes that are no
        // longer on disk must not land, and must leave the newer bytes alone.
        let s = Scratch::new("pre");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/a.md");
        io.write(&p, b"v1", &Precondition::Absent, &*noop())
            .unwrap();

        let stale = Precondition::Unchanged(hash_bytes(b"v0"));
        assert!(!io.write(&p, b"merged", &stale, &*noop()).unwrap());
        assert_eq!(io.read(&p).unwrap(), b"v1");
        assert!(!io.delete(&p, &stale, &*noop()).unwrap());
        assert!(io.exists(&p));

        // The matching precondition still lands, and leaves no temp file behind.
        let fresh = Precondition::Unchanged(hash_bytes(b"v1"));
        assert!(io.write(&p, b"merged", &fresh, &*noop()).unwrap());
        assert_eq!(io.read(&p).unwrap(), b"merged");
        assert_eq!(io.scan().unwrap().len(), 1);
    }

    #[test]
    fn sync_holds_the_shared_lock_through_the_before_write_hook() {
        let s = Scratch::new("shared-lock");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/a.md");
        io.write(&p, b"base\n", &Precondition::Absent, &*noop()).unwrap();
        let hook = |_: &VaultPath| {
            let error = kairn_core::append_line(&io.abs(&p), "concurrent").unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        };
        assert!(io.write(&p, b"remote\n", &Precondition::Any, &hook).unwrap());
        kairn_core::append_line(&io.abs(&p), "concurrent").unwrap();
        assert_eq!(io.read(&p).unwrap(), b"remote\nconcurrent\n");
        let lock = kairn_core::NoteLock::acquire(&io.abs(&p)).unwrap();
        assert_eq!(io.delete(&p, &Precondition::Any, &*noop()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock);
        drop(lock);
    }

    #[test]
    fn absent_precondition_refuses_to_overwrite() {
        let s = Scratch::new("absent");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/a.md");
        io.write(&p, b"there", &Precondition::Any, &*noop())
            .unwrap();
        assert!(
            !io.write(&p, b"new", &Precondition::Absent, &*noop())
                .unwrap()
        );
        assert_eq!(io.read(&p).unwrap(), b"there");
    }

    #[test]
    fn delete_is_idempotent() {
        let s = Scratch::new("del");
        let io = VaultIo::new(&s.0, "MAC");
        let p = VaultPath::new("Notes/a.md");
        io.write(&p, b"x", &Precondition::Any, &*noop()).unwrap();
        let clean = Precondition::Unchanged(hash_bytes(b"x"));
        assert!(io.delete(&p, &clean, &*noop()).unwrap());
        assert!(!io.exists(&p));
        // Deleting again is still success.
        assert!(io.delete(&p, &Precondition::Absent, &*noop()).unwrap());
    }

    #[test]
    fn conflict_copy_uses_the_syncthing_pattern() {
        let io = VaultIo::new("/tmp/whatever", "IPHONE");
        let now = Local.with_ymd_and_hms(2026, 8, 8, 10, 11, 12).unwrap();
        let copy = io.conflict_copy_path(&VaultPath::new("Calendar/20260808.md"), now, 1);
        assert_eq!(
            copy,
            VaultPath::new("Calendar/20260808.sync-conflict-20260808-101112-IPHONE.md")
        );
        // kairn-core detects it: same folder, `{stem}.sync-conflict-` prefix.
        assert!(copy.file_name().starts_with("20260808.sync-conflict-"));
        // The counter keeps that prefix intact so the app's banner still fires.
        let second = io.conflict_copy_path(&VaultPath::new("Calendar/20260808.md"), now, 2);
        assert_eq!(
            second,
            VaultPath::new("Calendar/20260808.sync-conflict-20260808-101112-IPHONE-2.md")
        );
    }

    #[test]
    fn two_conflict_copies_in_one_second_both_survive() {
        // Second resolution means the second copy would land on the first's
        // name; the losing text it holds must not be overwritten (§15.2).
        let s = Scratch::new("cc");
        let io = VaultIo::new(&s.0, "MAC");
        let original = VaultPath::new("Notes/a.md");
        let first = io
            .write_conflict_copy(&original, b"lost one\n", &*noop())
            .unwrap();
        let second = io
            .write_conflict_copy(&original, b"lost two\n", &*noop())
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(io.read(&first).unwrap(), b"lost one\n");
        assert_eq!(io.read(&second).unwrap(), b"lost two\n");
        assert_eq!(
            kairn_core::conflict_copies(&s.0.join("Notes/a.md")).len(),
            2
        );
    }

    #[test]
    fn conflict_copy_reuse_survives_a_restart_and_a_different_timestamp() {
        let s = Scratch::new("cc-restart");
        let original = VaultPath::new("Notes/a.md");
        let io = VaultIo::new(&s.0, "MAC");
        let old = io.conflict_copy_path(&original,
            Local.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap(), 1);
        io.write(&old, b"local\n", &Precondition::Absent, &*noop()).unwrap();
        let restarted = VaultIo::new(&s.0, "MAC");
        assert_eq!(restarted.write_conflict_copy(&original, b"local\n", &*noop()).unwrap(), old);
        assert_eq!(restarted.scan().unwrap().len(), 1);
    }

    #[test]
    fn scan_skips_ignored_and_hashes_content() {
        let s = Scratch::new("scan");
        let io = VaultIo::new(&s.0, "MAC");
        io.write(
            &VaultPath::new("Notes/a.md"),
            b"a",
            &Precondition::Any,
            &*noop(),
        )
        .unwrap();
        io.write(
            &VaultPath::new(".kairn/local/dev.json"),
            b"secret",
            &Precondition::Any,
            &*noop(),
        )
        .unwrap();
        std::fs::write(s.0.join(".DS_Store"), b"junk").unwrap();
        let files = io.scan().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, VaultPath::new("Notes/a.md"));
        assert_eq!(files[0].hash, hash_bytes(b"a"));
    }

    #[test]
    fn a_write_cannot_recreate_a_vault_that_disappeared_after_reading() {
        let s = Scratch::new("lost-root-write");
        let root = s.0.join("vault");
        std::fs::create_dir(&root).unwrap();
        let io = VaultIo::new(&root, "MAC");
        let path = VaultPath::new("Notes/deep/a.md");
        assert!(io.read_opt(&path).unwrap().is_none());
        std::fs::remove_dir(&root).unwrap();
        assert!(io.write(&path, b"remote", &Precondition::Absent, &*noop()).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn a_root_replaced_in_the_before_write_hook_aborts_replacement() {
        let s = Scratch::new("root-at-rename");
        let root = s.0.join("vault");
        let old = s.0.join("old");
        std::fs::create_dir(&root).unwrap();
        let io = VaultIo::new(&root, "MAC");
        let path = VaultPath::new("a.md");
        io.write(&path, b"local", &Precondition::Absent, &*noop()).unwrap();
        let hook = |_: &VaultPath| {
            std::fs::rename(&root, &old).unwrap();
            std::fs::create_dir(&root).unwrap();
        };
        assert!(io.write(&path, b"remote", &Precondition::Any, &hook).is_err());
        assert_eq!(std::fs::read(old.join("a.md")).unwrap(), b"local");
        assert!(!root.join("a.md").exists());
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_vault() {
        // The mass-tombstone guard at its source: "no files" must never be the
        // answer for a root that isn't there.
        let s = Scratch::new("missing");
        let gone = s.0.join("not-mounted");
        let io = VaultIo::new(&gone, "MAC");
        let err = io.scan().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("vault root is missing"));

        // A root that exists but is a file, not a directory, fails the same way.
        std::fs::write(&gone, b"not a folder").unwrap();
        assert!(io.scan().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_cached_scan_rehashes_nothing_when_metadata_holds_still() {
        let s = Scratch::new("cache");
        let io = VaultIo::new(&s.0, "MAC");
        io.write(
            &VaultPath::new("Notes/a.md"),
            b"a",
            &Precondition::Any,
            &*noop(),
        )
        .unwrap();
        io.write(
            &VaultPath::new("Notes/b.md"),
            b"bb",
            &Precondition::Any,
            &*noop(),
        )
        .unwrap();
        // Backdate both files past the settle window so the memo is trusted.
        backdate(&s.0.join("Notes/a.md"), 60);
        backdate(&s.0.join("Notes/b.md"), 60);

        let cache = CountingCache::default();
        let first = io.scan_cached(&cache).unwrap();
        assert_eq!(cache.hashed.get(), 2);
        let second = io.scan_cached(&cache).unwrap();
        assert_eq!(second, first);
        assert_eq!(cache.hashed.get(), 2, "a settled file must not be re-read");

        // A changed file is picked up: same length, new bytes, new mtime.
        std::fs::write(s.0.join("Notes/b.md"), b"cc").unwrap();
        let third = io.scan_cached(&cache).unwrap();
        assert_eq!(cache.hashed.get(), 3);
        assert_eq!(third[1].hash, hash_bytes(b"cc"));
    }

    #[cfg(unix)]
    fn backdate(path: &Path, secs: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let t = libc::timeval {
            tv_sec: (now - secs) as libc::time_t,
            tv_usec: 0,
        };
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::utimes(c.as_ptr(), [t, t].as_ptr()) }, 0);
    }
}
