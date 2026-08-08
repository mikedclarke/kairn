//! Line and span classification for read-only styling. UniFFI mirrors of
//! [`kairn_core::Line`] / [`kairn_core::Span`] / their kinds, with `From`
//! conversions so the classification logic stays in `kairn-core`.

use kairn_core::{Line, SpanKind, TaskState};

/// Task marker state, mirrors [`kairn_core::TaskState`].
#[derive(uniffi::Enum)]
pub enum FfiTaskState {
    Open,
    Done,
    Scheduled,
    Cancelled,
}

impl From<TaskState> for FfiTaskState {
    fn from(s: TaskState) -> Self {
        match s {
            TaskState::Open => Self::Open,
            TaskState::Done => Self::Done,
            TaskState::Scheduled => Self::Scheduled,
            TaskState::Cancelled => Self::Cancelled,
        }
    }
}

/// Inline span styling kind, mirrors [`kairn_core::SpanKind`].
#[derive(uniffi::Enum)]
pub enum FfiSpanKind {
    Text,
    WikiLink,
    Tag,
    Mention,
    DateRef,
    Highlight,
    Bold,
    Italic,
    Link,
    Url,
    Hidden,
}

impl From<SpanKind> for FfiSpanKind {
    fn from(k: SpanKind) -> Self {
        match k {
            SpanKind::Text => Self::Text,
            SpanKind::WikiLink => Self::WikiLink,
            SpanKind::Tag => Self::Tag,
            SpanKind::Mention => Self::Mention,
            SpanKind::DateRef => Self::DateRef,
            SpanKind::Highlight => Self::Highlight,
            SpanKind::Bold => Self::Bold,
            SpanKind::Italic => Self::Italic,
            SpanKind::Link => Self::Link,
            SpanKind::Url => Self::Url,
            SpanKind::Hidden => Self::Hidden,
        }
    }
}

/// One inline fragment: its kind and its raw text. `kairn-core`'s `Span` is a
/// `(SpanKind, String)` tuple; UniFFI needs named fields, so it becomes a
/// record. Concatenating every span's `text` in order reproduces the raw line.
#[derive(uniffi::Record)]
pub struct FfiSpan {
    pub kind: FfiSpanKind,
    pub text: String,
}

impl From<kairn_core::Span> for FfiSpan {
    fn from((kind, text): kairn_core::Span) -> Self {
        Self {
            kind: kind.into(),
            text,
        }
    }
}

fn spans(v: Vec<kairn_core::Span>) -> Vec<FfiSpan> {
    v.into_iter().map(Into::into).collect()
}

/// One classified line, mirrors [`kairn_core::Line`].
#[derive(uniffi::Enum)]
pub enum FfiLine {
    Heading { level: u8, spans: Vec<FfiSpan> },
    Task { state: FfiTaskState, spans: Vec<FfiSpan> },
    Bullet { spans: Vec<FfiSpan> },
    Quote { spans: Vec<FfiSpan> },
    Rule,
    Blank,
    Text { spans: Vec<FfiSpan> },
}

impl From<Line> for FfiLine {
    fn from(line: Line) -> Self {
        match line {
            Line::Heading { level, spans: s } => Self::Heading { level, spans: spans(s) },
            Line::Task { state, spans: s } => Self::Task {
                state: state.into(),
                spans: spans(s),
            },
            Line::Bullet { spans: s } => Self::Bullet { spans: spans(s) },
            Line::Quote { spans: s } => Self::Quote { spans: spans(s) },
            Line::Rule => Self::Rule,
            Line::Blank => Self::Blank,
            Line::Text { spans: s } => Self::Text { spans: spans(s) },
        }
    }
}

/// Classify every line of a note for styling (one entry per line).
#[uniffi::export]
pub fn parse_note(text: String) -> Vec<FfiLine> {
    kairn_core::parse(&text).into_iter().map(Into::into).collect()
}

/// Classify a single line for styling.
#[uniffi::export]
pub fn parse_line(line: String) -> FfiLine {
    kairn_core::parse_line(&line).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_line_carries_level_and_spans() {
        match parse_line("## Title".into()) {
            FfiLine::Heading { level, spans } => {
                assert_eq!(level, 2);
                // Hidden `## ` prefix, then the title text.
                assert_eq!(spans.iter().map(|s| s.text.as_str()).collect::<String>(), "## Title");
            }
            other => panic!("expected heading, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn open_task_is_classified() {
        assert!(matches!(
            parse_line("* [ ] do it".into()),
            FfiLine::Task { state: FfiTaskState::Open, .. }
        ));
    }

    #[test]
    fn parse_note_is_one_entry_per_line() {
        assert_eq!(parse_note("a\n\n# b".into()).len(), 3);
    }
}
