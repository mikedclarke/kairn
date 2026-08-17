use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// A saved launch shortcut: a command that opens in its own session, either
/// on this machine (`Settings::local_apps`) or on the SSH host that owns it
/// (`SshHost::apps`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HostApp {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub command: String,
}

impl HostApp {
    /// The label shown for the shortcut: the trimmed name, else the command's
    /// first word capitalized ("tmux attach" reads as "Tmux"), so a blank
    /// name still yields something readable.
    pub fn display_name(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        let word = self.command.trim().split_whitespace().next().unwrap_or("App");
        let mut chars = word.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => "App".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SshHost {
    pub name: String,
    /// Connection target as the ssh CLI takes it, e.g. `user@host`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Launch shortcuts that run on this host, in the user's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<HostApp>,
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
    /// Extra local directories browsable in the sidebar's Library section.
    /// Per-machine by design: each machine lists the paths that exist on it,
    /// and the list never syncs between devices. A leading `~` is expanded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_roots: Vec<String>,
    /// Library file ordering: "modified" (newest first) or "name". Folders
    /// always sort by name. Unknown values read as "modified".
    #[serde(default = "default_library_sort")]
    pub library_sort: String,
    #[serde(default)]
    pub ssh_hosts: Vec<SshHost>,
    /// Launch shortcuts that run on this machine, in the user's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_apps: Vec<HostApp>,
    /// Whether the sidebar shows the Agents activity section; hidden by
    /// default (it stays empty until agents drive the CLI on this machine).
    #[serde(default)]
    pub show_agents: bool,
    /// Whether the sidebar shows the calendar area: the mini calendar, the
    /// timeline / Daily / Weekly / Monthly switcher, and the day timeline.
    #[serde(default = "default_true")]
    pub show_daily: bool,
    /// Whether the sidebar shows the Tasks section; hidden by default.
    #[serde(default)]
    pub show_tasks: bool,
    /// Direction of the retired three-day sidebar list. Kept so settings
    /// files that carry it still load; nothing reads it.
    #[serde(default = "default_true")]
    pub daily_forward: bool,
    /// Sidebar sections the user has collapsed, by their header labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_collapsed: Vec<String>,
    /// The 7-day week strip over the note pane: "always", "daily" (only on
    /// daily notes), or "off". Unknown values read as "always".
    #[serde(default = "default_week_strip")]
    pub week_strip: String,
    /// When the daily template seeds new days: "always", "weekdays", or
    /// "off". Unknown values read as "always".
    #[serde(default = "default_template_rule")]
    pub daily_template_rule: String,
    /// Font overrides on top of the active theme. UI chrome (unset keeps
    /// the system font), the notes editor (unset follows the UI font), and
    /// the terminal/mono font (unset keeps the auto-resolved mono).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_font: Option<String>,
    /// Editor body size in px; headings scale with it. Unset means 13.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_font_size: Option<f32>,
    /// Interface text size in px: the whole app chrome (sidebar, calendar,
    /// panes, dialogs) scales from it, leaving the notes editor to its own
    /// `editor_font_size`. Unset means 13.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_font_size: Option<f32>,
    /// Set when the settings file existed but couldn't be parsed. While
    /// degraded, [`Settings::save`] refuses to run: auto-saving defaults
    /// over a corrupt file is how SSH hosts and the notes root get wiped.
    /// An explicit settings-dialog apply clears it.
    #[serde(skip)]
    pub degraded: bool,
}

fn default_theme() -> String {
    "menlo".to_string()
}

fn default_true() -> bool {
    true
}

fn default_week_strip() -> String {
    "daily".to_string()
}

fn default_template_rule() -> String {
    "always".to_string()
}

fn default_library_sort() -> String {
    "modified".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            notes_root: None,
            library_roots: Vec::new(),
            library_sort: default_library_sort(),
            ssh_hosts: Vec::new(),
            local_apps: Vec::new(),
            show_agents: false,
            show_daily: true,
            show_tasks: false,
            daily_forward: true,
            sidebar_collapsed: Vec::new(),
            week_strip: default_week_strip(),
            daily_template_rule: default_template_rule(),
            // Fresh-install look: Menlo theme, Noto Sans chrome, everything
            // at 14px. The editor and mono fonts stay unset so the theme
            // drives them (Menlo brings its own); a settings value here
            // would override every theme's fonts. Existing configs keep
            // their own values; these only seed a new install (no
            // settings.json yet).
            ui_font: Some("Noto Sans".to_string()),
            editor_font: None,
            mono_font: None,
            editor_font_size: Some(14.0),
            ui_font_size: Some(14.0),
            degraded: false,
        }
    }
}

fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    // KAIRN_CONFIG points the settings file elsewhere for this process only
    // (dev, testing, screenshots), the way KAIRN_ROOT does the notes root.
    if let Ok(p) = std::env::var("KAIRN_CONFIG")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
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

    /// Resolved library roots, `~` expanded, in the configured order.
    pub fn library_roots(&self) -> Vec<PathBuf> {
        self.library_roots
            .iter()
            .filter(|raw| !raw.is_empty())
            .map(|raw| match raw.strip_prefix("~/") {
                Some(rest) => home_dir().join(rest),
                None => PathBuf::from(raw),
            })
            .collect()
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
