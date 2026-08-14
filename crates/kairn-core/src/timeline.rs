//! Time-blocked lines of a daily note, for the day's timeline pill row:
//! `09:00 gerrards checks`, `14:00-15:30 call`, `2:30pm review`.

use std::ops::Range;

use chrono::NaiveTime;

use crate::parse::{self, Line, SpanKind, TaskState};

/// One time-blocked line: when it starts, when it ends if the line says,
/// the line's text with the time token and bookkeeping stripped, and where
/// the line lives in its note so edits can write back to it.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeBlock {
    pub start: NaiveTime,
    pub end: Option<NaiveTime>,
    pub label: String,
    pub line_idx: usize,
    pub line: String,
}

/// The time-blocked lines of a note, in start order. A block is any task,
/// bullet, or plain text line whose visible text carries a time: `HH:MM`
/// 24-hour or `H:MM` with am/pm, optionally a `-HH:MM` range. Cancelled
/// tasks don't block time; headings, quotes, and rules aren't schedulable
/// lines. Labels drop the time token, `>date` refs, and `@done(...)`-style
/// bookkeeping; URLs are never searched for times (10:30 inside a link is
/// not an appointment).
pub fn time_blocks(text: &str) -> Vec<TimeBlock> {
    let mut blocks: Vec<TimeBlock> = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let parsed = parse::parse_line(line);
        let spans = match &parsed {
            Line::Task { state, spans } if *state != TaskState::Cancelled => spans,
            Line::Bullet { spans } | Line::Text { spans } => spans,
            _ => continue,
        };
        let mut found: Option<(usize, Range<usize>, NaiveTime, Option<NaiveTime>)> = None;
        for (i, (kind, text)) in spans.iter().enumerate() {
            if !searchable(*kind) {
                continue;
            }
            if let Some((range, start, end)) = find_time(text) {
                found = Some((i, range, start, end));
                break;
            }
        }
        let Some((span_ix, token, start, end)) = found else { continue };
        let mut label = String::new();
        for (i, (kind, text)) in spans.iter().enumerate() {
            if !keep_in_label(kind, text) {
                continue;
            }
            if i == span_ix {
                // The token goes, and so does the connective it hung from:
                // "call simon at 14:00" reads "call simon". The connective
                // is trimmed off the label built so far because the token
                // may open its own span (SpanKind::Time), leaving "at" at
                // the tail of the span before it.
                label.push_str(&text[..token.start]);
                let kept = strip_connective(label.trim_end()).len();
                label.truncate(kept);
                label.push_str(&text[token.end..]);
            } else {
                label.push_str(text);
            }
        }
        let label = clean_label(&label);
        blocks.push(TimeBlock { start, end, label, line_idx, line: line.to_string() });
    }
    blocks.sort_by_key(|b| b.start);
    blocks
}

/// `line` with its time token rewritten to say `start` (and `end`, when
/// given). The token keeps its written style: am/pm stays am/pm, a padded
/// 24-hour hour stays padded, and an existing range keeps its separator.
/// `None` when the line has no time token to rewrite.
pub fn retime_line(line: &str, start: NaiveTime, end: Option<NaiveTime>) -> Option<String> {
    match parse::parse_line(line) {
        Line::Task { state, .. } if state != TaskState::Cancelled => {}
        Line::Bullet { .. } | Line::Text { .. } => {}
        _ => return None,
    }
    // Spans carry owned text, so the token is located in the raw line with
    // the same scan `find_time` uses; the parse above only decides that
    // this line kind is retimeable at all.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let boundary =
            i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b':';
        if boundary
            && bytes[i].is_ascii_digit()
            && let Some((end_ix, ..)) = parse::parse_time_token(line, i)
        {
            // A time-shaped path segment inside a URL is not a schedule.
            let word_start = line[..i].rfind(char::is_whitespace).map_or(0, |w| w + 1);
            let word_end = line[i..]
                .find(char::is_whitespace)
                .map_or(line.len(), |w| i + w);
            if line[word_start..word_end].contains("://") {
                i = word_end;
                continue;
            }
            let token = &line[i..end_ix];
            let lower = token.to_ascii_lowercase();
            let meridiem = lower.contains("am") || lower.contains("pm");
            let padded = token.starts_with('0');
            let sep = [" – ", " - ", "–", "-"]
                .into_iter()
                .find(|s| token.contains(s))
                .unwrap_or(" - ");
            let mut new_token = fmt_time(start, meridiem, padded);
            if let Some(end) = end {
                new_token.push_str(sep);
                new_token.push_str(&fmt_time(end, meridiem, padded));
            }
            return Some(format!("{}{}{}", &line[..i], new_token, &line[end_ix..]));
        }
        i += 1;
    }
    None
}

/// A clock time in the written style of the token it replaces.
fn fmt_time(t: NaiveTime, meridiem: bool, padded: bool) -> String {
    use chrono::Timelike;
    if meridiem {
        let h = match t.hour() % 12 {
            0 => 12,
            h => h,
        };
        let suffix = if t.hour() < 12 { "am" } else { "pm" };
        format!("{}:{:02}{}", h, t.minute(), suffix)
    } else if padded {
        format!("{:02}:{:02}", t.hour(), t.minute())
    } else {
        format!("{}:{:02}", t.hour(), t.minute())
    }
}

/// Span kinds whose text can contain the line's time token. Plain text
/// times arrive pre-split as [`SpanKind::Time`] spans; emphasis spans keep
/// their times inline, so their text is still searched.
fn searchable(kind: SpanKind) -> bool {
    matches!(
        kind,
        SpanKind::Time | SpanKind::Text | SpanKind::Bold | SpanKind::Italic | SpanKind::Highlight
    )
}

/// Span kinds and values that belong in a pill label: everything visible
/// minus schedule refs and `@done(...)`-style bookkeeping mentions.
fn keep_in_label(kind: &SpanKind, text: &str) -> bool {
    match kind {
        SpanKind::Hidden | SpanKind::DateRef => false,
        SpanKind::Mention => !text.contains('('),
        _ => true,
    }
}

/// The connective a time token hung from, dropped along with it.
fn strip_connective(text: &str) -> &str {
    for connective in ["at", "from", "until", "till"] {
        if let Some(rest) = text.strip_suffix(connective)
            && (rest.is_empty() || rest.ends_with(' '))
        {
            return rest.trim_end();
        }
    }
    text
}

/// Collapse the whitespace the token splice leaves behind and trim
/// dangling separators.
fn clean_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for word in label.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.trim_matches(|c: char| c == '-' || c == '–' || c == ':' || c == ',' || c == ' ')
        .to_string()
}

/// The first time token in `text`: its byte range and the parsed start and
/// optional end. Tokens must sit on a word boundary so `14:000`, `x9:30`,
/// and digits inside longer runs don't match.
fn find_time(text: &str) -> Option<(Range<usize>, NaiveTime, Option<NaiveTime>)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0
            || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b':';
        if boundary
            && bytes[i].is_ascii_digit()
            && let Some((end_ix, start, end)) = parse::parse_time_token(text, i)
        {
            return Some((i..end_ix, start, end));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).expect("valid time")
    }

    #[test]
    fn finds_and_sorts_time_blocked_lines() {
        let text = "# Thursday\n\
                    * 14:00 call simon\n\
                    * [x] 09:00-10:30 kairn pass\n\
                    - 16:00 net monitor review\n\
                    * plain task without a time\n";
        let blocks = time_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].start, t(9, 0));
        assert_eq!(blocks[0].end, Some(t(10, 30)));
        assert_eq!(blocks[0].label, "kairn pass");
        assert_eq!(blocks[1].start, t(14, 0));
        assert_eq!(blocks[1].label, "call simon");
        assert_eq!(blocks[2].start, t(16, 0));
    }

    #[test]
    fn mid_line_times_and_connectives() {
        let blocks = time_blocks("* call simon at 14:00\n* lunch from 12:30, outside\n");
        assert_eq!(blocks[0].start, t(12, 30));
        assert_eq!(blocks[0].label, "lunch, outside");
        assert_eq!(blocks[1].start, t(14, 0));
        assert_eq!(blocks[1].label, "call simon");
    }

    #[test]
    fn twelve_hour_times() {
        let blocks = time_blocks("* 2:30pm review\n* 9:15 am standup\n* 12:00pm lunch\n* 12:30am late\n");
        assert_eq!(blocks[0].start, t(0, 30));
        assert_eq!(blocks[1].start, t(9, 15));
        assert_eq!(blocks[1].label, "standup");
        assert_eq!(blocks[2].start, t(12, 0));
        assert_eq!(blocks[3].start, t(14, 30));
        assert_eq!(blocks[3].label, "review");
    }

    #[test]
    fn rejects_non_times_and_unschedulable_lines() {
        // Invalid clock values, digits mid-word, headings, quotes,
        // cancelled tasks, and times inside URLs are all skipped.
        let text = "* 25:00 not a time\n\
                    * 9:99 not a time\n\
                    * x14:00 glued to a word\n\
                    ## 09:00 heading standup\n\
                    > 10:00 quoted\n\
                    * [-] 11:00 cancelled\n\
                    * see https://example.com/a/10:30/page\n";
        assert!(time_blocks(text).is_empty());
    }

    #[test]
    fn labels_drop_refs_and_bookkeeping() {
        let blocks = time_blocks(
            "* [x] 09:00 gerrards checks >2026-08-07 @done(2026-08-07 09:40) #ops\n",
        );
        assert_eq!(blocks[0].label, "gerrards checks #ops");
        // The am suffix is not stolen from a following word.
        let blocks = time_blocks("* 9:30amber alert\n");
        assert_eq!(blocks[0].start, t(9, 30));
        assert_eq!(blocks[0].label, "amber alert");
    }

    #[test]
    fn spaced_ranges() {
        let blocks = time_blocks("* 09:00 - 10:30 deep work\n");
        assert_eq!(blocks[0].start, t(9, 0));
        assert_eq!(blocks[0].end, Some(t(10, 30)));
        assert_eq!(blocks[0].label, "deep work");
    }

    #[test]
    fn carries_line_locations() {
        let blocks = time_blocks("# Thursday\n* 14:00 call simon\n\n* 09:00 standup\n");
        assert_eq!(blocks[0].line_idx, 3);
        assert_eq!(blocks[0].line, "* 09:00 standup");
        assert_eq!(blocks[1].line_idx, 1);
    }

    #[test]
    fn retime_preserves_written_style() {
        assert_eq!(
            retime_line("* [ ] 09:00-10:30 deep work", t(9, 30), Some(t(11, 0))),
            Some("* [ ] 09:30-11:00 deep work".into())
        );
        assert_eq!(
            retime_line("* 9:00 - 10:30 deep work", t(13, 5), Some(t(14, 0))),
            Some("* 13:05 - 14:00 deep work".into())
        );
        assert_eq!(
            retime_line("* 2:30pm review >2026-08-07", t(15, 0), Some(t(15, 45))),
            Some("* 3:00pm - 3:45pm review >2026-08-07".into())
        );
        assert_eq!(
            retime_line("* call simon at 14:00", t(9, 0), None),
            Some("* call simon at 9:00".into())
        );
        // Gaining an end time where the line had none.
        assert_eq!(
            retime_line("* 14:00 call simon", t(14, 0), Some(t(14, 30))),
            Some("* 14:00 - 14:30 call simon".into())
        );
        assert_eq!(retime_line("* no time here", t(9, 0), None), None);
        assert_eq!(retime_line("## 09:00 heading", t(10, 0), None), None);
        // A URL's time-shaped segment is left alone; the real token moves.
        assert_eq!(
            retime_line("* see https://example.com/10:30/x at 11:00", t(9, 0), None),
            Some("* see https://example.com/10:30/x at 9:00".into())
        );
    }
}

