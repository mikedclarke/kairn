//! Wiki-link resolution and linked mentions.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::parse::{Line, SpanKind, Span, parse_line};
use crate::vault::{daily_file, days_with_notes, notes_files, period_files, period_stem};

/// What a `[[wiki link]]` target refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum WikiTarget {
    /// `[[YYYY-MM-DD]]`: that day's daily note.
    Day(NaiveDate),
    /// An existing note under `Notes/`.
    Note(PathBuf),
    /// No note has this title yet; the path one would be created at.
    Missing(PathBuf),
    /// The title cannot name a note (path escapes, hidden components).
    /// Links are untrusted input from synced files; never create from these.
    Invalid,
}

/// The note title inside a wiki-link span: brackets stripped, any
/// `#heading` or `|alias` suffix dropped.
pub fn wiki_link_title(span_text: &str) -> &str {
    let inner = span_text.strip_prefix("[[").unwrap_or(span_text);
    let inner = inner.strip_suffix("]]").unwrap_or(inner);
    inner.split(['#', '|']).next().unwrap_or(inner).trim()
}

/// Resolve a wiki-link title: an ISO date goes to that day, anything else
/// matches a note stem case-insensitively anywhere under `Notes/` (the
/// shallowest match wins), a folder-qualified title like `Projects/Kairn`
/// matches its full path under `Notes/`, and an unknown title yields the
/// path a new note would be created at, provided the title stays under the
/// notes root.
pub fn resolve_wiki_target(root: &Path, title: &str) -> WikiTarget {
    if let Ok(date) = NaiveDate::parse_from_str(title, "%Y-%m-%d") {
        return WikiTarget::Day(date);
    }
    // Weekly/monthly/quarterly/yearly titles live in Calendar/, like
    // NotePlan's own period notes.
    if let Some(canon) = period_stem(title) {
        let md = root.join("Calendar").join(format!("{canon}.md"));
        if md.exists() {
            return WikiTarget::Note(md);
        }
        let txt = md.with_extension("txt");
        if txt.exists() {
            return WikiTarget::Note(txt);
        }
        return WikiTarget::Missing(md);
    }
    let lower = title.to_lowercase();
    let files = notes_files(root);
    let best = files
        .iter()
        .filter(|(_, p)| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.to_lowercase() == lower)
        })
        .min_by(|a, b| a.cmp(b));
    if let Some((_, p)) = best {
        return WikiTarget::Note(p.clone());
    }
    if title.contains('/') {
        let notes_dir = root.join("Notes");
        let hit = files.iter().find(|(_, p)| {
            p.strip_prefix(&notes_dir).ok().is_some_and(|rel| {
                rel.with_extension("")
                    .to_str()
                    .is_some_and(|s| s.to_lowercase() == lower)
            })
        });
        if let Some((_, p)) = hit {
            return WikiTarget::Note(p.clone());
        }
    }
    match creatable_note_path(root, title) {
        Some(path) => WikiTarget::Missing(path),
        None => WikiTarget::Invalid,
    }
}

/// The path a new note for `title` would be created at, or `None` when the
/// title cannot safely name a file under `Notes/`: empty or dot-leading
/// components (`.`, `..`, hidden files the scans would never show) and
/// backslashes are rejected, so a link in a synced note can never write
/// outside the notes root.
fn creatable_note_path(root: &Path, title: &str) -> Option<PathBuf> {
    if title.contains('\\') {
        return None;
    }
    let components: Vec<&str> = title.split('/').map(str::trim).collect();
    if components
        .iter()
        .any(|part| part.is_empty() || part.starts_with('.'))
    {
        return None;
    }
    let mut path = root.join("Notes");
    let (last, dirs) = components.split_last()?;
    for dir in dirs {
        path.push(dir);
    }
    path.push(format!("{last}.md"));
    Some(path)
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
/// itself is excluded; dailies come newest first, then period notes, then
/// notes in tree order.
pub fn mentions_of(root: &Path, title: &str, exclude: Option<&Path>) -> Vec<Mention> {
    let lower = title.to_lowercase();
    let mut out = Vec::new();
    let mut scan = |path: &Path, date: Option<NaiveDate>, name: &str| {
        if Some(path) == exclude {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else { return };
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
    for (name, path) in period_files(root) {
        scan(&path, None, &name);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;
    use crate::vault::search_notes;

    #[test]
    fn wiki_link_titles() {
        assert_eq!(wiki_link_title("[[kairn prd]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[kairn prd#phases]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[kairn prd|the plan]]"), "kairn prd");
        assert_eq!(wiki_link_title("[[ padded ]]"), "padded");
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
    fn wiki_resolution_folder_titles_and_escapes() {
        let root = ScratchRoot::new("resolve-safe");
        let nested = root.write("Notes/Projects/Kairn.md", "# a year of notes\n");

        // A folder-qualified title resolves to the existing nested note
        // instead of "missing" (which used to truncate it on click).
        assert_eq!(
            resolve_wiki_target(&root.0, "Projects/Kairn"),
            WikiTarget::Note(nested.clone())
        );
        assert_eq!(
            resolve_wiki_target(&root.0, "projects/kairn"),
            WikiTarget::Note(nested)
        );
        // An unknown folder-qualified title creates inside Notes/.
        assert_eq!(
            resolve_wiki_target(&root.0, "Projects/New Idea"),
            WikiTarget::Missing(root.0.join("Notes/Projects/New Idea.md"))
        );
        // Titles that would escape the notes root or hide the file never
        // yield a creatable path: links are untrusted input.
        for title in [
            "../Calendar/20260807",
            "..",
            ".",
            "a/../../etc/passwd",
            "a//b",
            ".hidden",
            "dir/.hidden",
            "back\\slash",
            "",
        ] {
            assert_eq!(resolve_wiki_target(&root.0, title), WikiTarget::Invalid, "{title:?}");
        }
    }

    #[test]
    fn period_notes_are_reachable() {
        let root = ScratchRoot::new("periods");
        let weekly = root.write("Calendar/2026-W32.md", "## Focus\nship the review fixes\n");
        root.write("Calendar/20260806.md", "* review [[2026-W32]]\n");

        // Wiki links resolve to the period note, case-insensitively; a
        // missing period resolves to the file it would be created at.
        assert_eq!(
            resolve_wiki_target(&root.0, "2026-W32"),
            WikiTarget::Note(weekly.clone())
        );
        assert_eq!(
            resolve_wiki_target(&root.0, "2026-w32"),
            WikiTarget::Note(weekly.clone())
        );
        assert_eq!(
            resolve_wiki_target(&root.0, "2026-Q4"),
            WikiTarget::Missing(root.0.join("Calendar/2026-Q4.md"))
        );

        // Search finds it by title and by body content.
        let hits = search_notes(&root.0, "2026-W32", 10);
        assert!(hits.iter().any(|h| h.path == weekly), "title hit");
        let hits = search_notes(&root.0, "review fixes", 10);
        assert!(hits.iter().any(|h| h.path == weekly), "body hit");

        // Mentions of the weekly note find the daily's link to it.
        let mentions = mentions_of(&root.0, "2026-W32", Some(&weekly));
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].date, NaiveDate::from_ymd_opt(2026, 8, 6));
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
    fn dateref_mentions_ignore_punctuation() {
        let root = ScratchRoot::new("dateref");
        root.write("Calendar/20260807.md", "* ship >2026-08-09.\n");
        assert_eq!(mentions_of(&root.0, "2026-08-09", None).len(), 1);
    }
}
