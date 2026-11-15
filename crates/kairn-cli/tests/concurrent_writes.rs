use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use kairn_core::{NoteBuffer, NoteLock};

struct Vault {
    root: PathBuf,
    _directory: tempfile::TempDir,
}

impl Vault {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        fs::create_dir_all(root.join("Notes")).unwrap();
        Self { root, _directory: directory }
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kairn"))
            .arg("--root")
            .arg(&self.root)
            .env("KAIRN_CONFIG", self.root.join("settings.json"))
            .args(args)
            .output()
            .unwrap()
    }
}

fn assert_busy(output: Output, path: &Path, expected: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("another Kairn writer"));
    assert_eq!(fs::read_to_string(path).unwrap(), expected);
}

#[test]
fn editor_transaction_excludes_cli_append_and_section_until_replacement_finishes() {
    let vault = Vault::new();
    let path = vault.root.join("Notes/Example.md");
    let base = "# Example\n\n## Status\nwaiting\n\n## Log\n";
    fs::write(&path, base).unwrap();
    let lock = NoteLock::acquire(&path).unwrap();
    assert_eq!(lock.read_text().unwrap(), base);
    assert_busy(
        vault.cli(&["append", "Example", "CLI addition"]),
        &path,
        base,
    );
    let section = ["section", "Example", "Status", "--replace", "ready"];
    assert_busy(vault.cli(&section), &path, base);
    let edited = format!("{base}\nEditor addition\n");
    lock.write_text(&edited).unwrap();
    assert_busy(
        vault.cli(&["append", "Example", "CLI addition"]),
        &path,
        &edited,
    );
    drop(lock);
    assert!(
        vault
            .cli(&["append", "Example", "CLI addition"])
            .status
            .success()
    );
    assert!(vault.cli(&section).status.success());
    let disk = fs::read_to_string(path).unwrap();
    assert!(disk.contains("ready"));
    assert!(disk.contains("Editor addition"));
    assert!(disk.contains("CLI addition"));
}

#[test]
fn cli_and_editor_insertions_both_survive_a_locked_buffer_save() {
    let vault = Vault::new();
    let path = vault.root.join("Notes/Example.md");
    let base = "# Example\n";
    fs::write(&path, base).unwrap();
    let mut buffer = NoteBuffer::new(base);
    buffer.edit(base.len()..base.len(), "Editor addition\n", base.len(), 0);
    assert!(
        vault
            .cli(&["append", "Example", "CLI addition"])
            .status
            .success()
    );
    let lock = NoteLock::acquire(&path).unwrap();
    buffer.reconcile(&lock.read_text().unwrap(), 0);
    lock.write_text(buffer.text()).unwrap();
    let disk = lock.read_text().unwrap();
    assert!(disk.contains("Editor addition"));
    assert!(disk.contains("CLI addition"));
}
