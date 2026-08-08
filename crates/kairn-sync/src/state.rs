//! The device's sync state (spec §6), kept in SQLite outside the vault so it
//! never syncs and never pollutes the notes tree. Three tables:
//!
//! - `state(path, rev, baseline_hash)` — what this device last synced per file.
//!   The `baseline_hash` is recorded for *every* file so local edits are
//!   detected by a hash mismatch; it is nullable only until the first sync.
//! - `baselines(hash, content)` — the baseline blob, kept for **markdown only**,
//!   because that is the one thing diff3 needs (spec §6, §8). Non-markdown
//!   conflicts don't merge, so their bytes aren't retained here.
//! - `meta(k, v)` — cursor, device id, server url, vault id.
//!
//! Crash-safety (invariant §15.4): the baseline blob and the state row for one
//! file move together in a single transaction, so a crash can never leave a
//! rev recorded without the baseline that anchors its next merge.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::hash::hash_bytes;
use crate::types::{ContentHash, DeviceId, Rev, Seq, VaultPath};

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
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);
";

pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    /// Open (creating if absent) the state DB at `path`, ensuring its parent
    /// directory exists. Journalling is left at SQLite's durable default.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating state dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening state db {}", path.display()))?;
        Self::init(conn)
    }

    /// An ephemeral store for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
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

    pub fn device_id(&self) -> Result<Option<DeviceId>> {
        Ok(self.meta_get("device_id")?.map(DeviceId))
    }

    pub fn set_device_id(&self, id: &DeviceId) -> Result<()> {
        self.meta_set("device_id", &id.0)
    }

    pub fn server_url(&self) -> Result<Option<String>> {
        self.meta_get("server_url")
    }

    pub fn set_server_url(&self, url: &str) -> Result<()> {
        self.meta_set("server_url", url)
    }

    pub fn vault_id(&self) -> Result<Option<String>> {
        self.meta_get("vault_id")
    }

    pub fn set_vault_id(&self, id: &str) -> Result<()> {
        self.meta_set("vault_id", id)
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

    #[test]
    fn meta_round_trips_device_and_vault() {
        let s = StateStore::open_in_memory().unwrap();
        assert!(s.device_id().unwrap().is_none());
        s.set_device_id(&DeviceId("iphone".into())).unwrap();
        s.set_vault_id("vault-1").unwrap();
        s.set_server_url("http://mini:8080").unwrap();
        assert_eq!(s.device_id().unwrap(), Some(DeviceId("iphone".into())));
        assert_eq!(s.vault_id().unwrap().as_deref(), Some("vault-1"));
        assert_eq!(s.server_url().unwrap().as_deref(), Some("http://mini:8080"));
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
