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
    merge_with(base, disk, ours, false)
}

/// [`merge3`], except where both sides only inserted lines at the same point
/// (a collided region whose base side is empty): there the result is disk's
/// inserted lines followed by ours', leaving out any of our lines that are
/// byte-identical to a line disk inserted there, and keeping our blank lines
/// only between our kept non-blank lines. Two insertions are never a conflict.
/// A region where either side changed or removed base lines resolves exactly
/// as in [`merge3`] (disk wins, ours reported).
///
/// This is for paths where both sides adding lines is the normal case (two
/// devices writing into one daily note), not for the desktop editor's saves.
pub fn merge3_keep_both(base: &str, disk: &str, ours: &str) -> Merged {
    merge_with(base, disk, ours, true)
}

fn merge_with(base: &str, disk: &str, ours: &str, keep_both: bool) -> Merged {
    if disk == base || disk == ours {
        return Merged { text: ours.to_string(), conflicts: Vec::new() };
    }
    if ours == base {
        return Merged { text: disk.to_string(), conflicts: Vec::new() };
    }

    // A terminal newline is not a blank line competing with an EOF insertion.
    let b: Vec<&str> = base.split_terminator('\n').collect();
    let d: Vec<&str> = disk.split_terminator('\n').collect();
    let o: Vec<&str> = ours.split_terminator('\n').collect();

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
        resolve(&b[bi..ai], &d[dj..aj], &o[ok..ak], keep_both, &mut out, &mut conflicts);
        out.push(b[ai]);
        (bi, dj, ok) = (ai + 1, aj + 1, ak + 1);
    }
    resolve(&b[bi..], &d[dj..], &o[ok..], keep_both, &mut out, &mut conflicts);

    let trailing_newline = if ours.ends_with('\n') == base.ends_with('\n') {
        disk.ends_with('\n')
    } else {
        ours.ends_with('\n')
    };
    let mut text = out.join("\n");
    if !out.is_empty() && trailing_newline {
        text.push('\n');
    }
    Merged { text, conflicts }
}

/// Resolve one hunk (a region between anchors) into the output.
fn resolve<'a>(
    base: &[&'a str],
    disk: &[&'a str],
    ours: &[&'a str],
    keep_both: bool,
    out: &mut Vec<&'a str>,
    conflicts: &mut Vec<String>,
) {
    if disk == base || disk == ours {
        out.extend_from_slice(ours);
    } else if ours == base {
        out.extend_from_slice(disk);
    } else if base.is_empty() && keep_both {
        out.extend_from_slice(disk);
        // Our lines disk already inserted here are not typed twice; our blank
        // lines survive only as spacing between lines we actually keep.
        let kept: Vec<bool> = ours.iter().map(|l| !is_blank(l) && !disk.contains(l)).collect();
        if let (Some(first), Some(last)) =
            (kept.iter().position(|k| *k), kept.iter().rposition(|k| *k))
        {
            for (i, line) in ours.iter().enumerate().take(last + 1).skip(first) {
                if kept[i] || is_blank(line) {
                    out.push(line);
                }
            }
        }
    } else if base.is_empty() {
        out.extend_from_slice(disk);
        out.extend_from_slice(ours);
    } else {
        out.extend_from_slice(disk);
        // Our side of the collision is preserved for the caller; a hunk of
        // nothing but empty lines carries no typed content worth surfacing.
        if ours.iter().any(|l| !l.is_empty()) {
            conflicts.push(ours.join("\n"));
        }
    }
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Slot the lines a user typed over a placeholder note into the real note that
/// has since arrived, instead of merging two unrelated versions of the file.
///
/// `seed` is the placeholder the editor showed (a blank day's template),
/// `edited` what the user turned it into, and `disk` the real note. The user's
/// lines are those a line diff from `seed` to `edited` shows as inserted, plus
/// the new form of any seed line they changed; seed lines they deleted are
/// ignored. Each run of those lines goes to the end of the section on `disk`
/// whose heading matches the nearest seed heading above it in `edited` (after
/// trimming and dropping `==` highlight marks): after the section's last
/// non-blank line, before the next heading of any level. Lines with no such
/// heading, or whose heading `disk` lacks, are appended after one blank line.
/// `disk`'s line endings and trailing newline are kept. An untouched or empty
/// `edited` returns `disk` as it is.
pub fn adopt_seeded(seed: &str, edited: &str, disk: &str) -> String {
    if edited == seed || edited.is_empty() {
        return disk.to_string();
    }
    let seed = seed.replace("\r\n", "\n");
    let edited = edited.replace("\r\n", "\n");
    let s: Vec<&str> = seed.split_terminator('\n').collect();
    let e: Vec<&str> = edited.split_terminator('\n').collect();
    let mut from_seed = vec![false; e.len()];
    for (_, j) in lcs_pairs(&s, &e) {
        from_seed[j] = true;
    }

    // Runs of the user's lines, each under the nearest seed heading above it.
    let mut groups: Vec<(Option<&str>, Vec<&str>)> = vec![(None, Vec::new())];
    for (line, seeded) in e.iter().zip(&from_seed) {
        if !seeded {
            groups.last_mut().expect("one group always").1.push(line);
        } else if is_heading(line) {
            groups.push((Some(line), Vec::new()));
        }
    }

    let crlf = disk.contains("\r\n");
    let normalized = disk.replace("\r\n", "\n");
    let trailing_newline = normalized.ends_with('\n');
    let mut lines: Vec<&str> = normalized.split_terminator('\n').collect();
    let mut changed = false;
    for (heading, group) in &groups {
        let Some(first) = group.iter().position(|l| !is_blank(l)) else {
            continue;
        };
        let last = group.iter().rposition(|l| !is_blank(l)).expect("a non-blank line");
        // A line the note already holds is not added again, so adopting the
        // same typed text twice (a stale draft restored after its lines had
        // landed) cannot double it.
        let group: Vec<&str> = group[first..=last]
            .iter()
            .copied()
            .filter(|l| is_blank(l) || !lines.contains(l))
            .collect();
        let Some(first) = group.iter().position(|l| !is_blank(l)) else {
            continue;
        };
        let last = group.iter().rposition(|l| !is_blank(l)).expect("a non-blank line");
        let group = &group[first..=last];
        changed = true;
        match heading.and_then(|h| section_end(&lines, h)) {
            Some(at) => {
                lines.splice(at..at, group.iter().copied());
            }
            None => {
                if lines.last().is_some_and(|l| !is_blank(l)) {
                    lines.push("");
                }
                lines.extend_from_slice(group);
            }
        }
    }
    if !changed {
        return disk.to_string();
    }
    let ending = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(ending);
    if trailing_newline && !lines.is_empty() {
        text.push_str(ending);
    }
    text
}

/// A heading line: one or more `#` then a space.
fn is_heading(line: &str) -> bool {
    let rest = line.trim_start_matches('#');
    rest.len() < line.len() && rest.starts_with(' ')
}

/// A heading's identity for matching across versions: trimmed, with `==`
/// highlight marks removed.
fn heading_key(line: &str) -> String {
    line.trim().replace("==", "").trim().to_string()
}

/// Where lines appended to the section headed `heading` go in `lines`: after
/// the section's last non-blank line, before the next heading of any level or
/// the end. `None` when `lines` has no such heading.
fn section_end(lines: &[&str], heading: &str) -> Option<usize> {
    let key = heading_key(heading);
    let start = lines.iter().position(|l| is_heading(l) && heading_key(l) == key)?;
    let next = lines[start + 1..]
        .iter()
        .position(|l| is_heading(l))
        .map_or(lines.len(), |i| start + 1 + i);
    Some(
        lines[start + 1..next]
            .iter()
            .rposition(|l| !is_blank(l))
            .map_or(start + 1, |i| start + 2 + i),
    )
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
    fn simultaneous_insertions_keep_both_in_order() {
        for (base, disk, ours, expected) in [
            ("a\nb\n", "a\ndisk\nb\n", "a\nours\nb\n", "a\ndisk\nours\nb\n"),
            ("a\n", "disk\na\n", "ours\na\n", "disk\nours\na\n"),
            ("a\n", "a\ndisk\n", "a\nours\n", "a\ndisk\nours\n"),
            ("a\n", "a\ndisk\n", "a\nours", "a\ndisk\nours"),
            ("a", "a\ndisk", "a\nours", "a\ndisk\nours"),
            ("", "disk\n", "ours", "disk\nours\n"),
            ("a\n", "a\nsame\n", "a\nsame", "a\nsame"),
            ("a\n\n", "a\ndisk\n\n", "a\nours\n\n", "a\ndisk\nours\n\n"),
        ] {
            let merged = merge3(base, disk, ours);
            assert_eq!(merged.text, expected, "base: {base:?}");
            assert!(merged.conflicts.is_empty(), "base: {base:?}");
        }
    }

    #[test]
    fn newline_edits_merge_without_losing_blank_lines() {
        for (base, disk, ours, expected) in [
            ("a\nb\n", "A\nb\n", "a\nb", "A\nb"),
            ("a\nb", "A\nb", "a\nb\n", "A\nb\n"),
            ("a\nkeep\nb\n\n", "A\nkeep\nb\n\n", "a\nkeep\nB\n\n", "A\nkeep\nB\n\n"),
            ("a\n", "", "A\n", ""),
        ] {
            assert_eq!(merge3(base, disk, ours).text, expected);
        }
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

    #[test]
    fn adopting_the_same_typed_lines_twice_adds_them_once() {
        let seed = "### Todays Tasks\n\n### Daily Notes\n\n";
        let edited = "### Todays Tasks\n\n### Daily Notes\nRang the plumber\nBoiler part is ordered\n";
        let disk = "### Todays Tasks\n* Post the letter\n\n### Daily Notes\n\n### Desk\n- Waiting on you: 0\n";
        let once = adopt_seeded(seed, edited, disk);
        assert!(once.contains("### Daily Notes\nRang the plumber\nBoiler part is ordered\n"), "{once}");
        assert_eq!(adopt_seeded(seed, edited, &once), once);
    }

    // ---- merge3_keep_both --------------------------------------------------

    #[test]
    fn keep_both_keeps_two_insertions_at_one_point_disk_first_without_duplicates() {
        let base = "### Notes\nfirst\n\n### Desk\n";
        let disk = "### Notes\nfirst\nfrom disk\nshared line\n\n### Desk\n";
        let ours = "### Notes\nfirst\nshared line\nfrom ours\n\n### Desk\n";
        let m = merge3_keep_both(base, disk, ours);
        assert_eq!(m.text, "### Notes\nfirst\nfrom disk\nshared line\nfrom ours\n\n### Desk\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn keep_both_keeps_our_blank_lines_only_between_our_kept_lines() {
        let base = "a\n";
        let disk = "a\nd\n";
        let ours = "a\n\no1\n\no2\n\n";
        let m = merge3_keep_both(base, disk, ours);
        assert_eq!(m.text, "a\nd\no1\n\no2\n");
        assert!(m.conflicts.is_empty());
        // Nothing of ours but blank lines and lines disk already has: disk alone.
        let m = merge3_keep_both("a\n", "a\nd\n", "a\n\nd\n");
        assert_eq!(m.text, "a\nd\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn keep_both_on_an_empty_base_is_disk_then_our_new_lines() {
        let disk = "### Todays Tasks\n* Water the plants\n\n### Daily Notes\n";
        let ours = "### Daily Notes\nPicked up the parcel\n";
        let m = merge3_keep_both("", disk, ours);
        assert_eq!(m.text, "### Todays Tasks\n* Water the plants\n\n### Daily Notes\nPicked up the parcel\n");
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn keep_both_leaves_a_same_line_collision_exactly_as_merge3() {
        for (base, disk, ours) in [
            ("x\n", "x disk\n", "x ours\n"),
            ("keep\ndoomed\nkeep2\n", "keep\nkeep2\n", "keep\ndoomed but edited\nkeep2\n"),
            ("a\ngone\nb\n", "a\ngone edited\nb\n", "a\nb\n"),
        ] {
            let plain = merge3(base, disk, ours);
            let both = merge3_keep_both(base, disk, ours);
            assert_eq!(both.text, plain.text, "{base:?}");
            assert_eq!(both.conflicts, plain.conflicts, "{base:?}");
        }
        let m = merge3_keep_both("x\n", "x disk\n", "x ours\n");
        assert_eq!(m.text, "x disk\n");
        assert_eq!(m.conflicts, vec!["x ours".to_string()]);
    }

    #[test]
    fn keep_both_leaves_adjacent_non_colliding_edits_exactly_as_merge3() {
        for (base, disk, ours) in [
            ("top\nmiddle\nbottom\n", "top\nmiddle\nbottom\n* new agent task\n", "top edited\nmiddle\nbottom\n"),
            ("a\nb\nc\n", "A\nb\nc\n", "a\nB\nc\n"),
            ("a\nb\n", "a\nB\n", "a\nb\n"),
        ] {
            let plain = merge3(base, disk, ours);
            let both = merge3_keep_both(base, disk, ours);
            assert_eq!(both.text, plain.text, "{base:?}");
            assert_eq!(both.conflicts, plain.conflicts, "{base:?}");
        }
        // Edits to neighbouring lines that do not touch the same line merge.
        let m = merge3_keep_both("a
b
c
d
", "A
b
c
d
", "a
b
c
D
");
        assert_eq!(m.text, "A
b
c
D
");
        assert!(m.conflicts.is_empty());
    }

    // ---- adopt_seeded ------------------------------------------------------

    const SEED: &str = "### Todays Tasks\n\n### Daily Notes\n\n";

    #[test]
    fn typed_lines_on_a_blank_day_land_at_the_end_of_their_section() {
        let edited = "### Todays Tasks\n\n### Daily Notes\n* Call the plumber about the leak\n* Buy stamps\n\n";
        let disk = "### Todays Tasks\n* Water the plants\n\n### Daily Notes\nMorning walk by the river\n\n### Desk\n- Sorted the post\n- Reviewed the garden plan\n";
        assert_eq!(
            adopt_seeded(SEED, edited, disk),
            "### Todays Tasks\n* Water the plants\n\n### Daily Notes\nMorning walk by the river\n* Call the plumber about the leak\n* Buy stamps\n\n### Desk\n- Sorted the post\n- Reviewed the garden plan\n"
        );
    }

    #[test]
    fn a_highlighted_heading_on_disk_still_matches() {
        let edited = "### Todays Tasks\n\n### Daily Notes\n* Buy stamps\n\n";
        let disk = "### Todays Tasks\n\n### ==Daily Notes==\nMorning walk\n\n### Desk\n- Sorted the post\n";
        assert_eq!(
            adopt_seeded(SEED, edited, disk),
            "### Todays Tasks\n\n### ==Daily Notes==\nMorning walk\n* Buy stamps\n\n### Desk\n- Sorted the post\n"
        );
    }

    #[test]
    fn a_heading_missing_on_disk_appends_after_one_blank_line() {
        let edited = "### Todays Tasks\n\n### Daily Notes\n* Buy stamps\n\n";
        let disk = "### Todays Tasks\n* Water the plants\n\n### Desk\n- Sorted the post\n";
        assert_eq!(
            adopt_seeded(SEED, edited, disk),
            "### Todays Tasks\n* Water the plants\n\n### Desk\n- Sorted the post\n\n* Buy stamps\n"
        );
    }

    #[test]
    fn an_untouched_or_empty_seed_returns_disk_unchanged() {
        let disk = "### Todays Tasks\r\n* Water the plants\r\n";
        assert_eq!(adopt_seeded(SEED, SEED, disk), disk);
        assert_eq!(adopt_seeded(SEED, "", disk), disk);
    }

    #[test]
    fn adoption_keeps_disk_line_endings_and_trailing_newline_state() {
        let edited = "### Todays Tasks\n* Water the plants\n\n### Daily Notes\n\n";
        let disk = "### Todays Tasks\r\nMorning walk\r\n\r\n### Desk";
        assert_eq!(
            adopt_seeded(SEED, edited, disk),
            "### Todays Tasks\r\nMorning walk\r\n* Water the plants\r\n\r\n### Desk"
        );
        // Lines typed above the first heading form a group with no heading.
        let edited = "Before anything\n### Todays Tasks\n\n### Daily Notes\n\n";
        assert_eq!(
            adopt_seeded(SEED, edited, "### Desk\n- Sorted the post\n"),
            "### Desk\n- Sorted the post\n\nBefore anything\n"
        );
        // A seed line the user changed counts as theirs; one they deleted is ignored.
        let edited = "### Todays Tasks\n\n### Daily Notes (call day)\n";
        assert_eq!(
            adopt_seeded(SEED, edited, "### Todays Tasks\n* Water the plants\n"),
            "### Todays Tasks\n* Water the plants\n### Daily Notes (call day)\n"
        );
    }
}
