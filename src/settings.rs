use std::fs;
use std::path::PathBuf;

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
    #[serde(default)]
    pub ssh_hosts: Vec<SshHost>,
}

fn default_theme() -> String {
    "dark".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            ssh_hosts: Vec::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    #[allow(deprecated)]
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("kairn").join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    eprintln!("kairn: ignoring malformed {}: {e}", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).context("creating config directory")?;
        }
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}
