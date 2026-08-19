//! The engine facade the host drives (spec §14). One `SyncEngine` per vault:
//! `sync_now()` runs a blocking cycle, `start()`/`stop()` own a background
//! thread that runs the cycle on a filesystem-watcher wake (desktop), on a
//! remote wake from the server's freshness WebSocket (spec §12), and on a
//! 30-minute safety timer, and `status()` reports the latest state.
//!
//! The same object serves both platforms: hosts call `start()` while they can
//! keep a worker alive (desktop always; iOS while foregrounded) and
//! `sync_now()` from moments that demand a cycle (app-foreground,
//! background-fetch/push handlers). The watcher is desktop-only; the remote
//! wake runs everywhere a server URL is configured. The shapes here are
//! UniFFI-friendly (plain config in, plain report out, an event callback) so
//! the iOS bridge can wrap them without reshaping the engine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;

use crate::cycle::{SyncContext, run_cycle};
use crate::state::StateStore;
use crate::transport::Transport;
use crate::types::{CycleReport, SyncConfig, SyncEvent, SyncStatus};
use crate::vaultio::VaultIo;

/// Where the freshness wake socket connects and how it authenticates (spec
/// §12). Mirrors the transport's identity. Defined here, not in the wake
/// module, so hosts can carry one in [`EngineConfig`] whether or not the
/// `http` feature (which brings the actual listener) is compiled in.
#[derive(Clone)]
pub struct RemoteWakeConfig {
    /// The server origin, e.g. `http://100.121.119.52:8787`. Plain HTTP on
    /// purpose, the same terms as the HTTP transport.
    pub server_url: String,
    pub vault_id: String,
    /// The device bearer token; the wake route sits behind the same auth as
    /// the rest of the API.
    pub token: String,
}

/// The safety-net full cycle interval when nothing wakes the engine sooner
/// (spec §7). The watcher and explicit `sync_now()` handle the common case.
const SAFETY_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Where one vault's engine lives and how it labels itself.
pub struct EngineConfig {
    /// The vault root (the notes folder).
    pub vault_root: PathBuf,
    /// The device's sync-state database, kept outside the vault (spec §6).
    pub state_db: PathBuf,
    /// The label stamped into conflict-copy names, e.g. `IPHONE` (spec §8).
    pub device_label: String,
    /// The server origin this engine talks to, recorded in the state DB so the
    /// store can refuse to be reused against a different one. `None` skips the
    /// check (an engine over a fake transport has no URL).
    pub server_url: Option<String>,
    /// The vault id on that server, recorded for the same reason.
    pub vault_id: Option<String>,
    /// Let a cycle push an unbounded number of deletes (see [`SyncConfig`]).
    pub allow_bulk_delete: bool,
    /// Listen on the server's freshness WebSocket while the worker runs, so a
    /// remote edit triggers a cycle in seconds instead of on the safety timer
    /// (spec §12). `None` (and engines over a fake transport) rely on the
    /// watcher and timer alone; without the `http` feature the value is
    /// carried but no listener is built.
    pub remote_wake: Option<RemoteWakeConfig>,
}

/// The mutable half, guarded so a cycle never overlaps itself. A cycle is
/// serialised through this lock; there is never more than one in flight.
struct Inner {
    state: StateStore,
    vault: VaultIo,
    transport: Box<dyn Transport>,
}

/// Everything a cycle needs that outlives one call, shared with the worker
/// thread. The event sink lives here rather than inside [`Inner`] so the
/// terminal event of a cycle can be delivered with the cycle lock already
/// released: a listener that calls back into the engine then blocks nothing.
struct Shared {
    inner: Mutex<Inner>,
    status: Mutex<SyncStatus>,
    emit: Box<dyn Fn(SyncEvent) + Send + Sync>,
    config: SyncConfig,
    /// Set by `stop()`, checked by the cycle between transport calls.
    cancel: AtomicBool,
}

struct Worker {
    handle: JoinHandle<()>,
}

/// Shared worker wake state. `pending` is the fix for a change that arrives
/// while a cycle is already running (nobody waiting on the condvar): the
/// watcher sets it, and the worker checks it before sleeping, so a lone edit is
/// never stranded until the safety timer.
#[derive(Default)]
struct WakeState {
    stop: bool,
    pending: bool,
}

pub struct SyncEngine {
    shared: Arc<Shared>,
    worker: Mutex<Option<Worker>>,
    vault_root: PathBuf,
    /// The worker waits on the condvar; the watcher and the remote wake set
    /// `pending` and notify it to run sooner, and `stop()` sets `stop` and
    /// notifies it to exit.
    wake: Arc<(Mutex<WakeState>, Condvar)>,
    /// Where the remote wake listener connects, if configured.
    remote_wake: Option<RemoteWakeConfig>,
}

impl SyncEngine {
    /// Build an engine over `transport`, delivering events to `on_event`.
    /// Events raised *during* a cycle arrive while the cycle lock is held, so a
    /// listener must not call back into the engine from one.
    ///
    /// Fails when the state store cannot be opened, when another engine already
    /// holds it, or when it belongs to a different vault or server (spec §6).
    pub fn new(
        config: EngineConfig,
        transport: Box<dyn Transport>,
        on_event: Box<dyn Fn(SyncEvent) + Send + Sync>,
    ) -> Result<Self> {
        let state = StateStore::open(&config.state_db)?;
        state.bind_identity(
            &config.vault_root,
            config.server_url.as_deref(),
            config.vault_id.as_deref(),
        )?;
        let vault = VaultIo::new(&config.vault_root, &config.device_label);
        let remote_wake = config.remote_wake.clone();
        Ok(Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    state,
                    vault,
                    transport,
                }),
                status: Mutex::new(SyncStatus::default()),
                emit: on_event,
                config: SyncConfig {
                    allow_bulk_delete: config.allow_bulk_delete,
                },
                cancel: AtomicBool::new(false),
            }),
            worker: Mutex::new(None),
            vault_root: config.vault_root,
            wake: Arc::new((Mutex::new(WakeState::default()), Condvar::new())),
            remote_wake,
        })
    }

    /// Run one cycle now and return what it did. Blocking (it talks to the
    /// server), so never call it on a UI thread. Safe from any other thread, and
    /// while the background worker is running: cycles are serialised.
    pub fn sync_now(&self) -> Result<CycleReport> {
        run_once(&self.shared)
    }

    /// A snapshot of engine state for the host UI (spec §14).
    pub fn status(&self) -> SyncStatus {
        let mut s = self.shared.status.lock().unwrap().clone();
        s.running = self.worker.lock().unwrap().is_some();
        s
    }

    /// Start the background worker: an immediate cycle, then cycles on watcher
    /// wakes (desktop), remote wakes from the server's freshness socket, and
    /// the safety timer. Idempotent — a second call while running does nothing.
    pub fn start(&self) {
        let mut w = self.worker.lock().unwrap();
        if w.is_some() {
            return;
        }
        *self.wake.0.lock().unwrap() = WakeState::default();
        self.shared.cancel.store(false, Ordering::SeqCst);
        let shared = self.shared.clone();
        let wake = self.wake.clone();
        let root = self.vault_root.clone();
        let remote = self.remote_wake.clone();
        let handle = std::thread::spawn(move || worker_loop(shared, wake, root, remote));
        *w = Some(Worker { handle });
        self.shared.status.lock().unwrap().running = true;
    }

    /// Stop the background worker and wait for it to finish. Idempotent. iOS
    /// calls this on protected-data-unavailable (locked phone), inside a short
    /// background window, so an in-flight cycle is cancelled at its next step
    /// boundary rather than run to completion behind a two-minute HTTP timeout.
    pub fn stop(&self) {
        let worker = self.worker.lock().unwrap().take();
        if let Some(w) = worker {
            self.shared.cancel.store(true, Ordering::SeqCst);
            {
                let (lock, cvar) = &*self.wake;
                lock.lock().unwrap().stop = true;
                cvar.notify_all();
            }
            let _ = w.handle.join();
            self.shared.cancel.store(false, Ordering::SeqCst);
        }
        self.shared.status.lock().unwrap().running = false;
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run one cycle under the cycle lock, fold the outcome into `status`, then
/// deliver the terminal event with both locks released, so a listener that calls
/// straight back into the engine cannot deadlock.
fn run_once(shared: &Shared) -> Result<CycleReport> {
    let result = {
        let guard = shared.inner.lock().unwrap();
        let cancel = || shared.cancel.load(Ordering::SeqCst);
        let ctx = SyncContext {
            transport: guard.transport.as_ref(),
            state: &guard.state,
            vault: &guard.vault,
            emit: &*shared.emit,
            config: &shared.config,
            cancel: &cancel,
        };
        run_cycle(&ctx)
    };
    let event = {
        let mut s = shared.status.lock().unwrap();
        match &result {
            Ok(report) => {
                s.cursor = report.cursor;
                s.last_cycle = Some(report.clone());
                s.last_error = None;
                SyncEvent::CycleFinished(report.clone())
            }
            Err(e) => {
                let msg = e.to_string();
                s.last_error = Some(msg.clone());
                SyncEvent::Error(msg)
            }
        }
    };
    (shared.emit)(event);
    result
}

fn worker_loop(
    shared: Arc<Shared>,
    wake: Arc<(Mutex<WakeState>, Condvar)>,
    root: PathBuf,
    remote: Option<RemoteWakeConfig>,
) {
    // The watcher lives for the thread's lifetime and is dropped (unwatched)
    // when the loop exits. On iOS it is never built (no persistent watcher).
    #[cfg(not(target_os = "ios"))]
    let _watcher = make_watcher(&root, wake.clone());
    #[cfg(target_os = "ios")]
    let _ = &root;

    // The remote wake listener nudges the worker exactly the way the watcher
    // does, and dies with the loop the same way. Without the `http` feature
    // there is no listener to build.
    #[cfg(feature = "http")]
    let _remote_wake = remote.map(|config| {
        let wake = wake.clone();
        crate::wake::RemoteWake::spawn(config, move || {
            let (lock, cvar) = &*wake;
            lock.lock().unwrap().pending = true;
            cvar.notify_all();
        })
    });
    #[cfg(not(feature = "http"))]
    let _ = remote;

    loop {
        let _ = run_once(&shared);
        let (lock, cvar) = &*wake;
        let mut g = lock.lock().unwrap();
        if g.stop {
            break;
        }
        // A change that arrived during the cycle just run isn't lost: skip the
        // wait and run again now. Otherwise wait for the watcher or safety timer.
        if !g.pending {
            let (next, _timeout) = cvar.wait_timeout(g, SAFETY_INTERVAL).unwrap();
            g = next;
        }
        if g.stop {
            break;
        }
        g.pending = false;
    }
}

/// A recursive filesystem watcher that wakes the worker on any change. The
/// worker runs one full cycle per wake; a cycle that finds nothing dirty is a
/// cheap no-op, so a burst of edits settles in a cycle or two. The engine's own
/// writes do generate events, but they resolve to no-op cycles because the file
/// already matches its baseline (compared by hash), so they don't loop. Returns
/// `None` if the platform watcher can't start; the safety timer still covers
/// changes.
#[cfg(not(target_os = "ios"))]
fn make_watcher(
    root: &std::path::Path,
    wake: Arc<(Mutex<WakeState>, Condvar)>,
) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            // Record that a change happened, then wake the worker. The flag is
            // what makes the wake survive if the worker is mid-cycle.
            let (lock, cvar) = &*wake;
            lock.lock().unwrap().pending = true;
            cvar.notify_all();
        }
    })
    .ok()?;
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakeserver::FakeServer;
    use crate::types::VaultPath;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TAG: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        state_db: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "kairn-sync-eng-{label}-{}-{}",
                std::process::id(),
                TAG.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(base.join("vault")).unwrap();
            Self {
                root: base.join("vault"),
                state_db: base.join("state.db"),
            }
        }

        fn engine(&self, server: &FakeServer, label: &str) -> SyncEngine {
            self.try_engine(server, label, &self.root).unwrap()
        }

        fn try_engine(
            &self,
            server: &FakeServer,
            label: &str,
            root: &std::path::Path,
        ) -> Result<SyncEngine> {
            SyncEngine::new(
                EngineConfig {
                    vault_root: root.to_path_buf(),
                    state_db: self.state_db.clone(),
                    device_label: label.to_uppercase(),
                    server_url: Some("http://mini:8787".into()),
                    vault_id: Some("default".into()),
                    allow_bulk_delete: false,
                    remote_wake: None,
                },
                Box::new(server.client(label)),
                Box::new(|_e| {}),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(parent) = self.root.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn sync_now_pushes_and_status_tracks_the_last_cycle() {
        let server = FakeServer::new();
        let fx = Fixture::new("mac");
        let engine = fx.engine(&server, "mac");

        std::fs::write(fx.root.join("a.md"), b"hi\n").unwrap();
        let report = engine.sync_now().unwrap();
        assert_eq!(report.pushed, 1);

        let status = engine.status();
        assert_eq!(status.last_cycle.as_ref().map(|c| c.pushed), Some(1));
        assert!(status.last_error.is_none());
        assert!(!status.running); // never started the worker
    }

    #[test]
    fn start_then_stop_runs_at_least_one_cycle_and_reports_running() {
        let server = FakeServer::new();
        let fx = Fixture::new("mac");
        let engine = fx.engine(&server, "mac");
        std::fs::write(fx.root.join("a.md"), b"hi\n").unwrap();

        engine.start();
        assert!(engine.status().running);
        // The worker's immediate cycle pushes the file; wait briefly for it.
        let mut pushed = false;
        for _ in 0..200 {
            if server.head(&VaultPath::new("a.md")).is_some() {
                pushed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pushed, "worker should have pushed within the grace period");

        engine.stop();
        assert!(!engine.status().running);
    }

    #[test]
    fn a_state_db_bound_to_another_folder_refuses_to_open() {
        // Re-pointing an engine at a different folder while keeping its state
        // would read as a vault-wide delete; it must not get that far.
        let server = FakeServer::new();
        let fx = Fixture::new("bound");
        drop(fx.engine(&server, "mac"));
        let other = fx.root.parent().unwrap().join("other-vault");
        std::fs::create_dir_all(&other).unwrap();
        let err = fx
            .try_engine(&server, "mac", &other)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("Refusing to run"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_second_engine_over_the_same_state_db_refuses_to_start() {
        let server = FakeServer::new();
        let fx = Fixture::new("solo");
        let _first = fx.engine(&server, "mac");
        let err = fx
            .try_engine(&server, "mac", &fx.root)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("already running"), "{err}");
    }

    /// A journal-advance frame on the freshness socket wakes the worker into
    /// a cycle (spec §12): the remote-wake path end to end, over a real
    /// WebSocket handshake, without waiting out the safety timer.
    #[cfg(feature = "http")]
    #[test]
    fn a_remote_wake_frame_triggers_a_cycle() {
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // A one-connection wake server: accept, wait for the go signal, send
        // one journal-advance frame, hold the socket open.
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            go_rx.recv().unwrap();
            ws.send(tungstenite::Message::Text("{\"seq\":1}".into()))
                .unwrap();
            let _ = ws.read(); // stay open until the client hangs up
        });

        let server = FakeServer::new();
        let fx = Fixture::new("wake");
        let cycles = Arc::new(AtomicU64::new(0));
        let counted = cycles.clone();
        let engine = SyncEngine::new(
            EngineConfig {
                vault_root: fx.root.clone(),
                state_db: fx.state_db.clone(),
                device_label: "MAC".into(),
                server_url: Some("http://mini:8787".into()),
                vault_id: Some("default".into()),
                allow_bulk_delete: false,
                remote_wake: Some(RemoteWakeConfig {
                    server_url: format!("http://127.0.0.1:{port}"),
                    vault_id: "default".into(),
                    token: "tok".into(),
                }),
            },
            Box::new(server.client("mac")),
            Box::new(move |e| {
                if matches!(e, crate::types::SyncEvent::CycleFinished(_)) {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            }),
        )
        .unwrap();

        engine.start();
        // Wait out the worker's immediate first cycle.
        for _ in 0..200 {
            if cycles.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let before = cycles.load(Ordering::SeqCst);
        assert!(before >= 1, "no initial cycle ran");

        // Fire the wake frame; a fresh cycle should follow well inside the
        // safety interval.
        go_tx.send(()).unwrap();
        let mut woke = false;
        for _ in 0..200 {
            if cycles.load(Ordering::SeqCst) > before {
                woke = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(woke, "the wake frame did not trigger a cycle");

        engine.stop();
        server_thread.join().unwrap();
    }

    #[test]
    fn transport_error_is_recorded_in_status_not_swallowed() {
        let server = FakeServer::new();
        let fx = Fixture::new("mac");
        let engine = fx.engine(&server, "mac");
        server.set_offline(true);
        assert!(engine.sync_now().is_err());
        assert!(engine.status().last_error.is_some());
    }
}
