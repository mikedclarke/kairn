//! Tasks: the whole-vault scan behind the task views and calendar
//! indicators, plus line editing (toggling done/open, rescheduling) exposed
//! as pure string transforms. The rules (NotePlan markers, `@done` stamps,
//! `>date` tokens, due-date semantics) live in [`kairn_core::tasks`].

use std::path::Path;

use crate::parse::FfiSpan;
use crate::vault::FfiDate;

/// One open task found in the vault, addressable for toggling. `path` is
/// absolute; `due` is the task's `>date` token, or for a daily-note task
/// without one, the daily's own date; `file_date` is set when the task lives
/// in a daily note. `line` is the raw line at scan time, passed back on
/// toggle so a file that changed since the scan is never clobbered.
#[derive(uniffi::Record)]
pub struct FfiTaskRef {
    pub path: String,
    pub due: FfiDate,
    pub file_date: Option<FfiDate>,
    pub line_idx: u64,
    pub line: String,
    pub spans: Vec<FfiSpan>,
}

/// Per-day open/done tallies for calendar indicators, by due date. A day
/// with no tasks has no entry. Cancelled counts as done.
#[derive(uniffi::Record)]
pub struct FfiDayTaskStats {
    pub date: FfiDate,
    pub open: u32,
    pub done: u32,
}

/// One pass over every daily and note line: the open tasks (newest due
/// first) plus the per-day tallies, so calendar indicators don't cost a
/// second parse of the vault. Daily tasks fall back to the daily's date when
/// undated; tasks in other notes join only when they carry a `>date` token.
#[derive(uniffi::Record)]
pub struct FfiTaskScan {
    pub open: Vec<FfiTaskRef>,
    pub day_stats: Vec<FfiDayTaskStats>,
    pub complete: bool,
    pub errors: Vec<FfiScanError>,
}

#[derive(uniffi::Record)]
pub struct FfiScanError {
    pub path: String,
    pub message: String,
}

/// Scan the whole vault for tasks. Mirrors the desktop task views and
/// calendar indicators exactly.
#[uniffi::export]
pub fn scan_vault_tasks(root: String) -> FfiTaskScan {
    let root = Path::new(&root);
    let result = kairn_core::scan_vault_tasks(root);
    FfiTaskScan {
        complete: result.errors.is_empty(),
        errors: result.errors.into_iter().map(|error| FfiScanError {
            path: error.path.to_string_lossy().into_owned(),
            message: error.message,
        }).collect(),
        open: result
            .open
            .into_iter()
            .map(|t| FfiTaskRef {
                path: t.path.to_string_lossy().into_owned(),
                due: t.due.into(),
                file_date: t.file_date.map(Into::into),
                line_idx: t.line_idx as u64,
                line: t.line,
                spans: t.spans.into_iter().map(Into::into).collect(),
            })
            .collect(),
        day_stats: result
            .day_stats
            .into_iter()
            .map(|(date, stats)| FfiDayTaskStats {
                date: date.into(),
                open: stats.open as u32,
                done: stats.done as u32,
            })
            .collect(),
    }
}

/// Toggle a task line between open and done. Completion adds no `@done(...)`
/// stamp — the note's day already dates it — while reopening strips a trailing
/// stamp left by an import or an older version. Returns the rewritten line, or
/// `None` if the line is not a toggleable task (a plain `-` bullet, an unknown
/// bracket state, or not a task at all).
#[uniffi::export]
pub fn toggle_task_line(line: String) -> Option<String> {
    kairn_core::toggle_task_line(&line)
}

/// Set an open task line's due date to `year-month-day`: rewrites the first
/// `>YYYY-MM-DD` token in place, or appends ` >date` if there is none. Returns
/// `None` for an invalid date, a non-open task, or a line already due that day.
#[uniffi::export]
pub fn reschedule_task_line(line: String, year: i32, month: u32, day: u32) -> Option<String> {
    let due = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    kairn_core::reschedule_task_line(&line, due)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_completes_and_reopens() {
        let done = toggle_task_line("* [ ] buy milk".into()).unwrap();
        assert_eq!(done, "* [x] buy milk");
        let reopened = toggle_task_line(done).unwrap();
        assert_eq!(reopened, "* [ ] buy milk");
    }

    #[test]
    fn plain_bullet_does_not_toggle() {
        assert!(toggle_task_line("- just a bullet".into()).is_none());
    }

    #[test]
    fn reschedule_appends_due_token() {
        let out = reschedule_task_line("* [ ] ship it".into(), 2026, 8, 20).unwrap();
        assert_eq!(out, "* [ ] ship it >2026-08-20");
    }

    #[test]
    fn reschedule_rejects_invalid_date() {
        assert!(reschedule_task_line("* [ ] x".into(), 2026, 13, 40).is_none());
    }

    #[test]
    fn scan_finds_open_tasks_and_day_stats() {
        let root = std::env::temp_dir().join(format!("kairn-ffi-scan-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Calendar")).unwrap();
        std::fs::create_dir_all(root.join("Notes")).unwrap();
        std::fs::write(
            root.join("Calendar/20260805.md"),
            "* open here\n* [x] done here\n",
        )
        .unwrap();
        std::fs::write(root.join("Notes/Project.md"), "* dated >2026-08-20\n* undated\n").unwrap();

        let scan = scan_vault_tasks(root.to_string_lossy().into_owned());
        assert!(scan.complete);
        assert!(scan.errors.is_empty());
        let lines: Vec<&str> = scan.open.iter().map(|t| t.line.as_str()).collect();
        // Newest due first; the undated note task stays out.
        assert_eq!(lines, vec!["* dated >2026-08-20", "* open here"]);
        assert_eq!((scan.open[1].due.month, scan.open[1].due.day), (8, 5));
        assert!(scan.open[1].file_date.is_some());
        assert!(scan.open[0].file_date.is_none());
        let day5 = scan
            .day_stats
            .iter()
            .find(|s| s.date.day == 5)
            .expect("stats for the 5th");
        assert_eq!((day5.open, day5.done), (1, 1));
        let broken = root.join("Notes/Broken.md");
        std::fs::write(&broken, [0xff]).unwrap();
        let partial = scan_vault_tasks(root.to_string_lossy().into_owned());
        assert!(!partial.complete);
        assert_eq!(partial.open.len(), 2);
        assert_eq!(partial.errors.len(), 1);
        assert_eq!(partial.errors[0].path, broken.to_string_lossy());
        assert!(!partial.errors[0].message.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
