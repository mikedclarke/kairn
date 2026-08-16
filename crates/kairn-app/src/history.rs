//! The workspace's structural undo history: mutations that cross file
//! boundaries (a block dragged to another day's note, a timeline retime),
//! which the per-note buffer undo cannot represent. Each op stores enough
//! to invert itself on disk with content verification, so an undo landing
//! after an external edit (agent, sync, the other machine) refuses quietly
//! instead of guessing.
//!
//! The undo chord is owned by the workspace, which orders these ops against
//! the note buffer's own undo steps by timestamp: whichever changed the
//! vault most recently is what Cmd+Z takes back.

use std::path::PathBuf;

pub(crate) enum VaultOp {
    /// A block moved from one note to another.
    Transfer {
        ms: u64,
        /// The note the block was removed from.
        from: PathBuf,
        /// The note the block was inserted into.
        to: PathBuf,
        /// The exact text moved (several dragged blocks arrive joined):
        /// the verification token for both directions.
        block: String,
        /// Line index the block started at in `from`; undo puts it back
        /// there.
        from_line_idx: usize,
        /// Line index the block landed at in `to`; redo aims there again.
        to_line_idx: usize,
    },
    /// A timeline drag rewrote one line's time range in place.
    Retime {
        ms: u64,
        path: PathBuf,
        line_idx: usize,
        /// The raw line before and after the retime; each direction
        /// verifies against the side it expects to find.
        before: String,
        after: String,
    },
}

impl VaultOp {
    pub fn ms(&self) -> u64 {
        match self {
            VaultOp::Transfer { ms, .. } | VaultOp::Retime { ms, .. } => *ms,
        }
    }
}

#[derive(Default)]
pub(crate) struct VaultHistory {
    undo: Vec<VaultOp>,
    redo: Vec<VaultOp>,
}

impl VaultHistory {
    /// Record a fresh op. Anything undone and not redone stops being
    /// reachable, matching buffer-undo semantics.
    pub fn push(&mut self, op: VaultOp) {
        self.undo.push(op);
        self.redo.clear();
    }

    pub fn last_undo_ms(&self) -> Option<u64> {
        self.undo.last().map(VaultOp::ms)
    }

    pub fn last_redo_ms(&self) -> Option<u64> {
        self.redo.last().map(VaultOp::ms)
    }

    pub fn pop_undo(&mut self) -> Option<VaultOp> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<VaultOp> {
        self.redo.pop()
    }

    /// An op that was just inverted on disk moves to the redo side.
    pub fn push_undone(&mut self, op: VaultOp) {
        self.redo.push(op);
    }

    /// An op that was just re-applied moves back to the undo side.
    pub fn push_redone(&mut self, op: VaultOp) {
        self.undo.push(op);
    }
}
