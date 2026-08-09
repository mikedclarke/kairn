//! Disk writes, all never-clobber: every path re-reads the file it is about
//! to change and verifies (or safely relocates) what it expects to find
//! there, so an edit rendered from a stale snapshot can't destroy content
//! written meanwhile. All writes are atomic (temp file + rename).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};

use crate::tasks::toggle_task_line;
use crate::vault::{daily_file, daily_path};

/// Append a captured line to a day's note as an open task, creating the file
/// if the day has none yet. A day from today onward that is created here
/// starts from the daily template, matching what the app shows for an empty
/// day, so a capture never flattens the template layout the user was
/// looking at. Returns the file written.
pub fn append_to_day(root: &Path, date: NaiveDate, text: &str) -> io::Result<PathBuf> {
    let path = daily_file(root, date).unwrap_or_else(|| daily_path(root, date));
    if !path.exists()
        && date >= Local::now().date_naive()
        && let Some(seed) = crate::template::daily_template(root)
    {
        // The day's masthead titles it, so drop a redundant leading `# title`.
        create_note_if_absent(&path, crate::template::strip_daily_title(&seed))?;
    }
    append_line(&path, &format!("* {}", text.trim()))?;
    Ok(path)
}

/// Capture a line of input into a day's note: the quick-capture flow the app
/// and the CLI share. Blank input is a no-op; anything else lands as an open
/// task. Returns the file written, `None` when there was nothing to write.
pub fn capture(root: &Path, date: NaiveDate, text: &str) -> io::Result<Option<PathBuf>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    append_to_day(root, date, text).map(Some)
}

/// Append `line` (which may contain newlines) to a note, re-reading the file
/// first so a buffer that went stale since render can never clobber content
/// written meanwhile. A missing file is created. The file's own trailing
/// newline and line-ending conventions are preserved. Returns the index the
/// appended text starts at.
pub fn append_line(path: &Path, line: &str) -> io::Result<usize> {
    let mut text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let idx = text.lines().count();
    let crlf = text.contains("\r\n");
    let ending = if crlf { "\r\n" } else { "\n" };
    let line = if crlf {
        line.replace('\n', "\r\n")
    } else {
        line.to_string()
    };
    let trailing_newline = text.is_empty() || text.ends_with('\n');
    if !trailing_newline {
        text.push_str(ending);
    }
    text.push_str(&line);
    if trailing_newline {
        text.push_str(ending);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    atomic_write(path, &text)?;
    Ok(idx)
}

/// Create a note only if nothing exists at `path`; an existing file is left
/// untouched (a wiki link in a synced note must never overwrite a real
/// note). Returns whether the file was created.
pub fn create_note_if_absent(path: &Path, content: &str) -> io::Result<bool> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    match fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(content.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Write a note's full content atomically (temp file + rename, permissions
/// preserved). The whole-buffer save path: callers own staleness protection
/// via `NoteBuffer::reconcile`, which is the only sanctioned route here for
/// content that was edited from a snapshot.
pub fn write_note(path: &Path, content: &str) -> io::Result<()> {
    atomic_write(path, content)
}

/// A file stem the user typed for a new or renamed note, checked before it
/// touches the filesystem: non-empty, no path separators, and not dot- or
/// `@`-prefixed (hidden files and NotePlan's special folders).
fn checked_stem(stem: &str) -> io::Result<&str> {
    let stem = stem.trim();
    let bad = stem.is_empty()
        || stem.contains(['/', '\\'])
        || stem.starts_with('.')
        || stem.starts_with('@');
    if bad {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "note names can't be empty or start with '.' or '@', and can't contain '/'",
        ));
    }
    Ok(stem)
}

/// Move a note into the vault's trash folder (`Notes/@Trash/`), NotePlan's
/// soft-delete convention; nothing here ever hard-deletes. A name collision
/// in the trash gets a numbered suffix. Returns where the note ended up.
pub fn trash_note(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let trash = root.join("Notes").join("@Trash");
    if path.starts_with(&trash) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "note is already in the trash",
        ));
    }
    fs::create_dir_all(&trash)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let stem = path.file_stem().unwrap_or(name).to_string_lossy().into_owned();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let mut dest = trash.join(name);
    let mut n = 2;
    while dest.exists() {
        let candidate = match &ext {
            Some(ext) => format!("{stem} {n}.{ext}"),
            None => format!("{stem} {n}"),
        };
        dest = trash.join(candidate);
        n += 1;
    }
    fs::rename(path, &dest)?;
    Ok(dest)
}

/// Rename a note in place: same folder, extension preserved. Never
/// overwrites — a stem that already names a sibling file is an error.
/// Returns the new path.
pub fn rename_note(path: &Path, new_stem: &str) -> io::Result<PathBuf> {
    let new_stem = checked_stem(new_stem)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("md");
    let dest = path.with_file_name(format!("{new_stem}.{ext}"));
    if dest == path {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("a note named \"{new_stem}\" already exists here"),
        ));
    }
    fs::rename(path, &dest)?;
    Ok(dest)
}

/// Create a note named by the user inside `dir`, seeded with a title
/// heading. An existing note of that name is left untouched and returned
/// as-is (same posture as wiki-link creation). Returns the note's path.
pub fn new_note_in(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let name = checked_stem(name)?;
    let path = dir.join(format!("{name}.md"));
    create_note_if_absent(&path, &format!("# {name}\n"))?;
    Ok(path)
}

/// Create a fresh, untitled note in `dir`, seeded with an empty `# ` heading
/// so the caret can land after it and the user just types the title — which
/// then renames the file (see [`note_title_stem`]), NotePlan-style. Picks the
/// first free "Untitled" name. Returns the note's path.
pub fn new_untitled_note_in(dir: &Path) -> io::Result<PathBuf> {
    let mut path = dir.join("Untitled.md");
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Untitled {n}.md"));
        n += 1;
    }
    create_note_if_absent(&path, "# \n")?;
    Ok(path)
}

/// The filename stem a note's title implies: the text of its first heading
/// (the `# Title` line new notes are seeded with), sanitised into something
/// safe to name a file. `None` when the first non-empty line isn't a heading,
/// or nothing usable is left after cleaning — the caller then leaves the name
/// alone. This is how a regular note's file follows its title.
pub fn note_title_stem(text: &str) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    // ATX headings need whitespace after the hashes; "#project" is a tag-like
    // first line, not a title.
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    sanitize_title_stem(rest.trim())
}

/// Turn heading text into a safe file stem: path separators and colons become
/// dashes, and the leading/trailing characters that would make a bad or hidden
/// filename are trimmed. `None` when nothing usable is left.
fn sanitize_title_stem(title: &str) -> Option<String> {
    let cleaned: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches(['.', '@'])
        .trim_end_matches('.')
        .trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    // The temp name carries pid + counter so two running instances (or two
    // quick writes) never collide on it.
    let tmp = path.with_file_name(format!(
        ".{}.kairn-tmp.{}.{}",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let write = (|| {
        {
            use std::io::Write as _;
            let mut file = fs::File::create(&tmp)?;
            file.write_all(content.as_bytes())?;
            // Rename is atomic for the *name*, not for the data behind it: with
            // no fsync here a power cut can leave the renamed file holding
            // stale or empty blocks, which is a silently lost edit rather than
            // a half-written note.
            file.sync_all()?;
        }
        // Rename replaces the inode; carry the original's permissions over
        // so a private note (0600) doesn't silently become world-readable.
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)?;
        // The new directory entry is durable only once the directory itself is
        // synced. Not every platform allows opening a directory for that, so a
        // failure here is not fatal.
        if let Some(dir) = path.parent() {
            let _ = fs::File::open(dir).and_then(|d| d.sync_all());
        }
        Ok(())
    })();
    if write.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write
}

/// Rewrite one line of a note's full text. `expected` is the line as it read
/// when rendered: if the text has changed underneath us and the line has
/// moved, it is found again by content, but only when the match is
/// unambiguous. Duplicate matches (blank lines above all) mean the right
/// line cannot be known, so nothing is edited and the caller reloads rather
/// than guessing. `edit` maps the current line to its replacement (which may
/// span multiple lines); line endings are preserved. Returns the new text
/// and the index the edit actually landed on, so a caller tracking the line
/// can stay honest after relocation.
fn edit_line_in_text(
    text: &str,
    line_idx: usize,
    expected: &str,
    edit: impl FnOnce(&str) -> Option<String>,
) -> Option<(String, usize)> {
    fn content(seg: &str) -> &str {
        let s = seg.strip_suffix('\n').unwrap_or(seg);
        s.strip_suffix('\r').unwrap_or(s)
    }
    let segs: Vec<&str> = text.split_inclusive('\n').collect();
    let idx = if segs.get(line_idx).is_some_and(|s| content(s) == expected) {
        line_idx
    } else {
        let mut matches = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| content(s) == expected)
            .map(|(i, _)| i);
        let only = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        only
    };
    let line = content(segs[idx]);
    let ending = &segs[idx][line.len()..];
    let new_line = edit(line)?;
    // A replacement that splits the line must split with the file's own
    // ending; injecting a bare LF into a CRLF file leaves mixed endings.
    let new_line = if text.contains("\r\n") {
        new_line.replace('\n', "\r\n")
    } else {
        new_line
    };
    let mut out = String::with_capacity(text.len() + 32);
    for (i, seg) in segs.iter().enumerate() {
        if i == idx {
            out.push_str(&new_line);
            out.push_str(ending);
        } else {
            out.push_str(seg);
        }
    }
    Some((out, idx))
}

/// Toggle the task at `line_idx` between open and done. See
/// [`edit_line_in_text`] for the relocation contract.
pub fn toggle_task_in_text(text: &str, line_idx: usize, expected: &str) -> Option<String> {
    edit_line_in_text(text, line_idx, expected, toggle_task_line)
        .map(|(new_text, _)| new_text)
}

/// Replace the line at `line_idx` with `new_line` (which may contain
/// newlines, splitting the line). Relocates by content like toggling; `None`
/// when the line is gone or ambiguous. Writes atomically. Returns the index
/// the replacement landed on, `None` when nothing was written.
pub fn replace_line_on_disk(
    path: &Path,
    line_idx: usize,
    expected: &str,
    new_line: &str,
) -> io::Result<Option<usize>> {
    let text = fs::read_to_string(path)?;
    let Some((new_text, idx)) =
        edit_line_in_text(&text, line_idx, expected, |_| Some(new_line.to_string()))
    else {
        return Ok(None);
    };
    atomic_write(path, &new_text)?;
    Ok(Some(idx))
}

/// Replace two adjacent lines with one `replacement` line. The pair is
/// verified (or found again, requiring a unique match) by content like
/// [`edit_line_in_text`]; `None` when the adjacent pair no longer exists or
/// is ambiguous. Returns the new text and the index the pair was found at.
fn join_lines_in_text(
    text: &str,
    first_idx: usize,
    expected_first: &str,
    expected_second: &str,
    replacement: &str,
) -> Option<(String, usize)> {
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
        let mut matches = (0..segs.len().saturating_sub(1)).filter(|&i| pair_at(i));
        let only = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        only
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
    Some((out, idx))
}

/// Join two adjacent lines of a note into `replacement`, atomically. Returns
/// the index the pair was found at, `None` when the pair is gone from the
/// file (or matched more than once) and nothing was written.
pub fn join_lines_on_disk(
    path: &Path,
    first_idx: usize,
    expected_first: &str,
    expected_second: &str,
    replacement: &str,
) -> io::Result<Option<usize>> {
    let text = fs::read_to_string(path)?;
    let Some((new_text, idx)) =
        join_lines_in_text(&text, first_idx, expected_first, expected_second, replacement)
    else {
        return Ok(None);
    };
    atomic_write(path, &new_text)?;
    Ok(Some(idx))
}

/// Move the line at `from_idx` so it sits before the line currently at
/// `before_idx` (`before_idx` at or past the end moves it to the end). The
/// moved line is verified (or relocated to a unique content match) like
/// every other edit; `None` when it is gone or ambiguous and nothing was
/// written. Returns the index the line landed on.
pub fn move_line_on_disk(
    path: &Path,
    from_idx: usize,
    expected: &str,
    before_idx: usize,
) -> io::Result<Option<usize>> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let from = if lines.get(from_idx).is_some_and(|l| *l == expected) {
        from_idx
    } else {
        let mut matches = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| **l == expected)
            .map(|(i, _)| i);
        let Some(only) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Ok(None);
        }
        only
    };
    let mut order: Vec<&str> = lines.clone();
    let moved = order.remove(from);
    let mut target = before_idx.min(lines.len());
    if target > from {
        target -= 1;
    }
    if target == from {
        return Ok(Some(from));
    }
    order.insert(target, moved);
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = order.join(ending);
    if text.ends_with('\n') {
        out.push_str(ending);
    }
    atomic_write(path, &out)?;
    Ok(Some(target))
}

/// Set the due date of the open task at `line_idx`, rewriting (or adding)
/// its `>date` token. Same relocation and staleness contract as
/// [`toggle_task_on_disk`]; atomic write. Returns whether a change was
/// applied — `false` also covers the task already being due that day.
pub fn reschedule_task_on_disk(
    path: &Path,
    line_idx: usize,
    expected: &str,
    due: NaiveDate,
) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let Some((new_text, _)) = edit_line_in_text(&text, line_idx, expected, |line| {
        crate::tasks::reschedule_task_line(line, due)
    }) else {
        return Ok(false);
    };
    atomic_write(path, &new_text)?;
    Ok(true)
}

/// Toggle a task in a note on disk. The file is re-read fresh so a change made
/// since it was rendered is never clobbered: the single-line edit is re-applied
/// against current content, and if the line no longer exists nothing is
/// written. The write is atomic (temp file + rename). Returns whether a
/// change was applied.
pub fn toggle_task_on_disk(path: &Path, line_idx: usize, expected: &str) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let Some(new_text) = toggle_task_in_text(&text, line_idx, expected) else {
        return Ok(false);
    };
    atomic_write(path, &new_text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    #[test]
    fn toggle_in_text_tracks_moved_lines() {
        let text = "# Day\n* one\n* two\n";
        // Straightforward: index matches.
        assert_eq!(
            toggle_task_in_text(text, 2, "* two").as_deref(),
            Some("# Day\n* one\n* [x] two\n")
        );
        // A line was inserted above since render: found again by content.
        let shifted = "# Day\nnew line\n* one\n* two\n";
        assert_eq!(
            toggle_task_in_text(shifted, 2, "* two").as_deref(),
            Some("# Day\nnew line\n* one\n* [x] two\n")
        );
        // The line is gone: nothing is written.
        assert_eq!(toggle_task_in_text("# Day\n* other\n", 1, "* two"), None);
        // No trailing newline is preserved as-is.
        assert_eq!(
            toggle_task_in_text("* one", 0, "* one").as_deref(),
            Some("* [x] one")
        );
    }

    #[test]
    fn trash_note_moves_and_numbers_collisions() {
        let root = ScratchRoot::new("trash");
        let a = root.write("Notes/Ideas.md", "# Ideas\nfirst\n");
        let dest = trash_note(&root.0, &a).expect("trash");
        assert_eq!(dest, root.0.join("Notes/@Trash/Ideas.md"));
        assert!(!a.exists());
        assert_eq!(fs::read_to_string(&dest).expect("read"), "# Ideas\nfirst\n");

        // A second note of the same name lands beside it, numbered.
        let b = root.write("Notes/Projects/Ideas.md", "# Ideas\nsecond\n");
        let dest2 = trash_note(&root.0, &b).expect("trash");
        assert_eq!(dest2, root.0.join("Notes/@Trash/Ideas 2.md"));
        assert_eq!(fs::read_to_string(&dest).expect("read"), "# Ideas\nfirst\n");

        // Trashing from the trash is refused.
        assert!(trash_note(&root.0, &dest).is_err());
    }

    #[test]
    fn rename_note_preserves_extension_and_refuses_overwrite() {
        let root = ScratchRoot::new("rename");
        let a = root.write("Notes/Old.txt", "# Old\n");
        let dest = rename_note(&a, "New").expect("rename");
        assert_eq!(dest, root.0.join("Notes/New.txt"));
        assert!(!a.exists());

        // The target name is taken: nothing moves.
        let b = root.write("Notes/Other.txt", "other\n");
        assert!(rename_note(&b, "New").is_err());
        assert!(b.exists());

        // Bad stems are refused before touching the disk.
        for bad in ["", "  ", "a/b", ".hidden", "@Trash"] {
            assert!(rename_note(&dest, bad).is_err(), "stem {bad:?} should fail");
        }
        // Renaming to the same name is a quiet no-op.
        assert_eq!(rename_note(&dest, "New").expect("noop"), dest);
    }

    #[test]
    fn new_note_in_seeds_title_and_keeps_existing() {
        let root = ScratchRoot::new("newnote");
        let dir = root.0.join("Notes");
        let path = new_note_in(&dir, " Meeting Notes ").expect("create");
        assert_eq!(path, dir.join("Meeting Notes.md"));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "# Meeting Notes\n"
        );
        // Creating again never overwrites what's there.
        fs::write(&path, "real content\n").expect("write");
        let again = new_note_in(&dir, "Meeting Notes").expect("existing");
        assert_eq!(again, path);
        assert_eq!(fs::read_to_string(&path).expect("read"), "real content\n");
    }

    #[test]
    fn untitled_note_picks_a_free_name() {
        let root = ScratchRoot::new("untitled");
        let dir = root.0.join("Notes");
        let first = new_untitled_note_in(&dir).expect("create");
        assert_eq!(first, dir.join("Untitled.md"));
        assert_eq!(fs::read_to_string(&first).expect("read"), "# \n");
        // A second one steps to a numbered name rather than colliding.
        let second = new_untitled_note_in(&dir).expect("create");
        assert_eq!(second, dir.join("Untitled 2.md"));
    }

    #[test]
    fn title_stem_follows_first_heading() {
        assert_eq!(note_title_stem("# Groceries\nmilk\n").as_deref(), Some("Groceries"));
        assert_eq!(note_title_stem("\n\n##  Weekly Plan \n").as_deref(), Some("Weekly Plan"));
        // Path separators and colons are made filename-safe.
        assert_eq!(note_title_stem("# Meeting: A/B\n").as_deref(), Some("Meeting- A-B"));
        // A leading '@' (NotePlan's special prefix) is trimmed off.
        assert_eq!(note_title_stem("# @Home\n").as_deref(), Some("Home"));
        // No usable title: empty heading, a non-heading first line, or a
        // tag-like "#word" with no space after the hashes.
        assert_eq!(note_title_stem("# \nbody\n"), None);
        assert_eq!(note_title_stem("just text\n"), None);
        assert_eq!(note_title_stem("#project first\n"), None);
    }

    #[test]
    fn reschedule_on_disk_relocates_and_verifies() {
        let root = ScratchRoot::new("resched");
        let path = root.write("Calendar/20260805.md", "# Day\n* pay >2026-08-09\n* other\n");
        let due = chrono::NaiveDate::from_ymd_opt(2026, 8, 20).expect("valid");

        // Line moved since render: relocated by content and rewritten.
        assert!(reschedule_task_on_disk(&path, 3, "* pay >2026-08-09", due).expect("io"));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "# Day\n* pay >2026-08-20\n* other\n"
        );
        // The expected line is gone: nothing is written.
        assert!(!reschedule_task_on_disk(&path, 1, "* pay >2026-08-09", due).expect("io"));
        // Already due that day: reported as no change.
        assert!(!reschedule_task_on_disk(&path, 1, "* pay >2026-08-20", due).expect("io"));
    }

    #[test]
    fn edit_line_replaces_and_splits() {
        let text = "# Day\n* one\n* two\n";
        assert_eq!(
            edit_line_in_text(text, 1, "* one", |_| Some("* one edited".into())),
            Some(("# Day\n* one edited\n* two\n".into(), 1))
        );
        // A replacement containing a newline splits the line in place.
        assert_eq!(
            edit_line_in_text(text, 1, "* one", |_| Some("* one\n* [ ] ".into())),
            Some(("# Day\n* one\n* [ ] \n* two\n".into(), 1))
        );
        // Vanished line: no edit.
        assert_eq!(edit_line_in_text(text, 1, "* gone", |_| Some("x".into())), None);
    }

    #[test]
    fn edit_line_relocation_is_unique_or_nothing() {
        // The file shifted and the line matches exactly once: relocated, and
        // the caller learns the resolved index.
        let shifted = "new\n# Day\n* one\n* two\n";
        assert_eq!(
            edit_line_in_text(shifted, 1, "* one", |_| Some("* one!".into())),
            Some(("new\n# Day\n* one!\n* two\n".into(), 2))
        );
        // The line moved AND has duplicates (blank lines are the everyday
        // case): the right one cannot be known, so nothing is edited.
        let dupes = "x\ntext\n\nmore\n\nend\n";
        assert_eq!(edit_line_in_text(dupes, 1, "", |_| Some("typed".into())), None);
        // A duplicate elsewhere is fine while the index still matches.
        assert_eq!(
            edit_line_in_text(dupes, 2, "", |_| Some("typed".into())),
            Some(("x\ntext\ntyped\nmore\n\nend\n".into(), 2))
        );
    }

    #[test]
    fn join_lines() {
        let text = "# Day\n* one\n* two\n* three\n";
        // Straightforward adjacent pair.
        assert_eq!(
            join_lines_in_text(text, 1, "* one", "* two", "* onetwo"),
            Some(("# Day\n* onetwo\n* three\n".into(), 1))
        );
        // Pair moved since render: found again by content, index resolved.
        let shifted = "new\n# Day\n* one\n* two\n* three\n";
        assert_eq!(
            join_lines_in_text(shifted, 1, "* one", "* two", "* onetwo"),
            Some(("new\n# Day\n* onetwo\n* three\n".into(), 2))
        );
        // Second line changed underneath: nothing is written.
        assert_eq!(join_lines_in_text(text, 1, "* one", "* other", "x"), None);
        // The pair moved and appears twice: ambiguous, nothing is written.
        let twice = "pad\n* a\n* b\npad\n* a\n* b\n";
        assert_eq!(join_lines_in_text(twice, 0, "* a", "* b", "* ab"), None);
        // Joining the last pair with no trailing newline keeps it that way.
        assert_eq!(
            join_lines_in_text("* a\n* b", 0, "* a", "* b", "* ab"),
            Some(("* ab".into(), 0))
        );
    }

    #[test]
    fn move_line_reorders() {
        let root = ScratchRoot::new("move");
        let path = root.write("Calendar/20260807.md", "# Day\n* one\n* two\n* three\n");
        // Move "one" below "two" (before "three").
        assert_eq!(move_line_on_disk(&path, 1, "* one", 3).expect("io"), Some(2));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "# Day\n* two\n* one\n* three\n"
        );
        // Move "three" to the very top.
        assert_eq!(move_line_on_disk(&path, 3, "* three", 0).expect("io"), Some(0));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "* three\n# Day\n* two\n* one\n"
        );
        // Past-the-end target moves to the end.
        assert_eq!(move_line_on_disk(&path, 0, "* three", 99).expect("io"), Some(3));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "# Day\n* two\n* one\n* three\n"
        );
        // Dropping a line onto itself writes nothing and keeps its index.
        assert_eq!(move_line_on_disk(&path, 1, "* two", 1).expect("io"), Some(1));
        // The line shifted since render: relocated by unique content.
        assert_eq!(move_line_on_disk(&path, 0, "* one", 0).expect("io"), Some(0));
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "* one\n# Day\n* two\n* three\n"
        );
        // Gone or ambiguous: nothing is written.
        assert_eq!(move_line_on_disk(&path, 0, "* gone", 2).expect("io"), None);
        let dupes = root.write("Notes/Dupes.md", "* a\n\nx\n\n* b\n");
        assert_eq!(move_line_on_disk(&dupes, 0, "", 4).expect("io"), None);
        // No trailing newline stays that way.
        let bare = root.write("Notes/Bare.md", "one\ntwo");
        assert_eq!(move_line_on_disk(&bare, 1, "two", 0).expect("io"), Some(0));
        assert_eq!(fs::read_to_string(&bare).expect("read"), "two\none");
    }

    #[test]
    fn create_note_never_clobbers() {
        let root = ScratchRoot::new("create");
        let path = root.write("Notes/Existing.md", "# a year of notes\n");

        assert!(!create_note_if_absent(&path, "# Existing\n").expect("io"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "# a year of notes\n");

        let fresh = root.0.join("Notes/Fresh.md");
        assert!(create_note_if_absent(&fresh, "# Fresh\n").expect("io"));
        assert_eq!(fs::read_to_string(&fresh).expect("read"), "# Fresh\n");
    }

    #[test]
    fn append_line_rereads_the_file() {
        let root = ScratchRoot::new("append");
        // The file grew after our snapshot was taken; the append must land
        // after the line another writer added, clobbering nothing.
        let path = root.write("Calendar/20260807.md", "* ours\n* theirs\n");
        assert_eq!(append_line(&path, "* typed").expect("io"), 2);
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "* ours\n* theirs\n* typed\n"
        );
        // A file with no trailing newline keeps its convention.
        let bare = root.write("Notes/Bare.md", "one");
        assert_eq!(append_line(&bare, "two").expect("io"), 1);
        assert_eq!(fs::read_to_string(&bare).expect("read"), "one\ntwo");
        // A missing file is created.
        let fresh = root.0.join("Calendar/20260808.md");
        assert_eq!(append_line(&fresh, "* first").expect("io"), 0);
        assert_eq!(fs::read_to_string(&fresh).expect("read"), "* first\n");
    }

    #[test]
    fn capture_appends_or_skips() {
        let root = ScratchRoot::new("capture");
        let date = chrono::NaiveDate::from_ymd_opt(2026, 8, 7).expect("valid");
        // Blank input writes nothing at all.
        assert_eq!(capture(&root.0, date, "   ").expect("io"), None);
        assert!(!root.0.join("Calendar/20260807.md").exists());
        // Real input lands as an open task.
        let path = capture(&root.0, date, "call the bank").expect("io").expect("written");
        assert_eq!(fs::read_to_string(&path).expect("read"), "* call the bank\n");
    }

    #[test]
    fn capture_seeds_new_days_from_template() {
        let root = ScratchRoot::new("capture-seed");
        root.write("Notes/@Templates/Daily.md", "### Tasks\n\n### Notes\n");
        // A capture into a brand-new future day lands under the template.
        let future = Local::now().date_naive() + chrono::Days::new(1);
        let path = capture(&root.0, future, "pack bags").expect("io").expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "### Tasks\n\n### Notes\n* pack bags\n"
        );
        // A past day is never dressed up with today's template.
        let past = chrono::NaiveDate::from_ymd_opt(2020, 1, 2).expect("valid");
        let past_path = capture(&root.0, past, "old note").expect("io").expect("written");
        assert_eq!(fs::read_to_string(&past_path).expect("read"), "* old note\n");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = ScratchRoot::new("perms");
        let path = root.write("Notes/Private.md", "* secret\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");

        assert!(replace_line_on_disk(&path, 0, "* secret", "* edited").expect("io").is_some());
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::read_to_string(&path).expect("read"), "* edited\n");
    }

    #[test]
    fn crlf_files_stay_crlf() {
        // A replacement that splits a line uses the file's own ending.
        assert_eq!(
            edit_line_in_text("* one\r\n* two\r\n", 0, "* one", |_| {
                Some("* one\n* [ ] rest".into())
            }),
            Some(("* one\r\n* [ ] rest\r\n* two\r\n".into(), 0))
        );
        // Appending matches the ending too, for separator and content.
        let root = ScratchRoot::new("crlf");
        let path = root.write("Notes/Dos.md", "* one\r\n");
        append_line(&path, "* two").expect("io");
        assert_eq!(fs::read_to_string(&path).expect("read"), "* one\r\n* two\r\n");
    }
}
