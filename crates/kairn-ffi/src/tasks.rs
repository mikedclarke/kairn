//! Task line editing: toggling done/open and rescheduling, exposed as pure
//! string transforms. The line-level rules (NotePlan markers, `@done` stamps,
//! `>date` tokens) live in [`kairn_core::tasks`].

/// Toggle a task line between open and done. `now` is the `@done(...)` stamp to
/// write when completing (the caller supplies the formatted timestamp, so this
/// stays clock-free and testable). Returns the rewritten line, or `None` if the
/// line is not a toggleable task (a plain `-` bullet, an unknown bracket state,
/// or not a task at all).
#[uniffi::export]
pub fn toggle_task_line(line: String, now: String) -> Option<String> {
    kairn_core::toggle_task_line(&line, &now)
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
        let done = toggle_task_line("* [ ] buy milk".into(), "2026-08-08".into()).unwrap();
        assert_eq!(done, "* [x] buy milk @done(2026-08-08)");
        let reopened = toggle_task_line(done, "2026-08-08".into()).unwrap();
        assert_eq!(reopened, "* [ ] buy milk");
    }

    #[test]
    fn plain_bullet_does_not_toggle() {
        assert!(toggle_task_line("- just a bullet".into(), "2026-08-08".into()).is_none());
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
}
