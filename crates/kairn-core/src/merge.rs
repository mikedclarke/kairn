//! Line-based three-way merge: how buffered edits and disk changes combine
//! without either side being clobbered. Deliberately line-granular: a
//! same-line collision is surfaced as a conflict rather than character-merged,
//! because an honest banner beats a silently wrong splice.

/// The outcome of a three-way merge. `conflicts` holds the local side of any
/// hunk that collided with a disk change (the disk side is already in
/// `text`); callers surface these so nothing typed is silently dropped.
pub struct Merged {
    pub text: String,
    pub conflicts: Vec<String>,
}

/// Merge two descendants of `base`: `disk` (what the file holds now) and
/// `ours` (the edited buffer). Non-conflicting hunks combine; where both
/// sides changed the same region differently, the disk side wins and our
/// side is returned as a conflict. Line endings are the caller's problem:
/// this operates on `\n` only.
pub fn merge3(base: &str, disk: &str, ours: &str) -> Merged {
    if disk == base || disk == ours {
        return Merged { text: ours.to_string(), conflicts: Vec::new() };
    }
    if ours == base {
        return Merged { text: disk.to_string(), conflicts: Vec::new() };
    }

    // `split` (not `lines`) so a trailing newline survives the round-trip.
    let b: Vec<&str> = base.split('\n').collect();
    let d: Vec<&str> = disk.split('\n').collect();
    let o: Vec<&str> = ours.split('\n').collect();

    let mut to_disk: Vec<Option<usize>> = vec![None; b.len()];
    for (i, j) in lcs_pairs(&b, &d) {
        to_disk[i] = Some(j);
    }
    let mut to_ours: Vec<Option<usize>> = vec![None; b.len()];
    for (i, k) in lcs_pairs(&b, &o) {
        to_ours[i] = Some(k);
    }

    let mut out: Vec<&str> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();
    let (mut bi, mut dj, mut ok) = (0usize, 0usize, 0usize);

    // Anchors are base lines still present on both sides. Each side's LCS is
    // monotone, so walking anchors in base order keeps all three pointers
    // advancing; the regions between anchors are the hunks to resolve.
    let anchors: Vec<(usize, usize, usize)> = (0..b.len())
        .filter_map(|i| Some((i, to_disk[i]?, to_ours[i]?)))
        .collect();

    for (ai, aj, ak) in anchors {
        resolve(&b[bi..ai], &d[dj..aj], &o[ok..ak], &mut out, &mut conflicts);
        out.push(b[ai]);
        (bi, dj, ok) = (ai + 1, aj + 1, ak + 1);
    }
    resolve(&b[bi..], &d[dj..], &o[ok..], &mut out, &mut conflicts);

    Merged { text: out.join("\n"), conflicts }
}

/// Resolve one hunk (a region between anchors) into the output.
fn resolve<'a>(
    base: &[&'a str],
    disk: &[&'a str],
    ours: &[&'a str],
    out: &mut Vec<&'a str>,
    conflicts: &mut Vec<String>,
) {
    if disk == base || disk == ours {
        out.extend_from_slice(ours);
    } else if ours == base {
        out.extend_from_slice(disk);
    } else {
        out.extend_from_slice(disk);
        // Our side of the collision is preserved for the caller; a hunk of
        // nothing but empty lines carries no typed content worth surfacing.
        if ours.iter().any(|l| !l.is_empty()) {
            conflicts.push(ours.join("\n"));
        }
    }
}

/// Matched line pairs of a longest common subsequence of `a` and `b`,
/// as `(index_in_a, index_in_b)`, ascending in both coordinates.
fn lcs_pairs(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    // Trim the common prefix/suffix first: notes usually differ in one small
    // region, and this keeps the DP table tiny in the common case.
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start && end < b.len() - start
        && a[a.len() - 1 - end] == b[b.len() - 1 - end]
    {
        end += 1;
    }
    let (am, bm) = (&a[start..a.len() - end], &b[start..b.len() - end]);

    let mut pairs: Vec<(usize, usize)> = (0..start).map(|i| (i, i)).collect();

    // Guard rail, not a tuning knob: a pathological pair of huge, totally
    // rewritten files skips the DP and treats the whole middle as one hunk,
    // which merges coarsely (likely one big conflict) but never wrongly.
    const MAX_CELLS: usize = 4_000_000;
    if !am.is_empty() && !bm.is_empty() && am.len() * bm.len() <= MAX_CELLS {
        let cols = bm.len() + 1;
        let mut table = vec![0u32; (am.len() + 1) * cols];
        for i in (0..am.len()).rev() {
            for j in (0..bm.len()).rev() {
                table[i * cols + j] = if am[i] == bm[j] {
                    table[(i + 1) * cols + j + 1] + 1
                } else {
                    table[(i + 1) * cols + j].max(table[i * cols + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < am.len() && j < bm.len() {
            if am[i] == bm[j] {
                pairs.push((start + i, start + j));
                (i, j) = (i + 1, j + 1);
            } else if table[(i + 1) * cols + j] >= table[i * cols + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }

    pairs.extend((0..end).rev().map(|k| (a.len() - 1 - k, b.len() - 1 - k)));
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_unchanged_keeps_ours() {
        let m = merge3("a\nb\n", "a\nb\n", "a\nB\n");
        assert_eq!(m.text, "a\nB\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn ours_unchanged_takes_disk() {
        let m = merge3("a\nb\n", "a\nB\n", "a\nb\n");
        assert_eq!(m.text, "a\nB\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn disjoint_edits_combine() {
        // Agent appends at the bottom while we edit the top.
        let base = "top\nmiddle\nbottom\n";
        let disk = "top\nmiddle\nbottom\n* new agent task\n";
        let ours = "top edited\nmiddle\nbottom\n";
        let m = merge3(base, disk, ours);
        assert_eq!(m.text, "top edited\nmiddle\nbottom\n* new agent task\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn same_line_collision_keeps_disk_and_reports_ours() {
        let m = merge3("x\n", "x disk\n", "x ours\n");
        assert_eq!(m.text, "x disk\n");
        assert_eq!(m.conflicts, vec!["x ours".to_string()]);
    }

    #[test]
    fn identical_change_on_both_sides_is_no_conflict() {
        let m = merge3("a\n", "a\nsame new line\n", "a\nsame new line\n");
        assert_eq!(m.text, "a\nsame new line\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn edit_under_external_delete_reports_ours() {
        // Disk deleted the line we were editing: nothing of it remains in
        // the merge, but the typed version comes back as a conflict.
        let base = "keep\ndoomed\nkeep2\n";
        let disk = "keep\nkeep2\n";
        let ours = "keep\ndoomed but edited\nkeep2\n";
        let m = merge3(base, disk, ours);
        assert_eq!(m.text, "keep\nkeep2\n");
        assert_eq!(m.conflicts, vec!["doomed but edited".to_string()]);
    }

    #[test]
    fn our_delete_of_externally_changed_lines_is_silent() {
        // We deleted, disk edited: disk wins, and there is no typed text to
        // surface, so no conflict banner.
        let base = "a\ngone\nb\n";
        let disk = "a\ngone edited\nb\n";
        let ours = "a\nb\n";
        let m = merge3(base, disk, ours);
        assert_eq!(m.text, "a\ngone edited\nb\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn trailing_newline_round_trips() {
        let m = merge3("a", "a", "a\nb");
        assert_eq!(m.text, "a\nb");
        let m = merge3("a\n", "a\n", "a\n");
        assert_eq!(m.text, "a\n");
    }
}
