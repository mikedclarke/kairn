use std::fs::{self, File};
use std::io;
use std::path::Path;

pub(crate) fn publish(
    path: &Path,
    overwrite: bool,
    write_contents: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<bool> {
    let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let permissions = match fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let prefix = format!(".{}.kairn-tmp.", name.to_string_lossy());
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix);
    if let Some(permissions) = &permissions {
        builder.permissions(permissions.clone());
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o666));
        }
    }
    let mut temporary = builder.tempfile_in(parent)?;
    write_contents(temporary.as_file_mut())?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    let result = if overwrite {
        temporary.persist(path)
    } else {
        temporary.persist_noclobber(path)
    };
    match result {
        Ok(_) => (),
        Err(error) if !overwrite && error.error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(false);
        }
        Err(error) => return Err(error.error),
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn partial_writes_leave_the_original_intact_or_the_new_file_absent() {
        let root = crate::ScratchRoot::new("atomic-failure");
        let existing = root.write("Notes/@Templates/Daily.md", "original template\n");
        let fresh = root.0.join("Notes/Fresh.md");
        for (path, overwrite) in [(&existing, true), (&fresh, false)] {
            let result = publish(path, overwrite, |file| {
                file.write_all(b"partial")?;
                if overwrite {
                    assert_eq!(fs::read_to_string(path).unwrap(), "original template\n");
                } else {
                    assert!(!path.exists());
                }
                Err(io::Error::other("disk full"))
            });
            assert!(result.is_err());
            assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".kairn-tmp.")
            }));
        }
        assert_eq!(fs::read_to_string(existing).unwrap(), "original template\n");
        assert!(!fresh.exists());
    }

    #[test]
    fn initial_publication_refuses_a_file_created_during_the_write() {
        let root = crate::ScratchRoot::new("atomic-create-race");
        let path = root.0.join("Notes/Fresh.md");
        assert!(
            !publish(&path, false, |file| {
                file.write_all(b"our complete note")?;
                fs::write(&path, "concurrent note")
            })
            .unwrap()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "concurrent note");
    }

    #[cfg(unix)]
    #[test]
    fn private_permissions_apply_before_any_content_is_written() {
        use std::os::unix::fs::PermissionsExt;
        let root = crate::ScratchRoot::new("atomic-private");
        let path = root.write("Notes/Private.md", "old secret");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        publish(&path, true, |file| {
            assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            file.write_all(b"new secret")
        })
        .unwrap();
    }
}
