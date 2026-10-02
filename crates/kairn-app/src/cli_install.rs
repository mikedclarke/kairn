//! Linking the bundled `kairn` CLI onto the user's PATH.
//!
//! The GUI ships the CLI beside its own executable — `Contents/MacOS/kairn`
//! inside the macOS bundle, `/usr/bin/kairn` from a Linux package. This links
//! it into a directory on PATH so `kairn` works from any terminal and agents
//! can drive it.

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Where the CLI is linked on PATH.
const DEST: &str = "/usr/local/bin/kairn";

#[cfg(target_os = "macos")]
const INSTALL_SCRIPT: &str = "on run argv\n do shell script (item 1 of argv) with administrator privileges\nend run";

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn link_command(src: &Path, dest: &Path) -> Option<String> {
    Some(format!("mkdir -p {} && ln -sfn -- {} {}",
        shell_quote(dest.parent()?.to_str()?),
        shell_quote(src.to_str()?), shell_quote(dest.to_str()?)))
}

fn replace_link(src: &Path, dest: &Path) -> std::io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    loop {
        let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = dest.with_file_name(format!(".kairn-install-{}-{seq}", std::process::id()));
        match std::os::unix::fs::symlink(src, &temp) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
            Ok(()) => {
                let result = std::fs::rename(&temp, dest);
                let _ = std::fs::remove_file(temp);
                return result;
            }
        }
    }
}

/// The CLI shipped beside this executable, if present.
pub fn bundled_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe.parent()?.join("kairn");
    cli.exists().then_some(cli)
}

/// Is `kairn` already on PATH? Checked by looking for the file rather than
/// spawning it, since a Finder-launched app has a minimal PATH that would not
/// find an installed link. A Linux package installs it to `/usr/bin`.
pub fn already_installed() -> bool {
    Path::new(DEST).exists() || Path::new("/usr/bin/kairn").exists()
}

/// The outcome of an install attempt.
pub enum Outcome {
    /// Linked into a directory on PATH.
    Linked(PathBuf),
    /// Kairn could not link it; the reason and, when possible, the one command
    /// the user can run to finish the job.
    Manual { reason: String, command: String },
}

/// Link the bundled CLI onto PATH, elevating with a native auth prompt if the
/// target directory is not writable.
pub fn install() -> Outcome {
    let Some(src) = bundled_cli() else {
        return Outcome::Manual {
            reason: "Kairn could not find the bundled kairn CLI beside the app.".into(),
            command: String::new(),
        };
    };
    let dest = PathBuf::from(DEST);

    // Fast path: the target directory is writable (e.g. a Homebrew setup where
    // /usr/local/bin is user-owned), so no elevation is needed.
    if let Some(parent) = dest.parent()
        && parent.is_dir()
        && replace_link(&src, &dest).is_ok()
    {
        return Outcome::Linked(dest);
    }

    let Some(command) = link_command(&src, &dest).filter(|_| !dest.is_dir()) else {
        return Outcome::Manual {
            reason: "Kairn could not safely link this path. Move the app or check /usr/local/bin/kairn.".into(),
            command: String::new(),
        };
    };

    // Otherwise ask for administrator rights. macOS shows a native auth dialog
    // via osascript; elsewhere fall back to printing the command.
    #[cfg(target_os = "macos")]
    {
        if let Ok(status) = Command::new("osascript").args(["-e", INSTALL_SCRIPT, "--", &command]).status()
            && status.success()
        {
            return Outcome::Linked(dest);
        }
    }

    Outcome::Manual {
        reason: "Kairn couldn't write to /usr/local/bin.".into(),
        command: format!("sudo sh -c {}", shell_quote(&command)),
    }
}

#[cfg(test)]
mod tests {
    use super::{link_command, replace_link, shell_quote};
    use std::process::Command;

    #[test]
    fn shell_and_applescript_keep_unusual_paths_as_data() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("App's \"quoted\" \\ $(touch injected) `touch injected`\nκ");
        std::fs::write(&src, b"fixture").unwrap();
        let dest = dir.path().join("bin/cli");
        let command = link_command(&src, &dest).unwrap();
        #[cfg(target_os = "macos")]
        let command = {
            let output = Command::new("osascript")
                .args(["-e", "on run argv\nreturn item 1 of argv\nend run", "--", &command])
                .output().unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim_end(), command);
            command
        };
        let status = Command::new("sh").args(["-c", &format!("sh -c {}", shell_quote(&command))])
            .current_dir(dir.path()).status().unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_link(&dest).unwrap(), src);
        assert!(!dir.path().join("injected").exists());
    }

    #[test]
    fn replaces_links_without_removing_the_destination_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cli");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        replace_link(&first, &dest).unwrap();
        replace_link(&second, &dest).unwrap();
        assert_eq!(std::fs::read_link(&dest).unwrap(), second);
        std::fs::remove_file(&dest).unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("keep"), b"existing").unwrap();
        assert!(replace_link(&first, &dest).is_err());
        assert_eq!(std::fs::read(dest.join("keep")).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
