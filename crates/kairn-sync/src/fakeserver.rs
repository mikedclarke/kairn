//! An in-memory implementation of the server's revision model (spec §5): the
//! single ordering authority, its append-only journal, and a content-addressed
//! blob store, with compare-and-swap on every write. It is the conformance
//! oracle the engine's tests run against (spec §16) and — behind the `testkit`
//! feature — is reusable by the real server crate to check its own
//! behaviour against the same model.
//!
//! One [`FakeServer`] is shared by any number of [`FakeClient`]s, each carrying
//! a `device_id`, exactly as several devices share one real server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::transport::{Transport, TransportError, TransportResult};
use crate::types::{
    ChangesPage, ContentHash, DeviceId, FileHead, JournalEntry, PutOutcome, Rev, Seq, VaultPath,
};

/// The current head of one path (spec §5 `files`).
#[derive(Clone)]
struct HeadRow {
    rev: Rev,
    hash: Option<ContentHash>,
    deleted: bool,
    size: u64,
}

#[derive(Default)]
struct Inner {
    files: HashMap<VaultPath, HeadRow>,
    journal: Vec<JournalEntry>,
    blobs: HashMap<ContentHash, Vec<u8>>,
    acks: HashMap<DeviceId, Seq>,
    /// Test knob: when true, every request fails as a network error, standing
    /// in for a partition (used by the resumability and echo tests).
    offline: bool,
}

impl Inner {
    fn head_rev(&self, path: &VaultPath) -> Rev {
        self.files.get(path).map(|h| h.rev).unwrap_or(0)
    }
}

/// The shared server. Clone is cheap (shared state) so tests can hold a handle
/// for assertions while clients talk to it.
#[derive(Clone)]
pub struct FakeServer {
    state: Arc<Mutex<Inner>>,
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeServer {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// A transport handle for one device (its writes are attributed to it).
    pub fn client(&self, device: &str) -> FakeClient {
        FakeClient {
            state: self.state.clone(),
            device_id: DeviceId(device.to_string()),
        }
    }

    /// Simulate the network coming and going (spec §16 partition tests).
    pub fn set_offline(&self, offline: bool) {
        self.state.lock().unwrap().offline = offline;
    }

    /// The current head of a path, for assertions.
    pub fn head(&self, path: &VaultPath) -> Option<FileHead> {
        let g = self.state.lock().unwrap();
        g.files.get(path).map(|h| FileHead {
            path: path.clone(),
            rev: h.rev,
            hash: h.hash.clone(),
            deleted: h.deleted,
            size: h.size,
        })
    }

    /// Number of accepted changes (== the latest seq), for assertions. Growth
    /// without bound across an echo test would mean the convergence rule failed.
    pub fn journal_len(&self) -> usize {
        self.state.lock().unwrap().journal.len()
    }

    /// The last cursor a device acked.
    pub fn ack_of(&self, device: &str) -> Seq {
        let g = self.state.lock().unwrap();
        g.acks
            .get(&DeviceId(device.to_string()))
            .copied()
            .unwrap_or(0)
    }
}

/// One device's transport to a [`FakeServer`].
pub struct FakeClient {
    state: Arc<Mutex<Inner>>,
    device_id: DeviceId,
}

impl FakeClient {
    fn guard(&self) -> TransportResult<std::sync::MutexGuard<'_, Inner>> {
        let g = self.state.lock().unwrap();
        if g.offline {
            return Err(TransportError::Network("offline".into()));
        }
        Ok(g)
    }
}

impl Transport for FakeClient {
    fn changes(&self, since: Seq, limit: u32) -> TransportResult<ChangesPage> {
        let g = self.guard()?;
        let limit = limit.max(1) as usize;
        let mut entries: Vec<JournalEntry> = g
            .journal
            .iter()
            .filter(|e| e.seq > since)
            .take(limit)
            .cloned()
            .collect();
        let total_after = g.journal.iter().filter(|e| e.seq > since).count();
        let has_more = total_after > entries.len();
        // The cursor advances to the last entry returned; if the page is empty
        // it stays put, so a caller with nothing new re-asks from the same seq.
        let cursor = entries.last().map(|e| e.seq).unwrap_or(since);
        entries.sort_by_key(|e| e.seq);
        Ok(ChangesPage {
            entries,
            cursor,
            has_more,
        })
    }

    fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
        let g = self.guard()?;
        let hash = match rev {
            None => {
                let head = g.files.get(path).ok_or(TransportError::NotFound)?;
                if head.deleted {
                    return Err(TransportError::NotFound);
                }
                head.hash.clone().ok_or(TransportError::NotFound)?
            }
            Some(r) => g
                .journal
                .iter()
                .rev()
                .find(|e| &e.path == path && e.rev == r)
                .and_then(|e| e.hash.clone())
                .ok_or(TransportError::NotFound)?,
        };
        g.blobs.get(&hash).cloned().ok_or(TransportError::NotFound)
    }

    fn put_blob(
        &self,
        path: &VaultPath,
        base_rev: Rev,
        hash: &ContentHash,
        content: &[u8],
    ) -> TransportResult<PutOutcome> {
        let mut g = self.guard()?;
        let head_rev = g.head_rev(path);
        if base_rev != head_rev {
            let head_hash = g.files.get(path).and_then(|h| h.hash.clone());
            return Ok(PutOutcome::Conflict {
                head_rev,
                head_hash,
            });
        }
        // A no-op write (same hash) still bumps rev by spec; clients avoid
        // sending one by comparing hashes first, which is what kills echoes.
        g.blobs.insert(hash.clone(), content.to_vec());
        let rev = head_rev + 1;
        let seq = g.journal.len() as Seq + 1;
        g.journal.push(JournalEntry {
            seq,
            path: path.clone(),
            rev,
            hash: Some(hash.clone()),
            deleted: false,
            device_id: self.device_id.clone(),
        });
        g.files.insert(
            path.clone(),
            HeadRow {
                rev,
                hash: Some(hash.clone()),
                deleted: false,
                size: content.len() as u64,
            },
        );
        Ok(PutOutcome::Accepted { rev, seq })
    }

    fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
        let mut g = self.guard()?;
        let head_rev = g.head_rev(path);
        if base_rev != head_rev {
            let head_hash = g.files.get(path).and_then(|h| h.hash.clone());
            return Ok(PutOutcome::Conflict {
                head_rev,
                head_hash,
            });
        }
        let rev = head_rev + 1;
        let seq = g.journal.len() as Seq + 1;
        g.journal.push(JournalEntry {
            seq,
            path: path.clone(),
            rev,
            hash: None,
            deleted: true,
            device_id: self.device_id.clone(),
        });
        g.files.insert(
            path.clone(),
            HeadRow {
                rev,
                hash: None,
                deleted: true,
                size: 0,
            },
        );
        Ok(PutOutcome::Accepted { rev, seq })
    }

    fn ack(&self, cursor: Seq) -> TransportResult<()> {
        let mut g = self.guard()?;
        g.acks.insert(self.device_id.clone(), cursor);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_bytes;

    fn put(c: &FakeClient, path: &str, base: Rev, body: &[u8]) -> PutOutcome {
        c.put_blob(&VaultPath::new(path), base, &hash_bytes(body), body)
            .unwrap()
    }

    #[test]
    fn first_write_is_accepted_at_rev_one() {
        let s = FakeServer::new();
        let c = s.client("mac");
        assert_eq!(
            put(&c, "Notes/a.md", 0, b"hi"),
            PutOutcome::Accepted { rev: 1, seq: 1 }
        );
    }

    #[test]
    fn stale_base_rev_is_a_conflict_carrying_the_head() {
        let s = FakeServer::new();
        let mac = s.client("mac");
        let phone = s.client("phone");
        put(&mac, "Notes/a.md", 0, b"one");
        // The phone still thinks the file is new; its write loses the CAS.
        let out = put(&phone, "Notes/a.md", 0, b"two");
        let PutOutcome::Conflict {
            head_rev,
            head_hash,
        } = out
        else {
            panic!("expected conflict, got {out:?}");
        };
        assert_eq!(head_rev, 1);
        assert_eq!(head_hash, Some(hash_bytes(b"one")));
    }

    #[test]
    fn changes_pages_in_seq_order_with_a_resumable_cursor() {
        let s = FakeServer::new();
        let c = s.client("mac");
        put(&c, "Notes/a.md", 0, b"a");
        put(&c, "Notes/b.md", 0, b"b");
        put(&c, "Notes/c.md", 0, b"c");

        let page = c.changes(0, 2).unwrap();
        assert_eq!(
            page.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(page.cursor, 2);
        assert!(page.has_more);

        let page2 = c.changes(page.cursor, 2).unwrap();
        assert_eq!(
            page2.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3]
        );
        assert!(!page2.has_more);

        // Nothing new: cursor holds, empty page.
        let page3 = c.changes(page2.cursor, 2).unwrap();
        assert!(page3.entries.is_empty());
        assert_eq!(page3.cursor, 3);
    }

    #[test]
    fn delete_tombstones_then_head_read_is_not_found() {
        let s = FakeServer::new();
        let c = s.client("mac");
        put(&c, "Notes/a.md", 0, b"a");
        let out = c.delete(&VaultPath::new("Notes/a.md"), 1).unwrap();
        assert_eq!(out, PutOutcome::Accepted { rev: 2, seq: 2 });
        assert!(matches!(
            c.get_blob(&VaultPath::new("Notes/a.md"), None),
            Err(TransportError::NotFound)
        ));
        // ...but the historical rev is still served (retention).
        assert_eq!(
            c.get_blob(&VaultPath::new("Notes/a.md"), Some(1)).unwrap(),
            b"a"
        );
    }

    #[test]
    fn resurrection_after_delete_is_an_ordinary_conditional_write() {
        let s = FakeServer::new();
        let c = s.client("mac");
        put(&c, "Notes/a.md", 0, b"a"); // rev 1
        c.delete(&VaultPath::new("Notes/a.md"), 1).unwrap(); // rev 2 tombstone
        // Deleted head keeps its rev, so the next write bases on rev 2.
        assert_eq!(
            put(&c, "Notes/a.md", 2, b"back"),
            PutOutcome::Accepted { rev: 3, seq: 3 }
        );
    }

    #[test]
    fn offline_surfaces_as_a_network_error() {
        let s = FakeServer::new();
        let c = s.client("mac");
        s.set_offline(true);
        assert!(matches!(c.changes(0, 10), Err(TransportError::Network(_))));
    }
}
