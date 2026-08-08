//! Linking the bundled `kairn` CLI onto the user's PATH.
//!
//! The GUI ships the CLI beside its own executable — `Contents/MacOS/kairn`
//! inside the macOS bundle, `/usr/bin/kairn` from a Linux package. This links
//! it into a directory on PATH so `kairn` works from any terminal and agents
//! can drive it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the CLI is linked on PATH.
const DEST: &str = "/usr/local/bin/kairn";

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
    {
        let _ = std::fs::remove_file(&dest);
        if std::os::unix::fs::symlink(&src, &dest).is_ok() {
            return Outcome::Linked(dest);
        }
    }

    // Otherwise ask for administrator rights. macOS shows a native auth dialog
    // via osascript; elsewhere fall back to printing the command.
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "do shell script \"mkdir -p /usr/local/bin && ln -sf '{}' '{}'\" \
             with administrator privileges",
            src.display(),
            dest.display()
        );
        if let Ok(status) = Command::new("osascript").arg("-e").arg(&script).status()
            && status.success()
        {
            return Outcome::Linked(dest);
        }
    }

    Outcome::Manual {
        reason: "Kairn couldn't write to /usr/local/bin.".into(),
        command: format!("sudo ln -sf '{}' '{}'", src.display(), dest.display()),
    }
}
