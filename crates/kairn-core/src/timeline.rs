//! Time-blocked lines of a daily note, for the day's timeline pill row:
//! `09:00 gerrards checks`, `14:00-15:30 call`, `2:30pm review`.

use std::ops::Range;

use chrono::NaiveTime;

use crate::parse::{self, Line, SpanKind, TaskState};

/// One time-blocked line: when it starts, when it ends if the line says,
/// and the line's text with the time token and bookkeeping stripped.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeBlock {
    pub start: NaiveTime,
    pub end: Option<NaiveTime>,
    pub label: String,
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
    for line in text.lines() {
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
                // "call simon at 14:00" reads "call simon".
                label.push_str(strip_connective(text[..token.start].trim_end()));
                label.push_str(&text[token.end..]);
            } else {
                label.push_str(text);
            }
        }
        let label = clean_label(&label);
        blocks.push(TimeBlock { start, end, label });
    }
    blocks.sort_by_key(|b| b.start);
    blocks
}

/// Span kinds whose text can contain the line's time token.
fn searchable(kind: SpanKind) -> bool {
    matches!(
        kind,
        SpanKind::Text | SpanKind::Bold | SpanKind::Italic | SpanKind::Highlight
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
            && let Some((end_ix, start, end)) = parse_token(text, i)
        {
            return Some((i..end_ix, start, end));
        }
        i += 1;
    }
    None
}

/// Parse a full time token at `at`: a time, optionally `-`/`–` (spaces
/// allowed) and a second time. Returns the token's end index and the times.
fn parse_token(text: &str, at: usize) -> Option<(usize, NaiveTime, Option<NaiveTime>)> {
    let (mut i, start) = parse_one_time(text, at)?;
    // An optional range half: `-10:30`, ` - 10:30`, `–10:30`.
    let rest = &text[i..];
    let mut j = 0;
    let bytes = rest.as_bytes();
    if j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    let dash = if rest[j..].starts_with('-') {
        Some(1)
    } else if rest[j..].starts_with('–') {
        Some('–'.len_utf8())
    } else {
        None
    };
    if let Some(dash_len) = dash {
        let mut k = j + dash_len;
        if k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if let Some((end_ix, end)) = parse_one_time(text, i + k) {
            i = end_ix;
            return Some((i, start, Some(end)));
        }
    }
    Some((i, start, None))
}

/// Parse a single time at `at`: `H:MM`/`HH:MM`, optional am/pm (attached or
/// after one space). Returns the index after the time and the time itself.
/// A bare 24-hour time above 23:59 (or minutes above 59) is not a time.
fn parse_one_time(text: &str, at: usize) -> Option<(usize, NaiveTime)> {
    let bytes = text.as_bytes();
    let mut i = at;
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let hour_len = i - digits_start;
    if !(1..=2).contains(&hour_len) {
        return None;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    i += 1;
    let min_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i - min_start != 2 {
        return None;
    }
    let hour: u32 = text[digits_start..digits_start + hour_len].parse().ok()?;
    let minute: u32 = text[min_start..min_start + 2].parse().ok()?;
    if minute > 59 {
        return None;
    }
    // am/pm, attached or after one space; consumed only when it parses.
    let mut j = i;
    if j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    let suffix = text[j..].get(..2).map(str::to_ascii_lowercase);
    let meridiem = match suffix.as_deref() {
        Some("am") | Some("pm") if !followed_by_word(bytes, j + 2) => suffix,
        _ => None,
    };
    let (hour, end_ix) = match meridiem.as_deref() {
        Some("am") if (1..=12).contains(&hour) => (hour % 12, j + 2),
        Some("pm") if (1..=12).contains(&hour) => (hour % 12 + 12, j + 2),
        _ if hour <= 23 => (hour, i),
        _ => return None,
    };
    NaiveTime::from_hms_opt(hour, minute, 0).map(|t| (end_ix, t))
}

/// Whether an alphanumeric continues at `at` (so `9:30amber` isn't `9:30am`).
fn followed_by_word(bytes: &[u8], at: usize) -> bool {
    bytes.get(at).is_some_and(|b| b.is_ascii_alphanumeric())
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
}
