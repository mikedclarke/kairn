//! The device's sync state (spec §6), kept in SQLite outside the vault so it
//! never syncs and never pollutes the notes tree. Three tables:
//!
//! - `state(path, rev, baseline_hash)` — what this device last synced per file.
//!   The `baseline_hash` is recorded for *every* file so local edits are
//!   detected by a hash mismatch; it is nullable only until the first sync.
//! - `baselines(hash, content)` — the baseline blob, kept for **markdown only**,
//!   because that is the one thing diff3 needs (spec §6, §8). Non-markdown
//!   conflicts don't merge, so their bytes aren't retained here.
//! - `scan_cache(path, size, mtime_ns, hash)` — a memo so the push scan only
//!   re-hashes files whose metadata moved (spec §7: the steady state is cheap).
//! - `meta(k, v)` — cursor, device id, server url, vault id.
//!
//! Crash-safety (invariant §15.4): the baseline blob and the state row for one
//! file move together in a single transaction, so a crash can never leave a
//! rev recorded without the baseline that anchors its next merge.
//!
//! The store also carries the engine's two "am I who I think I am" guards: the
//! DB is bound to the vault root, server, and vault id it was created for
//! (a state DB describing a *different* folder reads as "every file was
//! deleted"), and it holds an exclusive advisory lock for its lifetime, so two
//! engines can never interleave cycles over one vault.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::hash::hash_bytes;
use crate::types::{ContentHash, DeviceId, Rev, Seq, VaultPath};
use crate::vaultio::HashCache;

/// What this device last synced for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileState {
    pub rev: Rev,
    pub baseline_hash: Option<ContentHash>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS state (
    path          TEXT PRIMARY KEY,
    rev           INTEGER NOT NULL,
    baseline_hash TEXT
);
CREATE TABLE IF NOT EXISTS baselines (
    hash    TEXT PRIMARY KEY,
    content BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS scan_cache (
    path     TEXT PRIMARY KEY,
    size     INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    hash     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);
";

pub struct StateStore {
    conn: Connection,
    /// The advisory lock on the DB file, held open for as long as the store is.
    /// Dropping the handle releases it.
    #[allow(dead_code)]
    lock: Option<std::fs::File>,
}

impl StateStore {
    /// Open (creating if absent) the state DB at `path`, ensuring its parent
    /// directory exists, and take the single-instance lock. Journalling is left
    /// at SQLite's durable default.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let lock = lock_exclusive(path)?;
        let conn = Connection::open(path)
            .with_context(|| format!("opening state db {}", path.display()))?;
        Self::init(conn, lock)
    }

    /// An ephemeral store for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, lock: Option<std::fs::File>) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn, lock })
    }

    /// Make every write to this store fail, standing in for a crash between a
    /// vault write and the state update that records it (spec §16).
    #[cfg(any(test, feature = "testkit"))]
    pub fn set_read_only(&self, on: bool) -> Result<()> {
        self.conn.pragma_update(None, "query_only", on)?;
        Ok(())
    }

    // ---- meta ------------------------------------------------------------

    fn meta_get(&self, k: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![k, v],
        )?;
        Ok(())
    }

    /// The last seq fully applied (spec §2 cursor). Zero when nothing has synced.
    pub fn cursor(&self) -> Result<Seq> {
        Ok(self
            .meta_get("cursor")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// Advance the cursor. The cycle calls this only after the corresponding
    /// changes have landed on disk and been acked (invariant §15.4).
    pub fn set_cursor(&self, cursor: Seq) -> Result<()> {
        self.meta_set("cursor", &cursor.to_string())
    }

    /// The cursor last reported to the server, so a cycle that changed nothing
    /// can skip the ack and stay at one round trip (spec §7).
    pub fn acked_cursor(&self) -> Result<Option<Seq>> {
        Ok(self.meta_get("acked_cursor")?.and_then(|s| s.parse().ok()))
    }

    /// Record an ack the server accepted.
    pub fn set_acked_cursor(&self, cursor: Seq) -> Result<()> {
        self.meta_set("acked_cursor", &cursor.to_string())
    }

    pub fn device_id(&self) -> Result<Option<DeviceId>> {
        Ok(self.meta_get("device_id")?.map(DeviceId))
    }

    pub fn set_device_id(&self, id: &DeviceId) -> Result<()> {
        self.meta_set("device_id", &id.0)
    }

    pub fn vault_root(&self) -> Result<Option<String>> {
        self.meta_get("vault_root")
    }

    pub fn server_url(&self) -> Result<Option<String>> {
        self.meta_get("server_url")
    }

    pub fn vault_id(&self) -> Result<Option<String>> {
        self.meta_get("vault_id")
    }

    /// Bind this store to the vault root, server, and vault id it belongs to,
    /// recording them on first open and refusing to run on a later mismatch.
    ///
    /// The state DB *is* this device's record of what every file looked like
    /// last sync. Opened against a different folder it would report every
    /// tracked path as deleted and push a vault-wide tombstone run; against a
    /// different server it would replay another journal's cursor. Both are
    /// unrecoverable-by-the-user, so they fail here instead.
    ///
    /// One deliberate exception: if the recorded root no longer exists, the
    /// folder moved with the store (an iOS app container is re-created with a
    /// new UUID on some restores) and the new root is adopted. A recorded root
    /// that *does* still exist and differs is the dangerous case and is fatal.
    pub fn bind_identity(
        &self,
        vault_root: &Path,
        server_url: Option<&str>,
        vault_id: Option<&str>,
    ) -> Result<()> {
        let root = vault_root.canonicalize().with_context(|| {
            format!(
                "vault root is unreachable: {} (unmounted volume, or the wrong path?)",
                vault_root.display()
            )
        })?;
        let root = root.to_string_lossy().into_owned();
        match self.vault_root()? {
            Some(old) if old != root && !Path::new(&old).exists() => {
                self.meta_set("vault_root", &root)?;
            }
            Some(old) if old != root => bail!(
                "this sync state database belongs to the vault at {old}, but the engine was \
                 pointed at {root}. Refusing to run: reusing one vault's sync state for another \
                 folder would push a delete for every file it does not find. Point it back at \
                 {old}, or start a fresh state database for {root}."
            ),
            Some(_) => {}
            None => self.meta_set("vault_root", &root)?,
        }
        if let Some(url) = server_url {
            self.bind_meta("server_url", url, "sync server")?;
        }
        if let Some(id) = vault_id {
            self.bind_meta("vault_id", id, "vault id")?;
        }
        Ok(())
    }

    fn bind_meta(&self, key: &str, value: &str, label: &str) -> Result<()> {
        match self.meta_get(key)? {
            Some(old) if old != value => bail!(
                "this sync state database was built against {label} {old}, but the engine was \
                 opened with {value}. Refusing to run: its cursor and per-file revs mean nothing \
                 there. Point it back, or start a fresh state database."
            ),
            Some(_) => Ok(()),
            None => self.meta_set(key, value),
        }
    }

    // ---- per-file state --------------------------------------------------

    pub fn file_state(&self, path: &VaultPath) -> Result<Option<FileState>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rev, baseline_hash FROM state WHERE path = ?1",
                [&path.0],
                |r| {
                    let rev: Rev = r.get(0)?;
                    let bh: Option<String> = r.get(1)?;
                    Ok(FileState {
                        rev,
                        baseline_hash: bh.map(ContentHash),
                    })
                },
            )
            .optional()?)
    }

    /// Every file this device has synced, for the push scan (spec §7 step 2).
    pub fn all_states(&self) -> Result<Vec<(VaultPath, FileState)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, rev, baseline_hash FROM state")?;
        let rows = stmt.query_map([], |r| {
            let path: String = r.get(0)?;
            let rev: Rev = r.get(1)?;
            let bh: Option<String> = r.get(2)?;
            Ok((
                VaultPath(path),
                FileState {
                    rev,
                    baseline_hash: bh.map(ContentHash),
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Record that `path` is now synced at `rev` with `content` as its baseline.
    /// The baseline blob is retained only for markdown (the only mergeable
    /// kind); the hash is recorded for all files so future local edits are
    /// detectable. Blob + state row commit together (invariant §15.4).
    pub fn record_synced(
        &self,
        path: &VaultPath,
        rev: Rev,
        content: &[u8],
        is_markdown: bool,
    ) -> Result<ContentHash> {
        let hash = hash_bytes(content);
        let tx = self.conn.unchecked_transaction()?;
        if is_markdown {
            tx.execute(
                "INSERT INTO baselines (hash, content) VALUES (?1, ?2)
                 ON CONFLICT(hash) DO NOTHING",
                params![hash.0, content],
            )?;
        }
        tx.execute(
            "INSERT INTO state (path, rev, baseline_hash) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET rev = excluded.rev, baseline_hash = excluded.baseline_hash",
            params![path.0, rev, hash.0],
        )?;
        tx.commit()?;
        Ok(hash)
    }

    /// Forget a file's state (after a tombstone is applied locally, spec §7).
    pub fn remove_file_state(&self, path: &VaultPath) -> Result<()> {
        self.conn
            .execute("DELETE FROM state WHERE path = ?1", [&path.0])?;
        Ok(())
    }

    /// The baseline blob for a hash, if retained. `None` means "no baseline" —
    /// the caller must degrade safely (treat as conflict, never guess; spec §6).
    pub fn baseline(&self, hash: &ContentHash) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT content FROM baselines WHERE hash = ?1",
                [&hash.0],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?)
    }

    /// Drop baseline blobs no longer referenced by any file's state row. Cheap
    /// on a small vault; keeps the store from accumulating superseded baselines.
    pub fn prune_baselines(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM baselines WHERE hash NOT IN
                (SELECT baseline_hash FROM state WHERE baseline_hash IS NOT NULL)",
            [],
        )?)
    }

    // ---- scan memo -------------------------------------------------------

    /// Drop memo rows for paths no longer in the vault, so the memo can't grow
    /// without bound across a vault's lifetime of renames and deletes.
    pub fn prune_scan_cache(&self, keep: &HashSet<VaultPath>) -> Result<usize> {
        let stale: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT path FROM scan_cache")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|p| !keep.contains(&VaultPath(p.clone())))
                .collect()
        };
        let tx = self.conn.unchecked_transaction()?;
        for path in &stale {
            tx.execute("DELETE FROM scan_cache WHERE path = ?1", [path])?;
        }
        tx.commit()?;
        Ok(stale.len())
    }
}

/// The scan memo lives in the state DB because it has the same lifetime and the
/// same "one engine owns this vault" ownership as the rest of the sync state.
impl HashCache for StateStore {
    fn lookup(&self, path: &VaultPath, size: u64, mtime_ns: i64) -> Option<ContentHash> {
        self.conn
            .query_row(
                "SELECT hash FROM scan_cache WHERE path = ?1 AND size = ?2 AND mtime_ns = ?3",
                params![path.0, size, mtime_ns],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .map(ContentHash)
    }

    fn remember(&self, path: &VaultPath, size: u64, mtime_ns: i64, hash: &ContentHash) {
        let _ = self.conn.execute(
            "INSERT INTO scan_cache (path, size, mtime_ns, hash) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                size = excluded.size, mtime_ns = excluded.mtime_ns, hash = excluded.hash",
            params![path.0, size, mtime_ns, hash.0],
        );
    }
}

/// Take an exclusive advisory lock for this state DB, held until the process
/// exits or the store is dropped.
///
/// Two engines over one vault interleave cycles undetected, which the spec
/// forbids outright ("exactly one bridge device, ever", §3). The everyday way it
/// happens is launchd respawning the bridge while the old process is still
/// blocked in a two-minute HTTP call, so failing to start is exactly the right
/// outcome. `flock` is released by the kernel if a process dies, so a crash
/// never leaves a stale lock behind.
///
/// The lock lives on a sidecar file rather than the database itself: on macOS
/// `flock` and the POSIX locks SQLite uses share one lock slot per file, so
/// locking the DB would lock SQLite out of it too.
#[cfg(unix)]
fn lock_exclusive(path: &Path) -> Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state.db".into());
    let lock_path = path.with_file_name(format!("{name}.lock"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening sync lock file {}", lock_path.display()))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!(
            "another Kairn sync engine is already running against {} (its lock file {} is held). \
             Stop that one first: two engines over one vault interleave cycles.",
            path.display(),
            lock_path.display(),
        );
    }
    Ok(Some(file))
}

#[cfg(not(unix))]
fn lock_exclusive(_path: &Path) -> Result<Option<std::fs::File>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(s: &str) -> VaultPath {
        VaultPath::new(s)
    }

    #[test]
    fn cursor_defaults_to_zero_then_persists() {
        let s = StateStore::open_in_memory().unwrap();
        assert_eq!(s.cursor().unwrap(), 0);
        s.set_cursor(42).unwrap();
        assert_eq!(s.cursor().unwrap(), 42);
    }

    /// A scratch directory that cleans itself up.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "kairn-sync-state-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn sub(&self, name: &str) -> std::path::PathBuf {
            let p = self.0.join(name);
            std::fs::create_dir_all(&p).unwrap();
            p
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn meta_round_trips_device_and_vault() {
        let sc = Scratch::new("meta");
        let s = StateStore::open(&sc.0.join("sync.db")).unwrap();
        assert!(s.device_id().unwrap().is_none());
        s.set_device_id(&DeviceId("iphone".into())).unwrap();
        s.bind_identity(&sc.sub("vault"), Some("http://mini:8080"), Some("vault-1"))
            .unwrap();
        assert_eq!(s.device_id().unwrap(), Some(DeviceId("iphone".into())));
        assert_eq!(s.vault_id().unwrap().as_deref(), Some("vault-1"));
        assert_eq!(s.server_url().unwrap().as_deref(), Some("http://mini:8080"));
    }

    #[test]
    fn a_state_db_refuses_a_different_vault_or_server() {
        // Re-pointing an engine at another folder under the same vault id would
        // otherwise read as "every tracked file was deleted".
        let sc = Scratch::new("identity");
        let db = sc.0.join("sync.db");
        let first = sc.sub("notes");
        let other = sc.sub("other-notes");
        {
            let s = StateStore::open(&db).unwrap();
            s.bind_identity(&first, Some("http://mini:8787"), Some("default"))
                .unwrap();
            // Re-binding to exactly the same identity is fine (every open does).
            s.bind_identity(&first, Some("http://mini:8787"), Some("default"))
                .unwrap();
        }
        {
            let s = StateStore::open(&db).unwrap();
            let err = s
                .bind_identity(&other, Some("http://mini:8787"), Some("default"))
                .err()
                .unwrap()
                .to_string();
            assert!(err.contains("belongs to the vault at"), "{err}");
            assert!(err.contains("Refusing to run"), "{err}");

            let err = s
                .bind_identity(&first, Some("http://elsewhere:8787"), Some("default"))
                .err()
                .unwrap()
                .to_string();
            assert!(err.contains("sync server"), "{err}");

            let err = s
                .bind_identity(&first, Some("http://mini:8787"), Some("other-vault"))
                .err()
                .unwrap()
                .to_string();
            assert!(err.contains("vault id"), "{err}");
        }
        // A root that no longer exists means the folder moved with the store
        // (an iOS container re-created under a new UUID): adopt the new one.
        {
            std::fs::remove_dir_all(&first).unwrap();
            let s = StateStore::open(&db).unwrap();
            s.bind_identity(&other, Some("http://mini:8787"), Some("default"))
                .unwrap();
            assert_eq!(
                s.vault_root().unwrap(),
                Some(other.canonicalize().unwrap().to_string_lossy().into_owned())
            );
        }
        // A vault root that isn't there at all is an error, not a fresh start.
        {
            let s = StateStore::open(&sc.0.join("other.db")).unwrap();
            assert!(s.bind_identity(&sc.0.join("nope"), None, None).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_second_engine_cannot_open_the_same_state_db() {
        let sc = Scratch::new("lock");
        let db = sc.0.join("sync.db");
        let first = StateStore::open(&db).unwrap();
        let err = StateStore::open(&db).err().unwrap().to_string();
        assert!(err.contains("already running"), "{err}");
        // Once the first store is dropped the lock is released.
        drop(first);
        assert!(StateStore::open(&db).is_ok());
    }

    #[test]
    fn the_scan_memo_matches_on_metadata_and_prunes_the_gone() {
        let s = StateStore::open_in_memory().unwrap();
        let p = vp("Notes/a.md");
        let h = hash_bytes(b"a");
        assert!(s.lookup(&p, 1, 42).is_none());
        s.remember(&p, 1, 42, &h);
        assert_eq!(s.lookup(&p, 1, 42), Some(h.clone()));
        // Either half of the metadata moving is a miss, so the file is re-read.
        assert!(s.lookup(&p, 2, 42).is_none());
        assert!(s.lookup(&p, 1, 43).is_none());

        s.remember(&vp("Notes/gone.md"), 5, 7, &h);
        let keep: HashSet<VaultPath> = [p.clone()].into_iter().collect();
        assert_eq!(s.prune_scan_cache(&keep).unwrap(), 1);
        assert_eq!(s.lookup(&p, 1, 42), Some(h));
        assert!(s.lookup(&vp("Notes/gone.md"), 5, 7).is_none());
    }

    #[test]
    fn acked_cursor_tracks_the_last_report() {
        let s = StateStore::open_in_memory().unwrap();
        assert_eq!(s.acked_cursor().unwrap(), None);
        s.set_acked_cursor(9).unwrap();
        assert_eq!(s.acked_cursor().unwrap(), Some(9));
    }

    #[test]
    fn markdown_keeps_a_baseline_blob_non_markdown_does_not() {
        let s = StateStore::open_in_memory().unwrap();
        let md = vp("Notes/a.md");
        let png = vp("Notes/pic.png");
        let md_hash = s.record_synced(&md, 1, b"# hi\n", true).unwrap();
        let png_hash = s.record_synced(&png, 1, b"\x89PNG...", false).unwrap();

        assert_eq!(
            s.baseline(&md_hash).unwrap().as_deref(),
            Some(&b"# hi\n"[..])
        );
        // Non-markdown: state tracked, but no blob to merge from.
        assert!(s.baseline(&png_hash).unwrap().is_none());
        assert_eq!(
            s.file_state(&png).unwrap(),
            Some(FileState {
                rev: 1,
                baseline_hash: Some(png_hash)
            })
        );
    }

    #[test]
    fn state_survives_reopen_and_updates_in_place() {
        let dir = std::env::temp_dir().join(format!("kairn-sync-state-{}", std::process::id()));
        let path = dir.join("sync.db");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let s = StateStore::open(&path).unwrap();
            s.record_synced(&vp("Notes/a.md"), 1, b"one", true).unwrap();
            s.set_cursor(7).unwrap();
        }
        {
            let s = StateStore::open(&path).unwrap();
            assert_eq!(s.cursor().unwrap(), 7);
            let st = s.file_state(&vp("Notes/a.md")).unwrap().unwrap();
            assert_eq!(st.rev, 1);
            // Update to rev 2 with new content.
            s.record_synced(&vp("Notes/a.md"), 2, b"two", true).unwrap();
            assert_eq!(s.file_state(&vp("Notes/a.md")).unwrap().unwrap().rev, 2);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_drops_orphaned_baselines_only() {
        let s = StateStore::open_in_memory().unwrap();
        let p = vp("Notes/a.md");
        let old = s.record_synced(&p, 1, b"one", true).unwrap();
        let new = s.record_synced(&p, 2, b"two", true).unwrap(); // supersedes `old`
        assert_eq!(s.prune_baselines().unwrap(), 1);
        assert!(s.baseline(&old).unwrap().is_none());
        assert!(s.baseline(&new).unwrap().is_some());
    }

    #[test]
    fn removing_state_forgets_the_file() {
        let s = StateStore::open_in_memory().unwrap();
        let p = vp("Notes/a.md");
        s.record_synced(&p, 1, b"x", true).unwrap();
        s.remove_file_state(&p).unwrap();
        assert!(s.file_state(&p).unwrap().is_none());
    }
}
