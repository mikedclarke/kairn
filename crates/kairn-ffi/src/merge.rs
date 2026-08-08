//! The stateless three-way merge, exposed directly for callers that merge two
//! versions without a live buffer. The stateful editor path is
//! [`crate::buffer::Buffer::reconcile`]; both delegate to the same
//! [`kairn_core::merge3`], so the two sync invariants (never clobber a disk
//! edit, never silently drop typed text) hold identically on every platform.

/// The outcome of a three-way merge: the merged text plus the local side of any
/// hunk that collided with a disk change (the disk side is already in `text`).
#[derive(uniffi::Record)]
pub struct MergeResult {
    pub text: String,
    pub conflicts: Vec<String>,
}

/// Merge two descendants of `base`: `disk` (what the file holds now) and `ours`
/// (the edited text). Non-conflicting hunks combine; where both sides changed
/// the same region, the disk side wins and ours surfaces as a conflict.
#[uniffi::export]
pub fn merge3(base: String, disk: String, ours: String) -> MergeResult {
    let merged = kairn_core::merge3(&base, &disk, &ours);
    MergeResult {
        text: merged.text,
        conflicts: merged.conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_conflicting_edits_combine() {
        let r = merge3("a\nb\nc\n".into(), "A\nb\nc\n".into(), "a\nb\nC\n".into());
        assert_eq!(r.text, "A\nb\nC\n");
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn same_line_collision_surfaces() {
        let r = merge3("x\n".into(), "disk\n".into(), "ours\n".into());
        assert_eq!(r.text, "disk\n");
        assert_eq!(r.conflicts, vec!["ours".to_string()]);
    }
}
