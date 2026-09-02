//! Planning for `kairn carry`: which open tasks leave which past daily
//! notes, which `**group**` (or `*group*`) headers travel with them, and what the
//! destination note gains. Everything here is pure text logic so the whole
//! plan is testable; main.rs applies it with kairn-core's never-clobber
//! writes.

use chrono::NaiveDate;
use kairn_core::{Line, SpanKind, TaskRef, parse_line, spans_start_col};

/// One task leaving its old day: the source reference, the `**group**`
/// header it sat under (if any), and the line the destination gains — the
/// stale `>date` token stripped, since the task's new home dates it.
pub struct Move {
    pub task: TaskRef,
    pub header: Option<String>,
    pub carried_line: String,
}

/// A line that groups the tasks under it without being a heading:
/// bold or italic text and nothing else, NotePlan's task-group idiom.
/// `**Clients**`, `*Clients*` (NotePlan-flavour bold, which the editor
/// renders as bold too), `__Clients__` and `_Clients_` all count; a task
/// line (`* ...`) never does, and neither does prose that merely starts
/// and ends with a styled span (`*a* and *b*`).
pub fn is_group_header(line: &str) -> bool {
    let t = line.trim();
    for mark in ["**", "__", "*", "_"] {
        if t.len() > 2 * mark.len() && t.starts_with(mark) && t.ends_with(mark) {
            let inner = &t[mark.len()..t.len() - mark.len()];
            let ch = mark.chars().next().expect("marker");
            return !inner.starts_with(char::is_whitespace)
                && !inner.ends_with(char::is_whitespace)
                && !inner.contains(ch);
        }
    }
    false
}

/// The `**group**` header the task at `task_idx` sits under: the nearest
/// one above it, unless a blank line or a real heading closes the group
/// first. Prose between tasks does not break the group.
pub fn group_header(lines: &[&str], task_idx: usize) -> Option<String> {
    for line in lines[..task_idx].iter().rev() {
        if is_group_header(line) {
            return Some((*line).to_string());
        }
        match parse_line(line) {
            Line::Blank | Line::Heading { .. } => return None,
            _ => {}
        }
    }
    None
}

/// The task line without its first `>YYYY-MM-DD` token (and the space that
/// carried it). A carried task's new home dates it; keeping the old token
/// would leave it overdue forever. Lines without a token pass through.
pub fn strip_due_token(line: &str) -> String {
    let Line::Task { spans, .. } = parse_line(line) else {
        return line.to_string();
    };
    let mut offset = spans_start_col(line);
    for (kind, s) in &spans {
        if *kind == SpanKind::DateRef
            && s.strip_prefix('>')
                .is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        {
            let before = &line[..offset];
            let after = &line[offset + s.len()..];
            let before = before.strip_suffix(' ').unwrap_or(before);
            return format!("{before}{after}");
        }
        offset += s.len();
    }
    line.to_string()
}

/// The block the destination note gains: groups in order of first
/// appearance, each header once (identical headers from different source
/// days merge), headerless tasks under no header at all.
pub fn destination_block(moves: &[Move]) -> String {
    let mut groups: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    for m in moves {
        let header = m.header.as_deref();
        match groups.iter_mut().find(|(h, _)| *h == header) {
            Some((_, lines)) => lines.push(&m.carried_line),
            None => groups.push((header, vec![&m.carried_line])),
        }
    }
    let mut out = Vec::new();
    for (header, lines) in groups {
        if let Some(h) = header {
            out.push(h);
        }
        out.extend(lines);
    }
    out.join("\n")
}

/// Whether the group header at `header_idx` has nothing left under it now
/// its tasks are gone: the group ends (blank, heading, another header, or
/// end of file) before any task, bullet, or prose line appears.
pub fn header_now_empty(lines: &[&str], header_idx: usize) -> bool {
    let Some(next) = lines.get(header_idx + 1) else {
        return true;
    };
    is_group_header(next)
        || matches!(parse_line(next), Line::Blank | Line::Heading { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mv(header: Option<&str>, line: &str) -> Move {
        Move {
            task: TaskRef {
                path: PathBuf::new(),
                due: NaiveDate::from_ymd_opt(2026, 8, 10).expect("date"),
                file_date: None,
                line_idx: 0,
                line: line.to_string(),
                spans: Vec::new(),
            },
            header: header.map(str::to_string),
            carried_line: line.to_string(),
        }
    }

    #[test]
    fn headers_found_within_their_group_only() {
        let lines = vec![
            "### Tasks",
            "* loose one",
            "",
            "**Clients**",
            "* first",
            "some context under it",
            "* second",
            "",
            "* loose two",
        ];
        assert_eq!(group_header(&lines, 1), None);
        assert_eq!(group_header(&lines, 4).as_deref(), Some("**Clients**"));
        // Prose between tasks does not break the group.
        assert_eq!(group_header(&lines, 6).as_deref(), Some("**Clients**"));
        // The blank line does.
        assert_eq!(group_header(&lines, 8), None);
    }

    #[test]
    fn italic_and_underscore_headers_count_too() {
        // 2026-09-02: a `*Gerrards Pending Sessions*` header was left behind
        // by the carry while its tasks moved, because only `**bold**` counted.
        assert!(is_group_header("**Clients**"));
        assert!(is_group_header("*Gerrards Pending Sessions*"));
        assert!(is_group_header("__Admin__"));
        assert!(is_group_header("_Admin_"));
        assert!(is_group_header("  *Indented*  "));
        // Tasks and bullets are never headers, whatever they end in.
        assert!(!is_group_header("* task"));
        assert!(!is_group_header("* [ ] fix the *bold* thing*"));
        assert!(!is_group_header("- bullet*"));
        // Styled prose is not a header, and neither is an empty marker pair.
        assert!(!is_group_header("*a* and *b*"));
        assert!(!is_group_header("**"));
        assert!(!is_group_header("*"));
        assert!(!is_group_header("* *"));
        let lines = vec!["*Gerrards Pending Sessions*", "* one", "* two"];
        assert_eq!(
            group_header(&lines, 2).as_deref(),
            Some("*Gerrards Pending Sessions*")
        );
        assert!(header_now_empty(&["*Gerrards Pending Sessions*", ""], 0));
    }

    #[test]
    fn due_tokens_strip_cleanly() {
        assert_eq!(strip_due_token("* pay VAT >2026-08-08"), "* pay VAT");
        assert_eq!(
            strip_due_token("* pay >2026-08-08 then file"),
            "* pay then file"
        );
        // No token: untouched. Non-tasks: untouched.
        assert_eq!(strip_due_token("* just a task"), "* just a task");
        assert_eq!(strip_due_token("plain text >2026-08-08"), "plain text >2026-08-08");
        // A date inside a wiki link is not the token.
        assert_eq!(
            strip_due_token("* see [[log >2026-08-01]] >2026-08-08"),
            "* see [[log >2026-08-01]]"
        );
    }

    #[test]
    fn destination_groups_merge_by_header() {
        let moves = vec![
            mv(None, "* loose"),
            mv(Some("**Clients**"), "* one"),
            mv(None, "* loose two"),
            mv(Some("**Clients**"), "* two"),
            mv(Some("**Admin**"), "* three"),
        ];
        assert_eq!(
            destination_block(&moves),
            "* loose\n* loose two\n**Clients**\n* one\n* two\n**Admin**\n* three"
        );
    }

    #[test]
    fn emptied_headers_detected() {
        let lines = vec!["**Clients**", "", "* task later"];
        assert!(header_now_empty(&lines, 0));
        let with_task = vec!["**Clients**", "* still here"];
        assert!(!header_now_empty(&with_task, 0));
        let with_prose = vec!["**Clients**", "notes about it"];
        assert!(!header_now_empty(&with_prose, 0));
        let at_end = vec!["text", "**Clients**"];
        assert!(header_now_empty(&at_end, 1));
        let next_header = vec!["**A**", "**B**", "* b task"];
        assert!(header_now_empty(&next_header, 0));
    }
}
