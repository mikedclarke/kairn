use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Publish a complete, durable conflict copy without replacing an earlier copy.
pub fn write_conflict_copy(note: &Path, content: &str) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = note
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = note
        .file_stem()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "note has no file name"))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    loop {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = stem.to_os_string();
        name.push(format!(
            ".sync-conflict-{stamp}-LOCAL-{}-{id}",
            std::process::id()
        ));
        if let Some(ext) = note.extension() {
            name.push(".");
            name.push(ext);
        }
        let copy = dir.join(&name);
        let mut tmp_name = std::ffi::OsString::from(".");
        tmp_name.push(&name);
        tmp_name.push(".kairn-tmp");
        let tmp = dir.join(tmp_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&tmp) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result: io::Result<()> = (|| {
            if let Ok(meta) = fs::metadata(note) {
                file.set_permissions(meta.permissions())?;
            }
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            // Linking publishes without replacing a concurrent writer's copy.
            fs::hard_link(&tmp, &copy)?;
            #[cfg(unix)]
            fs::File::open(dir)?.sync_all()?;
            Ok(())
        })();
        drop(file);
        let _ = fs::remove_file(&tmp);
        match result {
            Ok(()) => return Ok(copy),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoteBuffer, ScratchRoot};

    #[test]
    fn conflicts_survive_closed_buffers_and_rediscovery() {
        let root = ScratchRoot::new("recovery");
        let first = root.write("Calendar/20261001.md", "first\ncontext\n");
        let second = root.write("Notes/Second.md", "second\ncontext\n");
        let mut expected = Vec::new();
        for (note, base, ours, disk) in [
            (&first, "first", "first local", "first remote"),
            (&second, "second", "second local", "second remote"),
            (&first, "first remote", "another local", "another remote"),
        ] {
            let mut buffer = NoteBuffer::new(format!("{base}\ncontext\n"));
            buffer.edit(0..base.len(), ours, 0, 1000);
            let local = buffer.text().to_string();
            let remote = format!("{disk}\ncontext\n");
            fs::write(note, &remote).unwrap();
            buffer
                .try_reconcile(&remote, 0, |text| {
                    let copy = write_conflict_copy(note, text)?;
                    expected.push((note.clone(), copy, local.clone()));
                    Ok::<_, io::Error>(())
                })
                .unwrap();
            assert_eq!(buffer.text(), remote);
        }
        let found = crate::vault_conflicts(&root.0);
        assert_eq!(found.len(), 3);
        for (owner, copy, local) in expected {
            assert!(found.contains(&(owner, copy.clone())));
            assert_eq!(fs::read_to_string(copy).unwrap(), local);
        }
    }

    #[test]
    fn failed_recovery_leaves_the_buffer_and_undo_intact() {
        let root = ScratchRoot::new("recovery-failure");
        let note = root.0.join("missing/Note.md");
        let mut buffer = NoteBuffer::new("base\n");
        buffer.edit(0..4, "local", 0, 1000);
        let revision = buffer.revision();
        let result = buffer.try_reconcile("remote\n", 0, |text| {
            write_conflict_copy(&note, text).map(|_| ())
        });
        assert!(result.is_err());
        assert_eq!(buffer.text(), "local\n");
        assert_eq!(buffer.baseline(), "base\n");
        assert_eq!(buffer.revision(), revision);
        assert!(buffer.is_dirty());
        assert!(!note.parent().unwrap().exists());
        buffer.undo();
        assert_eq!(buffer.text(), "base\n");
        buffer.redo();
        assert_eq!(buffer.text(), "local\n");
        fs::create_dir(note.parent().unwrap()).unwrap();
        buffer
            .try_reconcile("remote\n", 0, |text| {
                write_conflict_copy(&note, text).map(|_| ())
            })
            .unwrap();
        let copies = crate::conflict_copies(&note);
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read_to_string(&copies[0]).unwrap(), "local\n");
    }

    #[test]
    fn concurrent_conflict_copies_are_distinct_and_complete() {
        let root = ScratchRoot::new("recovery-concurrent");
        let note = root.write("Notes/Note.txt", "disk\r\n");
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let note = &note;
                    scope.spawn(move || {
                        let content = format!("local {i}\r\n");
                        (write_conflict_copy(note, &content).unwrap(), content)
                    })
                })
                .collect();
            for handle in handles {
                let (copy, content) = handle.join().unwrap();
                assert_eq!(crate::conflict_owner(&copy), Some(note.clone()));
                assert_eq!(fs::read_to_string(copy).unwrap(), content);
            }
        });
        assert_eq!(crate::conflict_copies(&note).len(), 8);
        assert_eq!(fs::read_to_string(note).unwrap(), "disk\r\n");
    }
}
