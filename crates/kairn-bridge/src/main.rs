//! The Kairn sync bridge: headless bridge mode from the sync spec (§3 Phase B,
//! §15.6). It runs the shared [`SyncEngine`] over the HTTP transport against a
//! notes folder and mirrors it to the server on a short interval — pulling
//! remote changes and pushing local ones both ways — with no app running.
//!
//! One always-on machine (the Mac Mini) runs exactly one bridge per vault. That
//! is what puts a folder of notes onto the sync server so the phone can join,
//! and it is the safe path from "Syncthing everywhere" to "Kairn sync too":
//! the engine is baseline-aware and its writes are idempotent, so a change that
//! arrives via Syncthing is pushed once and an identical echo is a no-op by hash
//! (spec §3). **Never run two bridges against the same folder.**
//!
//! Latency is the poll interval (default a few seconds), which is ample for a
//! personal vault of tiny files; a filesystem-watcher / WebSocket upgrade for
//! sub-second freshness is a later optimisation (spec §12).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;

use kairn_sync::HttpTransport;
use kairn_sync::engine::{EngineConfig, SyncEngine};
use kairn_sync::types::{CycleReport, SyncEvent};

/// Mirror a notes folder to a Kairn sync server (headless bridge mode).
#[derive(Parser, Debug)]
#[command(name = "kairn-bridge", version, about)]
struct Args {
    /// The notes folder to sync (the vault root).
    #[arg(long, value_name = "DIR")]
    notes: PathBuf,

    /// The sync server origin (scheme + host + port). Required: a default here
    /// would silently point a vault at whichever host that address happens to be.
    #[arg(long, env = "KAIRN_BRIDGE_SERVER")]
    server: String,

    /// The vault id on the server.
    #[arg(long, env = "KAIRN_BRIDGE_VAULT", default_value = "default")]
    vault: String,

    /// The device bearer token (from the server's `enroll`). Prefer
    /// `--token-file` for a launchd agent so it never shows in the process list.
    #[arg(long, env = "KAIRN_BRIDGE_TOKEN")]
    token: Option<String>,

    /// A file whose contents are the device token (first line, trimmed).
    #[arg(long, value_name = "FILE")]
    token_file: Option<PathBuf>,

    /// The sync-state database, kept OUTSIDE the notes folder so it never syncs
    /// (spec §6). Defaults to `~/.kairn-bridge/<vault>.db`.
    #[arg(long, value_name = "FILE")]
    state_db: Option<PathBuf>,

    /// The label stamped into conflict-copy names (spec §8).
    #[arg(long, default_value = "MINI")]
    device_label: String,

    /// Seconds between sync cycles.
    #[arg(long, default_value_t = 3)]
    interval: u64,

    /// Let one cycle push deletes for more than a handful of files. Off by
    /// default: a notes folder that suddenly reads as mostly-empty is usually a
    /// volume that failed to mount, and those tombstones reach every device.
    /// Pass this only after checking the folder is really as you left it.
    #[arg(long)]
    allow_bulk_delete: bool,

    /// Run a single cycle and exit (useful for a one-off push or a smoke test).
    #[arg(long)]
    once: bool,
}

/// The longest the bridge waits between retries after repeated failures. A
/// revoked token or a server that is down should not mean a request and a log
/// line every few seconds until someone notices.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

fn main() -> Result<()> {
    let args = Args::parse();

    let notes = args
        .notes
        .canonicalize()
        .with_context(|| format!("notes folder not found: {}", args.notes.display()))?;
    if !notes.is_dir() {
        bail!("notes path is not a directory: {}", notes.display());
    }

    let token = resolve_token(&args)?;
    let state_db = resolve_state_db(&args)?;

    let transport = HttpTransport::new(args.server.clone(), args.vault.clone(), token);
    let engine = SyncEngine::new(
        EngineConfig {
            vault_root: notes.clone(),
            state_db,
            device_label: args.device_label.clone(),
            server_url: Some(args.server.clone()),
            vault_id: Some(args.vault.clone()),
            allow_bulk_delete: args.allow_bulk_delete,
        },
        Box::new(transport),
        Box::new(on_event),
    )
    .context("failed to open the sync engine (state store)")?;

    log(&format!(
        "bridge up: {} <-> {} (vault {}), every {}s",
        notes.display(),
        args.server,
        args.vault,
        args.interval
    ));

    if args.once {
        report_cycle(engine.sync_now().context("cycle failed")?);
        return Ok(());
    }

    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
            .context("failed to install signal handler")?;
    }

    let mut failures: u32 = 0;
    while running.load(Ordering::SeqCst) {
        match engine.sync_now() {
            Ok(report) => {
                if failures > 0 {
                    log("recovered; back to the normal interval");
                }
                failures = 0;
                report_cycle(report);
            }
            // A cycle error (server down, token rejected) is logged, not fatal:
            // the next cycle retries, so a transient outage self-heals. What
            // does not self-heal — a revoked token, a server that has moved —
            // backs off instead of hammering the server and the log forever.
            Err(e) => {
                failures = failures.saturating_add(1);
                log(&format!("cycle error (attempt {failures}): {e:#}"));
            }
        }
        // Sleep in small slices so Ctrl-C / SIGTERM is responsive.
        let wait = backoff(Duration::from_secs(args.interval), failures);
        for _ in 0..(wait.as_millis() / 100) {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    log("bridge stopping");
    Ok(())
}

/// How long to wait before the next cycle: the poll interval while things work,
/// doubling per consecutive failure up to [`MAX_BACKOFF`].
fn backoff(interval: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return interval;
    }
    let factor = 1u32 << failures.min(16).saturating_sub(1);
    interval.saturating_mul(factor).min(MAX_BACKOFF)
}

/// Resolve the token from `--token`, then `--token-file`, then the environment.
fn resolve_token(args: &Args) -> Result<String> {
    if let Some(t) = &args.token {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    if let Some(path) = &args.token_file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read token file: {}", path.display()))?;
        let t = raw.lines().next().unwrap_or("").trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
        bail!("token file is empty: {}", path.display());
    }
    bail!("no token: pass --token, --token-file, or set KAIRN_BRIDGE_TOKEN");
}

/// Default the state DB to `~/.kairn-bridge/<vault>.db`, creating the folder.
fn resolve_state_db(args: &Args) -> Result<PathBuf> {
    if let Some(p) = &args.state_db {
        return Ok(p.clone());
    }
    let home = std::env::var_os("HOME").context("HOME is not set; pass --state-db")?;
    let dir = PathBuf::from(home).join(".kairn-bridge");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create state dir: {}", dir.display()))?;
    Ok(dir.join(format!("{}.db", args.vault)))
}

/// Log a cycle only when it actually moved something, so a quiet steady state
/// does not spam the log.
fn report_cycle(r: CycleReport) {
    if r.pulled == 0 && r.pushed == 0 && r.merged == 0 && r.conflicts == 0 && r.deleted_local == 0 {
        return;
    }
    log(&format!(
        "synced: pulled {} pushed {} merged {} removed {} conflicts {} (cursor {})",
        r.pulled, r.pushed, r.merged, r.deleted_local, r.conflicts, r.cursor
    ));
}

/// Surface the events worth seeing on a headless bridge: conflict copies (so a
/// human knows to reconcile) and errors. The about-to-write echo hook is an
/// in-app concern the bridge does not need.
fn on_event(e: SyncEvent) {
    match e {
        SyncEvent::ConflictCopyCreated { original, copy } => {
            log(&format!(
                "conflict on {original}: kept your version as {copy}"
            ));
        }
        SyncEvent::Error(msg) => log(&format!("engine error: {msg}")),
        SyncEvent::CycleFinished(_) | SyncEvent::AboutToWrite(_) => {}
    }
}

fn log(msg: &str) {
    println!(
        "[{}] {msg}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_settles_at_the_ceiling() {
        let base = Duration::from_secs(3);
        assert_eq!(backoff(base, 0), base);
        assert_eq!(backoff(base, 1), base);
        assert_eq!(backoff(base, 2), Duration::from_secs(6));
        assert_eq!(backoff(base, 4), Duration::from_secs(24));
        // A revoked token retries every five minutes, not every three seconds.
        assert_eq!(backoff(base, 20), MAX_BACKOFF);
        assert_eq!(backoff(base, u32::MAX), MAX_BACKOFF);
    }
}
