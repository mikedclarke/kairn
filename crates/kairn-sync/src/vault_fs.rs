use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use cap_fs_ext::{DirExt, FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

use crate::types::VaultPath;

pub(super) fn parent(root: &Dir, path: &VaultPath, create: bool) -> io::Result<(Dir, String)> {
    if !path.is_safe() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe vault path",
        ));
    }
    let (dir, name) = match path.0.rsplit_once('/') {
        Some((parent, name)) => (directory(root, &VaultPath(parent.into()), create)?, name),
        None => (root.try_clone()?, path.0.as_str()),
    };
    Ok((dir, name.to_owned()))
}

pub(super) fn directory(root: &Dir, path: &VaultPath, create: bool) -> io::Result<Dir> {
    if !path.is_safe() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe vault path",
        ));
    }
    let mut dir = root.try_clone()?;
    for part in path.0.split('/') {
        dir = match dir.open_dir_nofollow(part) {
            Ok(child) => child,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match dir.create_dir(part) {
                    Ok(()) => (),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                    Err(error) => return Err(error),
                }
                dir.open_dir_nofollow(part)?
            }
            Err(error) => return Err(error),
        };
    }
    Ok(dir)
}

pub(super) fn same_directory(left: &Dir, right: &Dir) -> io::Result<bool> {
    let left = left.dir_metadata()?;
    let right = right.dir_metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
}

pub(super) fn open(dir: &Dir, name: &str) -> io::Result<File> {
    let file = dir.open_with(name, options().read(true))?.into_std();
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "vault entry is not a regular file",
        ));
    }
    Ok(file)
}

pub(super) fn read_opt(dir: &Dir, name: &str) -> io::Result<Option<Vec<u8>>> {
    match open(dir, name) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn lock(dir: &Dir, name: &str) -> io::Result<File> {
    // Match kairn_core::NoteLock's persistent sidecar, opened without symlinks.
    let file = dir
        .open_with(
            format!(".{name}.kairn-lock"),
            options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false),
        )?
        .into_std();
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "note lock is not a regular file",
        ));
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "another Kairn writer is editing this file; retry the save",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

pub(super) fn sync_directory(dir: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    // Capability handles may use O_PATH, which cannot be synced.
    dir.open(".")?.sync_all()?;
    Ok(())
}

pub(super) fn atomic_write(
    dir: &Dir,
    name: &str,
    content: &[u8],
    before_rename: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let permissions = match open(dir, name) {
        Ok(file) => Some(file.metadata()?.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut temporary_options = options();
    temporary_options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(permissions) = &permissions {
        use cap_std::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        temporary_options.mode(permissions.mode());
    }
    let (temp, mut file) = loop {
        let temp = format!(
            ".{name}.kairn-tmp.{}.{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        match dir.open_with(&temp, &temporary_options) {
            Ok(file) => break (temp, file.into_std()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(content)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        if !before_rename()? {
            return Ok(false);
        }
        dir.rename(&temp, dir, name)?;
        sync_directory(dir)?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = dir.remove_file(&temp);
    }
    result
}
