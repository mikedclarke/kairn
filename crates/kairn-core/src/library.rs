//! Library folders: external local directories browsable from the sidebar.
//! Libraries are documents, not notes: no task parsing, no calendar or
//! backlink integration, and their config is per-machine (never synced).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::vault::{SearchHit, contains_insensitive, fuzzy_score};

/// How a library file renders: the full markdown editor, monospace text
/// editing, an inline image view, or a metadata card with open/reveal
/// actions for everything else (PDF included until it grows a viewer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Text,
    Image,
    Other,
}

pub fn file_kind(path: &Path) -> FileKind {
    let Some(ext) = path.extension().and_then(|x| x.to_str()) else {
        return FileKind::Other;
    };
    match ext.to_ascii_lowercase().as_str() {
        "md" | "markdown" => FileKind::Markdown,
        "txt" | "html" | "htm" | "css" | "js" | "jsx" | "ts" | "tsx" | "json" | "xml"
        | "yml" | "yaml" | "toml" | "ini" | "conf" | "py" | "php" | "rb" | "rs" | "go"
        | "sh" | "zsh" | "bash" | "sql" | "csv" | "log" | "c" | "h" | "cpp" | "swift" => {
            FileKind::Text
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" => FileKind::Image,
        _ => FileKind::Other,
    }
}

/// One visible row of a library root's tree.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryEntry {
    pub path: PathBuf,
    /// Display name: the file or folder name, extension kept (a library
    /// holds `report.pdf` next to `report.md`; stems would collide).
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
}

/// Names never shown or searched: VCS and build internals, dependency
/// trees, and dotfiles. Sync-conflict copies are skipped too; libraries
/// have no banner surface for them. Public so the app's file watcher can
/// drop events from the same subtrees the tree never shows.
pub fn library_ignored_name(name: &str) -> bool {
    name.starts_with('.')
        || name.contains(".sync-conflict-")
        || matches!(
            name,
            "node_modules" | "target" | "venv" | "__pycache__" | "dist" | "build"
        )
}

/// Depth backstop matching the vault walks; symlinked directories are never
/// descended into (same cycle guard as `Notes/`).
const MAX_TREE_DEPTH: usize = 24;

fn is_real_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_dir())
}

/// The visible rows of a library root given the expanded folders: folders
/// before files, alphabetical. Lazy by construction — only expanded
/// directories are read, so a 10k-file root costs one `read_dir` until
/// folders are opened.
pub fn library_tree(root: &Path, expanded: &HashSet<PathBuf>) -> Vec<LibraryEntry> {
    let mut rows = Vec::new();
    push_level(root, 0, expanded, &mut rows);
    rows
}

fn push_level(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<LibraryEntry>,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut items: Vec<(bool, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if library_ignored_name(&name) {
            continue;
        }
        let is_dir = is_real_dir(&entry);
        // Sort key: folders before files, then name.
        items.push((!is_dir, name.to_lowercase(), path));
    }
    items.sort();
    for (not_dir, _, path) in items {
        let is_dir = !not_dir;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        rows.push(LibraryEntry { path: path.clone(), name, is_dir, depth });
        if is_dir && expanded.contains(&path) {
            push_level(&path, depth + 1, expanded, rows);
        }
    }
}

/// Every file under a library root (ignores applied), for search: `(path,
/// kind)` in tree order. Bounded by the same depth backstop as the tree.
fn library_files(root: &Path, out: &mut Vec<(PathBuf, FileKind)>) {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, FileKind)>) {
        if depth > MAX_TREE_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut items: Vec<(PathBuf, bool)> =
            entries.flatten().map(|e| (e.path(), is_real_dir(&e))).collect();
        items.sort();
        for (path, is_dir) in items {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if library_ignored_name(name) {
                continue;
            }
            if is_dir {
                walk(&path, depth + 1, out);
            } else {
                out.push((path.clone(), file_kind(&path)));
            }
        }
    }
    walk(root, 0, out);
}

/// Every image directly inside `dir` (ignores applied), sorted by name:
/// the sibling strip on the image view, which is how "pick image 1, 2 or 3"
/// reads as a gallery.
pub fn library_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !library_ignored_name(n))
                && file_kind(p) == FileKind::Image
        })
        .collect();
    out.sort();
    out
}

/// Files larger than this never get a body scan: a library can hold logs
/// and data dumps that would drown the search in reads.
const BODY_SEARCH_CAP: u64 = 1_000_000;

/// Search library roots the way the switcher searches the vault: fuzzy
/// filename matches first (best score wins), then one substring body match
/// per markdown file, capped at `limit`. Results carry no dates — library
/// files are documents, not days.
pub fn search_library(roots: &[PathBuf], query: &str, limit: usize) -> Vec<SearchHit> {
    let trimmed = query.trim();
    let q = trimmed.to_lowercase();
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut files = Vec::new();
    for root in roots {
        library_files(root, &mut files);
    }
    let mut titled: Vec<(i64, SearchHit)> = files
        .iter()
        .filter_map(|(path, _)| {
            let name = path.file_name().and_then(|n| n.to_str())?;
            let score = fuzzy_score(trimmed, name)?;
            Some((score, SearchHit {
                path: path.clone(),
                date: None,
                name: name.to_string(),
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
    let title_hits: HashSet<&PathBuf> = out.iter().map(|h| &h.path).collect();
    let bodies: Vec<&PathBuf> = files
        .iter()
        .filter(|(path, kind)| {
            *kind == FileKind::Markdown && !title_hits.contains(path)
        })
        .map(|(path, _)| path)
        .collect();
    for path in bodies {
        if out.len() >= limit {
            break;
        }
        if fs::metadata(path).is_ok_and(|m| m.len() > BODY_SEARCH_CAP) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else { continue };
        let Some(line) = text.lines().find(|l| contains_insensitive(l, &q)) else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        out.push(SearchHit {
            path: path.clone(),
            date: None,
            name,
            snippet: Some(line.trim().to_string()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(file_kind(Path::new("a/plan.md")), FileKind::Markdown);
        assert_eq!(file_kind(Path::new("a/mock.HTML")), FileKind::Text);
        assert_eq!(file_kind(Path::new("a/pic.PNG")), FileKind::Image);
        assert_eq!(file_kind(Path::new("a/report.pdf")), FileKind::Other);
        assert_eq!(file_kind(Path::new("a/no-extension")), FileKind::Other);
    }

    #[test]
    fn tree_is_lazy_and_ignores_junk() {
        let root = ScratchRoot::new("librarytree");
        root.write("projects/plan.md", "# plan\n");
        root.write("projects/deep/spec.md", "# spec\n");
        root.write("node_modules/pkg/index.js", "x\n");
        root.write(".git/config", "x\n");
        root.write(".DS_Store", "x\n");
        root.write("top.md", "# top\n");
        root.write("projects/old.sync-conflict-20260812-084345-IPHONE.md", "x\n");

        // Collapsed: only the top level, junk filtered, folders first.
        let rows = library_tree(&root.0, &HashSet::new());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // ScratchRoot pre-creates Calendar/Notes (unused here but present).
        assert!(names.contains(&"projects"));
        assert!(names.contains(&"top.md"));
        assert!(!names.iter().any(|n| n.contains("node_modules")));
        assert!(!names.iter().any(|n| n.contains("sync-conflict")));
        assert!(!rows.iter().any(|r| r.depth > 0));

        // Expanding a folder reveals its level, depth tracked.
        let expanded: HashSet<PathBuf> = [root.0.join("projects")].into();
        let rows = library_tree(&root.0, &expanded);
        let plan = rows.iter().find(|r| r.name == "plan.md").expect("plan row");
        assert_eq!(plan.depth, 1);
        assert!(!rows.iter().any(|r| r.name == "spec.md"));
    }

    #[test]
    fn search_titles_then_markdown_bodies() {
        let root = ScratchRoot::new("librarysearch");
        root.write("research/alpha-findings.md", "body text\n");
        root.write("research/beta.md", "mentions alpha inline\n");
        root.write("mockups/alpha.html", "<h1>alpha everywhere</h1>\n");
        root.write("node_modules/alpha.md", "never seen\n");

        let hits = search_library(&[root.0.clone()], "alpha", 10);
        // Title matches first (the html title counts; its body does not).
        assert!(hits.len() >= 3);
        assert!(hits[..2].iter().any(|h| h.name == "alpha-findings.md"));
        assert!(hits[..2].iter().any(|h| h.name == "alpha.html"));
        assert!(hits.iter().all(|h| h.date.is_none()));
        // Body match only from markdown, snippet trimmed.
        let beta = hits.iter().find(|h| h.name == "beta.md").expect("body hit");
        assert_eq!(beta.snippet.as_deref(), Some("mentions alpha inline"));
        // Ignored dirs never leak into results.
        assert!(!hits.iter().any(|h| h.path.components().any(|c| c.as_os_str() == "node_modules")));

        assert!(search_library(&[root.0.clone()], "  ", 10).is_empty());
    }
}
