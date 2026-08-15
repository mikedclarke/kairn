//! Time-blocked lines of a note, for the day timeline: `09:00 standup`,
//! `14:00-15:30 call`, `2:30pm review`. The parsing and rewrite rules live
//! in [`kairn_core::timeline`]. Times cross the FFI as minutes since
//! midnight, which is what a layout needs anyway.

use chrono::{NaiveTime, Timelike};

/// One time-blocked line: when it starts, when it ends if the line says,
/// the line's text with the time token and bookkeeping stripped, and where
/// the line lives in its note so a tap can land on it.
#[derive(uniffi::Record)]
pub struct FfiTimeBlock {
    pub start_minutes: u32,
    pub end_minutes: Option<u32>,
    pub label: String,
    pub line_idx: u64,
    pub line: String,
}

fn minutes(t: NaiveTime) -> u32 {
    t.hour() * 60 + t.minute()
}

fn time_of(minutes: u32) -> Option<NaiveTime> {
    NaiveTime::from_hms_opt(minutes / 60, minutes % 60, 0)
}

/// The time-blocked lines of `text`, in start order. A block is any
/// non-cancelled task, bullet, or plain text line whose visible text carries
/// a time; headings, quotes, and rules aren't schedulable, and times inside
/// URLs don't count.
#[uniffi::export]
pub fn time_blocks(text: String) -> Vec<FfiTimeBlock> {
    kairn_core::time_blocks(&text)
        .into_iter()
        .map(|b| FfiTimeBlock {
            start_minutes: minutes(b.start),
            end_minutes: b.end.map(minutes),
            label: b.label,
            line_idx: b.line_idx as u64,
            line: b.line,
        })
        .collect()
}

/// `line` with its time token rewritten to say `start_minutes` (and
/// `end_minutes`, when given), keeping the token's written style: am/pm
/// stays am/pm, a padded hour stays padded, an existing range keeps its
/// separator. `None` when the line has no time token or the minutes don't
/// name a time of day.
#[uniffi::export]
pub fn retime_line(line: String, start_minutes: u32, end_minutes: Option<u32>) -> Option<String> {
    let start = time_of(start_minutes)?;
    let end = match end_minutes {
        Some(m) => Some(time_of(m)?),
        None => None,
    };
    kairn_core::retime_line(&line, start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_carry_minutes_and_labels() {
        let blocks = time_blocks("# Day\n* 09:00 standup\n* 14:00-15:30 call simon\n".into());
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start_minutes, 540);
        assert_eq!(blocks[0].end_minutes, None);
        assert_eq!(blocks[0].label, "standup");
        assert_eq!(blocks[1].end_minutes, Some(930));
        assert_eq!(blocks[1].line_idx, 2);
    }

    #[test]
    fn retime_preserves_written_style() {
        assert_eq!(
            retime_line("* 09:00 standup".into(), 600, None).as_deref(),
            Some("* 10:00 standup")
        );
        assert!(retime_line("* no time here".into(), 600, None).is_none());
        assert!(retime_line("* 09:00 x".into(), 1500, None).is_none());
    }
}
