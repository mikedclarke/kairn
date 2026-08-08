//! What syncs and what doesn't (spec §4). Everything under the vault root syncs
//! recursively — including `.kairn/` and existing `*.sync-conflict-*` copies —
//! except the exclusions below. The rule is applied to vault-relative,
//! forward-slashed paths so it is identical on every platform.

use crate::types::VaultPath;

/// Whether a vault-relative path is excluded from sync (spec §4).
pub fn is_ignored(path: &VaultPath) -> bool {
    let rel = path.0.trim_start_matches('/');

    // `.kairn/local/` is reserved for per-device state that must never sync.
    if rel == ".kairn/local" || rel.starts_with(".kairn/local/") {
        return true;
    }

    rel.split('/').any(is_ignored_component)
}

/// A single path segment that, wherever it appears, excludes the file.
fn is_ignored_component(comp: &str) -> bool {
    // OS junk.
    if comp == ".DS_Store" || comp == "Thumbs.db" || comp.starts_with(".Trash") {
        return true;
    }
    // Syncthing internals (the folder marker, versions archive, ignore file).
    if comp == ".stfolder" || comp == ".stversions" || comp == ".stignore" {
        return true;
    }
    // Editor / office lock and scratch files.
    if comp.starts_with(".#") || comp.starts_with("~$") || comp.ends_with(".tmp") {
        return true;
    }
    // The engine's own atomic-write temp files (`.{name}.kairn-tmp.{pid}.{n}`,
    // matching write.rs): never treat one mid-rename as a real change.
    if comp.contains(".kairn-tmp.") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(p: &str) -> bool {
        is_ignored(&VaultPath::new(p))
    }

    #[test]
    fn ordinary_notes_and_kairn_metadata_sync() {
        assert!(!ignored("Calendar/20260808.md"));
        assert!(!ignored("Notes/Project/Idea.md"));
        assert!(!ignored(".kairn/templates/daily.md"));
        // NotePlan's trash is real content (note the @, not a dot).
        assert!(!ignored("Notes/@Trash/old.md"));
    }

    #[test]
    fn conflict_copies_must_sync() {
        // Users need to see conflict copies on every device (spec §4).
        assert!(!ignored(
            "Calendar/20260808.sync-conflict-20260808-101112-IPHONE.md"
        ));
    }

    #[test]
    fn per_device_state_never_syncs() {
        assert!(ignored(".kairn/local"));
        assert!(ignored(".kairn/local/device.json"));
        // ...but the rest of .kairn/ still does.
        assert!(!ignored(".kairn/vault.json"));
    }

    #[test]
    fn os_and_tool_junk_is_excluded() {
        assert!(ignored(".DS_Store"));
        assert!(ignored("Notes/.DS_Store"));
        assert!(ignored("Thumbs.db"));
        assert!(ignored(".Trashes/x"));
        assert!(ignored("Notes/.stfolder/marker"));
        assert!(ignored("Notes/.stversions/old.md"));
        assert!(ignored(".stignore"));
    }

    #[test]
    fn editor_and_atomic_write_temp_files_are_excluded() {
        assert!(ignored("Notes/.#open.md"));
        assert!(ignored("Notes/~$doc.md"));
        assert!(ignored("Notes/draft.tmp"));
        assert!(ignored("Calendar/.20260808.md.kairn-tmp.4321.7"));
    }
}
