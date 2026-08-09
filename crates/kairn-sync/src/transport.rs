//! The seam between the engine and the server (spec §10). A synchronous trait
//! so the engine stays runtime-free; the in-memory fake (`fakeserver`) drives
//! every test, and the concrete HTTP/WebSocket client implements the same trait
//! alongside the real server.

use crate::types::{ChangesPage, ContentHash, PutOutcome, Rev, Seq, VaultPath};

/// A transport failure. Note that a rejected conditional write is **not** an
/// error — it comes back as [`PutOutcome::Conflict`] for the client to resolve.
/// These variants are the genuinely exceptional cases: the network, the
/// protocol, auth, and a missing blob.
#[derive(Debug)]
pub enum TransportError {
    /// The request could not be completed (connection, timeout, I/O).
    Network(String),
    /// The server answered, but not in a shape the protocol allows.
    Protocol(String),
    /// The device token was rejected or revoked (spec §10).
    Auth,
    /// A requested blob/rev is no longer retained (spec §11).
    NotFound,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(m) => write!(f, "sync transport network error: {m}"),
            Self::Protocol(m) => write!(f, "sync transport protocol error: {m}"),
            Self::Auth => write!(f, "sync transport auth rejected"),
            Self::NotFound => write!(f, "sync transport blob not found"),
        }
    }
}

impl std::error::Error for TransportError {}

pub type TransportResult<T> = Result<T, TransportError>;

/// The client's view of the server. Every method is one HTTP round trip in the
/// real transport; the cycle (spec §7) is built entirely from these calls and
/// nothing else, which is what keeps it idempotent and resumable.
pub trait Transport: Send + Sync {
    /// `GET /changes?since=cursor&limit=N`: a page of journal entries in seq
    /// order plus the cursor to resume from (spec §10).
    fn changes(&self, since: Seq, limit: u32) -> TransportResult<ChangesPage>;

    /// `GET /files/{path}?rev=R`: the blob at that rev, or the head if `rev` is
    /// `None`. Historical revs are served while retained (spec §10, §11).
    fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>>;

    /// `PUT /files/{path}` with `base_rev` + content hash: a conditional write.
    /// Accepted when `base_rev` matches the head, otherwise a conflict carrying
    /// the current head (spec §5 CAS).
    fn put_blob(
        &self,
        path: &VaultPath,
        base_rev: Rev,
        hash: &ContentHash,
        content: &[u8],
    ) -> TransportResult<PutOutcome>;

    /// `DELETE /files/{path}` with `base_rev`: a conditional tombstone (spec §9).
    fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome>;

    /// `POST /ack {cursor}`: report the last seq fully applied, so the server can
    /// tell whether this device is behind (push, pruning safety — spec §7, §11).
    fn ack(&self, cursor: Seq) -> TransportResult<()>;
}
