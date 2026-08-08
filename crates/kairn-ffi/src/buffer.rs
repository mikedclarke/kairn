//! The editable note buffer, exposed as a UniFFI object. All the edit, undo,
//! and merge logic lives in [`kairn_core::NoteBuffer`]; this only guards it for
//! shared ownership across the FFI (UniFFI objects are `Arc`, methods take
//! `&self`) and maps offsets to `u64`. Offsets are UTF-8 byte offsets (see the
//! crate docs).

use std::sync::Mutex;

use kairn_core::NoteBuffer;

/// The result of absorbing disk content into the buffer: the clamped cursor and
/// any local hunks that collided with a disk change (the caller surfaces these
/// so nothing typed is ever silently dropped).
#[derive(uniffi::Record)]
pub struct ReconcileResult {
    pub cursor: u64,
    pub conflicts: Vec<String>,
}

/// One editable markdown note. Mirrors [`kairn_core::NoteBuffer`] method for
/// method; the `Mutex` is only interior mutability for the `&self` FFI methods,
/// never held across a call.
#[derive(uniffi::Object)]
pub struct Buffer {
    inner: Mutex<NoteBuffer>,
}

#[uniffi::export]
impl Buffer {
    /// A fresh buffer whose baseline (last known disk state) is `text`.
    #[uniffi::constructor]
    pub fn new(text: String) -> Self {
        Self {
            inner: Mutex::new(NoteBuffer::new(text)),
        }
    }

    /// The current buffer text.
    pub fn text(&self) -> String {
        self.inner.lock().unwrap().text().to_string()
    }

    /// The baseline: the file content the buffer's edits are relative to.
    pub fn baseline(&self) -> String {
        self.inner.lock().unwrap().baseline().to_string()
    }

    /// Whether the buffer holds edits the baseline does not.
    pub fn is_dirty(&self) -> bool {
        self.inner.lock().unwrap().is_dirty()
    }

    /// Record that the current text was just written to disk successfully.
    pub fn mark_saved(&self) {
        self.inner.lock().unwrap().mark_saved();
    }

    /// Replace the byte range `[start, end)` with `insert`. `now_ms` drives
    /// undo coalescing; out-of-range or mid-character bounds are clamped, never
    /// a panic. Returns the cursor byte offset after the edit.
    pub fn edit(&self, start: u64, end: u64, insert: String, cursor_before: u64, now_ms: u64) -> u64 {
        self.inner.lock().unwrap().edit(
            start as usize..end as usize,
            &insert,
            cursor_before as usize,
            now_ms,
        ) as u64
    }

    /// End any open coalescing run so the next edit starts a fresh undo step.
    /// Call when the cursor moves by tap or arrow.
    pub fn break_undo_group(&self) {
        self.inner.lock().unwrap().break_undo_group();
    }

    /// Undo the most recent group; returns the cursor to restore, or `None` if
    /// there was nothing to undo.
    pub fn undo(&self) -> Option<u64> {
        self.inner.lock().unwrap().undo().map(|c| c as u64)
    }

    /// Redo the most recently undone group; returns the cursor to restore, or
    /// `None` if there was nothing to redo.
    pub fn redo(&self) -> Option<u64> {
        self.inner.lock().unwrap().redo().map(|c| c as u64)
    }

    /// Enter at `offset`, NotePlan-style (list continuation, bare-marker clear,
    /// mid-line split). Returns the new cursor byte offset.
    pub fn split_line(&self, offset: u64, now_ms: u64) -> u64 {
        self.inner.lock().unwrap().split_line(offset as usize, now_ms) as u64
    }

    /// Move the line containing `offset` to the line boundary `target` (a
    /// line-start byte offset, or the text length for the end) as one undoable
    /// step. Returns the byte offset the moved line starts at afterwards.
    pub fn move_line(&self, offset: u64, target: u64, cursor_before: u64, now_ms: u64) -> u64 {
        self.inner.lock().unwrap().move_line(
            offset as usize,
            target as usize,
            cursor_before as usize,
            now_ms,
        ) as u64
    }

    /// Absorb the file's current content into the buffer without writing, via
    /// the three-way merge (the watcher/save-over-change path). After this the
    /// text holds the merge and the baseline is `disk`.
    pub fn reconcile(&self, disk: String, cursor: u64) -> ReconcileResult {
        let (cursor, conflicts) = self.inner.lock().unwrap().reconcile(&disk, cursor as usize);
        ReconcileResult {
            cursor: cursor as u64,
            conflicts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_then_undo_round_trips() {
        let b = Buffer::new("hello".into());
        let after = b.edit(5, 5, " world".into(), 5, 1_000);
        assert_eq!(b.text(), "hello world");
        assert_eq!(after, 11);
        assert!(b.is_dirty());
        b.undo();
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn reconcile_reports_conflicts() {
        let b = Buffer::new("line one\n".into());
        b.edit(0, 8, "ours".into(), 0, 1_000);
        let r = b.reconcile("theirs\n".into(), 0);
        // Both sides changed the one line: disk wins, our side surfaces.
        assert_eq!(b.text(), "theirs\n");
        assert_eq!(r.conflicts, vec!["ours".to_string()]);
    }

    #[test]
    fn mark_saved_clears_dirty() {
        let b = Buffer::new("a".into());
        b.edit(1, 1, "b".into(), 1, 1_000);
        assert!(b.is_dirty());
        b.mark_saved();
        assert!(!b.is_dirty());
        assert_eq!(b.baseline(), "ab");
    }
}
