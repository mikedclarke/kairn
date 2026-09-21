//! Library folders: external local directories browsable from the sidebar.
//! Libraries are documents, not notes: no task parsing, no calendar or
//! backlink integration, and their config is per-machine (never synced).

use std::collections::HashSet;
use std::fs;
use std::io;
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
    /// A symlink (followed for `is_dir`): symlinked folders browse like
    /// any other folder but carry a distinct icon.
    pub is_symlink: bool,
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

/// Depth backstop matching the vault walks. Unlike `Notes/`, symlinked
/// directories are followed (a library often points into synced or shared
/// trees); the search walk breaks link cycles with a canonical-path visited
/// set, and this backstop bounds the tree either way.
const MAX_TREE_DEPTH: usize = 24;

/// How a library level orders its files. Folders always sort by name:
/// they are navigation, and reshuffling them as contents change would
/// churn the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibrarySort {
    /// Most recently modified first — the freshest agent output on top.
    #[default]
    Modified,
    Name,
}

/// The visible rows of a library root given the expanded folders: folders
/// before files, folders by name, files per `sort`. Lazy by construction —
/// only expanded directories are read, so a 10k-file root costs one
/// `read_dir` until folders are opened.
pub fn library_tree(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    sort: LibrarySort,
) -> Vec<LibraryEntry> {
    let mut rows = Vec::new();
    push_level(root, 0, expanded, sort, &mut rows);
    rows
}

fn push_level(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    sort: LibrarySort,
    rows: &mut Vec<LibraryEntry>,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    struct Item {
        not_dir: bool,
        symlink: bool,
        lower: String,
        modified: std::time::SystemTime,
        path: PathBuf,
    }
    let mut items: Vec<Item> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if library_ignored_name(&name) {
            continue;
        }
        let symlink = entry.file_type().is_ok_and(|t| t.is_symlink());
        // Symlinks classify by their target (a broken link lists as a
        // plain file); everything else stays on the cheap DirEntry calls.
        let (is_dir, modified) = if symlink {
            let meta = fs::metadata(&path);
            (
                meta.as_ref().is_ok_and(|m| m.is_dir()),
                meta.and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        } else {
            (
                entry.file_type().is_ok_and(|t| t.is_dir()),
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        };
        items.push(Item {
            not_dir: !is_dir,
            symlink,
            lower: name.to_lowercase(),
            modified,
            path,
        });
    }
    items.sort_by(|a, b| {
        a.not_dir.cmp(&b.not_dir).then_with(|| match (a.not_dir, sort) {
            (true, LibrarySort::Modified) => {
                b.modified.cmp(&a.modified).then_with(|| a.lower.cmp(&b.lower))
            }
            _ => a.lower.cmp(&b.lower),
        })
    });
    for item in items {
        let path = item.path;
        let is_dir = !item.not_dir;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        rows.push(LibraryEntry {
            path: path.clone(),
            name,
            is_dir,
            is_symlink: item.symlink,
            depth,
        });
        if is_dir && expanded.contains(&path) {
            push_level(&path, depth + 1, expanded, sort, rows);
        }
    }
}

/// Every file under a library root (ignores applied), for search: `(path,
/// kind)` in tree order. Symlinked folders are searched like real ones;
/// the visited set of canonical paths breaks link cycles, and the depth
/// backstop bounds the walk like the tree's.
fn library_files(root: &Path, out: &mut Vec<(PathBuf, FileKind)>) {
    fn walk(
        dir: &Path,
        depth: usize,
        visited: &mut HashSet<PathBuf>,
        out: &mut Vec<(PathBuf, FileKind)>,
    ) {
        if depth > MAX_TREE_DEPTH {
            return;
        }
        if let Ok(canon) = dir.canonicalize()
            && !visited.insert(canon)
        {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut items: Vec<(PathBuf, bool)> = entries
            .flatten()
            .map(|e| {
                let path = e.path();
                let is_dir = fs::metadata(&path).is_ok_and(|m| m.is_dir());
                (path, is_dir)
            })
            .collect();
        items.sort();
        for (path, is_dir) in items {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if library_ignored_name(name) {
                continue;
            }
            if is_dir {
                walk(&path, depth + 1, visited, out);
            } else {
                out.push((path.clone(), file_kind(&path)));
            }
        }
    }
    walk(root, 0, &mut HashSet::new(), out);
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

/// Reject names that can't safely name a file in a library folder.
fn checked_library_name(name: &str) -> io::Result<&str> {
    let name = name.trim();
    let bad = name.is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\']);
    if bad {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file names can't be empty or start with '.', and can't contain slashes",
        ));
    }
    Ok(name)
}

/// Create a file named by the user inside a library folder. Unlike note
/// names, library names keep their extension; one is required to pick the
/// file's kind, so a bare name becomes markdown (`.md`). Markdown starts
/// with its title heading (the notes convention) so the editor opens on a
/// visible, editable document rather than a blank pane; every other kind
/// starts empty. Never overwrites. Returns the new path.
pub fn create_library_file(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let name = checked_library_name(name)?;
    let file_name = if Path::new(name).extension().is_some() {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    let path = dir.join(&file_name);
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("\"{file_name}\" already exists here"),
        ));
    }
    let content = match file_kind(&path) {
        FileKind::Markdown => {
            let stem = Path::new(&file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&file_name);
            format!("# {stem}\n")
        }
        _ => String::new(),
    };
    use std::io::Write as _;
    fs::File::create_new(&path)?.write_all(content.as_bytes())?;
    Ok(path)
}

/// Rename a library file or folder in place. The typed name is used as-is
/// when it carries an extension; a bare name on a file keeps the old
/// extension (renaming `report.pdf` to `final` gives `final.pdf`). Never
/// overwrites. Returns the new path.
pub fn rename_library_path(path: &Path, name: &str) -> io::Result<PathBuf> {
    let name = checked_library_name(name)?;
    let keep_ext = !path.is_dir() && Path::new(name).extension().is_none();
    let file_name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if keep_ext => format!("{name}.{ext}"),
        _ => name.to_string(),
    };
    let dest = path.with_file_name(&file_name);
    if dest == path {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("\"{file_name}\" already exists here"),
        ));
    }
    fs::rename(path, &dest)?;
    Ok(dest)
}

/// Move a library file or folder into `dest_dir`, keeping its name. Used by
/// the sidebar's drag-to-a-folder. Refuses to move a folder into itself or
/// one of its own descendants, refuses to overwrite an existing name, and
/// treats a move into the file's own current folder as a no-op. Returns the
/// new path.
pub fn move_library_path(path: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    let Some(file_name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nothing to move",
        ));
    };
    let dest = dest_dir.join(file_name);
    if dest == path {
        return Ok(dest);
    }
    // A directory can't be dropped into itself or anywhere beneath it: the
    // rename would either fail or orphan the subtree.
    if path.is_dir() && dest_dir.starts_with(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "can't move a folder into itself",
        ));
    }
    if dest.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "\"{}\" already exists here",
                file_name.to_string_lossy()
            ),
        ));
    }
    fs::rename(path, &dest)?;
    Ok(dest)
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
        let rows = library_tree(&root.0, &HashSet::new(), LibrarySort::Name);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // ScratchRoot pre-creates Calendar/Notes (unused here but present).
        assert!(names.contains(&"projects"));
        assert!(names.contains(&"top.md"));
        assert!(!names.iter().any(|n| n.contains("node_modules")));
        assert!(!names.iter().any(|n| n.contains("sync-conflict")));
        assert!(!rows.iter().any(|r| r.depth > 0));

        // Expanding a folder reveals its level, depth tracked.
        let expanded: HashSet<PathBuf> = [root.0.join("projects")].into();
        let rows = library_tree(&root.0, &expanded, LibrarySort::Name);
        let plan = rows.iter().find(|r| r.name == "plan.md").expect("plan row");
        assert_eq!(plan.depth, 1);
        assert!(!rows.iter().any(|r| r.name == "spec.md"));
    }

    #[test]
    fn files_sort_by_mtime_or_name_and_folders_stay_alphabetical() {
        let root = ScratchRoot::new("librarysort");
        root.write("zeta/x.md", "x\n");
        root.write("alpha/x.md", "x\n");
        root.write("older.md", "first\n");
        root.write("newer.md", "second\n");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(root.0.join("older.md"))
            .and_then(|f| f.set_modified(old))
            .expect("age older.md");

        let names = |sort| -> Vec<String> {
            library_tree(&root.0, &HashSet::new(), sort)
                .into_iter()
                .filter(|r| ["zeta", "alpha", "older.md", "newer.md"].contains(&r.name.as_str()))
                .map(|r| r.name)
                .collect()
        };
        // Newest file first; folders keep their alphabetical order.
        assert_eq!(names(LibrarySort::Modified), ["alpha", "zeta", "newer.md", "older.md"]);
        assert_eq!(names(LibrarySort::Name), ["alpha", "zeta", "newer.md", "older.md"]);

        // Touching the older file bubbles it to the top under Modified only.
        fs::write(root.0.join("older.md"), "updated\n").expect("touch");
        assert_eq!(names(LibrarySort::Modified), ["alpha", "zeta", "older.md", "newer.md"]);
        assert_eq!(names(LibrarySort::Name), ["alpha", "zeta", "newer.md", "older.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_browse_and_search_like_real_ones() {
        let root = ScratchRoot::new("librarysymlink");
        let outside = ScratchRoot::new("librarysymlinktarget");
        outside.write("linked/doc.md", "alpha inside the link\n");
        std::os::unix::fs::symlink(outside.0.join("linked"), root.0.join("shared"))
            .expect("symlink dir");
        std::os::unix::fs::symlink(root.0.join("missing"), root.0.join("broken"))
            .expect("symlink broken");

        // Collapsed: the link is a folder row, flagged, not a file.
        let rows = library_tree(&root.0, &HashSet::new(), LibrarySort::Name);
        let shared = rows.iter().find(|r| r.name == "shared").expect("shared row");
        assert!(shared.is_dir && shared.is_symlink);
        let broken = rows.iter().find(|r| r.name == "broken").expect("broken row");
        assert!(!broken.is_dir && broken.is_symlink);

        // Expanding descends through the link.
        let expanded: HashSet<PathBuf> = [root.0.join("shared")].into();
        let rows = library_tree(&root.0, &expanded, LibrarySort::Name);
        let doc = rows.iter().find(|r| r.name == "doc.md").expect("doc row");
        assert_eq!(doc.depth, 1);
        assert!(doc.path.starts_with(root.0.join("shared")));

        // Search reaches into the link; a link cycle doesn't hang or dupe.
        std::os::unix::fs::symlink(&outside.0, outside.0.join("linked/cycle"))
            .expect("symlink cycle");
        let hits = search_library(&[root.0.clone()], "alpha", 10);
        assert_eq!(
            hits.iter().filter(|h| h.name == "doc.md").count(),
            1,
            "one hit through the link, no cycle duplicates"
        );
    }

    #[test]
    fn create_library_file_defaults_extension_and_refuses_overwrite() {
        let root = ScratchRoot::new("librarycreate");
        // Bare name becomes markdown; markdown is seeded with its title
        // heading so the editor never opens on an invisible empty document.
        let plain = create_library_file(&root.0, "notes").expect("create");
        assert_eq!(plain.file_name().and_then(|n| n.to_str()), Some("notes.md"));
        assert_eq!(fs::read_to_string(&plain).expect("read"), "# notes\n");
        let explicit = create_library_file(&root.0, "plan.md").expect("create");
        assert_eq!(fs::read_to_string(&explicit).expect("read"), "# plan\n");
        let kept = create_library_file(&root.0, "style.css").expect("create");
        assert_eq!(kept.file_name().and_then(|n| n.to_str()), Some("style.css"));
        assert_eq!(fs::read_to_string(&kept).expect("read"), "");

        let err = create_library_file(&root.0, "notes").expect_err("collision");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        for bad in ["", "  ", ".hidden", "a/b", "a\\b"] {
            let err = create_library_file(&root.0, bad).expect_err("bad name");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn rename_library_path_keeps_extension_and_refuses_overwrite() {
        let root = ScratchRoot::new("libraryrename");
        root.write("report.pdf", "x");
        root.write("draft.md", "# draft\n");
        root.write("assets/pic.png", "x");

        // A bare name keeps the file's extension; an explicit one changes it.
        let renamed = rename_library_path(&root.0.join("report.pdf"), "final").expect("rename");
        assert_eq!(renamed.file_name().and_then(|n| n.to_str()), Some("final.pdf"));
        let retyped = rename_library_path(&renamed, "final.txt").expect("rename");
        assert_eq!(retyped.file_name().and_then(|n| n.to_str()), Some("final.txt"));
        assert!(!renamed.exists() && retyped.exists());

        // Folders rename plainly, dots and all.
        let dir = rename_library_path(&root.0.join("assets"), "img v0.2").expect("rename dir");
        assert!(dir.is_dir() && dir.join("pic.png").exists());
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("img v0.2"));

        // Never overwrites; a same-name rename is a quiet no-op.
        let err = rename_library_path(&retyped, "draft.md").expect_err("collision");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let same = rename_library_path(&retyped, "final.txt").expect("no-op");
        assert_eq!(same, retyped);
    }

    #[test]
    fn move_library_path_relocates_and_guards() {
        let root = ScratchRoot::new("librarymove");
        let file = root.write("report.md", "# Report\n");
        let sub = root.0.join("archive");
        fs::create_dir_all(&sub).expect("mkdir");

        // A move keeps the name and carries the contents.
        let dest = move_library_path(&file, &sub).expect("move");
        assert_eq!(dest, sub.join("report.md"));
        assert!(!file.exists());
        assert_eq!(fs::read_to_string(&dest).expect("read"), "# Report\n");

        // A name already at the target is refused; nothing moves.
        let clash = root.write("report.md", "# Other\n");
        let err = move_library_path(&clash, &sub).expect_err("collision");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(clash.exists());

        // A folder can't be dropped into itself or a descendant.
        let outer = root.0.join("outer");
        let inner = root.0.join("outer/inner");
        fs::create_dir_all(&inner).expect("mkdir");
        assert!(move_library_path(&outer, &inner).is_err());
        assert!(outer.exists());

        // A move into the item's own folder is a quiet no-op.
        assert_eq!(move_library_path(&clash, &root.0).expect("noop"), clash);
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
