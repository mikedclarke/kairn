//! The Kairn sync engine: one implementation of the sync spec
//! (`sync-spec.md`, v1) that compiles natively into the desktop app and, via
//! UniFFI, into the iOS app, so the cycle and conflict rules are identical on
//! every platform with no protocol drift.
//!
//! The engine is deliberately runtime-free: `sync_now()` is a blocking call and
//! background work runs on a plain std thread, so no async runtime enters the
//! iOS binary. Merging is never reimplemented here — it delegates to
//! `kairn_core::merge3`, the same three-way merge the editor's save path uses,
//! which keeps the two invariants sync must never weaken: never clobber an
//! external edit, never silently drop the user's text.
//!
//! The server is reached through the [`Transport`] trait, so the engine is
//! fully testable against an in-memory fake (spec §16); the concrete HTTP and
//! WebSocket transport lands with the server.

pub mod cycle;
pub mod engine;
pub mod hash;
#[cfg(feature = "http")]
pub mod http;
pub mod ignore;
pub mod resolve;
pub mod state;
pub mod transport;
pub mod types;
pub mod vaultio;
#[cfg(feature = "http")]
mod wake;

pub use engine::{EngineConfig, RemoteWakeConfig, SyncEngine};

/// The in-memory server model: always built for the crate's own tests, and
/// exposed under `testkit` so the real server crate can reuse it as an oracle.
#[cfg(any(test, feature = "testkit"))]
pub mod fakeserver;

#[cfg(feature = "http")]
pub use http::HttpTransport;
pub use transport::{Transport, TransportError};
pub use types::*;
