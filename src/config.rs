//! User configuration (TOML). Only the user and the settings window write this file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Master switch for automatic triggers. Manual "Anchor now" always works.
    pub auto_anchor: bool,
    /// Anchor when ClankShift starts (at login if "Start with Windows" is on).
    pub anchor_on_start: bool,
    /// Local time of day for a daily anchor, "HH:MM". None = off.
    pub daily_at: Option<String>,
    pub codex: ProviderConfig,
    pub claude: ProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub enabled: bool,
    /// Path to the CLI. Empty = find it on PATH.
    pub command: String,
    /// Model used for the anchor request. Empty = the provider's built-in cheap default.
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_anchor: true,
            anchor_on_start: true,
            daily_at: None,
            codex: ProviderConfig::default(),
            claude: ProviderConfig::default(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: String::new(),
            model: String::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("clankshift-test");
    }
    dirs::config_dir()
        .expect("no per-user config directory")
        .join("ClankShift")
}

fn path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    /// Missing file = defaults. A broken file is an error rather than silently reset.
    pub fn load() -> Result<Self, String> {
        match std::fs::read_to_string(path()) {
            Ok(s) => toml::from_str(&s).map_err(|e| format!("config.toml: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("config.toml: {e}")),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let s = toml::to_string_pretty(self).unwrap();
        std::fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
        crate::state::write_atomic(&path(), &s).map_err(|e| format!("config.toml: {e}"))
    }

    /// Parsed daily time, if set and valid.
    pub fn daily_time(&self) -> Option<jiff::civil::Time> {
        parse_hhmm(self.daily_at.as_deref()?)
    }
}

pub fn parse_hhmm(s: &str) -> Option<jiff::civil::Time> {
    let (h, m) = s.trim().split_once(':')?;
    jiff::civil::Time::new(h.parse().ok()?, m.parse().ok()?, 0, 0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(parse_hhmm("07:30"), Some(jiff::civil::time(7, 30, 0, 0)));
        assert_eq!(parse_hhmm(" 7:05 "), Some(jiff::civil::time(7, 5, 0, 0)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("7"), None);
    }
}
