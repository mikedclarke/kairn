//! The editable note buffer: one string of markdown, every mutation an
//! undoable operation, and a baseline snapshot anchoring merges with the
//! file on disk. UI-free on purpose: the app's editor and (later) the CLI
//! both drive this, and all the invariants live here where they can be
//! tested without a window.
//!
//! The save contract: disk is only ever compared against `baseline`, and
//! buffer text only ever reaches disk through [`NoteBuffer::reconcile`] (or
//! a clean write when disk still equals the baseline). External edits merge
//! in line-by-line; a genuine collision keeps the disk side and returns our
//! side as a conflict for the caller to surface, so neither side is ever
//! silently lost.

use std::ops::Range;

use crate::merge::merge3;
use crate::parse::{content_start_col, continuation_prefix};

/// Milliseconds within which consecutive typing coalesces into one undo step.
const COALESCE_MS: u64 = 750;

/// One text replacement: `removed` was replaced by `inserted` at byte `at`.
struct EditOp {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Copy, PartialEq)]
enum GroupKind {
    /// A run of plain insertions (typing forward).
    Insert,
    /// A run of plain deletions (backspace or delete).
    Delete,
    /// Anything else: selection replaces, structural edits, merges. Never
    /// coalesces.
    Other,
}

/// A unit of undo: one or more coalesced ops plus the cursor positions to
/// restore on undo (before) and redo (after).
struct UndoGroup {
    ops: Vec<EditOp>,
    cursor_before: usize,
    cursor_after: usize,
    last_ms: u64,
    kind: GroupKind,
}

pub struct NoteBuffer {
    text: String,
    /// The file content our edits are relative to: what was last loaded,
    /// saved, or absorbed from disk.
    baseline: String,
    /// The text is a pre-rendered seed (the daily template) over a blank
    /// file, untouched by the user: nothing saves until a real edit lands,
    /// and content arriving on disk replaces the seed instead of merging
    /// with it.
    seeded: bool,
    /// Bumped by every change to `text` and by nothing else, so a caller
    /// caching work derived from the text can compare one integer instead
    /// of the document.
    revision: u64,
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
}

impl NoteBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            baseline: text.clone(),
            text,
            seeded: false,
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// A buffer opened over a blank (or absent) file with `seed` rendered in
    /// place of the emptiness: the daily-template case. The baseline stays
    /// the real disk content — treating the seed as disk state would make the
    /// next reconcile read every seed line as externally deleted and wipe the
    /// note (the pre-created empty daily file bug).
    pub fn with_seed(disk: impl Into<String>, seed: impl Into<String>) -> Self {
        Self {
            baseline: disk.into(),
            text: seed.into(),
            seeded: true,
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn baseline(&self) -> &str {
        &self.baseline
    }

    /// How many times the text has changed since the buffer was opened.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the buffer holds edits the baseline (last known disk state)
    /// does not. An untouched seed is not an edit: a day the user only
    /// looked at never writes a file.
    pub fn is_dirty(&self) -> bool {
        !self.seeded && self.text != self.baseline
    }

    /// Record that `text()` was just written to disk successfully.
    pub fn mark_saved(&mut self) {
        self.baseline = self.text.clone();
    }

    /// Replace `range` with `insert`; the caller's timestamp drives undo
    /// coalescing. Out-of-range or mid-character bounds are clamped, never a
    /// panic: the app feeds this raw positions from hit-testing. Returns the
    /// cursor position after the edit.
    pub fn edit(&mut self, range: Range<usize>, insert: &str, cursor_before: usize, now_ms: u64) -> usize {
        let kind = match (range.is_empty(), insert.is_empty()) {
            (true, false) => GroupKind::Insert,
            (false, true) => GroupKind::Delete,
            _ => GroupKind::Other,
        };
        self.edit_with_kind(range, insert, cursor_before, now_ms, kind)
    }

    fn edit_with_kind(
        &mut self,
        range: Range<usize>,
        insert: &str,
        cursor_before: usize,
        now_ms: u64,
        kind: GroupKind,
    ) -> usize {
        let start = floor_boundary(&self.text, range.start);
        let end = floor_boundary(&self.text, range.end).max(start);
        let removed = self.text[start..end].to_string();
        if removed.is_empty() && insert.is_empty() {
            return floor_boundary(&self.text, cursor_before);
        }
        // The first real edit makes the seed the user's own content.
        self.seeded = false;
        self.text.replace_range(start..end, insert);
        self.revision += 1;
        let cursor_after = start + insert.len();
        self.redo.clear();

        let multiline = insert.contains('\n') || removed.contains('\n');
        let coalesce = !multiline
            && kind != GroupKind::Other
            && self.undo.last().is_some_and(|g| {
                g.kind == kind
                    && now_ms.saturating_sub(g.last_ms) <= COALESCE_MS
                    && match kind {
                        GroupKind::Insert => start == g.cursor_after,
                        // A backspace run walks backward (end meets the
                        // cursor); a delete run stands still (start does).
                        GroupKind::Delete => end == g.cursor_after || start == g.cursor_after,
                        GroupKind::Other => false,
                    }
            });

        let op = EditOp { at: start, removed, inserted: insert.to_string() };
        if coalesce {
            let g = self.undo.last_mut().expect("checked above");
            g.ops.push(op);
            g.cursor_after = cursor_after;
            g.last_ms = now_ms;
        } else {
            // `cursor_before` is relative to the pre-edit text; undo clamps
            // it against the restored text, so it is stored as given.
            self.undo.push(UndoGroup {
                ops: vec![op],
                cursor_before,
                cursor_after,
                last_ms: now_ms,
                kind,
            });
        }
        cursor_after
    }

    /// End any open coalescing run: the next edit starts a fresh undo step.
    /// Call when the cursor moves by click or arrow.
    pub fn break_undo_group(&mut self) {
        if let Some(g) = self.undo.last_mut() {
            g.kind = GroupKind::Other;
        }
    }

    /// Timestamp of the most recent undo step, `None` when the stack is
    /// empty. A history above the buffer (the workspace's cross-file move
    /// stack) orders its own entries against these.
    pub fn last_undo_ms(&self) -> Option<u64> {
        self.undo.last().map(|g| g.last_ms)
    }

    /// Timestamp of the most recently undone step (the next redo), `None`
    /// when there is nothing to redo.
    pub fn last_redo_ms(&self) -> Option<u64> {
        self.redo.last().map(|g| g.last_ms)
    }

    /// Discard the most recent undo step without applying it: for an edit
    /// whose undo a higher-level history owns (the buffer half of a
    /// cross-file move). The text keeps the edit; only the record goes.
    pub fn drop_last_undo(&mut self) {
        self.undo.pop();
    }

    /// Undo the most recent group. Returns the cursor position to restore,
    /// `None` if there was nothing to undo.
    pub fn undo(&mut self) -> Option<usize> {
        let g = self.undo.pop()?;
        for op in g.ops.iter().rev() {
            let end = op.at + op.inserted.len();
            self.text.replace_range(op.at..end, &op.removed);
        }
        self.revision += 1;
        let cursor = floor_boundary(&self.text, g.cursor_before);
        self.redo.push(g);
        Some(cursor)
    }

    /// Re-apply the most recently undone group. Returns the cursor position
    /// to restore, `None` if there was nothing to redo.
    pub fn redo(&mut self) -> Option<usize> {
        let g = self.redo.pop()?;
        for op in &g.ops {
            let end = op.at + op.removed.len();
            self.text.replace_range(op.at..end, &op.inserted);
        }
        self.revision += 1;
        let cursor = floor_boundary(&self.text, g.cursor_after);
        self.undo.push(g);
        Some(cursor)
    }

    /// Enter at `offset`, NotePlan-style: at the end of a list line the list
    /// continues on a new line below; a bare list marker clears itself
    /// instead; mid-line it splits at the cursor (never inside a marker or
    /// task bracket), the remainder keeping the list style. Returns the new
    /// cursor position.
    pub fn split_line(&mut self, offset: usize, now_ms: u64) -> usize {
        let offset = floor_boundary(&self.text, offset);
        let line_start = self.text[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.text[offset..].find('\n').map_or(self.text.len(), |i| offset + i);
        let line = self.text[line_start..line_end].to_string();

        // Never split inside a list marker or task bracket: the earliest
        // split point is the start of the line's content.
        let content_col = content_start_col(&line).min(line.len());
        let col = (offset - line_start).max(content_col).min(line.len());
        let split_at = line_start + floor_boundary(&line, col);
        let head = &self.text[line_start..split_at];
        let tail_empty = split_at == line_end;
        let prefix = continuation_prefix(head);

        if tail_empty && !head.is_empty() && head == prefix {
            // Enter on a bare marker: the list is over; the marker clears.
            return self.edit_with_kind(line_start..line_end, "", offset, now_ms, GroupKind::Other);
        }
        if offset <= line_start + content_col && !tail_empty && !head.is_empty() && head != prefix
        {
            // Enter at (or before) the visible start of a line whose prefix a
            // continuation would not recreate (heading hashes, the quote
            // marker, bare indentation): the whole line moves down intact,
            // like any editor at a line start, instead of stranding the
            // prefix above. The cursor stays at the content it sat on.
            self.edit_with_kind(line_start..line_start, "\n", offset, now_ms, GroupKind::Other);
            // The returned cursor tracks the moved content, not the insert
            // point; keep the undo record's redo cursor in step with it.
            if let Some(g) = self.undo.last_mut() {
                g.cursor_after = offset + 1;
            }
            return offset + 1;
        }
        let insert = format!("\n{prefix}");
        self.edit_with_kind(split_at..split_at, &insert, offset, now_ms, GroupKind::Other)
    }

    /// Move the whole line containing `offset` so it sits at the line
    /// boundary `target` (a line-start offset in the current text, or the
    /// text length to move it to the end). One undoable step; a drop back
    /// onto the source line is a no-op. Returns the byte offset the moved
    /// line starts at afterwards.
    pub fn move_line(
        &mut self,
        offset: usize,
        target: usize,
        cursor_before: usize,
        now_ms: u64,
    ) -> usize {
        let offset = floor_boundary(&self.text, offset);
        let line_start = self.text[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_end =
            self.text[line_start..].find('\n').map_or(self.text.len(), |i| line_start + i);
        let target = floor_boundary(&self.text, target);
        if target >= line_start && target <= line_end + 1 {
            return line_start;
        }

        let trailing_nl = self.text.ends_with('\n');
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if trailing_nl {
            lines.pop();
        }
        let src = self.text[..line_start].matches('\n').count();
        let mut dst = if target >= self.text.len() {
            lines.len()
        } else {
            self.text[..target].matches('\n').count()
        };
        if src < dst {
            dst -= 1;
        }
        let line = lines.remove(src);
        let dst = dst.min(lines.len());
        lines.insert(dst, line);
        let mut new_text = lines.join("\n");
        if trailing_nl {
            new_text.push('\n');
        }
        let new_start: usize = lines.iter().take(dst).map(|l| l.len() + 1).sum();
        self.apply_rewrite(&new_text, cursor_before, now_ms);
        new_start
    }

    /// Move a whole block of lines (`range` spans the first line's start to
    /// the last line's end, final newline excluded, as `block_range` returns)
    /// so its first line sits at the line boundary `target` (a line-start
    /// offset, or the text length for the end). One undoable step; a target
    /// inside the block's own span is a no-op. Returns the byte offset the
    /// block starts at afterwards.
    pub fn move_block(
        &mut self,
        range: Range<usize>,
        target: usize,
        cursor_before: usize,
        now_ms: u64,
    ) -> usize {
        let start = floor_boundary(&self.text, range.start.min(range.end));
        let block_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let end = floor_boundary(&self.text, range.end.min(self.text.len())).max(start);
        let block_end = self.text[end..].find('\n').map_or(self.text.len(), |i| end + i);
        let target = floor_boundary(&self.text, target);
        if target >= block_start && target <= block_end + 1 {
            return block_start;
        }

        let trailing_nl = self.text.ends_with('\n');
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if trailing_nl {
            lines.pop();
        }
        let src = self.text[..block_start].matches('\n').count();
        let count = self.text[block_start..block_end].matches('\n').count() + 1;
        let mut dst = if target >= self.text.len() {
            lines.len()
        } else {
            self.text[..target].matches('\n').count()
        };
        if dst > src {
            dst -= count;
        }
        let block: Vec<String> = lines.drain(src..(src + count).min(lines.len())).collect();
        let dst = dst.min(lines.len());
        lines.splice(dst..dst, block);
        let mut new_text = lines.join("\n");
        if trailing_nl {
            new_text.push('\n');
        }
        let new_start: usize = lines.iter().take(dst).map(|l| l.len() + 1).sum();
        self.apply_rewrite(&new_text, cursor_before, now_ms);
        new_start
    }

    /// Move several whole-line blocks (each range spanning first line start
    /// to last line end as [`crate::block_range`] returns, non-overlapping,
    /// in document order) so they sit together at the line boundary `target`
    /// (a line-start offset, or the text length for the end), keeping their
    /// document order. One undoable step; a move that changes nothing
    /// records nothing. Returns the byte offset the first moved block starts
    /// at afterwards.
    pub fn move_blocks(
        &mut self,
        ranges: &[Range<usize>],
        target: usize,
        cursor_before: usize,
        now_ms: u64,
    ) -> usize {
        match ranges {
            [] => return floor_boundary(&self.text, cursor_before),
            [only] => return self.move_block(only.clone(), target, cursor_before, now_ms),
            _ => {}
        }
        let trailing_nl = self.text.ends_with('\n');
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if trailing_nl {
            lines.pop();
        }
        let moved_flags = self.block_line_flags(ranges, lines.len());
        let target = floor_boundary(&self.text, target);
        let dst_line = if target >= self.text.len() {
            lines.len()
        } else {
            self.text[..target].matches('\n').count()
        };
        // The insert position among the lines that stay put: dragged lines
        // above the target no longer count.
        let dst = moved_flags[..dst_line.min(lines.len())].iter().filter(|d| !**d).count();
        let moved: Vec<String> = lines
            .iter()
            .zip(&moved_flags)
            .filter(|(_, d)| **d)
            .map(|(l, _)| l.clone())
            .collect();
        let mut remaining: Vec<String> = lines
            .into_iter()
            .zip(&moved_flags)
            .filter(|(_, d)| !**d)
            .map(|(l, _)| l)
            .collect();
        remaining.splice(dst..dst, moved);
        let mut new_text = remaining.join("\n");
        if trailing_nl {
            new_text.push('\n');
        }
        let new_start: usize = remaining.iter().take(dst).map(|l| l.len() + 1).sum();
        self.apply_rewrite(&new_text, cursor_before, now_ms);
        new_start
    }

    /// Remove several whole-line blocks (ranges as [`crate::block_range`]
    /// returns, non-overlapping, in document order), each taking its line
    /// ending with it. One undoable step. Returns the cursor position after
    /// the edit.
    pub fn remove_blocks(
        &mut self,
        ranges: &[Range<usize>],
        cursor_before: usize,
        now_ms: u64,
    ) -> usize {
        let trailing_nl = self.text.ends_with('\n');
        let mut lines: Vec<String> = self.text.split('\n').map(str::to_string).collect();
        if trailing_nl {
            lines.pop();
        }
        let removed_flags = self.block_line_flags(ranges, lines.len());
        let remaining: Vec<String> = lines
            .into_iter()
            .zip(&removed_flags)
            .filter(|(_, d)| !**d)
            .map(|(l, _)| l)
            .collect();
        let mut new_text = remaining.join("\n");
        if trailing_nl && !remaining.is_empty() {
            new_text.push('\n');
        }
        self.apply_rewrite(&new_text, cursor_before, now_ms)
    }

    /// Which line indices the byte ranges cover, each range extended to
    /// whole lines the way [`Self::move_block`] treats its span.
    fn block_line_flags(&self, ranges: &[Range<usize>], line_count: usize) -> Vec<bool> {
        let mut flags = vec![false; line_count];
        for range in ranges {
            let start = floor_boundary(&self.text, range.start.min(range.end));
            let block_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
            let end = floor_boundary(&self.text, range.end.min(self.text.len())).max(start);
            let block_end = self.text[end..].find('\n').map_or(self.text.len(), |i| end + i);
            let first = self.text[..block_start].matches('\n').count();
            let count = self.text[block_start..block_end].matches('\n').count() + 1;
            for flag in flags.iter_mut().skip(first).take(count) {
                *flag = true;
            }
        }
        flags
    }

    /// Replace the whole text with `new_text` as one undo step, recording
    /// only the region that actually changed. Identical texts record
    /// nothing. Returns the cursor position after the edit.
    fn apply_rewrite(&mut self, new_text: &str, cursor_before: usize, now_ms: u64) -> usize {
        let old_len = self.text.len();
        let mut prefix = self
            .text
            .bytes()
            .zip(new_text.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !self.text.is_char_boundary(prefix) || !new_text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = self
            .text
            .bytes()
            .rev()
            .zip(new_text.bytes().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(old_len - prefix)
            .min(new_text.len() - prefix);
        while !self.text.is_char_boundary(old_len - suffix)
            || !new_text.is_char_boundary(new_text.len() - suffix)
        {
            suffix -= 1;
        }
        self.edit_with_kind(
            prefix..old_len - suffix,
            &new_text[prefix..new_text.len() - suffix],
            cursor_before,
            now_ms,
            GroupKind::Other,
        )
    }

    /// Absorb the file's current content into the buffer without writing:
    /// the merge path for watcher events landing mid-edit and for saves over
    /// a changed file. After this, `text()` holds the merge and the baseline
    /// is `disk`, so a subsequent clean save writes exactly the merge.
    /// Returns the (clamped) cursor and any conflicting local hunks the
    /// caller must surface.
    pub fn reconcile(&mut self, disk: &str, cursor: usize) -> (usize, Vec<String>) {
        if disk == self.baseline {
            return (floor_boundary(&self.text, cursor), Vec::new());
        }
        if self.seeded {
            // The file changed under an untouched seed. Still blank: keep
            // showing the seed over the new baseline. Real content: the day
            // is no longer blank, the seed bows out, and the disk side is
            // adopted wholesale — nothing typed exists to conflict with.
            self.baseline = disk.to_string();
            if !disk.trim().is_empty() {
                self.text = disk.to_string();
                self.revision += 1;
                self.seeded = false;
                self.undo.clear();
                self.redo.clear();
            }
            return (floor_boundary(&self.text, cursor), Vec::new());
        }
        let merged = merge3(&self.baseline, disk, &self.text);
        if merged.text != self.text {
            let old = std::mem::replace(&mut self.text, merged.text);
            self.revision += 1;
            self.redo.clear();
            self.undo.push(UndoGroup {
                ops: vec![EditOp { at: 0, removed: old, inserted: self.text.clone() }],
                cursor_before: cursor,
                cursor_after: floor_boundary(&self.text, cursor),
                last_ms: 0,
                kind: GroupKind::Other,
            });
        }
        self.baseline = disk.to_string();
        (floor_boundary(&self.text, cursor), merged.conflicts)
    }
}

/// Largest char boundary at or below `i`, clamped to the text.
fn floor_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_run_is_one_undo_step() {
        let mut b = NoteBuffer::new("ab");
        let c = b.edit(2..2, "c", 2, 1000);
        let c = b.edit(c..c, "d", c, 1200);
        b.edit(c..c, "e", c, 1400);
        assert_eq!(b.text(), "abcde");
        assert_eq!(b.undo(), Some(2));
        assert_eq!(b.text(), "ab");
        assert_eq!(b.redo(), Some(5));
        assert_eq!(b.text(), "abcde");
    }

    #[test]
    fn pause_or_newline_starts_a_new_step() {
        let mut b = NoteBuffer::new("");
        let c = b.edit(0..0, "a", 0, 1000);
        let c = b.edit(c..c, "b", c, 5000); // long pause
        b.edit(c..c, "\n", c, 5100); // newline never coalesces
        assert_eq!(b.text(), "ab\n");
        b.undo();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "a");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn backspace_run_coalesces() {
        let mut b = NoteBuffer::new("abcd");
        let c = b.edit(3..4, "", 4, 1000);
        let c = b.edit(2..3, "", c, 1100);
        b.edit(1..2, "", c, 1200);
        assert_eq!(b.text(), "a");
        assert_eq!(b.undo(), Some(4));
        assert_eq!(b.text(), "abcd");
    }

    #[test]
    fn selection_replace_is_its_own_step() {
        let mut b = NoteBuffer::new("hello world");
        b.edit(6..11, "", 11, 1000);
        b.edit(0..5, "goodbye", 5, 1050);
        assert_eq!(b.text(), "goodbye ");
        b.undo();
        assert_eq!(b.text(), "hello ");
        b.undo();
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn break_undo_group_splits_a_typing_run() {
        let mut b = NoteBuffer::new("");
        let c = b.edit(0..0, "a", 0, 1000);
        b.break_undo_group();
        b.edit(c..c, "b", c, 1010);
        b.undo();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn edit_clamps_to_char_boundaries() {
        let mut b = NoteBuffer::new("aé b"); // é is 2 bytes at offset 1..3
        b.edit(2..2, "x", 2, 1000); // mid-character: clamps down to 1
        assert_eq!(b.text(), "axé b");
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut b = NoteBuffer::new("");
        b.edit(0..0, "a", 0, 1000);
        b.undo();
        b.edit(0..0, "b", 0, 2000);
        assert_eq!(b.redo(), None);
        assert_eq!(b.text(), "b");
    }

    #[test]
    fn split_line_continues_a_task_list() {
        let mut b = NoteBuffer::new("* [ ] buy milk");
        let c = b.split_line(14, 1000);
        assert_eq!(b.text(), "* [ ] buy milk\n* [ ] ");
        assert_eq!(c, b.text().len());
    }

    #[test]
    fn split_line_on_bare_marker_clears_it() {
        let mut b = NoteBuffer::new("* [ ] task\n* [ ] ");
        let c = b.split_line(17, 1000);
        assert_eq!(b.text(), "* [ ] task\n");
        assert_eq!(c, 11);
    }

    #[test]
    fn split_line_mid_line_keeps_list_style_and_marker_intact() {
        let mut b = NoteBuffer::new("* buy milk");
        // Cursor inside the marker clamps to the content start.
        let c = b.split_line(1, 1000);
        assert_eq!(b.text(), "* \n* buy milk");
        assert_eq!(c, 5);
        // A genuine mid-content split carries the remainder onto the new
        // list line.
        let mut b = NoteBuffer::new("* buy milk");
        let c = b.split_line(5, 2000);
        assert_eq!(b.text(), "* buy\n*  milk");
        assert_eq!(c, 8);
    }

    #[test]
    fn split_line_at_heading_start_moves_the_whole_line() {
        let mut b = NoteBuffer::new("## Title\nbody");
        // Cursor at the heading's visible start (after the hashes): the
        // heading moves down intact instead of stranding "## " above.
        let c = b.split_line(3, 1000);
        assert_eq!(b.text(), "\n## Title\nbody");
        assert_eq!(c, 4);
        b.undo();
        assert_eq!(b.text(), "## Title\nbody");
    }

    #[test]
    fn split_line_mid_heading_still_splits() {
        let mut b = NoteBuffer::new("## Title");
        let c = b.split_line(5, 1000);
        assert_eq!(b.text(), "## Ti\ntle");
        assert_eq!(c, 6);
    }

    #[test]
    fn split_line_at_end_of_bare_heading_opens_a_line_below() {
        let mut b = NoteBuffer::new("## ");
        let c = b.split_line(3, 1000);
        assert_eq!(b.text(), "## \n");
        assert_eq!(c, 4);
    }

    #[test]
    fn split_line_at_quote_start_moves_the_whole_line() {
        let mut b = NoteBuffer::new("> quoted");
        let c = b.split_line(2, 1000);
        assert_eq!(b.text(), "\n> quoted");
        assert_eq!(c, 3);
    }

    #[test]
    fn split_line_plain_text_has_no_prefix() {
        let mut b = NoteBuffer::new("just a sentence");
        let c = b.split_line(15, 1000);
        assert_eq!(b.text(), "just a sentence\n");
        assert_eq!(c, 16);
    }

    #[test]
    fn split_is_undoable() {
        let mut b = NoteBuffer::new("* task");
        b.split_line(6, 1000);
        assert_eq!(b.text(), "* task\n* ");
        b.undo();
        assert_eq!(b.text(), "* task");
    }

    #[test]
    fn move_line_down_and_up() {
        let mut b = NoteBuffer::new("a\nb\nc\n");
        // Move "a" to sit where "c" starts.
        assert_eq!(b.move_line(0, 4, 0, 1000), 2);
        assert_eq!(b.text(), "b\na\nc\n");
        // Move "c" (now still at 4) to the top.
        assert_eq!(b.move_line(4, 0, 0, 2000), 0);
        assert_eq!(b.text(), "c\nb\na\n");
    }

    #[test]
    fn move_line_to_end_without_trailing_newline() {
        let mut b = NoteBuffer::new("a\nb\nc");
        assert_eq!(b.move_line(0, 5, 0, 1000), 4);
        assert_eq!(b.text(), "b\nc\na");
        // And the last line back to the top.
        let mut b = NoteBuffer::new("a\nb\nc");
        assert_eq!(b.move_line(4, 0, 0, 1000), 0);
        assert_eq!(b.text(), "c\na\nb");
    }

    #[test]
    fn move_line_onto_itself_is_a_no_op() {
        let mut b = NoteBuffer::new("a\nb\nc\n");
        assert_eq!(b.move_line(2, 2, 0, 1000), 2);
        // Dropping on the boundary just below is the same position.
        assert_eq!(b.move_line(2, 4, 0, 1000), 2);
        assert_eq!(b.text(), "a\nb\nc\n");
        assert_eq!(b.undo(), None);
    }

    #[test]
    fn move_line_is_one_undo_step() {
        let mut b = NoteBuffer::new("a\nb\nc\n");
        b.move_line(0, 6, 5, 1000);
        assert_eq!(b.text(), "b\nc\na\n");
        assert_eq!(b.undo(), Some(5));
        assert_eq!(b.text(), "a\nb\nc\n");
        assert!(b.redo().is_some());
        assert_eq!(b.text(), "b\nc\na\n");
    }

    #[test]
    fn move_block_down_and_up_with_children() {
        // "* a" and its two children move as one; "z" stays put.
        let mut b = NoteBuffer::new("* a\n\tone\n\ttwo\nz\nend\n");
        let block = 0..13; // "* a\n\tone\n\ttwo"
        assert_eq!(b.move_block(block, 16, 0, 1000), 2);
        assert_eq!(b.text(), "z\n* a\n\tone\n\ttwo\nend\n");
        // And back to the top.
        assert_eq!(b.move_block(2..15, 0, 0, 2000), 0);
        assert_eq!(b.text(), "* a\n\tone\n\ttwo\nz\nend\n");
    }

    #[test]
    fn move_block_with_interior_blank_line() {
        let mut b = NoteBuffer::new("* a\n\tone\n\n\ttwo\nz\n");
        let block = 0..14; // includes the interior blank
        assert_eq!(b.move_block(block, 17, 0, 1000), 2);
        assert_eq!(b.text(), "z\n* a\n\tone\n\n\ttwo\n");
    }

    #[test]
    fn move_block_to_end_without_trailing_newline() {
        let mut b = NoteBuffer::new("* a\n\tsub\nz");
        assert_eq!(b.move_block(0..8, 10, 0, 1000), 2);
        assert_eq!(b.text(), "z\n* a\n\tsub");
    }

    #[test]
    fn move_block_onto_itself_is_a_no_op() {
        let mut b = NoteBuffer::new("* a\n\tsub\nz\n");
        // Anywhere inside its own span, including the boundary just below.
        assert_eq!(b.move_block(0..8, 0, 0, 1000), 0);
        assert_eq!(b.move_block(0..8, 4, 0, 1000), 0);
        assert_eq!(b.move_block(0..8, 9, 0, 1000), 0);
        assert_eq!(b.text(), "* a\n\tsub\nz\n");
        assert_eq!(b.undo(), None);
    }

    #[test]
    fn move_block_is_one_undo_step() {
        let mut b = NoteBuffer::new("* a\n\tsub\nz\nend\n");
        b.move_block(0..8, 11, 5, 1000);
        assert_eq!(b.text(), "z\n* a\n\tsub\nend\n");
        assert_eq!(b.undo(), Some(5));
        assert_eq!(b.text(), "* a\n\tsub\nz\nend\n");
        assert!(b.redo().is_some());
        assert_eq!(b.text(), "z\n* a\n\tsub\nend\n");
    }

    #[test]
    fn move_block_of_one_line_matches_move_line() {
        let mut b = NoteBuffer::new("a\nb\nc\n");
        assert_eq!(b.move_block(0..1, 4, 0, 1000), 2);
        assert_eq!(b.text(), "b\na\nc\n");
    }

    #[test]
    fn move_blocks_gathers_disjoint_blocks_in_order() {
        // "* a" (with child) and "* c" travel together past "z".
        let mut b = NoteBuffer::new("* a\n\tsub\nx\n* c\nz\nend\n");
        let new_start = b.move_blocks(&[0..8, 11..14], 17, 0, 1000);
        assert_eq!(b.text(), "x\nz\n* a\n\tsub\n* c\nend\n");
        assert_eq!(new_start, 4);
        assert_eq!(&b.text()[new_start..new_start + 3], "* a");
    }

    #[test]
    fn move_blocks_to_end_and_to_top() {
        let mut b = NoteBuffer::new("a\nb\nc\nd");
        assert_eq!(b.move_blocks(&[0..1, 4..5], 7, 0, 1000), 4);
        assert_eq!(b.text(), "b\nd\na\nc");
        let mut b = NoteBuffer::new("a\nb\nc\nd");
        assert_eq!(b.move_blocks(&[2..3, 6..7], 0, 0, 1000), 0);
        assert_eq!(b.text(), "b\nd\na\nc");
    }

    #[test]
    fn move_blocks_is_one_undo_step() {
        let mut b = NoteBuffer::new("a\nb\nc\nd\n");
        b.move_blocks(&[0..1, 4..5], 8, 3, 1000);
        assert_eq!(b.text(), "b\nd\na\nc\n");
        assert_eq!(b.undo(), Some(3));
        assert_eq!(b.text(), "a\nb\nc\nd\n");
        assert!(b.redo().is_some());
        assert_eq!(b.text(), "b\nd\na\nc\n");
    }

    #[test]
    fn move_blocks_noop_records_nothing() {
        // Both blocks already sit together right where the target is.
        let mut b = NoteBuffer::new("a\nb\nc\n");
        b.move_blocks(&[0..1, 2..3], 0, 0, 1000);
        assert_eq!(b.text(), "a\nb\nc\n");
        assert_eq!(b.undo(), None);
    }

    #[test]
    fn remove_blocks_takes_lines_and_endings() {
        let mut b = NoteBuffer::new("* a\n\tsub\nkeep\n* c\n");
        b.remove_blocks(&[0..8, 14..17], 0, 1000);
        assert_eq!(b.text(), "keep\n");
        assert_eq!(b.undo(), Some(0));
        assert_eq!(b.text(), "* a\n\tsub\nkeep\n* c\n");
    }

    #[test]
    fn remove_blocks_at_eof_without_trailing_newline() {
        let mut b = NoteBuffer::new("keep\n* a\n\tsub");
        b.remove_blocks(&[5..13], 0, 1000);
        assert_eq!(b.text(), "keep");
    }

    #[test]
    fn remove_blocks_everything_leaves_empty() {
        let mut b = NoteBuffer::new("a\nb\n");
        b.remove_blocks(&[0..1, 2..3], 0, 1000);
        assert_eq!(b.text(), "");
        b.undo();
        assert_eq!(b.text(), "a\nb\n");
    }

    #[test]
    fn undo_stack_peeks_and_drops() {
        let mut b = NoteBuffer::new("");
        assert_eq!(b.last_undo_ms(), None);
        b.edit(0..0, "a", 0, 1000);
        b.break_undo_group();
        b.edit(1..1, "b", 1, 2000);
        assert_eq!(b.last_undo_ms(), Some(2000));
        b.undo();
        assert_eq!(b.last_undo_ms(), Some(1000));
        assert_eq!(b.last_redo_ms(), Some(2000));
        // Dropping the record keeps the text but forgets the step.
        b.drop_last_undo();
        assert_eq!(b.text(), "a");
        assert_eq!(b.last_undo_ms(), None);
        assert_eq!(b.undo(), None);
    }

    #[test]
    fn reconcile_with_unchanged_disk_is_a_no_op() {
        let mut b = NoteBuffer::new("a\n");
        b.edit(2..2, "b", 2, 1000);
        let (cursor, conflicts) = b.reconcile("a\n", 3);
        assert_eq!(cursor, 3);
        assert!(conflicts.is_empty());
        assert_eq!(b.text(), "a\nb");
        assert!(b.is_dirty());
    }

    #[test]
    fn reconcile_absorbs_external_append_while_typing() {
        let mut b = NoteBuffer::new("top\nbottom\n");
        b.edit(0..3, "TOP", 3, 1000);
        let (_, conflicts) = b.reconcile("top\nbottom\n* agent line\n", 3);
        assert!(conflicts.is_empty());
        assert_eq!(b.text(), "TOP\nbottom\n* agent line\n");
        // Our edit is still pending relative to the new baseline.
        assert!(b.is_dirty());
        assert_eq!(b.baseline(), "top\nbottom\n* agent line\n");
    }

    #[test]
    fn reconcile_collision_keeps_disk_and_reports_ours() {
        let mut b = NoteBuffer::new("line\n");
        b.edit(0..4, "line ours", 4, 1000);
        let (_, conflicts) = b.reconcile("line disk\n", 9);
        assert_eq!(b.text(), "line disk\n");
        assert_eq!(conflicts, vec!["line ours".to_string()]);
        assert!(!b.is_dirty());
    }

    #[test]
    fn reconcile_is_undoable() {
        let mut b = NoteBuffer::new("a\nb\n");
        b.edit(0..1, "A", 1, 1000);
        b.reconcile("a\nb\nagent\n", 1);
        assert_eq!(b.text(), "A\nb\nagent\n");
        b.undo();
        assert_eq!(b.text(), "A\nb\n");
        b.undo();
        assert_eq!(b.text(), "a\nb\n");
    }

    #[test]
    fn typing_on_the_last_line_during_an_append_conflicts_honestly() {
        // Both sides changed the end of the file: line-granular merge calls
        // this a collision, keeps the agent's append, and hands the typed
        // text back rather than guessing an interleave.
        let mut b = NoteBuffer::new("a\n");
        b.edit(2..2, "typed", 2, 1000);
        let (_, conflicts) = b.reconcile("a\nagent\n", 7);
        assert_eq!(b.text(), "a\nagent\n");
        assert_eq!(conflicts, vec!["typed".to_string()]);
    }

    #[test]
    fn seeded_buffer_only_saves_after_a_real_edit() {
        let mut b = NoteBuffer::with_seed("", "## Tasks\n\n## Notes\n");
        assert!(!b.is_dirty());
        b.edit(9..9, "* call the bank\n", 9, 1000);
        assert!(b.is_dirty());
    }

    #[test]
    fn typing_into_a_seeded_day_survives_reconcile_with_the_empty_file() {
        // A pre-created empty daily file: the template renders as a seed
        // while disk holds "". Saving must not read the seed lines as
        // externally deleted and move the typed text into conflicts.
        let mut b = NoteBuffer::with_seed("", "## Tasks\n\n## Notes\n");
        b.edit(9..9, "* call the bank\n", 9, 1000);
        let (_, conflicts) = b.reconcile("", 25);
        assert!(conflicts.is_empty());
        assert_eq!(b.text(), "## Tasks\n* call the bank\n\n## Notes\n");
        assert!(b.is_dirty());
    }

    #[test]
    fn seeded_buffer_adopts_external_content_without_conflicts() {
        let mut b = NoteBuffer::with_seed("", "## Tasks\n\n## Notes\n");
        let (_, conflicts) = b.reconcile("* captured\n", 0);
        assert!(conflicts.is_empty());
        assert_eq!(b.text(), "* captured\n");
        assert!(!b.is_dirty());
    }

    #[test]
    fn seeded_buffer_keeps_the_seed_over_a_still_blank_rewrite() {
        let mut b = NoteBuffer::with_seed("", "## Tasks\n");
        let (_, conflicts) = b.reconcile("\n", 0);
        assert!(conflicts.is_empty());
        assert_eq!(b.text(), "## Tasks\n");
        assert_eq!(b.baseline(), "\n");
        assert!(!b.is_dirty());
    }

    #[test]
    fn revision_counts_text_changes_only() {
        let mut b = NoteBuffer::new("a\n");
        assert_eq!(b.revision(), 0);
        b.edit(2..2, "b", 2, 1000);
        assert_eq!(b.revision(), 1);
        // Reads and a successful save leave the text alone.
        assert_eq!(b.text(), "a\nb");
        b.mark_saved();
        assert_eq!(b.revision(), 1);
        // An edit that replaces nothing with nothing is not a change.
        b.edit(1..1, "", 1, 1100);
        assert_eq!(b.revision(), 1);
        assert_eq!(b.undo(), Some(2));
        assert_eq!(b.revision(), 2);
        // Nothing left to undo, nothing to bump.
        assert_eq!(b.undo(), None);
        assert_eq!(b.revision(), 2);
    }

    #[test]
    fn save_cycle_round_trips() {
        let mut b = NoteBuffer::new("a\n");
        b.edit(2..2, "b\n", 2, 1000);
        assert!(b.is_dirty());
        // Clean save path: disk still equals baseline, caller writes text().
        let (_, conflicts) = b.reconcile("a\n", 4);
        assert!(conflicts.is_empty());
        b.mark_saved();
        assert!(!b.is_dirty());
        assert_eq!(b.baseline(), "a\nb\n");
    }
}
