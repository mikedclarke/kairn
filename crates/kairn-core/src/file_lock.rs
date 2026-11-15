use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// A nonblocking lock shared by Kairn's note writers. The sidecar must stay in
/// place: replacing or removing it would let writers lock different inodes.
pub struct NoteLock {
    _file: File,
    path: PathBuf,
}

impl NoteLock {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
        let mut lock_name = std::ffi::OsString::from(".");
        lock_name.push(name);
        lock_name.push(".kairn-lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_file_name(lock_name))?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                "another Kairn writer is editing this file; retry the save",
            ),
            std::fs::TryLockError::Error(error) => error,
        })?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn read_text(&self) -> io::Result<String> {
        std::fs::read_to_string(&self.path)
    }

    pub fn write_text(&self, content: &str) -> io::Result<()> {
        crate::write::atomic_write(&self.path, content)
    }
}
