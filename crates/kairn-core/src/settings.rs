use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SshHost {
    pub name: String,
    /// Connection target as the ssh CLI takes it, e.g. `user@host`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl SshHost {
    pub fn command_args(&self) -> Vec<String> {
        let mut args = vec!["ssh".to_string()];
        if let Some(port) = self.port {
            args.push("-p".to_string());
            args.push(port.to_string());
        }
        args.push(self.target.clone());
        args
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Folder all notes load from (NotePlan-compatible layout). Unset means
    /// the default `~/kairn`. A leading `~` is expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_root: Option<String>,
    #[serde(default)]
    pub ssh_hosts: Vec<SshHost>,
    /// Dev flag: the single-buffer note editor instead of the per-line
    /// editor. No UI; set `"new_editor": true` in settings.json. Removed
    /// when the new editor becomes the only one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_editor: bool,
    /// Set when the settings file existed but couldn't be parsed. While
    /// degraded, [`Settings::save`] refuses to run: auto-saving defaults
    /// over a corrupt file is how SSH hosts and the notes root get wiped.
    /// An explicit settings-dialog apply clears it.
    #[serde(skip)]
    pub degraded: bool,
}

fn default_theme() -> String {
    "dark".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            notes_root: None,
            ssh_hosts: Vec::new(),
            new_editor: false,
            degraded: false,
        }
    }
}

fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    home_dir().join(".config").join("kairn").join("settings.json")
}

impl Settings {
    /// Resolved notes root: the configured folder, or `~/kairn`.
    pub fn notes_root(&self) -> PathBuf {
        match self.notes_root.as_deref() {
            None | Some("") => home_dir().join("kairn"),
            Some(raw) => {
                if let Some(rest) = raw.strip_prefix("~/") {
                    home_dir().join(rest)
                } else {
                    PathBuf::from(raw)
                }
            }
        }
    }

    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    // Keep the evidence and refuse to auto-save until the
                    // user acts: overwriting now would destroy their SSH
                    // hosts and notes-root with defaults.
                    eprintln!("kairn: malformed {}: {e}", path.display());
                    let backup = path.with_extension("json.corrupt");
                    match fs::rename(path, &backup) {
                        Ok(()) => eprintln!("kairn: kept the file as {}", backup.display()),
                        Err(e) => eprintln!("kairn: could not back it up: {e}"),
                    }
                    Self { degraded: true, ..Self::default() }
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if self.degraded {
            anyhow::bail!(
                "settings are running on defaults after a corrupt file; \
                 apply Settings once to start saving again"
            );
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).context("creating config directory")?;
        }
        let text = serde_json::to_string_pretty(self)?;
        // Atomic: a crash mid-write must never leave a half-written file,
        // which is exactly how settings corruption happens.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("kairn-settings-{name}-{nanos}"))
    }

    #[test]
    fn corrupt_settings_back_up_and_block_saving() {
        let dir = scratch("corrupt");
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        fs::write(&path, "{ not json").expect("write");

        let settings = Settings::load_from(&path);
        assert!(settings.degraded);
        // The corrupt original is preserved, not silently replaced.
        assert!(!path.exists());
        let backup = path.with_extension("json.corrupt");
        assert_eq!(fs::read_to_string(&backup).expect("read"), "{ not json");
        // Saving while degraded is refused, so nothing can clobber it.
        assert!(settings.save_to(&path).is_err());
        assert!(!path.exists());

        // An explicit user apply clears the flag and saving round-trips.
        let mut settings = settings;
        settings.degraded = false;
        settings.theme = "light".into();
        settings.save_to(&path).expect("save");
        let reloaded = Settings::load_from(&path);
        assert!(!reloaded.degraded);
        assert_eq!(reloaded.theme, "light");
        let _ = fs::remove_dir_all(&dir);
    }
}
