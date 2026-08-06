//! Notes on disk, NotePlan-compatible.
//!
//! One user-set notes root (default `~/kairn`) holding `Calendar/` for period
//! notes (daily `YYYYMMDD.md`, weekly `YYYY-Wnn.md`, monthly `YYYY-MM.md`,
//! quarterly `YYYY-Qn.md`, yearly `YYYY.md`), `Notes/` for everything else,
//! and a hidden `.kairn/` folder for app data that syncs with the notes.
//! Files are plain markdown; NotePlan must be able to read anything Kairn
//! writes and vice versa, so pointing the root at an existing NotePlan
//! directory just works.
//!
//! Task syntax follows NotePlan alongside standard markdown: a bare `* task`
//! is an open task, `[x]`/`[>]`/`[-]` mark done, scheduled, and cancelled,
//! `+ item` lines are checklists, `- item` is a plain bullet unless
//! bracketed.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

/// Create the root and its expected folders. Safe to call every launch:
/// existing folders (e.g. a NotePlan directory) are left untouched.
pub fn ensure_layout(root: &Path) {
    for dir in [
        root.join("Calendar"),
        root.join("Notes"),
        root.join(".kairn"),
    ] {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("kairn: could not create {}: {e}", dir.display());
        }
    }
}

/// The daily note path Kairn writes to: `Calendar/YYYYMMDD.md`.
pub fn daily_path(root: &Path, date: NaiveDate) -> PathBuf {
    root.join("Calendar")
        .join(format!("{}.md", date.format("%Y%m%d")))
}

/// Read a day's note, accepting NotePlan's optional `.txt` extension when no
/// `.md` file exists.
pub fn load_day(root: &Path, date: NaiveDate) -> Option<String> {
    let md = daily_path(root, date);
    if let Ok(text) = fs::read_to_string(&md) {
        return Some(text);
    }
    let txt = md.with_extension("txt");
    fs::read_to_string(&txt).ok()
}

/// Every date with a daily note, from one scan of `Calendar/`.
pub fn days_with_notes(root: &Path) -> HashSet<NaiveDate> {
    let mut days = HashSet::new();
    let Ok(entries) = fs::read_dir(root.join("Calendar")) else {
        return days;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|x| x.to_str());
        if !matches!(ext, Some("md") | Some("txt")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.len() == 8
            && stem.bytes().all(|b| b.is_ascii_digit())
            && let Ok(date) = NaiveDate::parse_from_str(stem, "%Y%m%d")
        {
            days.insert(date);
        }
    }
    days
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Open,
    Done,
    Scheduled,
    Cancelled,
}

/// One line of a note, classified for read-only rendering.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Heading { level: u8, spans: Vec<Span> },
    Task { state: TaskState, spans: Vec<Span> },
    Bullet { spans: Vec<Span> },
    Quote { spans: Vec<Span> },
    Rule,
    Blank,
    Text { spans: Vec<Span> },
}

/// Inline fragment with special styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Text,
    /// `[[wiki link]]`
    WikiLink,
    /// `#tag`
    Tag,
    /// `@mention`, including NotePlan `@done(...)` etc.
    Mention,
    /// `>YYYY-MM-DD` schedule reference.
    DateRef,
    /// `==highlighted==` text, markers stripped.
    Highlight,
}

pub type Span = (SpanKind, String);

pub fn parse(text: &str) -> Vec<Line> {
    text.lines().map(parse_line).collect()
}

pub fn open_task_count(text: &str) -> usize {
    parse(text)
        .iter()
        .filter(|l| matches!(l, Line::Task { state: TaskState::Open, .. }))
        .count()
}

fn parse_line(line: &str) -> Line {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return Line::Blank;
    }
    if trimmed.chars().all(|c| c == '-') && trimmed.len() >= 3 {
        return Line::Rule;
    }
    if let Some(rest) = trimmed.strip_prefix('#') {
        let level = 1 + rest.chars().take_while(|&c| c == '#').count() as u8;
        let text = rest.trim_start_matches('#');
        if let Some(text) = text.strip_prefix(' ') {
            return Line::Heading { level, spans: inline_spans(text) };
        }
    }
    if let Some(rest) = trimmed.strip_prefix("> ") {
        return Line::Quote { spans: inline_spans(rest) };
    }
    // List markers. NotePlan: bare `*` and `+` are tasks/checklists, `-` is a
    // plain bullet; any marker with `[ ]`-family brackets is a task.
    for marker in ["* ", "+ ", "- "] {
        let Some(rest) = trimmed.strip_prefix(marker) else {
            continue;
        };
        let rest_trimmed = rest.trim_start();
        if let Some(state) = bracket_state(rest_trimmed) {
            let content = rest_trimmed[3..].trim_start();
            return Line::Task { state, spans: inline_spans(content) };
        }
        return if marker == "- " {
            Line::Bullet { spans: inline_spans(rest_trimmed) }
        } else {
            Line::Task { state: TaskState::Open, spans: inline_spans(rest_trimmed) }
        };
    }
    Line::Text { spans: inline_spans(trimmed) }
}

fn bracket_state(rest: &str) -> Option<TaskState> {
    let mut chars = rest.chars();
    if chars.next() != Some('[') {
        return None;
    }
    let state = match chars.next()? {
        ' ' => TaskState::Open,
        'x' | 'X' => TaskState::Done,
        '>' => TaskState::Scheduled,
        '-' => TaskState::Cancelled,
        _ => return None,
    };
    (chars.next() == Some(']')).then_some(state)
}

/// Split a line into styled fragments: wiki links, #tags, @mentions, and
/// `>date` references. Everything else is plain text.
fn inline_spans(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    let flush = |plain: &mut String, spans: &mut Vec<Span>| {
        if !plain.is_empty() {
            spans.push((SpanKind::Text, std::mem::take(plain)));
        }
    };

    while i < bytes.len() {
        let rest = &text[i..];
        if rest.starts_with("[[")
            && let Some(end) = rest.find("]]")
        {
            flush(&mut plain, &mut spans);
            spans.push((SpanKind::WikiLink, rest[..end + 2].to_string()));
            i += end + 2;
            continue;
        }
        if rest.starts_with("==")
            && let Some(end) = rest[2..].find("==")
            && end > 0
        {
            flush(&mut plain, &mut spans);
            spans.push((SpanKind::Highlight, rest[2..end + 2].to_string()));
            i += end + 4;
            continue;
        }
        let at_word_start = i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'(';
        if at_word_start && (rest.starts_with('#') || rest.starts_with('@')) {
            let token: &str = rest
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or(rest);
            if token.len() > 1 {
                // `@done(2026-08-06 21:14)` style: swallow a directly
                // attached parenthesised argument, spaces and all.
                let mut token_len = token.len();
                if token.contains('(')
                    && !token.contains(')')
                    && let Some(close) = rest.find(')')
                {
                    token_len = close + 1;
                }
                let kind = if rest.starts_with('#') { SpanKind::Tag } else { SpanKind::Mention };
                flush(&mut plain, &mut spans);
                spans.push((kind, rest[..token_len].to_string()));
                i += token_len;
                continue;
            }
        }
        if at_word_start && rest.starts_with('>') && rest.len() > 1 {
            let token: &str = rest
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or(rest);
            if token.len() > 1 {
                flush(&mut plain, &mut spans);
                spans.push((SpanKind::DateRef, token.to_string()));
                i += token.len();
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty rest");
        plain.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut plain, &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Vec<Span> {
        vec![(SpanKind::Text, s.to_string())]
    }

    #[test]
    fn tasks_and_bullets() {
        assert_eq!(
            parse_line("* buy milk"),
            Line::Task { state: TaskState::Open, spans: plain("buy milk") }
        );
        assert_eq!(
            parse_line("* [x] done thing"),
            Line::Task { state: TaskState::Done, spans: plain("done thing") }
        );
        assert_eq!(
            parse_line("- [>] moved"),
            Line::Task { state: TaskState::Scheduled, spans: plain("moved") }
        );
        assert_eq!(
            parse_line("+ [-] cancelled"),
            Line::Task { state: TaskState::Cancelled, spans: plain("cancelled") }
        );
        assert_eq!(parse_line("- just a bullet"), Line::Bullet { spans: plain("just a bullet") });
        assert_eq!(
            parse_line("+ checklist item"),
            Line::Task { state: TaskState::Open, spans: plain("checklist item") }
        );
    }

    #[test]
    fn structure() {
        assert_eq!(
            parse_line("## Today"),
            Line::Heading { level: 2, spans: plain("Today") }
        );
        assert_eq!(parse_line("---"), Line::Rule);
        assert_eq!(parse_line("   "), Line::Blank);
        assert_eq!(parse_line("> quoted"), Line::Quote { spans: plain("quoted") });
    }

    #[test]
    fn inline() {
        assert_eq!(
            parse_line("see [[kairn prd]] for #plans >2026-08-12"),
            Line::Text {
                spans: vec![
                    (SpanKind::Text, "see ".into()),
                    (SpanKind::WikiLink, "[[kairn prd]]".into()),
                    (SpanKind::Text, " for ".into()),
                    (SpanKind::Tag, "#plans".into()),
                    (SpanKind::Text, " ".into()),
                    (SpanKind::DateRef, ">2026-08-12".into()),
                ]
            }
        );
        assert_eq!(
            parse_line("* [x] shipped @done(2026-08-06 18:00)"),
            Line::Task {
                state: TaskState::Done,
                spans: vec![
                    (SpanKind::Text, "shipped ".into()),
                    (SpanKind::Mention, "@done(2026-08-06 18:00)".into()),
                ]
            }
        );
    }

    #[test]
    fn highlight() {
        assert_eq!(
            parse_line("### ==Todays Tasks=="),
            Line::Heading { level: 3, spans: vec![(SpanKind::Highlight, "Todays Tasks".into())] }
        );
    }

    #[test]
    fn day_filenames() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid date");
        assert!(daily_path(Path::new("/root"), d).ends_with("Calendar/20260806.md"));
    }
}
