//! The sync surface, designed in now so the engine slots into this same
//! framework later without a second bridge.
//!
//! These are UniFFI mirrors of the value types [`kairn_sync`] hands out: what a
//! cycle did, the engine's status, and the events it emits. The phone's
//! foreground refresh and background-fetch/push handlers are written against
//! these shapes. The live `SyncEngine` object is deliberately **not** exposed
//! yet: it takes a concrete `Transport`, and the HTTP/WebSocket transport lands
//! with GDL-675. Adding it is a new object in this module against the same
//! `kairn-sync` already compiled in here (see the `From` impls below, which put
//! it in the iOS build graph today), not a new framework.
//!
//! The `From` impls are the contract that these mirrors never drift from the
//! engine's own types.

use kairn_sync::{CycleReport, SyncEvent, SyncStatus};

/// What one sync cycle did. Counts are for observability; `cursor` is the
/// resumable journal position after the cycle's ack.
#[derive(uniffi::Record)]
pub struct SyncCycleReport {
    pub pulled: u32,
    pub pushed: u32,
    pub deleted_local: u32,
    pub merged: u32,
    pub conflicts: u32,
    pub cursor: u64,
}

impl From<CycleReport> for SyncCycleReport {
    fn from(r: CycleReport) -> Self {
        Self {
            pulled: r.pulled,
            pushed: r.pushed,
            deleted_local: r.deleted_local,
            merged: r.merged,
            conflicts: r.conflicts,
            cursor: r.cursor,
        }
    }
}

/// A snapshot of engine state for the phone's UI.
#[derive(uniffi::Record)]
pub struct SyncEngineStatus {
    pub running: bool,
    pub cursor: u64,
    pub last_cycle: Option<SyncCycleReport>,
    /// Last error message, cleared by the next clean cycle.
    pub last_error: Option<String>,
}

impl From<SyncStatus> for SyncEngineStatus {
    fn from(s: SyncStatus) -> Self {
        Self {
            running: s.running,
            cursor: s.cursor,
            last_cycle: s.last_cycle.map(Into::into),
            last_error: s.last_error,
        }
    }
}

/// An event the engine emits to the host. Vault paths cross as plain strings
/// (forward-slashed, vault-relative), which is all the Swift side needs to feed
/// its own self-write suppression on `AboutToWrite`.
#[derive(uniffi::Enum)]
pub enum FfiSyncEvent {
    CycleFinished { report: SyncCycleReport },
    ConflictCopyCreated { original: String, copy: String },
    AboutToWrite { path: String },
    Error { message: String },
}

impl From<SyncEvent> for FfiSyncEvent {
    fn from(e: SyncEvent) -> Self {
        match e {
            SyncEvent::CycleFinished(r) => Self::CycleFinished { report: r.into() },
            SyncEvent::ConflictCopyCreated { original, copy } => Self::ConflictCopyCreated {
                original: original.to_string(),
                copy: copy.to_string(),
            },
            SyncEvent::AboutToWrite(path) => Self::AboutToWrite {
                path: path.to_string(),
            },
            SyncEvent::Error(message) => Self::Error { message },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairn_sync::VaultPath;

    #[test]
    fn cycle_report_mirror_matches() {
        let r = CycleReport {
            pulled: 1,
            pushed: 2,
            deleted_local: 0,
            merged: 3,
            conflicts: 1,
            cursor: 42,
        };
        let f: SyncCycleReport = r.into();
        assert_eq!(f.merged, 3);
        assert_eq!(f.cursor, 42);
    }

    #[test]
    fn conflict_event_carries_both_paths() {
        let e = SyncEvent::ConflictCopyCreated {
            original: VaultPath::new("Calendar/20260808.md"),
            copy: VaultPath::new("Calendar/20260808 (conflict IPHONE).md"),
        };
        match e.into() {
            FfiSyncEvent::ConflictCopyCreated { original, copy } => {
                assert_eq!(original, "Calendar/20260808.md");
                assert!(copy.contains("conflict"));
            }
            _ => panic!("wrong variant"),
        }
    }
}
