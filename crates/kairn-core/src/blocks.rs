//! Block structure over a note's lines: the indent-run a dragged line
//! carries with it, and the heading list a note offers as drop sections.

use std::ops::Range;

use crate::parse::{parse_line, Line, SpanKind};

/// Visual indent width of a line's leading whitespace: a tab counts 4, a
/// space 1, matching how mixed-indent NotePlan notes read.
pub fn line_indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Byte range of the draggable block starting at the line containing `at`:
/// the line itself plus the contiguous run of following lines indented
/// strictly deeper (its subtasks and notes). Blank lines are absorbed only
/// when a deeper-indented line follows them, so a block's interior gaps
/// travel with it while trailing blanks stay behind. A heading is always a
/// block of one. The end excludes the final newline.
pub fn block_range(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let first_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let first = &text[start..first_end];
    if matches!(parse_line(first), Line::Heading { .. } | Line::Blank) {
        return start..first_end;
    }
    let depth = line_indent_width(first);

    // Blank lines never extend `end` themselves; a deeper line beyond them
    // does, which is what lets interior gaps travel while trailing blanks
    // stay behind.
    let mut end = first_end;
    let mut rest = first_end;
    while rest < text.len() {
        let line_start = rest + 1;
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let line = &text[line_start..line_end];
        rest = line_end;
        if line.trim().is_empty() {
            continue;
        }
        if line_indent_width(line) <= depth || matches!(parse_line(line), Line::Heading { .. }) {
            break;
        }
        end = line_end;
    }
    start..end
}

/// The draggable blocks a text selection touches: for each selected line
/// not already inside an earlier block, its [`block_range`] (so children
/// travel with their parent even when the selection ends above them).
/// Blank lines are skipped; the result is non-overlapping, in document
/// order. A caret (empty selection) or a selection over nothing but blanks
/// yields nothing.
pub fn selection_block_ranges(text: &str, sel: Range<usize>) -> Vec<Range<usize>> {
    let start = sel.start.min(sel.end).min(text.len());
    let end = sel.start.max(sel.end).min(text.len());
    if start == end {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    // A selection ending exactly at a line start (triple-click style) does
    // not include that line.
    while line_start < end {
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        if text[line_start..line_end].trim().is_empty() {
            line_start = line_end + 1;
            continue;
        }
        let block = block_range(text, line_start);
        line_start = block.end + 1;
        ranges.push(block);
    }
    ranges
}

/// A heading of a note, addressed for section-targeted drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingRef {
    pub level: u8,
    /// Display text: markdown syntax stripped, as the heading renders.
    pub text: String,
    pub line_idx: usize,
}

/// Where an addition lands to sit at the end of the section whose heading
/// is at line `start` with `level`: after the section's last content line,
/// before any trailing blank lines or `---` rules (those belong to the
/// boundary, not the content). The section runs to the next heading of the
/// same or higher level.
pub fn section_end_line(lines: &[&str], start: usize, level: u8) -> usize {
    let level_of = |line: &str| match parse_line(line) {
        Line::Heading { level, .. } => Some(level),
        _ => None,
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(i, l)| level_of(l).filter(|lv| *lv <= level).map(|_| i))
        .unwrap_or(lines.len());
    let is_content = |i: usize| !matches!(parse_line(lines[i]), Line::Blank | Line::Rule);
    let last_content = (start..end).rev().find(|&i| is_content(i));
    last_content.map_or(end, |i| i + 1)
}

/// [`section_end_line`] addressed by heading line index over raw text:
/// `None` when the line isn't a heading.
pub fn section_insert_line(text: &str, heading_line_idx: usize) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let Line::Heading { level, .. } = parse_line(lines.get(heading_line_idx)?) else {
        return None;
    };
    Some(section_end_line(&lines, heading_line_idx, level))
}

/// Every heading in `text`, in order.
pub fn note_headings(text: &str) -> Vec<HeadingRef> {
    text.lines()
        .enumerate()
        .filter_map(|(line_idx, line)| match parse_line(line) {
            Line::Heading { level, spans } => Some(HeadingRef {
                level,
                text: spans
                    .iter()
                    .filter(|(kind, _)| *kind != SpanKind::Hidden)
                    .map(|(_, s)| s.as_str())
                    .collect::<String>()
                    .trim()
                    .to_string(),
                line_idx,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indent_width_counts_tabs_as_four() {
        assert_eq!(line_indent_width("plain"), 0);
        assert_eq!(line_indent_width("  two"), 2);
        assert_eq!(line_indent_width("\tone tab"), 4);
        assert_eq!(line_indent_width("\t  mixed"), 6);
    }

    #[test]
    fn block_is_line_plus_deeper_run() {
        let text = "* task\n\tsub one\n\t* sub two\nnext\n";
        let range = block_range(text, 0);
        assert_eq!(&text[range], "* task\n\tsub one\n\t* sub two");
    }

    #[test]
    fn block_with_spaces_and_tabs_mixed() {
        let text = "* task\n  spaced child\n\ttabbed child\nnext";
        let range = block_range(text, 2);
        assert_eq!(&text[range], "* task\n  spaced child\n\ttabbed child");
    }

    #[test]
    fn interior_blank_travels_trailing_blank_stays() {
        let text = "* task\n\ta\n\n\tb\n\nafter\n";
        let range = block_range(text, 0);
        assert_eq!(&text[range], "* task\n\ta\n\n\tb");
    }

    #[test]
    fn heading_is_a_block_of_one() {
        let text = "## Section\n\tindented under heading\n";
        let range = block_range(text, 3);
        assert_eq!(&text[range], "## Section");
    }

    #[test]
    fn deeper_child_heading_ends_the_block() {
        let text = "* task\n\tsub\n\t## odd heading\nnext";
        let range = block_range(text, 0);
        assert_eq!(&text[range], "* task\n\tsub");
    }

    #[test]
    fn block_at_eof_without_trailing_newline() {
        let text = "first\n* task\n\tsub";
        let range = block_range(text, 6);
        assert_eq!(range.end, text.len());
        assert_eq!(&text[range], "* task\n\tsub");
    }

    #[test]
    fn last_line_is_its_own_block() {
        let text = "a\nb\n";
        let range = block_range(text, 2);
        assert_eq!(&text[range], "b");
    }

    #[test]
    fn blank_line_is_a_bare_block() {
        let text = "a\n\nb\n";
        let range = block_range(text, 2);
        assert_eq!(&text[range], "");
    }

    #[test]
    fn sibling_at_same_depth_ends_the_block() {
        let text = "\t* one\n\t\tsub\n\t* two\n";
        let range = block_range(text, 1);
        assert_eq!(&text[range], "\t* one\n\t\tsub");
    }

    #[test]
    fn selection_blocks_split_at_siblings_and_keep_children() {
        let text = "* one\n\tsub\n* two\n* three\nrest\n";
        // Selection from inside "one" to inside "two": both blocks, the
        // child riding with its parent.
        let ranges = selection_block_ranges(text, 2..13);
        let blocks: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(blocks, vec!["* one\n\tsub", "* two"]);
    }

    #[test]
    fn selection_blocks_skip_blank_lines() {
        let text = "* a\n\n* b\n";
        let ranges = selection_block_ranges(text, 0..8);
        let blocks: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(blocks, vec!["* a", "* b"]);
    }

    #[test]
    fn selection_ending_at_a_line_start_excludes_that_line() {
        // Triple-click of "* a" selects through its newline; "* b" stays out.
        let text = "* a\n* b\n";
        let ranges = selection_block_ranges(text, 0..4);
        assert_eq!(ranges, vec![0..3]);
    }

    #[test]
    fn selection_children_extend_past_the_selection_end() {
        let text = "* a\n\tone\n\ttwo\nnext\n";
        // Selection stops inside the parent line; the whole block comes.
        let ranges = selection_block_ranges(text, 0..2);
        assert_eq!(&text[ranges[0].clone()], "* a\n\tone\n\ttwo");
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn caret_or_blank_selection_yields_nothing() {
        let text = "* a\n\n* b\n";
        assert!(selection_block_ranges(text, 2..2).is_empty());
        assert!(selection_block_ranges(text, 4..5).is_empty());
    }

    #[test]
    fn selection_starting_mid_child_takes_sibling_children_separately() {
        let text = "* a\n\tone\n\ttwo\nz\n";
        // From inside "\tone" to the end: the equal-depth children are
        // their own blocks, then "z".
        let ranges = selection_block_ranges(text, 5..15);
        let blocks: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(blocks, vec!["\tone", "\ttwo", "z"]);
    }

    #[test]
    fn section_insert_line_lands_at_section_end() {
        let text = "## A\n* one\n#### Deep\n* deep\n\n---\n## B\nprose\n";
        // Sub-headings don't end the section; trailing blanks and rules do.
        assert_eq!(section_insert_line(text, 0), Some(4));
        // Last section runs to the end.
        assert_eq!(section_insert_line(text, 6), Some(8));
        // Empty section: right under the heading, above the blank boundary.
        assert_eq!(section_insert_line("## A\n\n## B\n", 0), Some(1));
        // Not a heading (or out of range): None.
        assert_eq!(section_insert_line(text, 1), None);
        assert_eq!(section_insert_line(text, 99), None);
    }

    #[test]
    fn headings_listed_with_levels_and_indices() {
        let text = "# Title\nbody\n## ==Tasks==\n* a\n### Deep **bold**\n";
        let heads = note_headings(text);
        assert_eq!(
            heads,
            vec![
                HeadingRef { level: 1, text: "Title".into(), line_idx: 0 },
                HeadingRef { level: 2, text: "Tasks".into(), line_idx: 2 },
                HeadingRef { level: 3, text: "Deep bold".into(), line_idx: 4 },
            ]
        );
    }

    #[test]
    fn carriage_returns_tolerated() {
        let text = "* task\r\n\tsub\r\nnext\r\n";
        let range = block_range(text, 0);
        assert_eq!(&text[range], "* task\r\n\tsub\r");
    }
}
