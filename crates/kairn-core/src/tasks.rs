//! Tasks across the daily notes: scanning, filtering, and the pure
//! line-toggle logic. The app's task views and the CLI's `task list` share
//! the same predicate.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::parse::{Line, Span, SpanKind, TaskState, bracket_state, parse_line};
use crate::vault::{DayText, NoteText, VaultScan};

/// One open task found in a note, addressable for toggling.
#[derive(Clone, Debug)]
pub struct TaskRef {
    pub path: PathBuf,
    /// When the task is due: its `>date` token, or for a daily-note task
    /// without one, the daily's own date (NotePlan semantics).
    pub due: NaiveDate,
    /// The daily note's date when the task lives in one; `None` for tasks
    /// from regular or period notes, whose home is a note, not a day.
    pub file_date: Option<NaiveDate>,
    pub line_idx: usize,
    /// The raw line, passed back on toggle so a file that changed since the
    /// scan is never clobbered.
    pub line: String,
    pub spans: Vec<Span>,
}

/// The line's first `>YYYY-MM-DD` token as a date.
pub fn due_token(spans: &[Span]) -> Option<NaiveDate> {
    spans.iter().find_map(|(kind, s)| {
        if *kind != SpanKind::DateRef {
            return None;
        }
        NaiveDate::parse_from_str(s.strip_prefix('>')?, "%Y-%m-%d").ok()
    })
}

/// Every open task across the whole vault: the daily notes, plus dated
/// (`>date`) tasks from period and regular notes. Newest due date first.
pub fn open_tasks_in_vault(root: &Path) -> Vec<TaskRef> {
    let scan = VaultScan::new(root);
    let dailies = scan.read_dailies();
    let notes = scan.read_notes_cached(&mut Default::default());
    open_tasks_in(&dailies, &notes)
}

/// [`open_tasks_in_vault`] over files already read into memory, so one read
/// of each serves this and the mention scan. Daily tasks fall back to the
/// daily's date when they carry no `>date` token; tasks in other notes join
/// the list only when dated, so a reference note full of bullet-style task
/// lines doesn't swamp the Open view with undated noise.
pub fn open_tasks_in(dailies: &[DayText], notes: &[NoteText]) -> Vec<TaskRef> {
    scan_tasks(dailies, notes).open
}

/// Per-day task tallies for calendar indicators, by due date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DayTaskStats {
    pub open: usize,
    pub done: usize,
}

/// The result of one pass over every note line: the open tasks (see
/// [`open_tasks_in`]) plus per-day open/done tallies, so calendar
/// indicators don't cost a second parse of the whole vault.
pub struct TaskScan {
    pub open: Vec<TaskRef>,
    pub day_stats: std::collections::HashMap<NaiveDate, DayTaskStats>,
}

/// One parse of every daily and note line, producing both the open-task
/// list and the per-day tallies. Done and cancelled tasks count toward
/// their due day (token, or the daily's own date); cancelled counts as
/// done — it no longer needs attention.
pub fn scan_tasks(dailies: &[DayText], notes: &[NoteText]) -> TaskScan {
    let mut tasks = Vec::new();
    let mut day_stats: std::collections::HashMap<NaiveDate, DayTaskStats> =
        std::collections::HashMap::new();
    for day in dailies {
        for (line_idx, raw) in day.text.lines().enumerate() {
            let Line::Task { state, spans } = parse_line(raw) else { continue };
            let due = due_token(&spans).unwrap_or(day.date);
            let stats = day_stats.entry(due).or_default();
            match state {
                TaskState::Open => {
                    stats.open += 1;
                    tasks.push(TaskRef {
                        path: day.path.clone(),
                        due,
                        file_date: Some(day.date),
                        line_idx,
                        line: raw.to_string(),
                        spans,
                    });
                }
                TaskState::Scheduled => {}
                TaskState::Done | TaskState::Cancelled => stats.done += 1,
            }
        }
    }
    for note in notes {
        for (line_idx, raw) in note.text.lines().enumerate() {
            let Line::Task { state, spans } = parse_line(raw) else { continue };
            let Some(due) = due_token(&spans) else { continue };
            let stats = day_stats.entry(due).or_default();
            match state {
                TaskState::Open => {
                    stats.open += 1;
                    tasks.push(TaskRef {
                        path: note.path.clone(),
                        due,
                        file_date: None,
                        line_idx,
                        line: raw.to_string(),
                        spans,
                    });
                }
                TaskState::Scheduled => {}
                TaskState::Done | TaskState::Cancelled => stats.done += 1,
            }
        }
    }
    tasks.sort_by(|a, b| b.due.cmp(&a.due).then_with(|| a.path.cmp(&b.path)));
    TaskScan { open: tasks, day_stats }
}

/// The filters the task views (and the CLI's `task list`) run over the open
/// tasks, by due date.
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
/// style: indentation and list marker are preserved, only the bracket changes.
/// `* task` becomes `* [x] task`; a done task reopens as `[ ]`. Completion
/// carries no `@done(...)` stamp — the day a task lives on already dates it —
/// but reopening still strips a trailing stamp so imported or legacy notes get
/// tidied on the first toggle (the `-` marker needs the bracket to stay a task
/// at all, and `[ ]` reads identically everywhere). Returns `None` for
/// anything that isn't an open or done task.
pub fn toggle_task_line(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let marker = ["* ", "+ ", "- "].iter().find(|m| rest.starts_with(**m))?;
    let body = &rest[2..];
    let gap_len = body.len() - body.trim_start().len();
    let (gap, body) = body.split_at(gap_len);
    // Trailing whitespace is content (markdown hard breaks) and is preserved,
    // so a toggle round-trips the line byte-for-byte.
    match bracket_state(body) {
        Some(TaskState::Open) => {
            let content = &body[3..];
            Some(format!("{indent}{marker}{gap}[x]{content}"))
        }
        Some(TaskState::Done) => {
            let content = strip_trailing_done_stamp(&body[3..]);
            Some(format!("{indent}{marker}{gap}[ ]{content}"))
        }
        Some(_) => None,
        None if *marker == "- " || body.is_empty() || looks_bracketed(body) => None,
        None => Some(format!("{indent}{marker}{gap}[x] {body}")),
    }
}

/// Set an open task line's due date: the first `>YYYY-MM-DD` token is
/// rewritten in place, and a line with none gains ` >date` at the end
/// (before trailing whitespace, which is content: markdown hard breaks).
/// Only open tasks reschedule; anything else returns `None` untouched.
pub fn reschedule_task_line(line: &str, due: NaiveDate) -> Option<String> {
    let Line::Task { state: TaskState::Open, spans } = parse_line(line) else {
        return None;
    };
    let token = format!(">{}", due.format("%Y-%m-%d"));
    // Locate the first ISO date token by walking the spans, which cover
    // the raw line from the content start byte for byte; replacing by
    // offset can't hit a lookalike substring inside earlier plain text.
    let mut offset = crate::parse::spans_start_col(line);
    for (kind, s) in &spans {
        if *kind == SpanKind::DateRef
            && s.strip_prefix('>')
                .is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        {
            if line[offset..offset + s.len()] == token[..] {
                return None; // already due that day; nothing to write
            }
            let mut out = String::with_capacity(line.len() + 4);
            out.push_str(&line[..offset]);
            out.push_str(&token);
            out.push_str(&line[offset + s.len()..]);
            return Some(out);
        }
        offset += s.len();
    }
    let trimmed = line.trim_end();
    let trailing = &line[trimmed.len()..];
    Some(format!("{trimmed} {token}{trailing}"))
}

/// A `[c]`-shaped prefix whose state character isn't one Kairn knows
/// (`[!]`, `[?]`…). Such lines render as-is and must not toggle: wrapping
/// the whole body in a fresh bracket would corrupt them.
fn looks_bracketed(body: &str) -> bool {
    let mut chars = body.chars();
    chars.next() == Some('[') && chars.next().is_some() && chars.next() == Some(']')
}

/// Remove the single trailing ` @done(...)` stamp, so reopening a task that
/// still carries one (imported from NotePlan, or written by an older Kairn)
/// tidies it. Stamps anywhere else in the line are content the user (or
/// NotePlan) wrote and stay untouched, as does anything merely containing the
/// substring.
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
    use crate::ScratchRoot;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
    }

    #[test]
    fn due_dates_from_tokens_and_file_dates() {
        let root = ScratchRoot::new("due");
        root.write(
            "Calendar/20260805.md",
            "* plain task\n* dated >2026-08-20\n* punctuated >2026-08-21.\n",
        );
        root.write(
            "Notes/Project.md",
            "* dated in note >2026-08-19\n* undated in note\n+ [ ] checklist >2026-08-18\n",
        );

        let tasks = open_tasks_in_vault(&root.0);
        let due_of = |needle: &str| {
            tasks
                .iter()
                .find(|t| t.line.contains(needle))
                .map(|t| (t.due, t.file_date))
        };
        // Daily fallback: no token means due on the note's day.
        assert_eq!(due_of("plain"), Some((d(2026, 8, 5), Some(d(2026, 8, 5)))));
        // A token overrides the daily's date.
        assert_eq!(due_of("dated >"), Some((d(2026, 8, 20), Some(d(2026, 8, 5)))));
        // Trailing punctuation doesn't break the token.
        assert_eq!(due_of("punctuated"), Some((d(2026, 8, 21), Some(d(2026, 8, 5)))));
        // Note tasks join only when dated; the undated one stays out.
        assert_eq!(due_of("dated in note"), Some((d(2026, 8, 19), None)));
        assert_eq!(due_of("checklist"), Some((d(2026, 8, 18), None)));
        assert_eq!(due_of("undated in note"), None);
        assert_eq!(tasks.len(), 5);
        // Newest due first.
        let dues: Vec<NaiveDate> = tasks.iter().map(|t| t.due).collect();
        let mut sorted = dues.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(dues, sorted);
    }

    #[test]
    fn day_stats_tally_open_and_done_by_due_date() {
        let root = ScratchRoot::new("daystats");
        root.write(
            "Calendar/20260805.md",
            "* open here\n* [x] done here\n+ [-] cancelled here\n* moved out >2026-08-20\n",
        );
        root.write("Calendar/20260806.md", "* [x] all done\n* [x] both of them\n");
        root.write("Notes/Project.md", "* [x] dated done >2026-08-06\n");

        let scan = VaultScan::new(&root.0);
        let dailies = scan.read_dailies();
        let notes = scan.read_notes_cached(&mut Default::default());
        let result = scan_tasks(&dailies, &notes);

        // Open list unchanged by the tallies.
        assert_eq!(result.open.len(), 2);
        // 2026-08-05: one open, one done, one cancelled (counts as done);
        // the >date task moved its open count to the 20th.
        assert_eq!(
            result.day_stats[&d(2026, 8, 5)],
            DayTaskStats { open: 1, done: 2 }
        );
        assert_eq!(
            result.day_stats[&d(2026, 8, 20)],
            DayTaskStats { open: 1, done: 0 }
        );
        // 2026-08-06: two done in the daily plus a dated done from a note.
        assert_eq!(
            result.day_stats[&d(2026, 8, 6)],
            DayTaskStats { open: 0, done: 3 }
        );
        // A day with no tasks has no entry at all.
        assert!(!result.day_stats.contains_key(&d(2026, 8, 7)));
    }

    #[test]
    fn query_predicates_use_due_dates() {
        let today = d(2026, 8, 7);
        assert!(TaskQuery::Today.matches(today, today));
        assert!(!TaskQuery::Today.matches(d(2026, 8, 8), today));
        assert!(TaskQuery::Overdue.matches(d(2026, 8, 6), today));
        assert!(!TaskQuery::Overdue.matches(today, today));
        assert!(TaskQuery::Open.matches(d(2027, 1, 1), today));
    }

    #[test]
    fn reschedule_rewrites_or_appends_the_token() {
        let due = d(2026, 8, 20);
        // No token: appended at the end, trailing spaces preserved after it.
        assert_eq!(
            reschedule_task_line("* call bank", due).as_deref(),
            Some("* call bank >2026-08-20")
        );
        assert_eq!(
            reschedule_task_line("- [ ] call bank  ", due).as_deref(),
            Some("- [ ] call bank >2026-08-20  ")
        );
        // Existing token rewritten in place, position and punctuation kept.
        assert_eq!(
            reschedule_task_line("* pay >2026-08-09 then file", due).as_deref(),
            Some("* pay >2026-08-20 then file")
        );
        assert_eq!(
            reschedule_task_line("* pay >2026-08-09.", due).as_deref(),
            Some("* pay >2026-08-20.")
        );
        // A wiki link containing a date-shaped substring is not the token.
        assert_eq!(
            reschedule_task_line("* see [[log >2026-08-09]] >2026-08-10", due).as_deref(),
            Some("* see [[log >2026-08-09]] >2026-08-20")
        );
        // Already due that day: nothing to write.
        assert_eq!(reschedule_task_line("* pay >2026-08-20", due), None);
        // Only open tasks reschedule.
        assert_eq!(reschedule_task_line("* [x] shipped @done(x)", due), None);
        assert_eq!(reschedule_task_line("- just a bullet", due), None);
        assert_eq!(reschedule_task_line("plain text", due), None);
    }

    #[test]
    fn toggle_line_styles() {
        // Bare NotePlan task and checklist gain a bracket; no stamp is added.
        assert_eq!(toggle_task_line("* buy milk").as_deref(), Some("* [x] buy milk"));
        assert_eq!(toggle_task_line("+ pack bag").as_deref(), Some("+ [x] pack bag"));
        // Bracketed style keeps its marker, indentation survives.
        assert_eq!(
            toggle_task_line("  - [ ] call bank").as_deref(),
            Some("  - [x] call bank")
        );
        // Reopening strips a legacy/imported stamp and keeps a bracket.
        assert_eq!(
            toggle_task_line("* [x] shipped @done(2026-08-06 18:00)").as_deref(),
            Some("* [ ] shipped")
        );
        // Only the trailing stamp is stripped; earlier ones are user content.
        assert_eq!(
            toggle_task_line("- [x] paid @done(2026-08-05) again @done(2026-08-06)").as_deref(),
            Some("- [ ] paid @done(2026-08-05) again")
        );
        // Not toggleable: bullets, scheduled, cancelled, plain text.
        assert_eq!(toggle_task_line("- just a bullet"), None);
        assert_eq!(toggle_task_line("* [>] moved"), None);
        assert_eq!(toggle_task_line("+ [-] cancelled"), None);
        assert_eq!(toggle_task_line("plain text"), None);
    }

    #[test]
    fn reopen_leaves_user_stamps_and_links_alone() {
        // A wiki link containing the stamp substring must survive verbatim.
        assert_eq!(
            toggle_task_line("* [x] see [[log @done(old)]] @done(2026-08-06 18:00)").as_deref(),
            Some("* [ ] see [[log @done(old)]]")
        );
        // A done task with no stamp at all reopens cleanly.
        assert_eq!(toggle_task_line("* [x] no stamp").as_deref(), Some("* [ ] no stamp"));
        // A stamp mid-line (not trailing) is content and stays.
        assert_eq!(
            toggle_task_line("* [x] logged @done(2026-08-05) then more").as_deref(),
            Some("* [ ] logged @done(2026-08-05) then more")
        );
    }

    #[test]
    fn toggle_round_trips_exactly() {
        // Trailing spaces are markdown hard breaks: kept through a full
        // toggle cycle.
        let done = toggle_task_line("* task  ").expect("toggles");
        assert_eq!(done, "* [x] task  ");
        assert_eq!(toggle_task_line(&done).as_deref(), Some("* [ ] task  "));
        // Unknown bracket styles ([!], [?]) neither toggle nor corrupt.
        assert_eq!(toggle_task_line("* [!] important"), None);
        assert_eq!(toggle_task_line("+ [?] maybe"), None);
    }
}
