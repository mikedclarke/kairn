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

/// Slot the lines a user typed over a placeholder note (`seed`, a blank day's
/// template, edited into `edited`) into the real note that has since arrived
/// (`disk`), each under its heading, instead of merging two unrelated versions.
/// An untouched or empty `edited` returns `disk` unchanged. See
/// [`kairn_core::adopt_seeded`].
#[uniffi::export]
pub fn adopt_seeded_text(seed: String, edited: String, disk: String) -> String {
    kairn_core::adopt_seeded(&seed, &edited, &disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_seeded_text_places_typed_lines_under_their_heading() {
        let seed = "### Todays Tasks\n\n### Daily Notes\n\n";
        let edited = "### Todays Tasks\n\n### Daily Notes\n* Buy stamps\n\n";
        let disk = "### Daily Notes\nMorning walk\n\n### Desk\n- Sorted the post\n";
        assert_eq!(
            adopt_seeded_text(seed.into(), edited.into(), disk.into()),
            "### Daily Notes\nMorning walk\n* Buy stamps\n\n### Desk\n- Sorted the post\n"
        );
        assert_eq!(adopt_seeded_text(seed.into(), seed.into(), disk.into()), disk);
    }

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


#[derive(uniffi::Record)]
pub struct NoteSaveResult {
    pub saved: bool,
    pub text: String,
    pub conflict_copy: Option<String>,
    pub error: Option<String>,
}

/// The shared lock, conflict preservation and durable write are one operation.
#[uniffi::export]
pub fn save_note_text(path: String, baseline: String, text: String) -> NoteSaveResult {
    match kairn_core::write::save_note_text(std::path::Path::new(&path), &baseline, &text) {
        Ok(saved) => NoteSaveResult { saved: true, text: saved.text,
            conflict_copy: saved.conflict_copy.map(|p| p.to_string_lossy().into_owned()), error: None },
        Err(e) => NoteSaveResult { saved: false, text, conflict_copy: None, error: Some(e.to_string()) },
    }
}
