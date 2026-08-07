//! The agent activity log: an append-only JSONL file at
//! `.kairn/activity.jsonl` inside the notes root. Every write the CLI makes
//! lands here as one line, the app renders the recent entries in the
//! sidebar's Agents section, and because the log lives with the notes it
//! syncs between machines like any other content.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One logged action. `target` is relative to the notes root so entries
/// stay meaningful when the log syncs to a machine with a different root.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ActivityEntry {
    /// Local wall-clock time, `YYYY-MM-DD HH:MM:SS`.
    pub ts: String,
    /// Who acted: `$KAIRN_ACTOR`, or `cli` when unset.
    pub actor: String,
    /// The verb as the CLI spells it: `add`, `done`, `capture`.
    pub action: String,
    /// Root-relative path of the file written.
    pub target: String,
    /// Human-readable one-liner of what changed.
    pub detail: String,
}

/// Where the log lives inside a notes root.
pub fn activity_log_path(root: &Path) -> PathBuf {
    root.join(".kairn").join("activity.jsonl")
}

/// Append one entry to the log. Uses `O_APPEND` rather than the
/// read-rewrite path the note writes use: two agents logging at once must
/// both land, and a log line is small enough that appends stay atomic.
pub fn log_activity(root: &Path, entry: &ActivityEntry) -> io::Result<()> {
    let path = activity_log_path(root);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(entry)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())
}

/// The most recent `limit` entries, newest first. Unparseable lines are
/// skipped: the log is shared, synced state and one bad line must not hide
/// the rest.
pub fn recent_activity(root: &Path, limit: usize) -> Vec<ActivityEntry> {
    let Ok(text) = fs::read_to_string(activity_log_path(root)) else {
        return Vec::new();
    };
    let mut entries: Vec<ActivityEntry> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    entries.reverse();
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScratchRoot;

    fn entry(action: &str, detail: &str) -> ActivityEntry {
        ActivityEntry {
            ts: "2026-08-07 19:00:00".into(),
            actor: "test".into(),
            action: action.into(),
            target: "Calendar/20260807.md".into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn log_appends_and_reads_newest_first() {
        let root = ScratchRoot::new("activity");
        assert!(recent_activity(&root.0, 10).is_empty());

        log_activity(&root.0, &entry("add", "first")).expect("log");
        log_activity(&root.0, &entry("done", "second")).expect("log");

        let recent = recent_activity(&root.0, 10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].detail, "second");
        assert_eq!(recent[1].detail, "first");

        assert_eq!(recent_activity(&root.0, 1).len(), 1);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let root = ScratchRoot::new("activity-bad");
        log_activity(&root.0, &entry("add", "good")).expect("log");
        let path = activity_log_path(&root.0);
        let mut text = fs::read_to_string(&path).expect("read");
        text.push_str("not json\n");
        fs::write(&path, text).expect("write");

        let recent = recent_activity(&root.0, 10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].detail, "good");
    }
}
