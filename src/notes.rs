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
use std::io;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};

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

/// The daily note file that actually exists for a date, accepting NotePlan's
/// optional `.txt` extension when no `.md` file does.
pub fn daily_file(root: &Path, date: NaiveDate) -> Option<PathBuf> {
    let md = daily_path(root, date);
    if md.exists() {
        return Some(md);
    }
    let txt = md.with_extension("txt");
    txt.exists().then_some(txt)
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

/// One open task found in a daily note, addressable for toggling.
#[derive(Clone, Debug)]
pub struct TaskRef {
    pub path: PathBuf,
    pub date: NaiveDate,
    pub line_idx: usize,
    /// The raw line, passed back on toggle so a file that changed since the
    /// scan is never clobbered.
    pub line: String,
    pub spans: Vec<Span>,
}

/// Every open task across the daily notes, newest day first.
pub fn open_tasks_in_dailies(root: &Path) -> Vec<TaskRef> {
    let mut days: Vec<NaiveDate> = days_with_notes(root).into_iter().collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    let mut tasks = Vec::new();
    for date in days {
        let Some(path) = daily_file(root, date) else { continue };
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for (line_idx, raw) in text.lines().enumerate() {
            if let Line::Task { state: TaskState::Open, spans } = parse_line(raw) {
                tasks.push(TaskRef {
                    path: path.clone(),
                    date,
                    line_idx,
                    line: raw.to_string(),
                    spans,
                });
            }
        }
    }
    tasks
}

/// One visible row of the Notes browser.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEntry {
    pub path: PathBuf,
    /// Display name: folder name, or file stem without extension.
    pub name: String,
    pub is_dir: bool,
    /// NotePlan `@`-special folder (Archive, Templates, Trash…), shown last
    /// and de-emphasized.
    pub special: bool,
    pub depth: usize,
}

/// The visible rows of the `Notes/` tree given the expanded folders: folders
/// before files, alphabetical, `@`-special folders last within their level.
pub fn notes_tree(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<NoteEntry> {
    let mut rows = Vec::new();
    push_tree_level(&root.join("Notes"), 0, expanded, &mut rows);
    rows
}

fn push_tree_level(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<NoteEntry>,
) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut items: Vec<(bool, bool, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let is_dir = path.is_dir();
        if !is_dir {
            let ext = path.extension().and_then(|x| x.to_str());
            if !matches!(ext, Some("md") | Some("txt")) {
                continue;
            }
        }
        // Sort key: specials last, then folders before files, then name.
        items.push((name.starts_with('@'), !is_dir, name.to_lowercase(), path));
    }
    items.sort();
    for (special, not_dir, _, path) in items {
        let is_dir = !not_dir;
        let name = if is_dir {
            path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
        } else {
            path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string()
        };
        rows.push(NoteEntry { path: path.clone(), name, is_dir, special, depth });
        if is_dir && expanded.contains(&path) {
            push_tree_level(&path, depth + 1, expanded, rows);
        }
    }
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

// ----- writeback -----

/// Append a captured line to a day's note as an open task, creating the file
/// if the day has none yet. Returns the file written.
pub fn append_to_day(root: &Path, date: NaiveDate, text: &str) -> io::Result<PathBuf> {
    let path = daily_file(root, date).unwrap_or_else(|| daily_path(root, date));
    let mut content = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("* ");
    content.push_str(text.trim());
    content.push('\n');
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    atomic_write(&path, &content)?;
    Ok(path)
}

/// Write a whole note, atomically, ensuring a trailing newline.
pub fn write_note(path: &Path, content: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if content.is_empty() || content.ends_with('\n') {
        atomic_write(path, content)
    } else {
        atomic_write(path, &format!("{content}\n"))
    }
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = path.with_file_name(format!(".{}.kairn-tmp", name.to_string_lossy()));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

/// Toggle one task line between open and done, writing in the line's own
/// style: indentation and list marker are preserved, only the bracket and the
/// `@done(...)` stamp change. `* task` becomes `* [x] task @done(now)`; a done
/// task reopens as `[ ]` with the stamp stripped (the `-` marker needs the
/// bracket to stay a task at all, and `[ ]` reads identically everywhere).
/// Returns `None` for anything that isn't an open or done task.
pub fn toggle_task_line(line: &str, now: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let marker = ["* ", "+ ", "- "].iter().find(|m| rest.starts_with(**m))?;
    let body = &rest[2..];
    let gap_len = body.len() - body.trim_start().len();
    let (gap, body) = body.split_at(gap_len);
    match bracket_state(body) {
        Some(TaskState::Open) => {
            let content = body[3..].trim_end();
            Some(format!("{indent}{marker}{gap}[x]{content} @done({now})"))
        }
        Some(TaskState::Done) => {
            let content = strip_done_stamps(&body[3..]);
            Some(format!("{indent}{marker}{gap}[ ]{}", content.trim_end()))
        }
        Some(_) => None,
        None if *marker == "- " || body.is_empty() => None,
        None => Some(format!("{indent}{marker}{gap}[x] {} @done({now})", body.trim_end())),
    }
}

/// Remove every well-formed ` @done(...)` from a line's content.
fn strip_done_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("@done(") {
        let Some(close) = rest[pos..].find(')') else { break };
        let before = &rest[..pos];
        out.push_str(before.strip_suffix(' ').unwrap_or(before));
        rest = &rest[pos + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Toggle the task at `line_idx` in a note's full text. `expected` is the line
/// as it read when rendered: if the text has changed underneath us and the
/// line has moved, it is found again by content; if it's gone, `None` (the
/// caller reloads rather than guessing). Line endings are preserved.
pub fn toggle_task_in_text(
    text: &str,
    line_idx: usize,
    expected: &str,
    now: &str,
) -> Option<String> {
    fn content(seg: &str) -> &str {
        let s = seg.strip_suffix('\n').unwrap_or(seg);
        s.strip_suffix('\r').unwrap_or(s)
    }
    let segs: Vec<&str> = text.split_inclusive('\n').collect();
    let idx = if segs.get(line_idx).is_some_and(|s| content(s) == expected) {
        line_idx
    } else {
        segs.iter().position(|s| content(s) == expected)?
    };
    let line = content(segs[idx]);
    let ending = &segs[idx][line.len()..];
    let new_line = toggle_task_line(line, now)?;
    let mut out = String::with_capacity(text.len() + 32);
    for (i, seg) in segs.iter().enumerate() {
        if i == idx {
            out.push_str(&new_line);
            out.push_str(ending);
        } else {
            out.push_str(seg);
        }
    }
    Some(out)
}

/// Toggle a task in a note on disk. The file is re-read fresh so a change made
/// since it was rendered is never clobbered: the single-line edit is re-applied
/// against current content, and if the line no longer exists nothing is
/// written. The write is atomic (temp file + rename). Returns whether a
/// change was applied.
pub fn toggle_task_on_disk(path: &Path, line_idx: usize, expected: &str) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let Some(new_text) = toggle_task_in_text(&text, line_idx, expected, &now) else {
        return Ok(false);
    };
    atomic_write(path, &new_text)?;
    Ok(true)
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
    fn toggle_line_styles() {
        let now = "2026-08-06 21:30";
        // Bare NotePlan task and checklist gain a bracket and a stamp.
        assert_eq!(
            toggle_task_line("* buy milk", now).as_deref(),
            Some("* [x] buy milk @done(2026-08-06 21:30)")
        );
        assert_eq!(
            toggle_task_line("+ pack bag", now).as_deref(),
            Some("+ [x] pack bag @done(2026-08-06 21:30)")
        );
        // Bracketed style keeps its marker, indentation survives.
        assert_eq!(
            toggle_task_line("  - [ ] call bank", now).as_deref(),
            Some("  - [x] call bank @done(2026-08-06 21:30)")
        );
        // Reopening strips the stamp and keeps a bracket.
        assert_eq!(
            toggle_task_line("* [x] shipped @done(2026-08-06 18:00)", now).as_deref(),
            Some("* [ ] shipped")
        );
        assert_eq!(
            toggle_task_line("- [x] paid @done(2026-08-05) again @done(2026-08-06)", now)
                .as_deref(),
            Some("- [ ] paid again")
        );
        // Not toggleable: bullets, scheduled, cancelled, plain text.
        assert_eq!(toggle_task_line("- just a bullet", now), None);
        assert_eq!(toggle_task_line("* [>] moved", now), None);
        assert_eq!(toggle_task_line("+ [-] cancelled", now), None);
        assert_eq!(toggle_task_line("plain text", now), None);
    }

    #[test]
    fn toggle_in_text_tracks_moved_lines() {
        let now = "2026-08-06 21:30";
        let text = "# Day\n* one\n* two\n";
        // Straightforward: index matches.
        assert_eq!(
            toggle_task_in_text(text, 2, "* two", now).as_deref(),
            Some("# Day\n* one\n* [x] two @done(2026-08-06 21:30)\n")
        );
        // A line was inserted above since render: found again by content.
        let shifted = "# Day\nnew line\n* one\n* two\n";
        assert_eq!(
            toggle_task_in_text(shifted, 2, "* two", now).as_deref(),
            Some("# Day\nnew line\n* one\n* [x] two @done(2026-08-06 21:30)\n")
        );
        // The line is gone: nothing is written.
        assert_eq!(toggle_task_in_text("# Day\n* other\n", 1, "* two", now), None);
        // No trailing newline is preserved as-is.
        assert_eq!(
            toggle_task_in_text("* one", 0, "* one", now).as_deref(),
            Some("* [x] one @done(2026-08-06 21:30)")
        );
    }

    #[test]
    fn day_filenames() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid date");
        assert!(daily_path(Path::new("/root"), d).ends_with("Calendar/20260806.md"));
    }
}
