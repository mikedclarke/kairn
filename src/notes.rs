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

// ----- links, mentions, search -----

/// What a `[[wiki link]]` target refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum WikiTarget {
    /// `[[YYYY-MM-DD]]`: that day's daily note.
    Day(NaiveDate),
    /// An existing note under `Notes/`.
    Note(PathBuf),
    /// No note has this title yet; the path one would be created at.
    Missing(PathBuf),
}

/// The note title inside a wiki-link span: brackets stripped, any
/// `#heading` or `|alias` suffix dropped.
pub fn wiki_link_title(span_text: &str) -> &str {
    let inner = span_text.strip_prefix("[[").unwrap_or(span_text);
    let inner = inner.strip_suffix("]]").unwrap_or(inner);
    inner.split(['#', '|']).next().unwrap_or(inner).trim()
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

/// Every note file under `Notes/` with its folder depth, recursively,
/// deterministic order. `@Trash` is skipped everywhere links and search are
/// concerned.
fn notes_files(root: &Path) -> Vec<(usize, PathBuf)> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(usize, PathBuf)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') || name == "@Trash" {
                continue;
            }
            if path.is_dir() {
                walk(&path, depth + 1, out);
            } else if matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("md") | Some("txt")
            ) {
                out.push((depth, path));
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("Notes"), 0, &mut out);
    out
}

/// Resolve a wiki-link title: an ISO date goes to that day, anything else
/// matches a note stem case-insensitively anywhere under `Notes/` (the
/// shallowest match wins), and an unknown title yields the path a new note
/// would be created at.
pub fn resolve_wiki_target(root: &Path, title: &str) -> WikiTarget {
    if let Ok(date) = NaiveDate::parse_from_str(title, "%Y-%m-%d") {
        return WikiTarget::Day(date);
    }
    let lower = title.to_lowercase();
    let best = notes_files(root)
        .into_iter()
        .filter(|(_, p)| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.to_lowercase() == lower)
        })
        .min_by(|a, b| a.cmp(b));
    match best {
        Some((_, p)) => WikiTarget::Note(p),
        None => WikiTarget::Missing(root.join("Notes").join(format!("{title}.md"))),
    }
}

/// A line in another note that references this one.
#[derive(Clone, Debug)]
pub struct Mention {
    pub path: PathBuf,
    /// Daily notes carry their date, for navigation.
    pub date: Option<NaiveDate>,
    /// Source label: the note's stem, or the day spelled out.
    pub name: String,
    pub spans: Vec<Span>,
}

/// Every line elsewhere that links to `title`: a `[[title]]` in any form,
/// plus `>date` references when the title is a day's `YYYY-MM-DD`. The note
/// itself is excluded; dailies come newest first, then notes in tree order.
pub fn mentions_of(root: &Path, title: &str, exclude: Option<&Path>) -> Vec<Mention> {
    let lower = title.to_lowercase();
    let mut out = Vec::new();
    let mut scan = |path: &Path, date: Option<NaiveDate>, name: &str| {
        if Some(path) == exclude {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else { return };
        for raw in text.lines() {
            // Cheap gate; the span check below is authoritative.
            if !raw.to_lowercase().contains(&lower) {
                continue;
            }
            let spans = match parse_line(raw) {
                Line::Heading { spans, .. }
                | Line::Task { spans, .. }
                | Line::Bullet { spans }
                | Line::Quote { spans }
                | Line::Text { spans } => spans,
                Line::Rule | Line::Blank => continue,
            };
            let hit = spans.iter().any(|(kind, s)| match kind {
                SpanKind::WikiLink => wiki_link_title(s).to_lowercase() == lower,
                SpanKind::DateRef => s.strip_prefix('>').is_some_and(|d| d == title),
                _ => false,
            });
            if hit {
                out.push(Mention {
                    path: path.to_path_buf(),
                    date,
                    name: name.to_string(),
                    spans,
                });
            }
        }
    };
    let mut days: Vec<NaiveDate> = days_with_notes(root).into_iter().collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    for date in days {
        let Some(path) = daily_file(root, date) else { continue };
        scan(&path, Some(date), &date.format("%-d %b %Y").to_string());
    }
    for (_, path) in notes_files(root) {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        scan(&path, None, &name);
    }
    out
}

/// Score `needle` against `haystack` as a case-insensitive subsequence,
/// quick-switcher style: `None` when the characters don't all appear in
/// order, otherwise higher is better. Consecutive runs and matches at word
/// starts score up; skipped characters and longer targets score down, so
/// `kprd` ranks `kairn-prd` above a scattered match.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i64> {
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    if n.is_empty() {
        return Some(0);
    }
    let mut score = 0i64;
    let mut hi = 0usize;
    let mut prev_match: Option<usize> = None;
    for &nc in &n {
        let start = hi;
        while hi < h.len() && h[hi] != nc {
            hi += 1;
        }
        if hi >= h.len() {
            return None;
        }
        let consecutive = prev_match.is_some_and(|p| p + 1 == hi);
        let word_start =
            hi == 0 || matches!(h[hi - 1], ' ' | '-' | '_' | '/' | '.' | '(' | '[');
        score += 4;
        if consecutive {
            score += 6;
        }
        if word_start {
            score += 8;
        }
        score -= (hi - start).min(4) as i64;
        prev_match = Some(hi);
        hi += 1;
    }
    score -= (h.len() as i64 - n.len() as i64) / 4;
    Some(score)
}

/// One switcher search result.
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    /// Set when the hit is a daily note.
    pub date: Option<NaiveDate>,
    pub name: String,
    /// The matching body line, when the match wasn't in the title.
    pub snippet: Option<String>,
}

/// Search over every note: fuzzy title matches first, best score wins
/// (ties by name), then one plain-substring body match per file (dailies
/// newest first, then notes), capped at `limit`.
pub fn search_notes(root: &Path, query: &str, limit: usize) -> Vec<SearchHit> {
    let trimmed = query.trim();
    let q = trimmed.to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let files = notes_files(root);
    let mut titled: Vec<(i64, SearchHit)> = files
        .iter()
        .filter_map(|(_, path)| {
            let stem = path.file_stem().and_then(|s| s.to_str())?;
            let score = fuzzy_score(trimmed, stem)?;
            Some((score, SearchHit {
                path: path.clone(),
                date: None,
                name: stem.to_string(),
                snippet: None,
            }))
        })
        .collect();
    titled.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    let mut out: Vec<SearchHit> =
        titled.into_iter().take(limit).map(|(_, hit)| hit).collect();
    if out.len() >= limit {
        return out;
    }
    let mut days: Vec<NaiveDate> = days_with_notes(root).into_iter().collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    let dailies = days
        .into_iter()
        .filter_map(|d| daily_file(root, d).map(|p| (p, Some(d))));
    let title_hits: HashSet<PathBuf> = out.iter().map(|h| h.path.clone()).collect();
    let notes = files
        .into_iter()
        .filter(|(_, p)| !title_hits.contains(p))
        .map(|(_, p)| (p, None));
    for (path, date) in dailies.chain(notes) {
        if out.len() >= limit {
            break;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Some(line) = text.lines().find(|l| l.to_lowercase().contains(&q)) else {
            continue;
        };
        let name = match date {
            Some(d) => d.format("%-d %b %Y").to_string(),
            None => path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
        };
        out.push(SearchHit { path, date, name, snippet: Some(line.trim().to_string()) });
    }
    out
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

/// Rewrite one line of a note's full text. `expected` is the line as it read
/// when rendered: if the text has changed underneath us and the line has
/// moved, it is found again by content; if it's gone, `None` (the caller
/// reloads rather than guessing). `edit` maps the current line to its
/// replacement (which may span multiple lines); line endings are preserved.
fn edit_line_in_text(
    text: &str,
    line_idx: usize,
    expected: &str,
    edit: impl FnOnce(&str) -> Option<String>,
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
    let new_line = edit(line)?;
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

/// Toggle the task at `line_idx` between open and done. See
/// [`edit_line_in_text`] for the relocation contract.
pub fn toggle_task_in_text(
    text: &str,
    line_idx: usize,
    expected: &str,
    now: &str,
) -> Option<String> {
    edit_line_in_text(text, line_idx, expected, |line| toggle_task_line(line, now))
}

/// Replace the line at `line_idx` with `new_line` (which may contain
/// newlines, splitting the line). Relocates by content like toggling; `None`
/// when the line is gone. Writes atomically. Returns whether a change landed.
pub fn replace_line_on_disk(
    path: &Path,
    line_idx: usize,
    expected: &str,
    new_line: &str,
) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let Some(new_text) =
        edit_line_in_text(&text, line_idx, expected, |_| Some(new_line.to_string()))
    else {
        return Ok(false);
    };
    atomic_write(path, &new_text)?;
    Ok(true)
}

/// Replace two adjacent lines with one `replacement` line. The pair is
/// verified (or found again) by content like [`edit_line_in_text`]; `None`
/// when the adjacent pair no longer exists.
fn join_lines_in_text(
    text: &str,
    first_idx: usize,
    expected_first: &str,
    expected_second: &str,
    replacement: &str,
) -> Option<String> {
    fn content(seg: &str) -> &str {
        let s = seg.strip_suffix('\n').unwrap_or(seg);
        s.strip_suffix('\r').unwrap_or(s)
    }
    let segs: Vec<&str> = text.split_inclusive('\n').collect();
    let pair_at = |i: usize| {
        segs.get(i).is_some_and(|s| content(s) == expected_first)
            && segs.get(i + 1).is_some_and(|s| content(s) == expected_second)
    };
    let idx = if pair_at(first_idx) {
        first_idx
    } else {
        (0..segs.len().saturating_sub(1)).find(|&i| pair_at(i))?
    };
    let second = segs[idx + 1];
    let ending = &second[content(second).len()..];
    let mut out = String::with_capacity(text.len());
    for (i, seg) in segs.iter().enumerate() {
        if i == idx {
            out.push_str(replacement);
            out.push_str(ending);
        } else if i != idx + 1 {
            out.push_str(seg);
        }
    }
    Some(out)
}

/// Join two adjacent lines of a note into `replacement`, atomically. Returns
/// whether a change landed (`false` when the pair is gone from the file).
pub fn join_lines_on_disk(
    path: &Path,
    first_idx: usize,
    expected_first: &str,
    expected_second: &str,
    replacement: &str,
) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let Some(new_text) =
        join_lines_in_text(&text, first_idx, expected_first, expected_second, replacement)
    else {
        return Ok(false);
    };
    atomic_write(path, &new_text)?;
    Ok(true)
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
    fn edit_line_replaces_and_splits() {
        let text = "# Day\n* one\n* two\n";
        assert_eq!(
            edit_line_in_text(text, 1, "* one", |_| Some("* one edited".into())).as_deref(),
            Some("# Day\n* one edited\n* two\n")
        );
        // A replacement containing a newline splits the line in place.
        assert_eq!(
            edit_line_in_text(text, 1, "* one", |_| Some("* one\n* [ ] ".into())).as_deref(),
            Some("# Day\n* one\n* [ ] \n* two\n")
        );
        // Vanished line: no edit.
        assert_eq!(edit_line_in_text(text, 1, "* gone", |_| Some("x".into())), None);
    }

    #[test]
    fn join_lines() {
        let text = "# Day\n* one\n* two\n* three\n";
        // Straightforward adjacent pair.
        assert_eq!(
            join_lines_in_text(text, 1, "* one", "* two", "* onetwo").as_deref(),
            Some("# Day\n* onetwo\n* three\n")
        );
        // Pair moved since render: found again by content.
        let shifted = "new\n# Day\n* one\n* two\n* three\n";
        assert_eq!(
            join_lines_in_text(shifted, 1, "* one", "* two", "* onetwo").as_deref(),
            Some("new\n# Day\n* onetwo\n* three\n")
        );
        // Second line changed underneath: nothing is written.
        assert_eq!(join_lines_in_text(text, 1, "* one", "* other", "x"), None);
        // Joining the last pair with no trailing newline keeps it that way.
        assert_eq!(
            join_lines_in_text("* a\n* b", 0, "* a", "* b", "* ab").as_deref(),
            Some("* ab")
        );
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

    /// A scratch notes root with a NotePlan-shaped tree, removed on drop.
    struct ScratchRoot(PathBuf);

    impl ScratchRoot {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!("kairn-test-{tag}-{nanos}"));
            ensure_layout(&root);
            Self(root)
        }

        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(&path, content).expect("write");
            path
        }
    }

    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn wiki_link_titles() {
        assert_eq!(wiki_link_title("[[kairn prd]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[kairn prd#phases]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[kairn prd|the plan]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[ padded ]]"), "padded");
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

    #[test]
    fn wiki_resolution() {
        let root = ScratchRoot::new("resolve");
        let prd = root.write("Notes/Kairn PRD.md", "# Kairn PRD\n");
        root.write("Notes/deep/Kairn PRD.md", "# duplicate deeper\n");
        root.write("Notes/@Trash/Ghost.md", "# trashed\n");

        // Case-insensitive stem match; the shallowest copy wins.
        assert_eq!(
            resolve_wiki_target(&root.0, "kairn prd"),
            WikiTarget::Note(prd)
        );
        // ISO dates are days, whether or not a file exists.
        assert_eq!(
            resolve_wiki_target(&root.0, "2026-08-07"),
            WikiTarget::Day(NaiveDate::from_ymd_opt(2026, 8, 7).expect("valid"))
        );
        // Trash never resolves; unknown titles point at the file to create.
        assert_eq!(
            resolve_wiki_target(&root.0, "Ghost"),
            WikiTarget::Missing(root.0.join("Notes/Ghost.md"))
        );
    }

    #[test]
    fn mentions_find_links_and_daterefs() {
        let root = ScratchRoot::new("mentions");
        let plan = root.write("Notes/Plan.md", "# Plan\n");
        root.write("Notes/Other.md", "see [[plan#top]] for detail\nno link here\n");
        root.write("Calendar/20260806.md", "* review [[Plan]]\n");
        root.write("Calendar/20260807.md", "* ship >2026-08-09\n");

        let hits = mentions_of(&root.0, "Plan", Some(&plan));
        assert_eq!(hits.len(), 2);
        // Dailies first, then notes.
        assert_eq!(hits[0].date, NaiveDate::from_ymd_opt(2026, 8, 6));
        assert_eq!(hits[1].name, "Other");

        // A day's mentions include schedule refs pointing at it.
        let day_hits = mentions_of(&root.0, "2026-08-09", None);
        assert_eq!(day_hits.len(), 1);
        assert_eq!(day_hits[0].date, NaiveDate::from_ymd_opt(2026, 8, 7));

        // The note itself is excluded.
        root.write("Notes/Plan.md", "# Plan\nself link [[plan]]\n");
        assert_eq!(mentions_of(&root.0, "Plan", Some(&plan)).len(), 2);
    }

    #[test]
    fn fuzzy_scoring() {
        // In-order subsequences match, missing or out-of-order chars don't.
        assert!(fuzzy_score("kprd", "kairn-prd").is_some());
        assert_eq!(fuzzy_score("kprdx", "kairn-prd"), None);
        assert_eq!(fuzzy_score("drp", "kairn-prd"), None);
        // Case-insensitive.
        assert!(fuzzy_score("ALPHA", "Alpha project").is_some());
        // A consecutive run outranks the same chars scattered mid-word.
        let tight = fuzzy_score("plan", "planning").expect("matches");
        let scattered = fuzzy_score("plan", "pxlxaxnx").expect("matches");
        assert!(tight > scattered);
        // Word-start matches outrank mid-word ones.
        let word_start = fuzzy_score("kp", "kairn prd").expect("matches");
        let mid_word = fuzzy_score("kp", "akkkp").expect("matches");
        assert!(word_start > mid_word);
    }

    #[test]
    fn search_ranks_fuzzy_titles() {
        let root = ScratchRoot::new("fuzzy");
        root.write("Notes/kairn-prd.md", "spec\n");
        root.write("Notes/kitchen plans and records.md", "stuff\n");

        // Both titles contain k,p,r,d in order; the tight match ranks first.
        let hits = search_notes(&root.0, "kprd", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "kairn-prd");
        assert_eq!(hits[1].name, "kitchen plans and records");
    }

    #[test]
    fn search_titles_then_content() {
        let root = ScratchRoot::new("search");
        root.write("Notes/Alpha project.md", "body without the word\n");
        root.write("Notes/Beta.md", "mentions alpha inline\n");
        root.write("Calendar/20260805.md", "* alpha task\n");

        let hits = search_notes(&root.0, "ALPHA", 10);
        assert_eq!(hits.len(), 3);
        // Title match first, no snippet.
        assert_eq!(hits[0].name, "Alpha project");
        assert_eq!(hits[0].snippet, None);
        // Then body matches: the daily before the note, snippets trimmed.
        assert_eq!(hits[1].date, NaiveDate::from_ymd_opt(2026, 8, 5));
        assert_eq!(hits[1].snippet.as_deref(), Some("* alpha task"));
        assert_eq!(hits[2].name, "Beta");

        assert_eq!(search_notes(&root.0, "  ", 10).len(), 0);
        assert_eq!(search_notes(&root.0, "alpha", 1).len(), 1);
    }

    #[test]
    fn day_filenames() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid date");
        assert!(daily_path(Path::new("/root"), d).ends_with("Calendar/20260806.md"));
    }
}
