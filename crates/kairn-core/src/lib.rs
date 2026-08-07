//! Notes on disk, NotePlan-compatible. No UI dependencies: everything here
//! is shared by the app and the `kairn` CLI.
//!
//! One user-set notes root (default `~/kairn`) holding `Calendar/` for period
//! notes (daily `YYYYMMDD.md`, weekly `YYYY-Wnn.md`, monthly `YYYY-MM.md`,
//! quarterly `YYYY-Qn.md`, yearly `YYYY.md`), `Notes/` for everything else,
//! and a hidden `.kairn/` folder for app data that syncs with the notes.
//! Dailies drive the calendar and task views; the other period notes are
//! indexed for links, search, and mentions. Files are plain markdown;
//! NotePlan must be able to read anything Kairn writes and vice versa, so
//! pointing the root at an existing NotePlan directory just works.
//!
//! Task syntax follows NotePlan alongside standard markdown: a bare `* task`
//! is an open task, `[x]`/`[>]`/`[-]` mark done, scheduled, and cancelled,
//! `+ item` lines are checklists, `- item` is a plain bullet unless
//! bracketed.

pub mod buffer;
pub mod links;
pub mod merge;
pub mod parse;
pub mod settings;
pub mod tasks;
pub mod template;
pub mod vault;
pub mod write;

pub use buffer::*;
pub use links::*;
pub use merge::*;
pub use parse::*;
pub use tasks::*;
pub use template::*;
pub use vault::*;
pub use write::*;

/// A scratch notes root with a NotePlan-shaped tree, removed on drop.
#[cfg(test)]
pub(crate) struct ScratchRoot(pub std::path::PathBuf);

#[cfg(test)]
impl ScratchRoot {
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kairn-test-{tag}-{nanos}"));
        vault::ensure_layout(&root);
        Self(root)
    }

    pub fn write(&self, rel: &str, content: &str) -> std::path::PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write");
        path
    }
}

#[cfg(test)]
impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
