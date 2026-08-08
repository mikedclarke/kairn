//! Turning a collision into a decision (spec §8). A conflict exists when a
//! pulled change or a rejected push meets a locally dirty file (local differs
//! from baseline and remote differs from baseline). Markdown goes through
//! `kairn_core::merge3` — the same three-way merge the editor's save path uses,
//! so sync and the editor resolve identically and the disk/remote side wins a
//! collided hunk. Everything else is last-writer-wins with a conflict copy.
//!
//! This module is pure: it decides *what* bytes go where; `vaultio` performs the
//! writes. That keeps the honest-not-lossy floor (invariant §15.2) provable
//! here in isolation.

use kairn_core::merge3;

/// The decision for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Write `content` and produce no artifact — a clean pull or a clean merge.
    Write {
        content: Vec<u8>,
        merged_clean: bool,
    },
    /// Write `content` (remote won the collided hunks) and drop the pre-merge
    /// local bytes beside it as a conflict copy — nothing typed is lost.
    WriteWithConflict {
        content: Vec<u8>,
        conflict_copy: Vec<u8>,
    },
}

/// Resolve a markdown collision. `baseline` is the last-synced text if it is
/// still retained; `None` means the baseline is gone, so a three-way merge
/// isn't possible and the code degrades safely (spec §6): identical sides pass
/// clean, differing sides conflict with the remote winning.
pub fn resolve_markdown(baseline: Option<&str>, remote: &str, local: &str) -> Resolution {
    if remote == local {
        return Resolution::Write {
            content: remote.as_bytes().to_vec(),
            merged_clean: true,
        };
    }
    let Some(baseline) = baseline else {
        // No baseline to merge against: keep the remote head, preserve the
        // local text as a conflict copy rather than guess a merge.
        return Resolution::WriteWithConflict {
            content: remote.as_bytes().to_vec(),
            conflict_copy: local.as_bytes().to_vec(),
        };
    };
    let merged = merge3(baseline, remote, local);
    if merged.conflicts.is_empty() {
        Resolution::Write {
            content: merged.text.into_bytes(),
            merged_clean: true,
        }
    } else {
        // The merged text (remote winning the collisions) is the file; the full
        // pre-merge local content becomes the conflict copy (spec §8).
        Resolution::WriteWithConflict {
            content: merged.text.into_bytes(),
            conflict_copy: local.as_bytes().to_vec(),
        }
    }
}

/// Resolve a non-markdown collision: no merge. The remote head becomes the file,
/// the local version becomes a conflict copy (spec §8). Also the fallback for a
/// markdown file whose bytes aren't valid UTF-8.
pub fn resolve_binary(remote: &[u8], local: &[u8]) -> Resolution {
    if remote == local {
        Resolution::Write {
            content: remote.to_vec(),
            merged_clean: true,
        }
    } else {
        Resolution::WriteWithConflict {
            content: remote.to_vec(),
            conflict_copy: local.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(r: &Resolution) -> String {
        match r {
            Resolution::Write { content, .. } | Resolution::WriteWithConflict { content, .. } => {
                String::from_utf8(content.clone()).unwrap()
            }
        }
    }

    #[test]
    fn disjoint_markdown_edits_merge_clean() {
        // Remote appended a task; we edited the top. Both survive, no artifact.
        let base = "top\nmiddle\nbottom\n";
        let remote = "top\nmiddle\nbottom\n* agent task\n";
        let local = "top edited\nmiddle\nbottom\n";
        let r = resolve_markdown(Some(base), remote, local);
        assert_eq!(
            r,
            Resolution::Write {
                content: b"top edited\nmiddle\nbottom\n* agent task\n".to_vec(),
                merged_clean: true,
            }
        );
    }

    #[test]
    fn same_line_markdown_collision_keeps_remote_and_copies_local() {
        let r = resolve_markdown(Some("x\n"), "x remote\n", "x local\n");
        let Resolution::WriteWithConflict {
            content,
            conflict_copy,
        } = r
        else {
            panic!("expected a conflict copy");
        };
        // Remote wins the file; the whole local version is preserved.
        assert_eq!(content, b"x remote\n");
        assert_eq!(conflict_copy, b"x local\n");
    }

    #[test]
    fn identical_sides_are_clean_even_without_a_baseline() {
        let r = resolve_markdown(None, "same\n", "same\n");
        assert_eq!(
            r,
            Resolution::Write {
                content: b"same\n".to_vec(),
                merged_clean: true
            }
        );
    }

    #[test]
    fn missing_baseline_conflicts_rather_than_guesses() {
        let r = resolve_markdown(None, "remote\n", "local\n");
        assert_eq!(
            r,
            Resolution::WriteWithConflict {
                content: b"remote\n".to_vec(),
                conflict_copy: b"local\n".to_vec(),
            }
        );
    }

    #[test]
    fn binary_conflict_is_last_writer_wins_with_a_copy() {
        let r = resolve_binary(b"\x00remote", b"\x00local");
        assert_eq!(
            r,
            Resolution::WriteWithConflict {
                content: b"\x00remote".to_vec(),
                conflict_copy: b"\x00local".to_vec(),
            }
        );
    }

    #[test]
    fn no_real_difference_writes_clean() {
        assert!(matches!(
            resolve_markdown(Some("a\n"), "a\n", "a\n"),
            Resolution::Write {
                merged_clean: true,
                ..
            }
        ));
        let _ = text; // silence unused in some configurations
    }
}
