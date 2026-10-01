//! Runtime observations (JSON) — separate from user config. Written only when something changes.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub codex: ProviderState,
    pub claude: ProviderState,
    /// When the daily trigger was last handled (fired or skipped), epoch seconds.
    pub last_daily_run: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderState {
    /// 5-hour window reset time as last reported by the provider, epoch seconds.
    /// Authoritative until it passes; after that the window state is unknown.
    pub resets_at: Option<i64>,
    /// When the provider last reported state to us.
    pub checked_at: Option<i64>,
    pub last_error: Option<String>,
}

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .expect("no per-user data directory")
        .join("ClankShift")
}

fn path() -> PathBuf {
    data_dir().join("state.json")
}

impl State {
    /// Missing or unreadable state just means "nothing known yet".
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Err(e) = std::fs::write(path(), serde_json::to_string_pretty(self).unwrap()) {
            log(&format!("failed to save state: {e}"));
        }
    }
}

/// Append one line to clankshift.log. Only called on provider actions, never on idle.
pub fn log(msg: &str) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join("clankshift.log"))
    {
        let _ = writeln!(f, "{} {msg}", jiff::Zoned::now().strftime("%F %T %z"));
    }
}
