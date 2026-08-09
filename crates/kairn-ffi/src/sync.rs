//! The sync surface: the value types [`kairn_sync`] hands out and the live
//! [`FfiSyncEngine`] object the phone drives, both in one framework.
//!
//! The records below are UniFFI mirrors of what a cycle did, the engine's
//! status, and the events it emits; the phone's foreground refresh and
//! background-fetch/push handlers are written against these shapes. The `From`
//! impls are the contract that keeps them from ever drifting from the engine's
//! own types.
//!
//! [`FfiSyncEngine`] wraps [`kairn_sync::SyncEngine`] over the concrete
//! [`HttpTransport`] (spec §10). The Swift side constructs one with its server
//! URL, device token, and vault paths, implements [`SyncEventListener`] to
//! receive events, and calls `sync_now()` from its foreground and
//! background-fetch/push handlers. No sync logic lives here: edits, merge, undo,
//! the cycle, and conflict rules all stay in `kairn-sync`, so desktop and phone
//! run the one engine with no drift.

use std::sync::Arc;

use kairn_sync::engine::EngineConfig;
use kairn_sync::{CycleReport, HttpTransport, SyncEngine, SyncEvent, SyncStatus};

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

/// A sync failure crossing the FFI. The engine's errors are `anyhow`; they
/// arrive on the Swift side as one message string (opening the state store,
/// transport/auth failures during a cycle). A rejected conditional write is
/// never an error here — the engine resolves it internally (spec §8).
#[derive(Debug, uniffi::Error)]
pub enum SyncError {
    Engine { message: String },
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl SyncError {
    fn engine(e: impl std::fmt::Display) -> Self {
        Self::Engine {
            message: e.to_string(),
        }
    }
}

/// The Swift side implements this to receive engine events (spec §14): a
/// finished cycle, a conflict copy, the about-to-write hook the app feeds into
/// its self-write suppression, and errors. Called from the engine's threads, so
/// implementations must not block or call back into the engine.
#[uniffi::export(callback_interface)]
pub trait SyncEventListener: Send + Sync {
    fn on_event(&self, event: FfiSyncEvent);
}

/// The live sync engine, one per vault, over the HTTP transport (spec §10, §14).
/// Wraps [`kairn_sync::SyncEngine`] for shared ownership across the FFI; every
/// method delegates, so the cycle and conflict rules stay in `kairn-sync`.
#[derive(uniffi::Object)]
pub struct FfiSyncEngine {
    inner: SyncEngine,
}

#[uniffi::export]
impl FfiSyncEngine {
    /// Build an engine for one vault. `server_url` is the sync server origin
    /// (scheme + host + port), `token` the device's enrollment bearer token,
    /// `vault_root` the notes folder on device, `state_db` the sync-state
    /// database path (kept outside the vault, spec §6), and `device_label` the
    /// tag stamped into conflict-copy names (e.g. `IPHONE`, spec §8).
    ///
    /// Fails if the state store cannot be opened, if another engine already
    /// holds it, or if it belongs to a different vault or server — the store
    /// records the identity it was created against and refuses to be reused
    /// elsewhere, because sync state read against the wrong folder looks exactly
    /// like every file having been deleted.
    #[uniffi::constructor]
    pub fn new(
        server_url: String,
        vault_id: String,
        token: String,
        vault_root: String,
        state_db: String,
        device_label: String,
        listener: Box<dyn SyncEventListener>,
    ) -> Result<Arc<Self>, SyncError> {
        let transport = HttpTransport::new(server_url.clone(), vault_id.clone(), token);
        let on_event: Box<dyn Fn(SyncEvent) + Send + Sync> =
            Box::new(move |e: SyncEvent| listener.on_event(e.into()));
        let inner = SyncEngine::new(
            EngineConfig {
                vault_root: vault_root.into(),
                state_db: state_db.into(),
                device_label,
                server_url: Some(server_url),
                vault_id: Some(vault_id),
                // The phone never bulk-deletes: a vault that suddenly reads as
                // empty there is a container that failed to mount, not an
                // intention (spec §15.2).
                allow_bulk_delete: false,
            },
            Box::new(transport),
            on_event,
        )
        .map_err(SyncError::engine)?;
        Ok(Arc::new(Self { inner }))
    }

    /// Run one full sync cycle now and return what it did. The phone calls this
    /// on foreground and from its background-fetch/push handlers (spec §7, §12).
    ///
    /// **Never call this on the main thread.** It blocks for the whole cycle,
    /// which is network-bound and can run to the transport's two-minute ceiling
    /// on a bad link; dispatch it to a background queue and hand the report back
    /// to the main thread.
    pub fn sync_now(&self) -> Result<SyncCycleReport, SyncError> {
        self.inner
            .sync_now()
            .map(Into::into)
            .map_err(SyncError::engine)
    }

    /// Start the background worker (an immediate cycle, then a safety-timer
    /// cadence). Idempotent. The phone typically drives cycles explicitly
    /// instead, but this exists for parity with desktop. Returns immediately;
    /// the cycles run on the engine's own thread.
    pub fn start(&self) {
        self.inner.start();
    }

    /// Stop the background worker, waiting for the in-flight cycle to reach its
    /// next step boundary (it is cancelled there rather than run to completion,
    /// so this returns promptly even mid-request). Idempotent. The phone calls
    /// this when protected data becomes unavailable (locked).
    ///
    /// **Never call this on the main thread**: it joins the worker thread.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// A snapshot of engine state for the UI.
    pub fn status(&self) -> SyncEngineStatus {
        self.inner.status().into()
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
