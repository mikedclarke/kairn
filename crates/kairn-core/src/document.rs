use std::io;
use std::path::{Path, PathBuf};

use crate::{NoteBuffer, NoteLock, RecoverySnapshot, TextSave};

pub struct DocumentLifecycle {
    recovery_dir: PathBuf,
    recovery: RecoverySnapshot,
    error: Option<String>,
}

pub struct DocumentChange {
    pub cursor: usize,
    pub text_changed: bool,
    pub conflict_copy: Option<PathBuf>,
}

pub struct DocumentSave {
    pub change: DocumentChange,
    pub saved: bool,
    pub wrote: bool,
}

impl Default for DocumentLifecycle {
    fn default() -> Self {
        Self::new(crate::recovery_directory())
    }
}

impl DocumentLifecycle {
    pub fn new(recovery_dir: PathBuf) -> Self {
        Self {
            recovery_dir,
            recovery: RecoverySnapshot::default(),
            error: None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn preserve_failure(&mut self, path: &Path, content: &str, reason: &str) -> bool {
        let message = match self.recovery.preserve(&self.recovery_dir, path, content) {
            Ok(copy) => format!("{reason}. Recovery copy: {}", copy.display()),
            Err(error) => {
                format!("{reason}. Recovery also failed: {error}. Keep this document open.")
            }
        };
        let changed = self.error.as_ref() != Some(&message);
        self.error = Some(message);
        changed
    }

    pub fn reconcile_buffer(
        &mut self,
        path: &Path,
        buffer: &mut NoteBuffer,
        disk: &str,
        cursor: usize,
        crlf: bool,
    ) -> Option<DocumentChange> {
        match reconcile(path, buffer, disk, cursor, crlf) {
            Ok(change) => {
                if change.conflict_copy.is_some() || !buffer.is_dirty() {
                    self.error = None;
                }
                Some(change)
            }
            Err(error) => {
                self.preserve_failure(
                    path,
                    &encode(buffer.text(), crlf),
                    &format!("Could not preserve conflicting edits: {error}"),
                );
                None
            }
        }
    }

    pub fn save_buffer(
        &mut self,
        path: &Path,
        buffer: &mut NoteBuffer,
        cursor: usize,
        crlf: bool,
    ) -> DocumentSave {
        let mut outcome = DocumentSave {
            change: DocumentChange {
                cursor,
                text_changed: false,
                conflict_copy: None,
            },
            saved: false,
            wrote: false,
        };
        let result: io::Result<()> = (|| {
            if !buffer.is_dirty() {
                return Ok(());
            }
            let lock = NoteLock::acquire(path)?;
            match lock.read_text() {
                Ok(disk) => outcome.change = reconcile(path, buffer, &disk, cursor, crlf)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
            lock.write_text(&encode(buffer.text(), crlf))?;
            buffer.mark_saved();
            outcome.wrote = true;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.error = None;
                outcome.saved = true;
            }
            Err(error) => {
                self.preserve_failure(
                    path,
                    &encode(buffer.text(), crlf),
                    &format!("Could not save this document: {error}"),
                );
            }
        }
        outcome
    }

    pub fn save_text(&mut self, path: &Path, baseline: &str, local: &str) -> Option<TextSave> {
        match crate::save_text(path, baseline, local) {
            Ok(saved) => {
                self.error = None;
                Some(saved)
            }
            Err(error) => {
                self.preserve_failure(
                    path,
                    local,
                    &format!("Could not save this document: {error}"),
                );
                None
            }
        }
    }
}

fn encode(text: &str, crlf: bool) -> String {
    if crlf {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

fn reconcile(
    path: &Path,
    buffer: &mut NoteBuffer,
    disk: &str,
    cursor: usize,
    crlf: bool,
) -> io::Result<DocumentChange> {
    let disk = disk.replace("\r\n", "\n");
    let revision = buffer.revision();
    let mut conflict_copy = None;
    let (cursor, _) = buffer.try_reconcile(&disk, cursor, |local| {
        conflict_copy = Some(crate::write_conflict_copy(path, &encode(local, crlf))?);
        Ok::<_, io::Error>(())
    })?;
    Ok(DocumentChange {
        cursor,
        text_changed: buffer.revision() != revision,
        conflict_copy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_save_refuses_disposal_and_retry_preserves_both_writers() {
        let root = crate::ScratchRoot::new("document-retry");
        let recovery = root.0.join("recovery");
        let path = root.write("Notes/Example.md", "base\n");
        let mut buffer = NoteBuffer::new("base\n");
        buffer.edit(5..5, "local\n", 5, 0);
        let mut lifecycle = DocumentLifecycle::new(recovery.clone());
        let lock = NoteLock::acquire(&path).unwrap();
        assert!(!lifecycle.save_buffer(&path, &mut buffer, 11, false).saved);
        assert!(buffer.is_dirty());
        assert_eq!(buffer.baseline(), "base\n");
        assert!(lifecycle.error().is_some());
        let copies = crate::recovery_files(&recovery);
        assert_eq!(
            std::fs::read_to_string(&copies[0]).unwrap(),
            "base\nlocal\n"
        );
        lock.write_text("base\nexternal\n").unwrap();
        drop(lock);
        assert!(lifecycle.save_buffer(&path, &mut buffer, 11, false).saved);
        assert!(!buffer.is_dirty());
        assert!(lifecycle.error().is_none());
        let disk = std::fs::read_to_string(path).unwrap();
        assert!(disk.contains("local\n"));
        assert!(disk.contains("external\n"));
    }

    #[test]
    fn unreadable_reload_keeps_text_undo_and_recoverable_content() {
        let root = crate::ScratchRoot::new("document-unreadable");
        let recovery = root.0.join("recovery");
        let path = root.write("Notes/Example.md", "base\n");
        let mut lifecycle = DocumentLifecycle::new(recovery.clone());
        let mut buffer = NoteBuffer::new("base\n");
        buffer.edit(5..5, "local\n", 5, 0);
        std::fs::write(&path, [0xff]).unwrap();
        assert!(!lifecycle.save_buffer(&path, &mut buffer, 11, false).saved);
        assert_eq!(buffer.text(), "base\nlocal\n");
        assert!(buffer.undo().is_some());
        assert_eq!(buffer.text(), "base\n");
        assert!(
            lifecycle
                .save_text(&path, "base\n", "local input\n")
                .is_none()
        );
        let copies = crate::recovery_files(&recovery);
        assert_eq!(copies.len(), 2);
        assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
    }

    #[test]
    fn repeated_conflicts_survive_the_document_lifecycle() {
        let root = crate::ScratchRoot::new("document-conflicts");
        let path = root.write("Notes/Example.md", "base\n");
        let mut buffer = NoteBuffer::new("base\n");
        let recovery = root.0.join("recovery");
        let mut lifecycle = DocumentLifecycle::new(recovery.clone());
        for (local, disk) in [
            ("first local\n", "first disk\n"),
            ("second local\n", "second disk\n"),
        ] {
            buffer.edit(0..buffer.text().len(), local, 0, 0);
            std::fs::write(&path, disk).unwrap();
            let outcome = lifecycle.save_buffer(&path, &mut buffer, 0, false);
            assert!(outcome.saved);
            let copy = outcome.change.conflict_copy.unwrap();
            assert_eq!(std::fs::read_to_string(copy).unwrap(), local);
        }
        drop(lifecycle);
        drop(buffer);
        assert_eq!(crate::conflict_copies(&path).len(), 2);
    }

    #[test]
    fn conflict_preservation_failure_leaves_the_buffer_unchanged() {
        let root = crate::ScratchRoot::new("document-conflict-failure");
        let path = root.0.join("missing/Example.md");
        let mut buffer = NoteBuffer::new("base\n");
        buffer.edit(0..5, "local\n", 0, 0);
        let mut lifecycle = DocumentLifecycle::new(root.0.join("recovery"));
        assert!(
            lifecycle
                .reconcile_buffer(&path, &mut buffer, "external\n", 6, false)
                .is_none()
        );
        assert_eq!(buffer.text(), "local\n");
        assert_eq!(buffer.baseline(), "base\n");
        assert!(buffer.is_dirty());
    }
}
