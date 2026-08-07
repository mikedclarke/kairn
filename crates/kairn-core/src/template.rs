//! Plain-markdown note templates. A file at `Notes/@Templates/Daily.md`
//! seeds every daily note that doesn't exist on disk yet: the pane renders
//! the template immediately, and the first edit writes it to the file.
//! NotePlan-style `---` frontmatter is stripped so existing NotePlan
//! template files work as-is.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Weekday};

/// Whether the daily template seeds a given day under a rule: "always"
/// (any unknown value reads as this), "weekdays" (Mon–Fri only), or "off".
pub fn template_applies(rule: &str, date: NaiveDate) -> bool {
    match rule {
        "off" => false,
        "weekdays" => !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
        _ => true,
    }
}

/// The daily template file: whichever of `Daily.md`/`Daily.txt` exists,
/// or the `.md` path for a template not written yet.
pub fn daily_template_path(root: &Path) -> PathBuf {
    let dir = root.join("Notes").join("@Templates");
    ["Daily.md", "Daily.txt"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.exists())
        .unwrap_or_else(|| dir.join("Daily.md"))
}

/// The template body as an editor should show it: frontmatter stripped but
/// no empty-body filtering, so an empty template round-trips as empty.
pub fn daily_template_body(root: &Path) -> String {
    fs::read_to_string(daily_template_path(root))
        .map(|text| strip_frontmatter(&text).to_string())
        .unwrap_or_default()
}

/// Write an edited template body back, keeping any frontmatter the file
/// already carries: NotePlan reads it, so an in-app body edit must never
/// destroy it.
pub fn save_daily_template(root: &Path, body: &str) -> std::io::Result<()> {
    let path = daily_template_path(root);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let frontmatter = &existing[..existing.len() - strip_frontmatter(&existing).len()];
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, format!("{frontmatter}{body}"))
}

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
    fn rules_gate_by_weekday() {
        let mon = NaiveDate::from_ymd_opt(2026, 8, 3).expect("date");
        let sat = NaiveDate::from_ymd_opt(2026, 8, 8).expect("date");
        assert!(template_applies("always", mon));
        assert!(template_applies("always", sat));
        assert!(template_applies("weekdays", mon));
        assert!(!template_applies("weekdays", sat));
        assert!(!template_applies("off", mon));
        // Unknown rules read as "always", like the settings defaults.
        assert!(template_applies("", sat));
    }

    #[test]
    fn saving_a_body_keeps_frontmatter() {
        let root = ScratchRoot::new("template-save");
        // No file yet: the save creates Daily.md with just the body.
        save_daily_template(&root.0, "## Tasks\n* \n").expect("save");
        assert_eq!(
            std::fs::read_to_string(root.0.join("Notes/@Templates/Daily.md")).expect("read"),
            "## Tasks\n* \n"
        );
        assert_eq!(daily_template_body(&root.0), "## Tasks\n* \n");

        // A NotePlan-style file with frontmatter keeps it across body edits.
        root.write(
            "Notes/@Templates/Daily.md",
            "---\ntitle: Daily\ntype: empty-note\n---\n### Today\n",
        );
        save_daily_template(&root.0, "### Later\n+ \n").expect("save");
        assert_eq!(
            std::fs::read_to_string(root.0.join("Notes/@Templates/Daily.md")).expect("read"),
            "---\ntitle: Daily\ntype: empty-note\n---\n### Later\n+ \n"
        );
        assert_eq!(daily_template_body(&root.0), "### Later\n+ \n");
    }

    #[test]
    fn txt_template_is_edited_in_place() {
        let root = ScratchRoot::new("template-txt");
        root.write("Notes/@Templates/Daily.txt", "old\n");
        save_daily_template(&root.0, "new\n").expect("save");
        // The existing .txt is the template; no .md fork appears beside it.
        assert!(!root.0.join("Notes/@Templates/Daily.md").exists());
        assert_eq!(
            std::fs::read_to_string(root.0.join("Notes/@Templates/Daily.txt")).expect("read"),
            "new\n"
        );
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
