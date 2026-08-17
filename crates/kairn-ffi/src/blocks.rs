//! Block structure over a note's lines: the indent-run a dragged line
//! carries with it, and the heading list a note offers as drop sections.
//! The rules live in [`kairn_core::blocks`]. Offsets are UTF-8 bytes, per
//! the crate contract.

/// A byte range over a note's text, end-exclusive.
#[derive(uniffi::Record)]
pub struct FfiByteRange {
    pub start: u64,
    pub end: u64,
}

/// Byte range of the draggable block starting at the line containing `at`:
/// the line itself plus the contiguous run of following lines indented
/// strictly deeper (its subtasks and notes). A heading is always a block of
/// one. The end excludes the final newline.
#[uniffi::export]
pub fn block_range(text: String, at: u64) -> FfiByteRange {
    let range = kairn_core::block_range(&text, at as usize);
    FfiByteRange { start: range.start as u64, end: range.end as u64 }
}

/// A heading of a note, addressed for section-targeted drops. `text` is the
/// display text, markdown syntax stripped.
#[derive(uniffi::Record)]
pub struct FfiHeadingRef {
    pub level: u8,
    pub text: String,
    pub line_idx: u64,
}

/// Every heading in `text`, in order.
#[uniffi::export]
pub fn note_headings(text: String) -> Vec<FfiHeadingRef> {
    kairn_core::note_headings(&text)
        .into_iter()
        .map(|h| FfiHeadingRef {
            level: h.level,
            text: h.text,
            line_idx: h.line_idx as u64,
        })
        .collect()
}

/// The line index where an addition lands to sit at the end of the section
/// whose heading is at `heading_line_idx`: after the section's last content
/// line, before trailing blanks or rules. `None` when the line isn't a
/// heading.
#[uniffi::export]
pub fn section_insert_line(text: String, heading_line_idx: u64) -> Option<u64> {
    kairn_core::section_insert_line(&text, heading_line_idx as usize).map(|i| i as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_carries_deeper_indented_lines() {
        let text = "* parent\n    * child\nnext";
        let range = block_range(text.into(), 0);
        assert_eq!(&text[range.start as usize..range.end as usize], "* parent\n    * child");
    }

    #[test]
    fn headings_and_section_insert() {
        let text = "# One\nbody\n\n# Two\nmore\n";
        let heads = note_headings(text.into());
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[1].text, "Two");
        assert_eq!(heads[1].line_idx, 3);
        // End of section One's content: after "body", before the blank.
        assert_eq!(section_insert_line(text.into(), 0), Some(2));
        assert_eq!(section_insert_line(text.into(), 1), None);
    }
}
