//! Tasks across the daily notes: scanning, filtering, and the pure
//! line-toggle logic. The app's task views and the CLI's `task list` share
//! the same predicate.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::parse::{Line, Span, TaskState, bracket_state, parse_line};
use crate::vault::{DayText, VaultScan};

/// One open task found in a daily note, addressable for toggling.
#[derive(Clone, Debug)]
pub struct TaskRef {
    pub path: PathBuf,
    pub date: NaiveDate,
    pub line_idx: usize,
    /// The raw line, passed back on toggle so a file that changed since the
    /// scan is never clobbered.
    pub line: String,
    pub spans: Vec<Span>,
}

/// Every open task across the daily notes, newest day first.
pub fn open_tasks_in_dailies(root: &Path) -> Vec<TaskRef> {
    open_tasks_in(&VaultScan::new(root).read_dailies())
}

/// [`open_tasks_in_dailies`] over dailies already read into memory, so one
/// read of each file serves both this and the mention scan.
pub fn open_tasks_in(dailies: &[DayText]) -> Vec<TaskRef> {
    let mut tasks = Vec::new();
    for day in dailies {
        for (line_idx, raw) in day.text.lines().enumerate() {
            if let Line::Task { state: TaskState::Open, spans } = parse_line(raw) {
                tasks.push(TaskRef {
                    path: day.path.clone(),
                    date: day.date,
                    line_idx,
                    line: raw.to_string(),
                    spans,
                });
            }
        }
    }
    tasks
}

/// The filters the task views (and the CLI's `task list`) run over the open
/// tasks. A task's date is the daily note it lives in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskQuery {
    Today,
    Open,
    Overdue,
}

impl TaskQuery {
    pub fn matches(self, date: NaiveDate, today: NaiveDate) -> bool {
        match self {
            TaskQuery::Today => date == today,
            TaskQuery::Open => true,
            TaskQuery::Overdue => date < today,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            TaskQuery::Today => "Today's tasks",
            TaskQuery::Open => "Open tasks",
            TaskQuery::Overdue => "Overdue tasks",
        }
    }
}

/// Toggle one task line between open and done, writing in the line's own
/// style: indentation and list marker are preserved, only the bracket and the
/// `@done(...)` stamp change. `* task` becomes `* [x] task @done(now)`; a done
/// task reopens as `[ ]` with the stamp stripped (the `-` marker needs the
/// bracket to stay a task at all, and `[ ]` reads identically everywhere).
/// Returns `None` for anything that isn't an open or done task.
pub fn toggle_task_line(line: &str, now: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let marker = ["* ", "+ ", "- "].iter().find(|m| rest.starts_with(**m))?;
    let body = &rest[2..];
    let gap_len = body.len() - body.trim_start().len();
    let (gap, body) = body.split_at(gap_len);
    // Trailing whitespace is content (markdown hard breaks); the stamp goes
    // after it and reopening removes only the stamp and its separator space,
    // so a toggle round-trips the line byte-for-byte.
    match bracket_state(body) {
        Some(TaskState::Open) => {
            let content = &body[3..];
            Some(format!("{indent}{marker}{gap}[x]{content} @done({now})"))
        }
        Some(TaskState::Done) => {
            let content = strip_trailing_done_stamp(&body[3..]);
            Some(format!("{indent}{marker}{gap}[ ]{content}"))
        }
        Some(_) => None,
        None if *marker == "- " || body.is_empty() || looks_bracketed(body) => None,
        None => Some(format!("{indent}{marker}{gap}[x] {body} @done({now})")),
    }
}

/// A `[c]`-shaped prefix whose state character isn't one Kairn knows
/// (`[!]`, `[?]`…). Such lines render as-is and must not toggle: wrapping
/// the whole body in a fresh bracket would corrupt them.
fn looks_bracketed(body: &str) -> bool {
    let mut chars = body.chars();
    chars.next() == Some('[') && chars.next().is_some() && chars.next() == Some(']')
}

/// Remove the single trailing ` @done(...)` stamp, the one toggling appends.
/// Stamps anywhere else in the line are content the user (or NotePlan) wrote
/// and stay untouched, as does anything merely containing the substring.
fn strip_trailing_done_stamp(s: &str) -> &str {
    let trimmed = s.trim_end();
    let Some(pos) = trimmed.rfind("@done(") else {
        return s;
    };
    let stamp = &trimmed[pos..];
    if !stamp.ends_with(')') || stamp[..stamp.len() - 1].contains(')') {
        return s;
    }
    let before = &trimmed[..pos];
    before.strip_suffix(' ').unwrap_or(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_line_styles() {
        let now = "2026-08-06 21:30";
        // Bare NotePlan task and checklist gain a bracket and a stamp.
        assert_eq!(
            toggle_task_line("* buy milk", now).as_deref(),
            Some("* [x] buy milk @done(2026-08-06 21:30)")
        );
        assert_eq!(
            toggle_task_line("+ pack bag", now).as_deref(),
            Some("+ [x] pack bag @done(2026-08-06 21:30)")
        );
        // Bracketed style keeps its marker, indentation survives.
        assert_eq!(
            toggle_task_line("  - [ ] call bank", now).as_deref(),
            Some("  - [x] call bank @done(2026-08-06 21:30)")
        );
        // Reopening strips the stamp and keeps a bracket.
        assert_eq!(
            toggle_task_line("* [x] shipped @done(2026-08-06 18:00)", now).as_deref(),
            Some("* [ ] shipped")
        );
        // Only the trailing stamp is Kairn's; earlier ones are user content.
        assert_eq!(
            toggle_task_line("- [x] paid @done(2026-08-05) again @done(2026-08-06)", now)
                .as_deref(),
            Some("- [ ] paid @done(2026-08-05) again")
        );
        // Not toggleable: bullets, scheduled, cancelled, plain text.
        assert_eq!(toggle_task_line("- just a bullet", now), None);
        assert_eq!(toggle_task_line("* [>] moved", now), None);
        assert_eq!(toggle_task_line("+ [-] cancelled", now), None);
        assert_eq!(toggle_task_line("plain text", now), None);
    }

    #[test]
    fn reopen_leaves_user_stamps_and_links_alone() {
        let now = "2026-08-07 12:00";
        // A wiki link containing the stamp substring must survive verbatim.
        assert_eq!(
            toggle_task_line("* [x] see [[log @done(old)]] @done(2026-08-06 18:00)", now)
                .as_deref(),
            Some("* [ ] see [[log @done(old)]]")
        );
        // A done task with no stamp at all reopens cleanly.
        assert_eq!(toggle_task_line("* [x] no stamp", now).as_deref(), Some("* [ ] no stamp"));
        // A stamp mid-line (not trailing) is content and stays.
        assert_eq!(
            toggle_task_line("* [x] logged @done(2026-08-05) then more", now).as_deref(),
            Some("* [ ] logged @done(2026-08-05) then more")
        );
    }

    #[test]
    fn toggle_round_trips_exactly() {
        let now = "2026-08-07 12:00";
        // Trailing spaces are markdown hard breaks: kept through a full
        // toggle cycle.
        let done = toggle_task_line("* task  ", now).expect("toggles");
        assert_eq!(done, "* [x] task   @done(2026-08-07 12:00)");
        assert_eq!(toggle_task_line(&done, now).as_deref(), Some("* [ ] task  "));
        // Unknown bracket styles ([!], [?]) neither toggle nor corrupt.
        assert_eq!(toggle_task_line("* [!] important", now), None);
        assert_eq!(toggle_task_line("+ [?] maybe", now), None);
    }
}
