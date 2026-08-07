//! Line-level parsing of note markdown: line classification, inline spans,
//! and the raw-column math that maps rendered positions back to bytes.

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

pub fn parse_line(line: &str) -> Line {
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

pub(crate) fn bracket_state(rest: &str) -> Option<TaskState> {
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

/// The list prefix a new line under `line` should start with, NotePlan-style:
/// tasks and checklists continue with an open marker, bullets with a bullet,
/// anything else with nothing. Indentation is preserved.
pub fn continuation_prefix(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    for marker in ["* ", "+ ", "- "] {
        let Some(body) = rest.strip_prefix(marker) else {
            continue;
        };
        let body = body.trim_start();
        return if bracket_state(body).is_some() {
            format!("{indent}{marker}[ ] ")
        } else {
            format!("{indent}{marker}")
        };
    }
    String::new()
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
            // Only the date-shaped run after `>` is the reference;
            // trailing punctuation (`>2026-08-09.`) is plain text, so
            // matching and click navigation see a clean date.
            let token_len: usize = rest[1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .map(|c| c.len_utf8())
                .sum();
            if token_len > 0 {
                flush(&mut plain, &mut spans);
                spans.push((SpanKind::DateRef, rest[..1 + token_len].to_string()));
                i += 1 + token_len;
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

/// The styled span sitting `display_chars` characters into the line's
/// rendered content, for dispatching clicks on links.
pub fn span_at_display_char(raw: &str, display_chars: usize) -> Option<Span> {
    let line = parse_line(raw);
    let spans = match &line {
        Line::Heading { spans, .. }
        | Line::Task { spans, .. }
        | Line::Bullet { spans }
        | Line::Quote { spans }
        | Line::Text { spans } => spans,
        Line::Rule | Line::Blank => return None,
    };
    let mut seen = 0usize;
    for (kind, s) in spans {
        let chars = s.chars().count();
        if display_chars < seen + chars {
            return Some((*kind, s.clone()));
        }
        seen += chars;
    }
    None
}

/// Byte offset in `raw` where the rendered content begins: past indentation,
/// list markers, task brackets, heading hashes, or the quote marker,
/// mirroring [`parse_line`]'s stripping exactly.
fn content_start(raw: &str, line: &Line) -> usize {
    let indent = raw.len() - raw.trim_start().len();
    let trimmed = &raw[indent..];
    match line {
        Line::Heading { .. } => {
            let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
            indent + hashes + 1
        }
        Line::Quote { .. } => indent + 2,
        Line::Task { .. } | Line::Bullet { .. } => {
            let rest = &trimmed[2..];
            let gap = rest.len() - rest.trim_start().len();
            let body = &rest[gap..];
            let mut start = indent + 2 + gap;
            if matches!(line, Line::Task { .. }) && bracket_state(body).is_some() {
                let after = &body[3..];
                start += 3 + (after.len() - after.trim_start().len());
            }
            start
        }
        Line::Text { .. } => indent,
        Line::Rule | Line::Blank => raw.len(),
    }
}

/// Byte offset in `raw` where the rendered content begins, for clamping
/// edits: a split inside the list marker or task bracket would corrupt the
/// line. Rules and blank lines report their full length.
pub fn content_start_col(raw: &str) -> usize {
    content_start(raw, &parse_line(raw))
}

/// Byte offset in `raw` for a cursor sitting `display_chars` characters into
/// the line's rendered content (the concatenation of its spans; `==`
/// highlight markers are invisible when rendered). Past the end of the
/// content lands at the end of the line.
pub fn raw_col_for_display_char(raw: &str, display_chars: usize) -> usize {
    let line = parse_line(raw);
    let spans = match &line {
        Line::Heading { spans, .. }
        | Line::Task { spans, .. }
        | Line::Bullet { spans }
        | Line::Quote { spans }
        | Line::Text { spans } => spans,
        Line::Rule | Line::Blank => return raw.len(),
    };
    let mut raw_pos = content_start(raw, &line);
    let mut remaining = display_chars;
    for (kind, s) in spans {
        let marker = if *kind == SpanKind::Highlight { 2 } else { 0 };
        let chars = s.chars().count();
        if remaining <= chars {
            let byte = s
                .char_indices()
                .nth(remaining)
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            return raw_pos + marker + byte;
        }
        remaining -= chars;
        raw_pos += s.len() + marker * 2;
    }
    raw.len()
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
    fn dateref_sheds_trailing_punctuation() {
        assert_eq!(
            parse_line("* ship >2026-08-09."),
            Line::Task {
                state: TaskState::Open,
                spans: vec![
                    (SpanKind::Text, "ship ".into()),
                    (SpanKind::DateRef, ">2026-08-09".into()),
                    (SpanKind::Text, ".".into()),
                ]
            }
        );
    }

    #[test]
    fn continuation_prefixes() {
        assert_eq!(continuation_prefix("* buy milk"), "* ");
        assert_eq!(continuation_prefix("  * [x] done thing"), "  * [ ] ");
        assert_eq!(continuation_prefix("- [ ] task"), "- [ ] ");
        assert_eq!(continuation_prefix("- plain bullet"), "- ");
        assert_eq!(continuation_prefix("+ item"), "+ ");
        assert_eq!(continuation_prefix("## Heading"), "");
        assert_eq!(continuation_prefix("prose"), "");
    }

    #[test]
    fn display_char_to_raw_col() {
        // Task: content starts after "* [ ] ".
        assert_eq!(raw_col_for_display_char("* [ ] buy milk", 0), 6);
        assert_eq!(raw_col_for_display_char("* [ ] buy milk", 4), 10);
        // Past the content end: end of line.
        assert_eq!(raw_col_for_display_char("* [ ] buy milk", 99), 14);
        // Bare task marker and indentation.
        assert_eq!(raw_col_for_display_char("  * buy", 1), 5);
        // Heading.
        assert_eq!(raw_col_for_display_char("## Today", 2), 5);
        // Wiki links render with their brackets: offsets line up.
        assert_eq!(raw_col_for_display_char("see [[kairn]] now", 5), 5);
        // Highlight markers are stripped when rendered: clicking on the
        // first highlighted char lands inside the markers.
        assert_eq!(raw_col_for_display_char("== hot ==x", 0), 2);
        // Multi-byte characters stay on boundaries.
        assert_eq!(raw_col_for_display_char("* 中文", 1), "* 中".len());
        // Blank-ish lines land at the end.
        assert_eq!(raw_col_for_display_char("   ", 0), 3);
        assert_eq!(raw_col_for_display_char("---", 0), 3);
    }

    #[test]
    fn content_start_cols() {
        assert_eq!(content_start_col("* [ ] buy milk"), 6);
        assert_eq!(content_start_col("  * buy"), 4);
        assert_eq!(content_start_col("## Today"), 3);
        assert_eq!(content_start_col("> quoted"), 2);
        assert_eq!(content_start_col("plain"), 0);
        assert_eq!(content_start_col("---"), 3);
        assert_eq!(content_start_col("   "), 3);
    }

    #[test]
    fn span_under_display_char() {
        let raw = "* [ ] see [[kairn]] now";
        // Content renders as "see [[kairn]] now": char 0 is in the text span.
        assert_eq!(
            span_at_display_char(raw, 0),
            Some((SpanKind::Text, "see ".into()))
        );
        // Char 4 is the first bracket of the link.
        assert_eq!(
            span_at_display_char(raw, 4),
            Some((SpanKind::WikiLink, "[[kairn]]".into()))
        );
        assert_eq!(
            span_at_display_char(raw, 13),
            Some((SpanKind::Text, " now".into()))
        );
        // Past the end: nothing.
        assert_eq!(span_at_display_char(raw, 99), None);
        assert_eq!(span_at_display_char("---", 0), None);
    }
}
