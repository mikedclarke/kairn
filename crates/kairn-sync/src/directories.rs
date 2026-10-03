//! Retirement catalog for directories that file-only sync used to leave behind.
use crate::{
    cycle::SyncContext,
    types::VaultPath,
    vaultio::{Precondition, VaultIo},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CATALOG: &str = ".kairn/directories.json";

#[derive(Serialize, Deserialize, Default)]
pub struct Catalog {
    pub version: u32,
    pub present: BTreeSet<String>,
    pub retired: BTreeSet<String>,
}

fn eligible(path: &str) -> bool {
    path.starts_with("Notes/")
        && VaultPath::new(path).is_safe()
        && path
            .split('/')
            .skip(1)
            .all(|p| !p.starts_with('.') && !p.starts_with('@'))
}

fn decode(bytes: &[u8]) -> Result<Catalog> {
    let c: Catalog = serde_json::from_slice(bytes)?;
    ensure!(c.version == 1, "unsupported directory catalog version");
    ensure!(
        c.present
            .iter()
            .chain(c.retired.iter())
            .all(|p| eligible(p)),
        "unsafe directory catalog path"
    );
    ensure!(
        c.present.is_disjoint(&c.retired),
        "ambiguous directory catalog"
    );
    Ok(c)
}

pub fn publish(ctx: &SyncContext<'_>) -> Result<()> {
    let path = VaultPath::new(CATALOG);
    let before = ctx.vault.read_opt(&path)?;
    let mut known = BTreeSet::new();
    if let Some(bytes) = &before {
        let old = decode(bytes)?;
        known.extend(old.present);
        known.extend(old.retired);
    } else {
        // One migration read of retained history recovers ancestors of notes
        // deleted before catalogs existed. It reads metadata, never note bodies.
        let mut cursor = 0;
        loop {
            let page = ctx.transport.changes(cursor, 500)?;
            for entry in &page.entries {
                cursor = cursor.max(entry.seq);
                let mut ancestor = entry.path.0.as_str();
                while let Some((parent, _)) = ancestor.rsplit_once('/') {
                    if eligible(parent) {
                        known.insert(parent.to_owned());
                    }
                    ancestor = parent;
                }
            }
            if !page.has_more || page.entries.is_empty() {
                break;
            }
        }
    }
    let present = ctx.vault.note_directories()?;
    let retired = known.difference(&present).cloned().collect();
    let bytes = serde_json::to_vec(&Catalog {
        version: 1,
        present,
        retired,
    })?;
    if before.as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    ensure!(
        ctx.vault.write(
            &path,
            &bytes,
            &Precondition::from_local(before.as_deref()),
            &|_| {}
        )?,
        "directory catalog changed during publication; retry"
    );
    Ok(())
}

pub fn reconcile(vault: &VaultIo) -> Result<()> {
    let Some(bytes) = vault.read_opt(&VaultPath::new(CATALOG))? else {
        return Ok(());
    };
    let catalog = decode(&bytes)?;
    let mut retired: Vec<_> = catalog.retired.into_iter().collect();
    retired.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
    for directory in retired {
        vault.remove_empty_directory(&VaultPath::new(directory))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_refuses_roots_traversal_specials_and_unknown_versions() {
        for path in [
            "Notes",
            "../Notes/old",
            "Notes/@Trash",
            "Notes/../other",
            "Calendar/a",
        ] {
            let c = Catalog {
                version: 1,
                present: BTreeSet::new(),
                retired: [path.into()].into(),
            };
            assert!(decode(&serde_json::to_vec(&c).unwrap()).is_err());
        }
        assert!(decode(br#"{"version":2,"present":[],"retired":[]}"#).is_err());
    }
}
