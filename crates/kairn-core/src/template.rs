//! Plain-markdown note templates. A file at `Notes/@Templates/Daily.md`
//! seeds every daily note that doesn't exist on disk yet: the pane renders
//! the template immediately, and the first edit writes it to the file.
//! NotePlan-style `---` frontmatter is stripped so existing NotePlan
//! template files work as-is.

use std::fs;
use std::path::Path;

/// The daily-note template body: `Notes/@Templates/Daily.md` (or `.txt`)
/// with any frontmatter stripped. `None` when no template exists or it is
/// effectively empty.
pub fn daily_template(root: &Path) -> Option<String> {
    let dir = root.join("Notes").join("@Templates");
    ["Daily.md", "Daily.txt"]
        .iter()
        .find_map(|name| fs::read_to_string(dir.join(name)).ok())
        .map(|text| strip_frontmatter(&text).to_string())
        .filter(|body| !body.trim().is_empty())
}

/// Everything after a leading `---` frontmatter block, with the blank lines
/// that separated it from the body dropped. Text without frontmatter (or
/// with an unclosed block, which is really a rule) passes through unchanged.
pub fn strip_frontmatter(text: &str) -> &str {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return text;
    };
    if first.trim_end() != "---" {
        return text;
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            return text[offset..].trim_start_matches(['\r', '\n']);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    #[test]
    fn frontmatter_is_stripped() {
        assert_eq!(
            strip_frontmatter("---\ntitle: Daily\ntype: empty-note\n---\n\n## Tasks\n* \n"),
            "## Tasks\n* \n"
        );
        // No frontmatter: unchanged.
        assert_eq!(strip_frontmatter("## Tasks\n* \n"), "## Tasks\n* \n");
        // A leading rule with no closing marker is content, not frontmatter.
        assert_eq!(strip_frontmatter("---\njust text\n"), "---\njust text\n");
        // CRLF frontmatter.
        assert_eq!(strip_frontmatter("---\r\ntitle: x\r\n---\r\nbody\r\n"), "body\r\n");
    }

    #[test]
    fn daily_template_reads_and_filters() {
        let root = ScratchRoot::new("template");
        assert_eq!(daily_template(&root.0), None);
        root.write(
            "Notes/@Templates/Daily.md",
            "---\ntitle: Daily\n---\n### Today\n+ \n",
        );
        assert_eq!(daily_template(&root.0).as_deref(), Some("### Today\n+ \n"));
        // A template that is only frontmatter seeds nothing.
        let empty = ScratchRoot::new("template-empty");
        empty.write("Notes/@Templates/Daily.md", "---\ntitle: Daily\n---\n\n");
        assert_eq!(daily_template(&empty.0), None);
    }
}
