//! One sync cycle: pull, then push, then ack (spec §7). Always in that order,
//! idempotent, and resumable at any step — the cursor and each file's baseline
//! advance only *after* the matching disk write or server ack has landed
//! (invariant §15.3, §15.4), so a crash between any two steps re-runs cleanly.
//!
//! The cycle is built entirely from [`Transport`] calls and local state, and it
//! never reimplements merging: markdown collisions go through
//! [`crate::resolve`], which delegates to `kairn_core::merge3`.

use std::collections::HashSet;

use anyhow::Result;

use crate::hash::hash_bytes;
use crate::ignore::is_ignored;
use crate::resolve::{Resolution, resolve_binary, resolve_markdown};
use crate::state::StateStore;
use crate::transport::{Transport, TransportError};
use crate::types::{ContentHash, CycleReport, PutOutcome, Rev, SyncConfig, SyncEvent, VaultPath};
use crate::vaultio::{Precondition, VaultIo};

/// A page of journal entries per `changes` request. Files are tiny; a large page
/// keeps a normal day's sync to one round trip.
const PAGE_LIMIT: u32 = 500;

/// How many times one file re-resolves a CAS race in a single cycle before it is
/// left for the next cycle (spec §7: "a file that keeps racing just waits").
const MAX_PUSH_RETRIES: u32 = 8;

/// The floor under the bulk-delete guard, so a small vault can still delete a
/// handful of notes in one go.
const MIN_BULK_DELETE: usize = 10;

/// How many paths one cycle may tombstone before the delete phase is treated as
/// a symptom rather than an intention.
fn bulk_delete_limit(tracked: usize) -> usize {
    MIN_BULK_DELETE.max(tracked / 5)
}

/// Everything one cycle needs. Borrowed, so the engine owns the lifetimes.
pub struct SyncContext<'a> {
    pub transport: &'a dyn Transport,
    pub state: &'a StateStore,
    pub vault: &'a VaultIo,
    /// Emits engine events to the host (spec §14): the about-to-write echo hook
    /// and conflict-copy notifications.
    pub emit: &'a dyn Fn(SyncEvent),
    /// Host policy for this cycle (the bulk-delete valve).
    pub config: &'a SyncConfig,
    /// Checked between transport calls so a `stop()` can cut a cycle short
    /// rather than wait out a two-minute HTTP timeout. Every step boundary is a
    /// safe place to stop: the cycle is resumable by construction (§15.3/4).
    pub cancel: &'a dyn Fn() -> bool,
}

impl SyncContext<'_> {
    /// The echo hook of spec §7, fired just before every vault write/delete.
    fn announce(&self) -> impl Fn(&VaultPath) + '_ {
        move |p: &VaultPath| (self.emit)(SyncEvent::AboutToWrite(p.clone()))
    }

    fn cancelled(&self) -> bool {
        (self.cancel)()
    }
}

/// Run one full cycle and return what it did.
pub fn run_cycle(ctx: &SyncContext) -> Result<CycleReport> {
    let mut report = CycleReport::default();
    pull(ctx, &mut report)?;
    if ctx.cancelled() {
        return Ok(report);
    }
    let accepted = push(ctx, &mut report)?;
    if ctx.cancelled() {
        return Ok(report);
    }
    // Catch the cursor up over our own just-pushed entries: they come back in
    // the journal (spec §5), and the echo guard in `apply_entry` makes them
    // no-ops, so this only advances the cursor rather than re-pulling history.
    // With nothing accepted there is nothing of ours to catch up on, and the
    // steady-state cycle stays at the one round trip spec §7 asks for.
    if accepted {
        pull(ctx, &mut report)?;
    }
    let cursor = ctx.state.cursor()?;
    // The ack is a report, not a handshake: re-sending an unchanged cursor tells
    // the server nothing it does not already know.
    if ctx.state.acked_cursor()? != Some(cursor) {
        ctx.transport.ack(cursor)?;
        ctx.state.set_acked_cursor(cursor)?;
    }
    report.cursor = cursor;
    // Opportunistic GC of superseded baselines; cheap on a small vault.
    let _ = ctx.state.prune_baselines();
    Ok(report)
}

// ---- pull ----------------------------------------------------------------

fn pull(ctx: &SyncContext, report: &mut CycleReport) -> Result<()> {
    loop {
        let cursor = ctx.state.cursor()?;
        let page = ctx.transport.changes(cursor, PAGE_LIMIT)?;
        if page.entries.is_empty() {
            break;
        }
        for entry in &page.entries {
            if ctx.cancelled() {
                return Ok(());
            }
            apply_entry(ctx, entry, report)?;
            // Cursor moves only after the entry is fully applied to disk+state.
            ctx.state.set_cursor(entry.seq)?;
        }
        if !page.has_more {
            break;
        }
    }
    Ok(())
}

/// Fetch a blob and check the bytes against the hash the journal (or a CAS
/// outcome) promised. `None` means "skip this entry": either the rev's blob is
/// no longer retained (spec §11) or the server handed back bytes that are not
/// what it said they were. Both would otherwise wedge the cycle on the same
/// entry forever, so they are reported and stepped over — the whole safety model
/// rests on these hashes, so unverified bytes never reach the vault.
fn fetch_verified(
    ctx: &SyncContext,
    path: &VaultPath,
    rev: Rev,
    expect: &ContentHash,
) -> Result<Option<Vec<u8>>> {
    match ctx.transport.get_blob(path, Some(rev)) {
        Ok(bytes) => {
            if &hash_bytes(&bytes) != expect {
                (ctx.emit)(SyncEvent::Error(format!(
                    "skipped {path} rev {rev}: the server's bytes do not match the hash it \
                     published for them"
                )));
                return Ok(None);
            }
            Ok(Some(bytes))
        }
        Err(TransportError::NotFound) => {
            (ctx.emit)(SyncEvent::Error(format!(
                "skipped {path} rev {rev}: the server no longer keeps that revision's content \
                 (pruned). Any later change to this file still syncs normally."
            )));
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// A write resolved against a pre-image that is no longer what is on disk:
/// something outside the engine (the app, Syncthing on a bridged folder) wrote
/// the file while we were on the network. Leave it alone — the push phase later
/// in this same cycle sees a dirty file and resolves it through the merge rules
/// instead of clobbering it (invariant §15.1).
fn changed_underneath(ctx: &SyncContext, path: &VaultPath) {
    (ctx.emit)(SyncEvent::Error(format!(
        "{path} changed on disk mid-cycle; left as it is and resolved from its new content"
    )));
}

fn apply_entry(
    ctx: &SyncContext,
    entry: &crate::types::JournalEntry,
    report: &mut CycleReport,
) -> Result<()> {
    let path = &entry.path;
    // Server-supplied paths are untrusted: refuse anything that would resolve
    // outside the vault before it ever reaches the filesystem (spec trust model).
    if !path.is_safe() {
        (ctx.emit)(SyncEvent::Error(format!(
            "refused unsafe sync path: {path}"
        )));
        return Ok(());
    }
    if is_ignored(path) {
        return Ok(());
    }
    let local = ctx.vault.read_opt(path)?;
    let st = ctx.state.file_state(path)?;
    let baseline_hash = st.as_ref().and_then(|s| s.baseline_hash.clone());
    let announce = ctx.announce();

    // Echo/replay guard: we already hold exactly this rev and content (our own
    // just-pushed change coming back, an identical write via the bridge, or a
    // replayed page). Do nothing but let the cursor advance (spec §5, §7) — this
    // is what stops a device clobbering its own newer local edit with its own
    // older journal entry, and what makes bridge echoes converge.
    if let Some(s) = &st
        && s.rev == entry.rev
        && s.baseline_hash == entry.hash
    {
        return Ok(());
    }

    // Older than the rev we already hold for this path: a push-side merge in an
    // earlier phase already recorded a later rev, so this entry's content is
    // superseded and writing it would put yesterday's bytes back for one cycle.
    if let Some(s) = &st
        && entry.rev < s.rev
    {
        return Ok(());
    }

    if entry.deleted {
        // A tombstone for something we neither hold nor track: nothing to do.
        if local.is_none() && st.is_none() {
            return Ok(());
        }
        // Delete locally only when the local copy is clean; a dirty local file
        // keeps its edits and the push phase resurrects it via CAS (delete/edit
        // conflict, edit wins — spec §9).
        if is_local_clean(local.as_deref(), baseline_hash.as_ref()) {
            let pre = Precondition::from_local(local.as_deref());
            if !ctx.vault.delete(path, &pre, &announce)? {
                changed_underneath(ctx, path);
                return Ok(());
            }
            ctx.state.remove_file_state(path)?;
            report.deleted_local += 1;
        }
        return Ok(());
    }

    let remote_hash = match &entry.hash {
        Some(h) => h.clone(),
        None => return Ok(()), // malformed non-tombstone; nothing to apply
    };

    // Already hold these exact bytes (our own echo, an identical write via the
    // bridge, or a replayed page): record the rev, write nothing (spec §5, §7).
    if let Some(bytes) = &local
        && hash_bytes(bytes) == remote_hash
    {
        ctx.state
            .record_synced(path, entry.rev, bytes, path.is_markdown())?;
        return Ok(());
    }

    let Some(remote) = fetch_verified(ctx, path, entry.rev, &remote_hash)? else {
        return Ok(());
    };

    // What the file must still hold when the rename lands. The read above and
    // the write below straddle a network round trip; without this the write
    // would clobber anything that arrived in between (invariant §15.1).
    let pre = Precondition::from_local(local.as_deref());

    if is_local_clean(local.as_deref(), baseline_hash.as_ref()) {
        // Clean pull: adopt the remote content wholesale.
        if !ctx.vault.write(path, &remote, &pre, &announce)? {
            changed_underneath(ctx, path);
            return Ok(());
        }
        ctx.state
            .record_synced(path, entry.rev, &remote, path.is_markdown())?;
        report.pulled += 1;
        return Ok(());
    }

    // Conflict: local is dirty and remote differs. Resolve, keeping the remote
    // head as the new baseline so a merge that differs from remote is pushed.
    let local_bytes = local.expect("dirty implies a local file exists");
    let baseline_bytes = baseline_hash.and_then(|h| ctx.state.baseline(&h).ok().flatten());
    let resolution = resolve_change(path, baseline_bytes.as_deref(), &remote, &local_bytes);
    match resolution {
        Resolution::Write { content, .. } => {
            if !ctx.vault.write(path, &content, &pre, &announce)? {
                changed_underneath(ctx, path);
                return Ok(());
            }
            ctx.state
                .record_synced(path, entry.rev, &remote, path.is_markdown())?;
            report.merged += 1;
        }
        Resolution::WriteWithConflict {
            content,
            conflict_copy,
        } => {
            if !ctx.vault.write(path, &content, &pre, &announce)? {
                changed_underneath(ctx, path);
                return Ok(());
            }
            let copy = ctx
                .vault
                .write_conflict_copy(path, &conflict_copy, &announce)?;
            (ctx.emit)(SyncEvent::ConflictCopyCreated {
                original: path.clone(),
                copy,
            });
            ctx.state
                .record_synced(path, entry.rev, &remote, path.is_markdown())?;
            report.conflicts += 1;
        }
    }
    Ok(())
}

// ---- push ----------------------------------------------------------------

/// Returns whether the server accepted anything: our own accepted writes are the
/// only entries that can have joined the journal since this cycle's first pull.
fn push(ctx: &SyncContext, report: &mut CycleReport) -> Result<bool> {
    // A cached scan, so a quiet vault is a stat per file rather than a full
    // re-read (spec §7). A missing root is an error here, never an empty vault.
    let scanned = ctx.vault.scan_cached(ctx.state)?;
    let on_disk: HashSet<VaultPath> = scanned.iter().map(|f| f.path.clone()).collect();
    let _ = ctx.state.prune_scan_cache(&on_disk);
    let mut accepted = false;

    // New and locally-edited files.
    for f in &scanned {
        if ctx.cancelled() {
            return Ok(accepted);
        }
        let (base_rev, dirty) = match ctx.state.file_state(&f.path)? {
            None => (0, true),
            Some(s) => (s.rev, s.baseline_hash.as_ref() != Some(&f.hash)),
        };
        if dirty {
            accepted |= push_one(ctx, &f.path, base_rev, report)?;
        }
    }

    // Files we have synced but that are gone from disk: local deletes.
    let tracked = ctx.state.all_states()?;
    let missing: Vec<_> = tracked
        .iter()
        .filter(|(path, _)| !on_disk.contains(path) && !is_ignored(path))
        .collect();
    let limit = bulk_delete_limit(tracked.len());
    if missing.len() > limit && !ctx.config.allow_bulk_delete {
        // A vault that has mostly vanished is a symptom, and tombstones are the
        // one thing a cycle does that reaches every other device. Stop here and
        // leave the whole delete phase for a human (invariant §15.2).
        (ctx.emit)(SyncEvent::Error(format!(
            "refusing to sync {} deletions in one cycle (the safety limit is {limit} of {} \
             tracked files). Nothing was deleted, here or anywhere else. This is usually the \
             vault folder not being where the engine is looking (an unmounted volume, the wrong \
             path, a half-restored folder) rather than {} notes really being deleted. Check the \
             folder first; if the deletions are real, re-run with bulk deletes allowed.",
            missing.len(),
            tracked.len(),
            missing.len(),
        )));
        return Ok(accepted);
    }
    for (path, st) in missing {
        if ctx.cancelled() {
            break;
        }
        accepted |= push_delete(ctx, path, st.rev, report)?;
    }
    Ok(accepted)
}

fn push_one(
    ctx: &SyncContext,
    path: &VaultPath,
    mut base_rev: Rev,
    report: &mut CycleReport,
) -> Result<bool> {
    let announce = ctx.announce();
    let is_md = path.is_markdown();
    let mut content = match ctx.vault.read_opt(path)? {
        Some(c) => c,
        None => return Ok(false), // vanished since the scan; next cycle handles it
    };

    for _ in 0..MAX_PUSH_RETRIES {
        if ctx.cancelled() {
            return Ok(false);
        }
        let hash = hash_bytes(&content);
        match ctx.transport.put_blob(path, base_rev, &hash, &content)? {
            PutOutcome::Accepted { rev, .. } => {
                ctx.state.record_synced(path, rev, &content, is_md)?;
                report.pushed += 1;
                return Ok(true);
            }
            PutOutcome::Conflict {
                head_rev,
                head_hash,
            } => match head_hash {
                // Remote tombstoned the file we edited: edit wins, resurrect by
                // re-uploading the same content based on the tombstone rev (§9).
                None => base_rev = head_rev,
                // Remote changed: merge our content against the new head, write
                // the merge locally, then re-upload it based on the new head.
                Some(head_hash) => {
                    let Some(remote) = fetch_verified(ctx, path, head_rev, &head_hash)? else {
                        return Ok(false);
                    };
                    let baseline = ctx
                        .state
                        .file_state(path)?
                        .and_then(|s| s.baseline_hash)
                        .and_then(|h| ctx.state.baseline(&h).ok().flatten());
                    let resolution = resolve_change(path, baseline.as_deref(), &remote, &content);
                    // `content` is what we last read or wrote at this path, so it
                    // is the pre-image the merge above resolved against.
                    let pre = Precondition::Unchanged(hash);
                    let Some(merged) = apply_push_resolution(
                        ctx, path, resolution, &remote, head_rev, &pre, &announce, report,
                    )?
                    else {
                        changed_underneath(ctx, path);
                        return Ok(false);
                    };
                    content = merged;
                    // The all-conflicting case merges to the remote head itself:
                    // re-uploading it would mint a no-op rev every device then
                    // pulls, so converge silently instead (spec §5). Any conflict
                    // copy already written still syncs on its own next cycle.
                    if hash_bytes(&content) == head_hash {
                        return Ok(false);
                    }
                    base_rev = head_rev;
                }
            },
        }
    }
    Ok(false)
}

/// Land a push-side merge: write the merged content and any conflict copy, and
/// record the remote head as the baseline at `head_rev` (so a crash before the
/// re-upload still leaves the file dirty-vs-baseline and pushable next cycle).
/// Returns the content to re-upload, or `None` when the file changed underneath
/// the merge and the whole resolution must be redone from its new content.
#[allow(clippy::too_many_arguments)]
fn apply_push_resolution(
    ctx: &SyncContext,
    path: &VaultPath,
    resolution: Resolution,
    remote: &[u8],
    head_rev: Rev,
    pre: &Precondition,
    announce: &(dyn Fn(&VaultPath) + '_),
    report: &mut CycleReport,
) -> Result<Option<Vec<u8>>> {
    let is_md = path.is_markdown();
    let content = match resolution {
        Resolution::Write { content, .. } => {
            if !ctx.vault.write(path, &content, pre, announce)? {
                return Ok(None);
            }
            content
        }
        Resolution::WriteWithConflict {
            content,
            conflict_copy,
        } => {
            if !ctx.vault.write(path, &content, pre, announce)? {
                return Ok(None);
            }
            let copy = ctx
                .vault
                .write_conflict_copy(path, &conflict_copy, announce)?;
            (ctx.emit)(SyncEvent::ConflictCopyCreated {
                original: path.clone(),
                copy,
            });
            report.conflicts += 1;
            content
        }
    };
    ctx.state.record_synced(path, head_rev, remote, is_md)?;
    Ok(Some(content))
}

fn push_delete(
    ctx: &SyncContext,
    path: &VaultPath,
    base_rev: Rev,
    report: &mut CycleReport,
) -> Result<bool> {
    let announce = ctx.announce();
    match ctx.transport.delete(path, base_rev)? {
        PutOutcome::Accepted { .. } => {
            ctx.state.remove_file_state(path)?;
            Ok(true)
        }
        PutOutcome::Conflict {
            head_rev,
            head_hash,
        } => match head_hash {
            // Remote edited the file we deleted: the delete is dropped and the
            // file comes back (spec §9, notes are precious).
            Some(head_hash) => {
                let Some(remote) = fetch_verified(ctx, path, head_rev, &head_hash)? else {
                    return Ok(false);
                };
                // The file was missing when we scanned; if it is back, whatever
                // put it there is newer than anything we know.
                if !ctx
                    .vault
                    .write(path, &remote, &Precondition::Absent, &announce)?
                {
                    changed_underneath(ctx, path);
                    return Ok(false);
                }
                ctx.state
                    .record_synced(path, head_rev, &remote, path.is_markdown())?;
                report.pulled += 1;
                Ok(false)
            }
            // Already deleted remotely at a higher rev: converge, forget it.
            None => {
                ctx.state.remove_file_state(path)?;
                Ok(false)
            }
        },
    }
}

// ---- helpers -------------------------------------------------------------

/// A local file is clean when it is absent, or its bytes still hash to the
/// recorded baseline. A present file with no baseline is treated as dirty so it
/// is never silently overwritten (spec §6 degrade-safely).
fn is_local_clean(local: Option<&[u8]>, baseline_hash: Option<&crate::types::ContentHash>) -> bool {
    match (local, baseline_hash) {
        (None, _) => true,
        (Some(bytes), Some(bh)) => &hash_bytes(bytes) == bh,
        (Some(_), None) => false,
    }
}

/// Pick markdown three-way merge or binary last-writer-wins, falling back to
/// binary when a "markdown" file isn't valid UTF-8.
fn resolve_change(
    path: &VaultPath,
    baseline: Option<&[u8]>,
    remote: &[u8],
    local: &[u8],
) -> Resolution {
    if path.is_markdown()
        && let (Ok(r), Ok(l)) = (std::str::from_utf8(remote), std::str::from_utf8(local))
    {
        let b = baseline.and_then(|b| std::str::from_utf8(b).ok());
        return resolve_markdown(b, r, l);
    }
    resolve_binary(remote, local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakeserver::{FakeClient, FakeServer};
    use crate::transport::{TransportError, TransportResult};
    use crate::types::ChangesPage;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TAG: AtomicU64 = AtomicU64::new(0);

    /// Shared control over a [`Faulty`] transport, so a test can make it fail a
    /// chosen `get_blob` call (a mid-cycle "crash") and later heal it.
    #[derive(Clone)]
    struct FaultCtl {
        calls: Arc<AtomicU64>,
        fail_at: Arc<AtomicU64>,
    }

    impl FaultCtl {
        fn heal(&self) {
            self.fail_at.store(u64::MAX, Ordering::SeqCst);
        }
    }

    /// A transport that fails one chosen `get_blob` call, standing in for a
    /// process crash / partition partway through a cycle (spec §16).
    struct Faulty {
        inner: FakeClient,
        ctl: FaultCtl,
    }

    impl Transport for Faulty {
        fn changes(&self, since: crate::types::Seq, limit: u32) -> TransportResult<ChangesPage> {
            self.inner.changes(since, limit)
        }
        fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
            let n = self.ctl.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n == self.ctl.fail_at.load(Ordering::SeqCst) {
                return Err(TransportError::Network("injected crash".into()));
            }
            self.inner.get_blob(path, rev)
        }
        fn put_blob(
            &self,
            path: &VaultPath,
            base_rev: Rev,
            hash: &ContentHash,
            content: &[u8],
        ) -> TransportResult<PutOutcome> {
            self.inner.put_blob(path, base_rev, hash, content)
        }
        fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
            self.inner.delete(path, base_rev)
        }
        fn ack(&self, cursor: crate::types::Seq) -> TransportResult<()> {
            self.inner.ack(cursor)
        }
    }

    /// A transport that injects one competing write on its first `put_blob`,
    /// so the caller's push loses the CAS to a new head — the concurrent-write
    /// race that exercises the push-side merge path (spec §5, §8).
    struct RaceOnce {
        inner: FakeClient,
        competitor: FakeClient,
        fired: std::sync::atomic::AtomicBool,
        path: VaultPath,
        head_rev: Rev,
        content: Vec<u8>,
    }

    impl Transport for RaceOnce {
        fn changes(&self, since: crate::types::Seq, limit: u32) -> TransportResult<ChangesPage> {
            self.inner.changes(since, limit)
        }
        fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
            self.inner.get_blob(path, rev)
        }
        fn put_blob(
            &self,
            path: &VaultPath,
            base_rev: Rev,
            hash: &ContentHash,
            content: &[u8],
        ) -> TransportResult<PutOutcome> {
            if !self.fired.swap(true, Ordering::SeqCst) {
                let _ = self.competitor.put_blob(
                    &self.path,
                    self.head_rev,
                    &crate::hash::hash_bytes(&self.content),
                    &self.content,
                );
            }
            self.inner.put_blob(path, base_rev, hash, content)
        }
        fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
            self.inner.delete(path, base_rev)
        }
        fn ack(&self, cursor: crate::types::Seq) -> TransportResult<()> {
            self.inner.ack(cursor)
        }
    }

    /// A transport that counts every call, for the "steady state is one round
    /// trip" check (spec §7).
    struct Counting {
        inner: FakeClient,
        calls: Arc<AtomicU64>,
    }

    impl Counting {
        fn hit(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Transport for Counting {
        fn changes(&self, since: crate::types::Seq, limit: u32) -> TransportResult<ChangesPage> {
            self.hit();
            self.inner.changes(since, limit)
        }
        fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
            self.hit();
            self.inner.get_blob(path, rev)
        }
        fn put_blob(
            &self,
            path: &VaultPath,
            base_rev: Rev,
            hash: &ContentHash,
            content: &[u8],
        ) -> TransportResult<PutOutcome> {
            self.hit();
            self.inner.put_blob(path, base_rev, hash, content)
        }
        fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
            self.hit();
            self.inner.delete(path, base_rev)
        }
        fn ack(&self, cursor: crate::types::Seq) -> TransportResult<()> {
            self.hit();
            self.inner.ack(cursor)
        }
    }

    /// A transport whose `get_blob` for one path/rev answers `NotFound`, as a
    /// server that has pruned a historical rev does (spec §11).
    struct PrunedRev {
        inner: FakeClient,
        path: VaultPath,
        rev: Rev,
    }

    impl Transport for PrunedRev {
        fn changes(&self, since: crate::types::Seq, limit: u32) -> TransportResult<ChangesPage> {
            self.inner.changes(since, limit)
        }
        fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
            if path == &self.path && rev == Some(self.rev) {
                return Err(TransportError::NotFound);
            }
            self.inner.get_blob(path, rev)
        }
        fn put_blob(
            &self,
            path: &VaultPath,
            base_rev: Rev,
            hash: &ContentHash,
            content: &[u8],
        ) -> TransportResult<PutOutcome> {
            self.inner.put_blob(path, base_rev, hash, content)
        }
        fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
            self.inner.delete(path, base_rev)
        }
        fn ack(&self, cursor: crate::types::Seq) -> TransportResult<()> {
            self.inner.ack(cursor)
        }
    }

    /// A transport that drops content into the vault during a `get_blob`, which
    /// is where the engine is on the network and another writer (the app,
    /// Syncthing) can land a change it must not clobber.
    struct WriteDuringFetch {
        inner: FakeClient,
        path: PathBuf,
        content: Vec<u8>,
        fired: std::sync::atomic::AtomicBool,
    }

    impl Transport for WriteDuringFetch {
        fn changes(&self, since: crate::types::Seq, limit: u32) -> TransportResult<ChangesPage> {
            self.inner.changes(since, limit)
        }
        fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
            let out = self.inner.get_blob(path, rev);
            if !self.fired.swap(true, Ordering::SeqCst) {
                std::fs::create_dir_all(self.path.parent().unwrap()).unwrap();
                std::fs::write(&self.path, &self.content).unwrap();
            }
            out
        }
        fn put_blob(
            &self,
            path: &VaultPath,
            base_rev: Rev,
            hash: &ContentHash,
            content: &[u8],
        ) -> TransportResult<PutOutcome> {
            self.inner.put_blob(path, base_rev, hash, content)
        }
        fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
            self.inner.delete(path, base_rev)
        }
        fn ack(&self, cursor: crate::types::Seq) -> TransportResult<()> {
            self.inner.ack(cursor)
        }
    }

    /// One device: its own vault dir, state DB, transport to the shared server,
    /// and a captured event log.
    struct Device {
        root: PathBuf,
        state: StateStore,
        vault: VaultIo,
        client: Box<dyn Transport>,
        events: RefCell<Vec<SyncEvent>>,
        config: RefCell<SyncConfig>,
        cancelled: Cell<bool>,
        /// Fault injection: make the state store reject writes the moment the
        /// cycle announces a vault write, so the file lands on disk and the
        /// state update that records it does not (spec §16, invariant §15.4).
        fail_state_after_write: Cell<bool>,
    }

    impl Device {
        fn new(server: &FakeServer, label: &str) -> Self {
            Self::with_transport(server, label, |c| Box::new(c))
        }

        /// A device whose transport fails the `fail_at`-th `get_blob` call.
        fn new_faulty(server: &FakeServer, label: &str, fail_at: u64) -> (Self, FaultCtl) {
            let ctl = FaultCtl {
                calls: Arc::new(AtomicU64::new(0)),
                fail_at: Arc::new(AtomicU64::new(fail_at)),
            };
            let ctl2 = ctl.clone();
            let dev = Self::with_transport(server, label, move |c| {
                Box::new(Faulty {
                    inner: c,
                    ctl: ctl2,
                })
            });
            (dev, ctl)
        }

        fn with_transport(
            server: &FakeServer,
            label: &str,
            wrap: impl FnOnce(FakeClient) -> Box<dyn Transport>,
        ) -> Self {
            let root = std::env::temp_dir().join(format!(
                "kairn-sync-cyc-{label}-{}-{}",
                std::process::id(),
                TAG.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self {
                vault: VaultIo::new(&root, label.to_uppercase()),
                state: StateStore::open_in_memory().unwrap(),
                client: wrap(server.client(label)),
                events: RefCell::new(Vec::new()),
                config: RefCell::new(SyncConfig::default()),
                cancelled: Cell::new(false),
                fail_state_after_write: Cell::new(false),
                root,
            }
        }

        fn try_sync(&self) -> Result<CycleReport> {
            let emit = |e: SyncEvent| {
                if self.fail_state_after_write.get() && matches!(e, SyncEvent::AboutToWrite(_)) {
                    self.state.set_read_only(true).unwrap();
                }
                self.events.borrow_mut().push(e);
            };
            let config = self.config.borrow().clone();
            let cancel = || self.cancelled.get();
            let ctx = SyncContext {
                transport: self.client.as_ref(),
                state: &self.state,
                vault: &self.vault,
                emit: &emit,
                config: &config,
                cancel: &cancel,
            };
            run_cycle(&ctx)
        }

        /// The error messages this device surfaced to the host.
        fn errors(&self) -> Vec<String> {
            self.events
                .borrow()
                .iter()
                .filter_map(|e| match e {
                    SyncEvent::Error(m) => Some(m.clone()),
                    _ => None,
                })
                .collect()
        }

        fn sync(&self) -> CycleReport {
            self.try_sync().unwrap()
        }

        fn write(&self, rel: &str, content: &str) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }

        fn write_bytes(&self, rel: &str, content: &[u8]) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }

        fn read(&self, rel: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(rel)).ok()
        }

        fn exists(&self, rel: &str) -> bool {
            self.root.join(rel).exists()
        }

        fn delete(&self, rel: &str) {
            std::fs::remove_file(self.root.join(rel)).unwrap();
        }

        /// The conflict copies sitting next to a note (kairn-core's detector).
        fn conflict_copies(&self, rel: &str) -> Vec<String> {
            kairn_core::conflict_copies(&self.root.join(rel))
                .into_iter()
                .map(|p| std::fs::read_to_string(p).unwrap())
                .collect()
        }

        /// How many conflict copies sit next to a file (works for binary too).
        fn conflict_copy_count(&self, rel: &str) -> usize {
            kairn_core::conflict_copies(&self.root.join(rel)).len()
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn new_file_pushes_then_the_other_device_pulls_it() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "hello\n");
        let r = mac.sync();
        assert_eq!(r.pushed, 1);

        let r = phone.sync();
        assert_eq!(r.pulled, 1);
        assert_eq!(phone.read("Notes/a.md").as_deref(), Some("hello\n"));
    }

    #[test]
    fn empty_cycle_is_a_no_op_and_idempotent() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        mac.write("Notes/a.md", "x\n");
        mac.sync();
        let baseline_journal = s.journal_len();
        // Re-syncing with nothing changed adds no revs and pushes nothing.
        let r = mac.sync();
        assert_eq!((r.pushed, r.pulled, r.merged, r.conflicts), (0, 0, 0, 0));
        assert_eq!(s.journal_len(), baseline_journal);
    }

    #[test]
    fn disjoint_edits_merge_cleanly_with_no_conflict_copy() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Calendar/20260808.md", "top\nmiddle\nbottom\n");
        mac.sync();
        phone.sync(); // phone now has the file

        // Both edit different regions before either syncs again.
        mac.write(
            "Calendar/20260808.md",
            "top\nmiddle\nbottom\n* agent task\n",
        );
        mac.sync(); // mac's version is now the server head

        phone.write("Calendar/20260808.md", "top edited\nmiddle\nbottom\n");
        let r = phone.sync(); // pull mac's change, merge with phone's edit, push

        assert_eq!(r.merged, 1);
        assert_eq!(r.conflicts, 0);
        assert_eq!(
            phone.read("Calendar/20260808.md").as_deref(),
            Some("top edited\nmiddle\nbottom\n* agent task\n")
        );

        // Mac pulls the merged result; everything converges, no artifacts.
        mac.sync();
        assert_eq!(
            mac.read("Calendar/20260808.md").as_deref(),
            Some("top edited\nmiddle\nbottom\n* agent task\n")
        );
        assert!(mac.conflict_copies("Calendar/20260808.md").is_empty());
    }

    #[test]
    fn same_line_collision_keeps_remote_and_writes_a_conflict_copy() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "line\n");
        mac.sync();
        phone.sync();

        mac.write("Notes/a.md", "line from mac\n");
        mac.sync(); // server head = mac's line

        phone.write("Notes/a.md", "line from phone\n");
        let r = phone.sync();

        assert_eq!(r.conflicts, 1);
        // Remote (mac) wins the file; phone's text is preserved as a copy.
        assert_eq!(phone.read("Notes/a.md").as_deref(), Some("line from mac\n"));
        assert_eq!(
            phone.conflict_copies("Notes/a.md"),
            vec!["line from phone\n".to_string()]
        );
        // A ConflictCopyCreated event was emitted.
        assert!(
            phone
                .events
                .borrow()
                .iter()
                .any(|e| matches!(e, SyncEvent::ConflictCopyCreated { .. }))
        );
    }

    #[test]
    fn non_markdown_conflict_is_last_writer_wins_with_a_copy() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write_bytes("Notes/pic.png", b"\x89original");
        mac.sync();
        phone.sync();

        mac.write_bytes("Notes/pic.png", b"\x89mac-version");
        mac.sync();
        phone.write_bytes("Notes/pic.png", b"\x89phone-version");
        let r = phone.sync();

        assert_eq!(r.conflicts, 1);
        assert_eq!(
            std::fs::read(phone.root.join("Notes/pic.png")).unwrap(),
            b"\x89mac-version"
        );
        assert_eq!(phone.conflict_copy_count("Notes/pic.png"), 1);
    }

    #[test]
    fn clean_delete_propagates_to_the_other_device() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "bye\n");
        mac.sync();
        phone.sync();
        assert!(phone.exists("Notes/a.md"));

        mac.delete("Notes/a.md");
        mac.sync(); // pushes a tombstone

        let r = phone.sync();
        assert_eq!(r.deleted_local, 1);
        assert!(!phone.exists("Notes/a.md"));
    }

    #[test]
    fn delete_meeting_a_remote_edit_resurrects_the_file() {
        // §9: a local delete that races a remote edit is dropped; the file
        // comes back with the remote content.
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "v1\n");
        mac.sync();
        phone.sync();

        // Mac edits; phone deletes; phone syncs before seeing mac's edit.
        mac.write("Notes/a.md", "v2 from mac\n");
        mac.sync(); // server head = v2
        phone.delete("Notes/a.md");
        let r = phone.sync();

        assert!(phone.exists("Notes/a.md"), "the delete should be dropped");
        assert_eq!(phone.read("Notes/a.md").as_deref(), Some("v2 from mac\n"));
        assert_eq!(r.pulled, 1);
    }

    #[test]
    fn edit_meeting_a_remote_tombstone_resurrects_via_push() {
        // §9: a dirty local file that receives a remote tombstone is re-uploaded
        // as a new rev; the edit wins.
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "v1\n");
        mac.sync();
        phone.sync();

        // Mac deletes; phone edits; phone syncs and meets the tombstone.
        mac.delete("Notes/a.md");
        mac.sync(); // server head = tombstone
        phone.write("Notes/a.md", "v1 plus phone edit\n");
        phone.sync();

        // Phone keeps its edit and resurrects it on the server.
        assert_eq!(
            phone.read("Notes/a.md").as_deref(),
            Some("v1 plus phone edit\n")
        );
        // Mac pulls the resurrection back.
        mac.sync();
        assert_eq!(
            mac.read("Notes/a.md").as_deref(),
            Some("v1 plus phone edit\n")
        );
    }

    #[test]
    fn missing_baseline_conflicts_rather_than_clobbering() {
        // Both devices already hold a file (pre-existing, never synced): first
        // sync must not silently overwrite the second device's version.
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");

        mac.write("Notes/a.md", "mac pre-existing\n");
        phone.write("Notes/a.md", "phone pre-existing\n");
        mac.sync(); // mac wins the server head first

        let r = phone.sync(); // phone has a local file, no baseline -> conflict
        assert_eq!(r.conflicts, 1);
        assert_eq!(
            phone.read("Notes/a.md").as_deref(),
            Some("mac pre-existing\n")
        );
        assert_eq!(
            phone.conflict_copies("Notes/a.md"),
            vec!["phone pre-existing\n".to_string()]
        );
    }

    // ---- invariants (§15) and resilience (§16) ---------------------------

    #[test]
    fn resync_after_convergence_is_a_no_op() {
        // Idempotence (invariant §15.3): once converged, another cycle changes
        // nothing on disk and adds nothing to the journal.
        let s = FakeServer::new();
        let a = Device::new(&s, "a");
        let b = Device::new(&s, "b");
        a.write("Notes/x.md", "content\n");
        a.sync();
        b.sync();

        let journal = s.journal_len();
        let r = b.sync();
        assert_eq!(
            (r.pushed, r.pulled, r.merged, r.conflicts, r.deleted_local),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(s.journal_len(), journal);
        assert_eq!(b.read("Notes/x.md").as_deref(), Some("content\n"));
    }

    #[test]
    fn repeated_syncs_do_not_grow_the_journal() {
        // Echo convergence (§16): two engines syncing the same content back and
        // forth must not create endless no-op revs. This is the property that
        // keeps bridge mode from looping.
        let s = FakeServer::new();
        let a = Device::new(&s, "a");
        let b = Device::new(&s, "b");
        a.write("Notes/x.md", "content\n");
        a.sync();
        b.sync();
        let settled = s.journal_len();
        for _ in 0..5 {
            a.sync();
            b.sync();
        }
        assert_eq!(s.journal_len(), settled, "echoes must not create new revs");
    }

    #[test]
    fn crash_mid_pull_resumes_and_loses_nothing() {
        // Crash-safety (§15.4, §16): a cycle that dies partway leaves the
        // already-applied file on disk and its cursor advanced; re-running
        // finishes the rest. Nothing is lost, nothing is applied twice.
        let s = FakeServer::new();
        let src = Device::new(&s, "src");
        src.write("Notes/a.md", "AAA\n");
        src.sync();
        src.write("Notes/b.md", "BBB\n");
        src.sync();

        // dest crashes on the second blob download (b.md), after a.md landed.
        let (dest, ctl) = Device::new_faulty(&s, "dest", 2);
        assert!(dest.try_sync().is_err());
        assert_eq!(dest.read("Notes/a.md").as_deref(), Some("AAA\n"));
        assert!(!dest.exists("Notes/b.md"));

        // Recover: the next cycle resumes from the cursor and converges.
        ctl.heal();
        dest.sync();
        assert_eq!(dest.read("Notes/a.md").as_deref(), Some("AAA\n"));
        assert_eq!(dest.read("Notes/b.md").as_deref(), Some("BBB\n"));
    }

    #[test]
    fn concurrent_same_line_edits_preserve_both_sides_everywhere() {
        // Never lose typed text (§15.2): after a same-line collision, the loser's
        // text survives as a conflict copy that itself syncs to every device.
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        let phone = Device::new(&s, "phone");
        mac.write("Notes/a.md", "shared\n");
        mac.sync();
        phone.sync();

        mac.write("Notes/a.md", "mac wins the line\n");
        mac.sync();
        phone.write("Notes/a.md", "phone loses the line\n");
        phone.sync(); // conflict copy created locally, then pushed

        // The conflict copy propagates back to mac, so no device loses the text.
        mac.sync();
        let copies = mac.conflict_copies("Notes/a.md");
        assert_eq!(copies, vec!["phone loses the line\n".to_string()]);
        assert_eq!(
            mac.read("Notes/a.md").as_deref(),
            Some("mac wins the line\n")
        );
    }

    #[test]
    fn three_devices_converge_on_the_same_content() {
        // A small end-to-end soak: writes from three devices all land, and a
        // final round of syncs leaves every device byte-identical.
        let s = FakeServer::new();
        let a = Device::new(&s, "a");
        let b = Device::new(&s, "b");
        let c = Device::new(&s, "c");

        a.write("Calendar/20260808.md", "a-daily\n");
        b.write("Notes/idea.md", "b-idea\n");
        c.write("Notes/todo.md", "c-todo\n");
        for d in [&a, &b, &c] {
            d.sync();
        }
        // Two full rounds so every write reaches every device.
        for _ in 0..2 {
            for d in [&a, &b, &c] {
                d.sync();
            }
        }

        for d in [&a, &b, &c] {
            assert_eq!(d.read("Calendar/20260808.md").as_deref(), Some("a-daily\n"));
            assert_eq!(d.read("Notes/idea.md").as_deref(), Some("b-idea\n"));
            assert_eq!(d.read("Notes/todo.md").as_deref(), Some("c-todo\n"));
        }
    }

    #[test]
    fn push_side_merge_to_head_does_not_mint_a_no_op_rev() {
        // The push-side CAS path: another device writes between our pull and our
        // push, and our merge resolves to that new head. Re-uploading identical
        // bytes would mint a rev every device then pulls, so it must be skipped
        // (spec §5). We force the race with a transport that injects a competing
        // write on the first PUT.
        let s = FakeServer::new();
        let phone = Device::with_transport(&s, "phone", |inner| {
            Box::new(RaceOnce {
                inner,
                competitor: s.client("racer"),
                fired: std::sync::atomic::AtomicBool::new(false),
                path: VaultPath::new("Notes/a.md"),
                head_rev: 1,
                content: b"racer line\n".to_vec(),
            })
        });
        // Seed a shared base and give phone its baseline.
        s.client("seed")
            .put_blob(
                &VaultPath::new("Notes/a.md"),
                0,
                &hash_bytes(b"base\n"),
                b"base\n",
            )
            .unwrap();
        phone.sync(); // phone now holds base at rev 1
        let before = s.journal_len();

        // Phone edits, then pushes; the race makes "racer line" the head, and the
        // same-line merge resolves back to it -> no re-upload.
        phone.write("Notes/a.md", "phone line\n");
        phone.sync();

        // Exactly one new rev exists: the racer's. Phone minted none.
        assert_eq!(
            s.journal_len(),
            before + 1,
            "phone must not add a no-op rev"
        );
        assert_eq!(phone.read("Notes/a.md").as_deref(), Some("racer line\n"));
        assert_eq!(
            phone.conflict_copies("Notes/a.md"),
            vec!["phone line\n".to_string()]
        );
    }

    #[test]
    fn unsafe_server_path_is_refused_and_writes_nothing() {
        // A hostile or buggy server entry with a traversal path must never reach
        // the filesystem (path-traversal guard).
        let s = FakeServer::new();
        let dev = Device::new(&s, "dev");
        // Inject a malicious journal entry directly.
        let attacker = s.client("attacker");
        attacker
            .put_blob(
                &VaultPath::new("../escape.md"),
                0,
                &hash_bytes(b"pwned"),
                b"pwned",
            )
            .unwrap();

        dev.sync();

        // The vault is untouched and an error was surfaced, not swallowed.
        assert!(std::fs::read_dir(&dev.root).unwrap().next().is_none());
        assert!(
            dev.events
                .borrow()
                .iter()
                .any(|e| matches!(e, SyncEvent::Error(_)))
        );
    }

    // ---- the vault going missing -----------------------------------------

    #[test]
    fn a_vanished_vault_root_fails_the_cycle_instead_of_deleting_everything() {
        // An unmounted volume or a wrong path used to scan as an empty vault,
        // which the push phase read as "every file was deleted".
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        for n in 0..3 {
            mac.write(&format!("Notes/{n}.md"), "keep me\n");
        }
        mac.sync();
        let settled = s.journal_len();

        std::fs::remove_dir_all(&mac.root).unwrap();
        let err = mac.try_sync().unwrap_err().to_string();
        assert!(err.contains("vault root is missing"), "{err}");

        // Nothing was tombstoned, so the other devices still have the notes.
        assert_eq!(s.journal_len(), settled);
        let phone = Device::new(&s, "phone");
        phone.sync();
        assert_eq!(phone.read("Notes/1.md").as_deref(), Some("keep me\n"));
    }

    #[test]
    fn a_mass_delete_is_refused_until_it_is_explicitly_allowed() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        for n in 0..12 {
            mac.write(&format!("Notes/{n}.md"), "content\n");
        }
        mac.sync();
        let settled = s.journal_len();

        // The folder comes back empty (a container that failed to mount, a
        // half-restored backup): the deletes are held, loudly.
        for n in 0..12 {
            mac.delete(&format!("Notes/{n}.md"));
        }
        mac.sync();
        assert_eq!(s.journal_len(), settled, "no tombstone may be pushed");
        assert!(
            s.head(&VaultPath::new("Notes/1.md"))
                .is_some_and(|h| !h.deleted)
        );
        let errors = mac.errors();
        assert!(
            errors
                .iter()
                .any(|m| m.contains("refusing to sync 12 deletions")),
            "{errors:?}"
        );
        // And the device still tracks them, so nothing is lost by waiting.
        assert_eq!(mac.state.all_states().unwrap().len(), 12);

        // Content still flows while deletes are gated.
        mac.write("Notes/new.md", "fresh\n");
        let r = mac.sync();
        assert_eq!(r.pushed, 1);

        // Told explicitly that the deletes are real, the cycle pushes them.
        mac.config.borrow_mut().allow_bulk_delete = true;
        mac.delete("Notes/new.md");
        mac.sync();
        assert!(
            s.head(&VaultPath::new("Notes/1.md"))
                .is_some_and(|h| h.deleted)
        );
        assert!(mac.state.all_states().unwrap().is_empty());
    }

    #[test]
    fn an_everyday_delete_is_under_the_limit_and_still_syncs() {
        let s = FakeServer::new();
        let mac = Device::new(&s, "mac");
        for n in 0..30 {
            mac.write(&format!("Notes/{n}.md"), "content\n");
        }
        mac.sync();
        // Six of thirty is under the 20% limit; it is an ordinary tidy-up.
        for n in 0..6 {
            mac.delete(&format!("Notes/{n}.md"));
        }
        mac.sync();
        assert!(
            s.head(&VaultPath::new("Notes/0.md"))
                .is_some_and(|h| h.deleted)
        );
        assert!(mac.errors().is_empty(), "{:?}", mac.errors());
    }

    // ---- resilience -------------------------------------------------------

    #[test]
    fn a_pruned_historical_rev_is_skipped_so_the_cycle_still_progresses() {
        // Retention is finite (spec §11). A journal entry whose blob is gone
        // used to abort the cycle at the same entry every time, wedging the
        // device forever; it must step over it and keep going.
        let s = FakeServer::new();
        let src = Device::new(&s, "src");
        src.write("Notes/a.md", "v1\n");
        src.sync();
        src.write("Notes/a.md", "v2\n");
        src.sync();
        src.write("Notes/b.md", "b\n");
        src.sync();

        let dest = Device::with_transport(&s, "dest", |inner| {
            Box::new(PrunedRev {
                inner,
                path: VaultPath::new("Notes/a.md"),
                rev: 1,
            })
        });
        dest.sync();

        assert_eq!(dest.read("Notes/a.md").as_deref(), Some("v2\n"));
        assert_eq!(dest.read("Notes/b.md").as_deref(), Some("b\n"));
        let errors = dest.errors();
        assert!(
            errors.iter().any(|m| m.contains("no longer keeps")),
            "{errors:?}"
        );
        // And the next cycle is quiet: the cursor moved past the pruned entry.
        dest.events.borrow_mut().clear();
        dest.sync();
        assert!(dest.errors().is_empty());
    }

    #[test]
    fn a_write_landing_during_the_fetch_is_not_clobbered() {
        // The bridge shares its folder with Syncthing (spec §3 Phase B), so a
        // file can appear between the engine reading a path and renaming remote
        // content over it. The pre-image check turns that into a conflict, which
        // the push phase resolves the honest way.
        let s = FakeServer::new();
        let seed = Device::new(&s, "seed");
        seed.write("Notes/a.md", "from the server\n");
        seed.sync();

        let root = std::env::temp_dir().join(format!(
            "kairn-sync-cyc-toctou-{}-{}",
            std::process::id(),
            TAG.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        let dest = {
            let path = root.join("Notes/a.md");
            let mut d = Device::with_transport(&s, "dest", |inner| {
                Box::new(WriteDuringFetch {
                    inner,
                    path,
                    content: b"typed here, mid-cycle\n".to_vec(),
                    fired: std::sync::atomic::AtomicBool::new(false),
                })
            });
            // Point the device at the folder the transport writes into.
            let _ = std::fs::remove_dir_all(&d.root);
            d.root = root.clone();
            d.vault = VaultIo::new(&root, "DEST");
            d
        };

        dest.sync();

        // The pull refused to land over bytes it had never seen...
        let errors = dest.errors();
        assert!(
            errors
                .iter()
                .any(|m| m.contains("changed on disk mid-cycle")),
            "{errors:?}"
        );
        // ...and the remote content wins the file the honest way (spec §8),
        // with the text that landed mid-cycle kept as a conflict copy.
        assert_eq!(
            dest.read("Notes/a.md").as_deref(),
            Some("from the server\n")
        );
        assert_eq!(
            dest.conflict_copies("Notes/a.md"),
            vec!["typed here, mid-cycle\n".to_string()]
        );
    }

    #[test]
    fn a_crash_between_the_vault_write_and_the_state_update_loses_nothing() {
        // Invariant §15.4 at its tightest boundary: the file lands, then the
        // process dies before the state row and cursor record it. Re-running
        // must converge on the same bytes with no conflict artifact and no new
        // rev — the write is idempotent because the content matches by hash.
        let s = FakeServer::new();
        let src = Device::new(&s, "src");
        src.write("Notes/a.md", "AAA\n");
        src.sync();
        let settled = s.journal_len();

        let dest = Device::new(&s, "dest");
        dest.fail_state_after_write.set(true);
        assert!(dest.try_sync().is_err());
        dest.fail_state_after_write.set(false);
        dest.state.set_read_only(false).unwrap();

        // The file is on disk; nothing recorded it.
        assert_eq!(dest.read("Notes/a.md").as_deref(), Some("AAA\n"));
        assert!(
            dest.state
                .file_state(&VaultPath::new("Notes/a.md"))
                .unwrap()
                .is_none()
        );
        assert_eq!(dest.state.cursor().unwrap(), 0);

        dest.sync();
        assert_eq!(dest.read("Notes/a.md").as_deref(), Some("AAA\n"));
        assert!(
            dest.state
                .file_state(&VaultPath::new("Notes/a.md"))
                .unwrap()
                .is_some()
        );
        assert!(dest.conflict_copies("Notes/a.md").is_empty());
        assert_eq!(s.journal_len(), settled, "no rev may be minted by recovery");
    }

    #[test]
    fn a_file_delivered_out_of_band_converges_on_both_engines() {
        // Phase B: Syncthing hands files to the bridge's folder without the
        // engine knowing, and the phone joins through the server. Nothing may
        // echo into an endless rev chain (spec §3, §16).
        let s = FakeServer::new();
        let bridge = Device::new(&s, "bridge");
        let phone = Device::new(&s, "phone");
        let note = "Notes/from-the-thinkpad.md";

        // Syncthing drops a new note in.
        bridge.write(note, "line one\n");
        bridge.sync();
        phone.sync();
        assert_eq!(phone.read(note).as_deref(), Some("line one\n"));

        // ...and later edits it out of band, under a path the bridge tracks.
        bridge.write(note, "line one\nline two\n");
        bridge.sync();
        phone.sync();
        assert_eq!(phone.read(note).as_deref(), Some("line one\nline two\n"));

        // The phone's own edit travels the other way.
        phone.write(note, "line one\nline two\nfrom the phone\n");
        phone.sync();
        bridge.sync();
        assert_eq!(
            bridge.read(note).as_deref(),
            Some("line one\nline two\nfrom the phone\n")
        );

        // Settled: further cycles add no revs and write no conflict copies.
        let settled = s.journal_len();
        for _ in 0..3 {
            bridge.sync();
            phone.sync();
        }
        assert_eq!(s.journal_len(), settled, "echoes must not create new revs");
        assert!(bridge.conflict_copies(note).is_empty());
        assert!(phone.conflict_copies(note).is_empty());
    }

    #[test]
    fn an_idle_cycle_is_one_round_trip() {
        // Spec §7: "a cycle with an empty pull and empty push is the steady
        // state and must be cheap (one HTTP round trip)". The bridge runs one
        // every few seconds, so anything more is paid all day.
        let s = FakeServer::new();
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        let dev = Device::with_transport(&s, "dev", move |inner| {
            Box::new(Counting {
                inner,
                calls: counter,
            })
        });
        dev.write("Notes/a.md", "content\n");
        dev.sync();
        dev.sync();

        calls.store(0, Ordering::SeqCst);
        let r = dev.sync();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an idle cycle should be a single `changes` call"
        );
        assert_eq!((r.pushed, r.pulled, r.merged, r.conflicts), (0, 0, 0, 0));
    }

    #[test]
    fn a_cancelled_cycle_stops_before_the_push_phase() {
        // `stop()` must be able to cut a cycle short (iOS runs it in a short
        // background window) rather than wait out a two-minute HTTP timeout.
        let s = FakeServer::new();
        let dev = Device::new(&s, "dev");
        dev.write("Notes/a.md", "content\n");
        dev.cancelled.set(true);
        dev.sync();
        assert_eq!(s.journal_len(), 0);

        dev.cancelled.set(false);
        assert_eq!(dev.sync().pushed, 1);
    }
}
