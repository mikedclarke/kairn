//! The vocabulary of the sync protocol (spec §2, §5). Kept to plain structs
//! and enums with named fields so the same shapes survive the UniFFI boundary
//! (GDL-679) without rework: no generics, no lifetimes, no tuple variants in
//! anything the engine hands out.

use serde::{Deserialize, Serialize};

/// Per-file revision, assigned by the server and incremented on every accepted
/// change to that path. `0` means "I believe this file is new" — the client's
/// opening bid in the conditional write (spec §5).
pub type Rev = u64;

/// Server-wide journal sequence: monotonic across the whole vault, one value
/// per accepted change. The *only* thing that orders changes — no code path
/// compares wall clocks (spec §5, invariant §15.5).
pub type Seq = u64;

/// A BLAKE3 content hash as lowercase hex. Two blobs with the same hash are the
/// same bytes, which is what lets bridge echoes die as no-op writes (spec §5).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub String);

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A vault-relative path, always forward-slashed regardless of platform so a
/// file has one identity across macOS, Linux and iOS. Never absolute, never
/// contains `.` or `..` components (the engine builds these from scans).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VaultPath(pub String);

impl VaultPath {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The final path segment (file name), or the whole path if it has none.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Whether this path is safe to resolve against the vault root: relative,
    /// with no empty, `.`, or `..` components and no drive/UNC prefix. Paths
    /// the engine builds from its own scans always are; a path arriving from the
    /// server must be checked before it becomes a filesystem path, so a buggy or
    /// hostile server can never write outside the vault (e.g. `../../.ssh/...`,
    /// or an absolute path that would replace the root on `join`).
    pub fn is_safe(&self) -> bool {
        let p = &self.0;
        if p.is_empty() || p.starts_with('/') || p.starts_with('\\') {
            return false;
        }
        if p.contains(":\\") || p.contains(":/") {
            return false; // Windows drive or UNC prefix
        }
        p.split('/')
            .all(|comp| !comp.is_empty() && comp != "." && comp != ".." && !comp.contains('\\'))
    }

    /// Whether this path names a markdown file, the only kind the engine
    /// three-way merges (everything else is last-writer-wins, spec §4, §8).
    pub fn is_markdown(&self) -> bool {
        let name = self.file_name();
        name.rsplit_once('.').is_some_and(|(_, ext)| {
            ext.eq_ignore_ascii_case("md")
                || ext.eq_ignore_ascii_case("markdown")
                || ext.eq_ignore_ascii_case("txt")
        })
    }
}

impl std::fmt::Display for VaultPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A stable per-install identifier, random at enrollment (spec §2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One accepted change in the server's append-only journal (spec §5). A
/// tombstone carries `deleted = true` and no `hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub seq: Seq,
    pub path: VaultPath,
    pub rev: Rev,
    pub hash: Option<ContentHash>,
    pub deleted: bool,
    pub device_id: DeviceId,
}

/// The current head of one path on the server (spec §5 `files`). A deleted head
/// keeps its rev (so a later resurrection is an ordinary conditional write) and
/// carries no hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHead {
    pub path: VaultPath,
    pub rev: Rev,
    pub hash: Option<ContentHash>,
    pub deleted: bool,
    pub size: u64,
}

/// The outcome of a conditional write (spec §5 CAS, §10 PUT/DELETE). A
/// `Conflict` returns the current head so the client can resolve and retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PutOutcome {
    Accepted {
        rev: Rev,
        seq: Seq,
    },
    Conflict {
        head_rev: Rev,
        head_hash: Option<ContentHash>,
    },
}

/// A page from `GET /changes` (spec §10): journal entries in seq order plus the
/// cursor to resume from and whether more pages remain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesPage {
    pub entries: Vec<JournalEntry>,
    pub cursor: Seq,
    pub has_more: bool,
}

/// What one sync cycle did, returned by `sync_now()` (spec §14). Counts are for
/// observability and tests; the cursor is the resumable position after the
/// cycle's ack.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleReport {
    /// Remote changes applied to the vault (clean writes).
    pub pulled: u32,
    /// Local changes accepted by the server.
    pub pushed: u32,
    /// Files deleted locally in response to a remote tombstone.
    pub deleted_local: u32,
    /// Clean three-way merges (no artifact produced).
    pub merged: u32,
    /// Conflict copies written (spec §8).
    pub conflicts: u32,
    /// Cursor after this cycle's ack.
    pub cursor: Seq,
}

/// A snapshot of engine state for the host UI (spec §14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub running: bool,
    pub cursor: Seq,
    pub last_cycle: Option<CycleReport>,
    /// Last error message, cleared by the next clean cycle.
    pub last_error: Option<String>,
}

/// Events the engine emits to the host (spec §14). `AboutToWrite` is the echo
/// hook of §7: the desktop app feeds the path into its own self-write
/// suppression before the rename lands, so an open buffer reconciles through
/// the already-built `NoteBuffer` merge path rather than a special case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncEvent {
    CycleFinished(CycleReport),
    ConflictCopyCreated {
        original: VaultPath,
        copy: VaultPath,
    },
    AboutToWrite(VaultPath),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_detection_covers_note_extensions() {
        assert!(VaultPath::new("Calendar/20260808.md").is_markdown());
        assert!(VaultPath::new("Notes/Idea.markdown").is_markdown());
        assert!(VaultPath::new("Notes/scratch.txt").is_markdown());
        assert!(!VaultPath::new("Notes/diagram.png").is_markdown());
        assert!(!VaultPath::new("Notes/no-extension").is_markdown());
    }

    #[test]
    fn file_name_is_the_last_segment() {
        assert_eq!(VaultPath::new("a/b/c.md").file_name(), "c.md");
        assert_eq!(VaultPath::new("top.md").file_name(), "top.md");
    }

    #[test]
    fn is_safe_rejects_traversal_and_absolute_paths() {
        assert!(VaultPath::new("Notes/a.md").is_safe());
        assert!(VaultPath::new(".kairn/templates/daily.md").is_safe());
        assert!(!VaultPath::new("../../.ssh/authorized_keys").is_safe());
        assert!(!VaultPath::new("Notes/../../etc/passwd").is_safe());
        assert!(!VaultPath::new("/etc/passwd").is_safe());
        assert!(!VaultPath::new("").is_safe());
        assert!(!VaultPath::new("a//b.md").is_safe());
        assert!(!VaultPath::new("C:\\Windows\\x").is_safe());
        assert!(!VaultPath::new("a\\..\\b").is_safe());
    }
}
