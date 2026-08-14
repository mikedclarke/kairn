//! The notes root on disk: layout, day and period-note discovery, the
//! Notes/ tree, conflict copies, and search across everything.

use std::collections::{HashMap, HashSet};
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

/// One shot of the vault's file lists: a single `Calendar/` scan and one
/// `Notes/` walk, shared by everything derived from them (tasks, mentions,
/// search) instead of each doing its own walks and `exists()` probes.
pub struct VaultScan {
    /// Daily-note file per date (`.md` preferred over NotePlan's `.txt`).
    pub days: HashMap<NaiveDate, PathBuf>,
    /// Non-daily period notes with their canonical stems, newest first.
    pub periods: Vec<(String, PathBuf)>,
    /// Every note file under `Notes/` with its depth, tree order.
    pub(crate) notes: Vec<(usize, PathBuf)>,
}

/// A daily note read into memory, so one read serves both the task scan and
/// the mention scan. The text is shared so a cache can hand it out across
/// reloads without copying.
pub struct DayText {
    pub date: NaiveDate,
    pub path: PathBuf,
    pub text: std::sync::Arc<str>,
}

/// Note text carried across reloads, invalidated per file by modification
/// time and length: a reload stats every file but re-reads only the ones
/// that actually changed. On a real NotePlan archive that turns hundreds
/// of reads per click into a handful. One instance per file population
/// (dailies, non-daily notes) so eviction stays a simple retain.
#[derive(Default)]
pub struct TextCache {
    map: HashMap<PathBuf, (std::time::SystemTime, u64, std::sync::Arc<str>)>,
}

impl TextCache {
    /// The file's text, from cache when its mtime and length are unchanged.
    /// `None` when the file can't be statted or read (vanished, unreadable).
    fn read(&mut self, path: &Path) -> Option<std::sync::Arc<str>> {
        let meta = fs::metadata(path).ok()?;
        let (mtime, len) = (meta.modified().ok()?, meta.len());
        match self.map.get(path) {
            Some((m, l, text)) if *m == mtime && *l == len => Some(text.clone()),
            _ => {
                let text: std::sync::Arc<str> = fs::read_to_string(path).ok()?.into();
                self.map
                    .insert(path.to_path_buf(), (mtime, len, text.clone()));
                Some(text)
            }
        }
    }

    /// Drop entries for files no longer in the live set.
    fn retain_live(&mut self, live: &HashSet<&PathBuf>) {
        self.map.retain(|path, _| live.contains(path));
    }
}

/// A non-daily note read into memory for the task scan.
pub struct NoteText {
    pub path: PathBuf,
    pub text: std::sync::Arc<str>,
}

impl VaultScan {
    pub fn new(root: &Path) -> Self {
        let mut days: HashMap<NaiveDate, PathBuf> = HashMap::new();
        let mut periods = Vec::new();
        if let Ok(entries) = fs::read_dir(root.join("Calendar")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path.extension().and_then(|x| x.to_str());
                let is_md = matches!(ext, Some("md"));
                if !(is_md || matches!(ext, Some("txt"))) || !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if stem.len() == 8
                    && stem.bytes().all(|b| b.is_ascii_digit())
                    && let Ok(date) = NaiveDate::parse_from_str(stem, "%Y%m%d")
                {
                    match days.entry(date) {
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert(path);
                        }
                        std::collections::hash_map::Entry::Occupied(mut o) => {
                            if is_md {
                                o.insert(path);
                            }
                        }
                    }
                } else if let Some(canon) = period_stem(stem) {
                    periods.push((canon, path));
                }
            }
        }
        periods.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Self { days, periods, notes: notes_files(root) }
    }

    /// Every note file under `Notes/`, tree order, without reading any of
    /// them: for callers that need paths (mtimes, counts), not content.
    pub fn note_files(&self) -> impl Iterator<Item = &PathBuf> {
        self.notes.iter().map(|(_, path)| path)
    }

    /// Every daily note read once, newest day first.
    pub fn read_dailies(&self) -> Vec<DayText> {
        self.read_dailies_cached(&mut TextCache::default())
    }

    /// [`Self::read_dailies`] through a cache that persists across reloads:
    /// unchanged files (same mtime and length) reuse their text, changed or
    /// new files are re-read, vanished files fall out of the cache.
    pub fn read_dailies_cached(&self, cache: &mut TextCache) -> Vec<DayText> {
        let mut days: Vec<(&NaiveDate, &PathBuf)> = self.days.iter().collect();
        days.sort_unstable_by(|a, b| b.0.cmp(a.0));
        let dailies: Vec<DayText> = days
            .into_iter()
            .filter_map(|(date, path)| {
                let text = cache.read(path)?;
                Some(DayText { date: *date, path: path.clone(), text })
            })
            .collect();
        let live: HashSet<&PathBuf> = self.days.values().collect();
        cache.retain_live(&live);
        dailies
    }

    /// Every non-daily note (period notes and `Notes/` files) read once,
    /// through a persistent cache like [`Self::read_dailies_cached`]. Feeds
    /// the task scan; keep it on its own cache instance, not the dailies'.
    pub fn read_notes_cached(&self, cache: &mut TextCache) -> Vec<NoteText> {
        let paths = self
            .periods
            .iter()
            .map(|(_, p)| p)
            .chain(self.notes.iter().map(|(_, p)| p));
        let notes: Vec<NoteText> = paths
            .clone()
            .filter_map(|path| {
                let text = cache.read(path)?;
                Some(NoteText { path: path.clone(), text })
            })
            .collect();
        let live: HashSet<&PathBuf> = paths.collect();
        cache.retain_live(&live);
        notes
    }
}

/// Every date with a daily note, from one scan of `Calendar/`.
pub fn days_with_notes(root: &Path) -> HashSet<NaiveDate> {
    VaultScan::new(root).days.into_keys().collect()
}

/// Canonical form of a `Calendar/` period-note stem that isn't a daily:
/// weekly `YYYY-Wnn`, monthly `YYYY-MM`, quarterly `YYYY-Qn`, or yearly
/// `YYYY` (`w`/`q` accepted case-insensitively). `None` for anything else.
pub fn period_stem(stem: &str) -> Option<String> {
    let (year, rest) = stem.split_at_checked(4)?;
    if !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if rest.is_empty() {
        return Some(year.to_string());
    }
    let rest = rest.strip_prefix('-')?;
    let upper = rest.to_ascii_uppercase();
    let ok = if let Some(week) = upper.strip_prefix('W') {
        week.len() == 2 && week.parse::<u8>().is_ok_and(|n| (1..=53).contains(&n))
    } else if let Some(quarter) = upper.strip_prefix('Q') {
        matches!(quarter, "1" | "2" | "3" | "4")
    } else {
        upper.len() == 2 && upper.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n))
    };
    ok.then(|| format!("{year}-{upper}"))
}

/// Every non-daily period note in `Calendar/` with its canonical stem,
/// newest period first.
pub fn period_files(root: &Path) -> Vec<(String, PathBuf)> {
    VaultScan::new(root).periods
}

/// Syncthing conflict copies sitting next to a note: files named
/// `{stem}.sync-conflict-…` in the same folder, the pattern Syncthing uses
/// when two machines changed the file at once. These fail the daily-stem
/// rule, so without this they would be unreachable from every surface.
pub fn conflict_copies(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (
        path.parent(),
        path.file_stem().and_then(|s| s.to_str()),
    ) else {
        return Vec::new();
    };
    let prefix = format!("{stem}.sync-conflict-");
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    out.sort();
    out
}

/// The note a Syncthing conflict copy shadows: the same folder and
/// extension, with the `.sync-conflict-…` infix stripped from the name.
/// `None` when the name doesn't carry the infix.
pub fn conflict_owner(copy: &Path) -> Option<PathBuf> {
    let name = copy.file_name()?.to_str()?;
    let idx = name.find(".sync-conflict-")?;
    let ext = copy.extension()?.to_str()?;
    Some(copy.parent()?.join(format!("{}.{ext}", &name[..idx])))
}

/// Every Syncthing conflict copy in the vault with the note it shadows,
/// sorted by owner: conflicts on notes that aren't open would otherwise be
/// invisible until the note happens to be viewed.
pub fn vault_conflicts(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let is_conflict = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(".sync-conflict-"))
    };
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("Calendar")) {
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_file()
                && is_conflict(&path)
                && let Some(owner) = conflict_owner(&path)
            {
                out.push((owner, path));
            }
        }
    }
    for (_, path) in notes_files(root) {
        if is_conflict(&path)
            && let Some(owner) = conflict_owner(&path)
        {
            out.push((owner, path));
        }
    }
    out.sort();
    out
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

/// Depth backstop for the `Notes/` walks. Symlinked directories are never
/// followed (a cycle would otherwise recurse until the stack blows before
/// the window even opens), so this only guards against absurdly deep trees.
const MAX_TREE_DEPTH: usize = 24;

/// Whether a directory entry is a real directory, without following
/// symlinks: a symlinked dir is treated as an opaque file so cycles can't
/// recurse.
fn is_real_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_dir())
}

fn push_tree_level(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<NoteEntry>,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
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
        let is_dir = is_real_dir(&entry);
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

/// Every note file under `Notes/` with its folder depth, recursively,
/// deterministic order. `@Trash` is skipped everywhere links and search are
/// concerned.
pub(crate) fn notes_files(root: &Path) -> Vec<(usize, PathBuf)> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(usize, PathBuf)>) {
        if depth > MAX_TREE_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut items: Vec<(PathBuf, bool)> =
            entries.flatten().map(|e| (e.path(), is_real_dir(&e))).collect();
        items.sort();
        for (path, is_dir) in items {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') || name == "@Trash" {
                continue;
            }
            if is_dir {
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

/// A day typed as a search query: ISO (`2026-08-12`) or a human month-day
/// like `aug 12`, `12 Aug`, or `August 12 2027`; a missing year means this
/// year. `None` when the query isn't date-shaped.
pub fn parse_day_query(query: &str, today: NaiveDate) -> Option<NaiveDate> {
    const FORMATS: [&str; 4] = ["%d %b %Y", "%b %d %Y", "%d %B %Y", "%B %d %Y"];
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if let Ok(date) = NaiveDate::parse_from_str(q, "%Y-%m-%d") {
        return Some(date);
    }
    for fmt in FORMATS {
        if let Ok(date) = NaiveDate::parse_from_str(q, fmt) {
            return Some(date);
        }
    }
    let with_year = format!("{q} {}", today.format("%Y"));
    for fmt in FORMATS {
        if let Ok(date) = NaiveDate::parse_from_str(&with_year, fmt) {
            return Some(date);
        }
    }
    None
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
/// newest first, then period notes, then notes), capped at `limit`.
pub fn search_notes(root: &Path, query: &str, limit: usize) -> Vec<SearchHit> {
    search_in(&VaultScan::new(root), query, limit)
}

/// [`search_notes`] over an existing scan, so a caller that already walked
/// the vault doesn't walk it again.
pub fn search_in(scan: &VaultScan, query: &str, limit: usize) -> Vec<SearchHit> {
    let trimmed = query.trim();
    let q = trimmed.to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let files = &scan.notes;
    let periods = &scan.periods;
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
        .chain(periods.iter().filter_map(|(name, path)| {
            let score = fuzzy_score(trimmed, name)?;
            Some((score, SearchHit {
                path: path.clone(),
                date: None,
                name: name.clone(),
                snippet: None,
            }))
        }))
        .collect();
    titled.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    let mut out: Vec<SearchHit> =
        titled.into_iter().take(limit).map(|(_, hit)| hit).collect();
    if out.len() >= limit {
        return out;
    }
    let mut days: Vec<(&NaiveDate, &PathBuf)> = scan.days.iter().collect();
    days.sort_unstable_by(|a, b| b.0.cmp(a.0));
    let dailies = days.into_iter().map(|(d, p)| (p.clone(), Some(*d)));
    let title_hits: HashSet<PathBuf> = out.iter().map(|h| h.path.clone()).collect();
    let period_bodies = periods
        .iter()
        .filter(|(_, p)| !title_hits.contains(p))
        .map(|(_, p)| (p.clone(), None));
    let notes = files
        .iter()
        .filter(|(_, p)| !title_hits.contains(p))
        .map(|(_, p)| (p.clone(), None));
    for (path, date) in dailies.chain(period_bodies).chain(notes) {
        if out.len() >= limit {
            break;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Some(line) = text.lines().find(|l| contains_insensitive(l, &q)) else {
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

/// Case-insensitive substring test against an already-lowercased needle,
/// without allocating a lowercased copy of every line: ASCII needles use a
/// windowed byte comparison, anything else falls back to `to_lowercase`.
pub(crate) fn contains_insensitive(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.is_ascii() {
        let n = needle_lower.as_bytes();
        haystack
            .as_bytes()
            .windows(n.len())
            .any(|w| w.eq_ignore_ascii_case(n))
    } else {
        haystack.to_lowercase().contains(needle_lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    #[test]
    fn daily_cache_reuses_invalidates_and_evicts() {
        let root = ScratchRoot::new("dailycache");
        let a = root.write("Calendar/20260805.md", "* task a\n");
        let b = root.write("Calendar/20260806.md", "* task b\n");

        let mut cache = TextCache::default();
        let dailies = VaultScan::new(&root.0).read_dailies_cached(&mut cache);
        assert_eq!(dailies.len(), 2);
        assert_eq!(cache.map.len(), 2);
        let first_arc = cache.map[&a].2.clone();

        // Unchanged files hand back the same shared text, no fresh read.
        let dailies = VaultScan::new(&root.0).read_dailies_cached(&mut cache);
        assert!(std::sync::Arc::ptr_eq(
            &dailies.iter().find(|d| d.path == a).expect("day a").text,
            &first_arc
        ));

        // A changed file re-reads (length change invalidates regardless of
        // mtime resolution).
        std::fs::write(&b, "* task b\n* task b2\n").expect("rewrite");
        let dailies = VaultScan::new(&root.0).read_dailies_cached(&mut cache);
        let day_b = dailies.iter().find(|d| d.path == b).expect("day b");
        assert!(day_b.text.contains("b2"));

        // A vanished file falls out of the cache.
        std::fs::remove_file(&b).expect("remove");
        let dailies = VaultScan::new(&root.0).read_dailies_cached(&mut cache);
        assert_eq!(dailies.len(), 1);
        assert_eq!(cache.map.len(), 1);
        assert!(cache.map.contains_key(&a));
    }

    #[test]
    fn day_filenames() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid date");
        assert!(daily_path(Path::new("/root"), d).ends_with("Calendar/20260806.md"));
    }

    #[test]
    fn period_stems() {
        assert_eq!(period_stem("2026-W32").as_deref(), Some("2026-W32"));
        assert_eq!(period_stem("2026-w32").as_deref(), Some("2026-W32"));
        assert_eq!(period_stem("2026-08").as_deref(), Some("2026-08"));
        assert_eq!(period_stem("2026-Q3").as_deref(), Some("2026-Q3"));
        assert_eq!(period_stem("2026").as_deref(), Some("2026"));
        for bad in ["20260807", "2026-W54", "2026-W1", "2026-13", "2026-Q5", "2026-", "plan", "202"] {
            assert_eq!(period_stem(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn day_scan_ignores_directories() {
        let root = ScratchRoot::new("daydirs");
        std::fs::create_dir_all(root.0.join("Calendar/20260805.md")).expect("mkdir");
        root.write("Calendar/20260806.md", "* real\n");
        let days = days_with_notes(&root.0);
        assert_eq!(days.len(), 1);
        assert!(days.contains(&NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid")));
    }

    #[test]
    fn conflict_copies_are_found() {
        let root = ScratchRoot::new("conflict");
        let day = root.write("Calendar/20260806.md", "* mine\n");
        let copy = root.write(
            "Calendar/20260806.sync-conflict-20260807-101112-AAAAAAA.md",
            "* theirs\n",
        );
        root.write("Calendar/20260807.md", "* unrelated\n");

        assert_eq!(conflict_copies(&day), vec![copy.clone()]);
        // The copy itself has no copies, and unrelated days are untouched.
        assert!(conflict_copies(&copy).is_empty());
        assert!(conflict_copies(&root.0.join("Calendar/20260807.md")).is_empty());
    }

    #[test]
    fn conflict_owner_strips_the_infix() {
        let copy = Path::new("/v/Calendar/20260811.sync-conflict-20260812-084345-IPHONE.md");
        assert_eq!(
            conflict_owner(copy),
            Some(PathBuf::from("/v/Calendar/20260811.md"))
        );
        assert_eq!(conflict_owner(Path::new("/v/Calendar/20260811.md")), None);
    }

    #[test]
    fn vault_conflicts_cover_calendar_and_notes() {
        let root = ScratchRoot::new("vault-conflicts");
        let day = root.write("Calendar/20260806.md", "* mine\n");
        let day_copy = root.write(
            "Calendar/20260806.sync-conflict-20260807-101112-AAAAAAA.md",
            "* theirs\n",
        );
        let note = root.write("Notes/Projects/Plan.md", "# plan\n");
        let note_copy = root.write(
            "Notes/Projects/Plan.sync-conflict-20260808-090000-BBBBBBB.md",
            "# stale plan\n",
        );
        root.write("Calendar/20260807.md", "* unrelated\n");

        assert_eq!(
            vault_conflicts(&root.0),
            vec![(day, day_copy), (note, note_copy)]
        );
    }

    #[test]
    fn day_queries_parse() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 7).expect("valid");
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).expect("valid");
        assert_eq!(parse_day_query("2026-08-12", today), Some(d(2026, 8, 12)));
        assert_eq!(parse_day_query("aug 12", today), Some(d(2026, 8, 12)));
        assert_eq!(parse_day_query("12 Aug", today), Some(d(2026, 8, 12)));
        assert_eq!(parse_day_query("August 12 2027", today), Some(d(2027, 8, 12)));
        assert_eq!(parse_day_query("kairn", today), None);
        assert_eq!(parse_day_query("", today), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycles_do_not_recurse() {
        let root = ScratchRoot::new("symlink");
        root.write("Notes/deep/Real.md", "# real\n");
        // A cycle: Notes/deep/loop -> Notes. Following it would recurse
        // until the stack blew, before the window even opened.
        std::os::unix::fs::symlink(root.0.join("Notes"), root.0.join("Notes/deep/loop"))
            .expect("symlink");

        let expanded: HashSet<PathBuf> =
            [root.0.join("Notes/deep"), root.0.join("Notes/deep/loop")].into();
        // Both walks terminate; the symlinked dir is not descended into.
        let rows = notes_tree(&root.0, &expanded);
        assert!(rows.iter().any(|r| r.name == "Real"));
        let hits = search_notes(&root.0, "real", 10);
        assert_eq!(hits.len(), 1);
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
}
