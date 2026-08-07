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
    /// Bold content: NotePlan-flavour `*bold*` as well as `**bold**`.
    Bold,
    /// Italic content: `_italic_`.
    Italic,
    /// An emphasis delimiter (`*`, `**`, `_`), kept visible but faded so the
    /// rendered line maps 1:1 onto the raw markdown.
    Marker,
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
        if let Some((marker, kind, consumed)) = emphasis_at(rest, at_word_start) {
            let content = &rest[marker.len()..consumed - marker.len()];
            flush(&mut plain, &mut spans);
            spans.push((SpanKind::Marker, marker.to_string()));
            spans.push((kind, content.to_string()));
            spans.push((SpanKind::Marker, marker.to_string()));
            i += consumed;
            continue;
        }
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

/// An emphasis run starting at `rest`, NotePlan flavour: `*bold*` and
/// `**bold**` are bold, `_italic_` is italic. Returns the delimiter, the
/// content kind, and the total bytes consumed. Content must not start or
/// end with whitespace (so `5 * 3 * 2` stays arithmetic), and underscores
/// only count on word boundaries (so snake_case identifiers stay plain).
fn emphasis_at(rest: &str, at_word_start: bool) -> Option<(&'static str, SpanKind, usize)> {
    let (marker, kind): (&'static str, SpanKind) = if rest.starts_with("**") {
        ("**", SpanKind::Bold)
    } else if rest.starts_with('*') {
        ("*", SpanKind::Bold)
    } else if rest.starts_with('_') {
        ("_", SpanKind::Italic)
    } else {
        return None;
    };
    if marker == "_" && !at_word_start {
        return None;
    }
    let m = marker.len();
    let close = rest[m..].find(marker)? + m;
    let content = &rest[m..close];
    let first = content.chars().next()?;
    let last = content.chars().last()?;
    if first.is_whitespace() || last.is_whitespace() {
        return None;
    }
    let end = close + m;
    // The closer must end the word: `_foo_bar` is an identifier, not italic.
    if marker == "_" && rest[end..].chars().next().is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    Some((marker, kind, end))
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

/// Inverse of [`raw_col_for_display_char`]: the rendered-content character
/// index for a byte offset into `raw`. Offsets inside the line's prefix or
/// a stripped highlight marker clamp to the nearest rendered character.
pub fn display_char_for_raw_col(raw: &str, raw_col: usize) -> usize {
    let line = parse_line(raw);
    let spans = match &line {
        Line::Heading { spans, .. }
        | Line::Task { spans, .. }
        | Line::Bullet { spans }
        | Line::Quote { spans }
        | Line::Text { spans } => spans,
        Line::Rule | Line::Blank => return 0,
    };
    let mut raw_pos = content_start(raw, &line);
    let mut display = 0usize;
    for (kind, s) in spans {
        let marker = if *kind == SpanKind::Highlight { 2 } else { 0 };
        let span_start = raw_pos + marker;
        if raw_col < span_start {
            return display;
        }
        if raw_col < span_start + s.len() {
            let local = raw_col - span_start;
            return display + s.char_indices().take_while(|(i, _)| *i < local).count();
        }
        display += s.chars().count();
        raw_pos = span_start + s.len() + marker;
    }
    display
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
    fn emphasis() {
        // NotePlan flavour: single asterisks are bold, markers stay visible.
        assert_eq!(
            parse_line("a *bold* word"),
            Line::Text {
                spans: vec![
                    (SpanKind::Text, "a ".into()),
                    (SpanKind::Marker, "*".into()),
                    (SpanKind::Bold, "bold".into()),
                    (SpanKind::Marker, "*".into()),
                    (SpanKind::Text, " word".into()),
                ]
            }
        );
        assert_eq!(
            parse_line("**Other clients — only if there's a gap**"),
            Line::Text {
                spans: vec![
                    (SpanKind::Marker, "**".into()),
                    (SpanKind::Bold, "Other clients — only if there's a gap".into()),
                    (SpanKind::Marker, "**".into()),
                ]
            }
        );
        assert_eq!(
            parse_line("stay _calm_ now"),
            Line::Text {
                spans: vec![
                    (SpanKind::Text, "stay ".into()),
                    (SpanKind::Marker, "_".into()),
                    (SpanKind::Italic, "calm".into()),
                    (SpanKind::Marker, "_".into()),
                    (SpanKind::Text, " now".into()),
                ]
            }
        );
        // Bold inside a task line, after the marker is stripped.
        assert_eq!(
            parse_line("* *this is just a note*"),
            Line::Task {
                state: TaskState::Open,
                spans: vec![
                    (SpanKind::Marker, "*".into()),
                    (SpanKind::Bold, "this is just a note".into()),
                    (SpanKind::Marker, "*".into()),
                ]
            }
        );
    }

    #[test]
    fn emphasis_false_positives_stay_plain() {
        // Arithmetic: content edges are whitespace.
        assert_eq!(parse_line("5 * 3 * 2"), Line::Text { spans: plain("5 * 3 * 2") });
        // Identifiers: underscores mid-word or closing into a word.
        assert_eq!(parse_line("file_name_here"), Line::Text { spans: plain("file_name_here") });
        assert_eq!(parse_line("_foo_bar"), Line::Text { spans: plain("_foo_bar") });
        // Unclosed markers.
        assert_eq!(parse_line("a *dangling star"), Line::Text { spans: plain("a *dangling star") });
        assert_eq!(parse_line("just ** stars"), Line::Text { spans: plain("just ** stars") });
        // Display text still maps 1:1 onto raw content for cursor math.
        let raw = "* a *bold* word";
        assert_eq!(raw_col_for_display_char(raw, 3), 5);
        assert_eq!(span_at_display_char(raw, 3), Some((SpanKind::Bold, "bold".into())));
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
    fn raw_col_to_display_char() {
        // Task content starts after "* [ ] "; anywhere in the prefix clamps
        // to the first rendered character.
        assert_eq!(display_char_for_raw_col("* [ ] buy milk", 6), 0);
        assert_eq!(display_char_for_raw_col("* [ ] buy milk", 3), 0);
        assert_eq!(display_char_for_raw_col("* [ ] buy milk", 10), 4);
        assert_eq!(display_char_for_raw_col("* [ ] buy milk", 14), 8);
        assert_eq!(display_char_for_raw_col("## Today", 5), 2);
        assert_eq!(display_char_for_raw_col("see [[kairn]] now", 5), 5);
        // Inside a stripped highlight marker: clamp to the highlight start.
        assert_eq!(display_char_for_raw_col("== hot ==x", 1), 0);
        assert_eq!(display_char_for_raw_col("== hot ==x", 8), 5);
        assert_eq!(display_char_for_raw_col("== hot ==x", 9), 5);
        assert_eq!(display_char_for_raw_col("* 中文", "* 中".len()), 1);
        assert_eq!(display_char_for_raw_col("---", 2), 0);
    }

    #[test]
    fn display_mapping_round_trips() {
        for raw in [
            "* [ ] buy *milk* ==now== #chore",
            "  - [x] done >2026-08-09",
            "## Section ==hot==",
            "see [[kairn]] and @mike",
            "plain text line",
        ] {
            let chars = {
                let mut n = 0;
                while raw_col_for_display_char(raw, n) < raw.len() {
                    n += 1;
                }
                n
            };
            for i in 0..chars {
                let col = raw_col_for_display_char(raw, i);
                assert_eq!(display_char_for_raw_col(raw, col), i, "{raw} char {i}");
            }
        }
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
