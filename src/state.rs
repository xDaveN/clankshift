//! Runtime observations (JSON) — separate from user config. Written only when something changes.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::providers::{claude, codex};

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
    /// Set when state.json could not be read: a 5h limit it recorded may run until then, so
    /// automatic starts wait. Cleared by the next provider report.
    pub unknown_until: Option<i64>,
}

impl ProviderState {
    /// Automatic starts wait while lost saved state may still hide a running 5h limit.
    pub fn auto_paused(&self, now: i64) -> bool {
        self.unknown_until.is_some_and(|u| u > now)
    }
}

/// Latest reset the parsers accept after the moment it was observed: Claude's bound (Codex's,
/// skew plus 1 s rounding, is smaller). Observations are saved after they are made, so a state
/// file holds no reset later than its modified time plus this.
const MAX_RESET_AFTER_WRITE: i64 = crate::schedule::WINDOW_SECS + claude::MAX_RESET_EXTRA_SECS;
const _: () = assert!(codex::CLOCK_SKEW_SECS < claude::MAX_RESET_EXTRA_SECS);

pub fn data_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("clankshift-test");
    }
    dirs::data_local_dir()
        .expect("no per-user data directory")
        .join("ClankShift")
}

fn path() -> PathBuf {
    data_dir().join("state.json")
}

impl State {
    /// Missing state means "nothing known yet". An unreadable file is not the same: it was last
    /// written at its modified time, so any 5h limit it held resets by `MAX_RESET_AFTER_WRITE`
    /// after that.
    pub fn load(now: i64) -> Self {
        Self::load_from(&path(), now)
    }

    fn load_from(path: &Path, now: i64) -> Self {
        let err = match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(st) => return st,
                Err(e) => e.to_string(),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => e.to_string(),
        };
        let written = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(now, |d| d.as_secs() as i64);
        let until = written + MAX_RESET_AFTER_WRITE;
        let mut st = Self::default();
        if until > now {
            let at = jiff::Timestamp::from_second(until).map_or(until.to_string(), |t| {
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%F %T")
                    .to_string()
            });
            let msg = format!(
                "state.json unreadable ({err}); automatic starts paused until {at}, \
                 the latest a 5h limit recorded there could reset"
            );
            log(&msg);
            for p in [&mut st.codex, &mut st.claude] {
                p.last_error = Some(msg.clone());
                p.unknown_until = Some(until);
            }
        } else {
            log(&format!(
                "state.json unreadable ({err}); any 5h limit it held has reset, starting fresh"
            ));
        }
        st
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Err(e) = self.save_to(&path()) {
            log(&format!("failed to save state: {e}"));
        }
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, &serde_json::to_string_pretty(self).unwrap())
    }
}

/// Replace `path` with `contents` so a crash or a concurrent writer leaves one complete file,
/// never a truncated or mixed one: write and flush a temp sibling that only this call owns
/// (unique name, created new), then rename it over the original.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = write_temp(path, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| _ = std::fs::remove_file(&tmp))
}

/// Write `contents` to a new temp sibling of `path` that no other writer can open, and flush it.
fn write_temp(path: &Path, contents: &str) -> std::io::Result<PathBuf> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{seq}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(contents.as_bytes())
        .and_then(|()| f.sync_all())
        .inspect_err(|_| _ = std::fs::remove_file(&tmp))?;
    Ok(tmp)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Outcome;
    use crate::schedule::{WINDOW_SECS, known_active};

    fn test_file(name: &str) -> PathBuf {
        let dir = data_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("state.json")
    }

    /// Truncate `p` as a crash would, last modified at `t`.
    fn corrupt(p: &Path, t: i64) {
        std::fs::write(p, "{\"claude\": {\"resets_at\": 17").unwrap();
        let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64);
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(mtime).unwrap();
    }

    /// What `App::anchor` lets an automatic start do.
    fn auto_would_send(st: &ProviderState, now: i64) -> bool {
        !known_active(st.resets_at, now) && !st.auto_paused(now)
    }

    #[test]
    fn missing_and_valid_state() {
        let p = test_file("valid");
        let now = jiff::Timestamp::now().as_second();
        let st = State::load_from(&p, now);
        assert!(auto_would_send(&st.claude, now) && st.claude.last_error.is_none());

        let mut st = State::default();
        st.claude.resets_at = Some(now + 3600);
        st.save_to(&p).unwrap();
        st.claude.resets_at = Some(now + 7200);
        st.save_to(&p).unwrap();
        let st = State::load_from(&p, now);
        assert_eq!(st.claude.resets_at, Some(now + 7200));
        assert!(!st.claude.auto_paused(now));
        let files = std::fs::read_dir(p.parent().unwrap()).unwrap().count();
        assert_eq!(files, 1, "temp file left behind");
    }

    #[test]
    fn unreadable_state_pauses_until_the_latest_accepted_reset() {
        let p = test_file("horizon");
        let t = 1_790_000_000;
        corrupt(&p, t);
        // The latest reset a Claude reply observed at the last write could carry.
        let latest = t + WINDOW_SECS + claude::MAX_RESET_EXTRA_SECS;
        for now in [t + 60, t + WINDOW_SECS + 1, latest - 1] {
            let st = State::load_from(&p, now);
            assert!(!auto_would_send(&st.codex, now) && !auto_would_send(&st.claude, now));
            assert!(st.claude.last_error.is_some());
        }
        let st = State::load_from(&p, latest);
        assert!(auto_would_send(&st.codex, latest) && auto_would_send(&st.claude, latest));
    }

    #[test]
    fn manual_start_during_pause_survives_restart() {
        let p = test_file("manual");
        let t = 1_790_000_000;
        corrupt(&p, t);
        let mut st = State::load_from(&p, t + 4 * 3600);
        // Manual Claude start at T+4h reports a reset at T+9h; the tray then saves.
        let reset = t + 9 * 3600;
        let result = Ok((Outcome::Anchored, Some(reset)));
        crate::tray::apply(&mut st.claude, result, t + 4 * 3600);
        st.save_to(&p).unwrap();

        // Restart after the unreadable file's horizon: Claude is known running, nothing sent.
        let after = t + WINDOW_SECS + claude::MAX_RESET_EXTRA_SECS + 1;
        let st = State::load_from(&p, after);
        assert_eq!(st.claude.resets_at, Some(reset));
        assert!(!auto_would_send(&st.claude, after));
        assert!(auto_would_send(&st.codex, after));
        // Restart within it: Codex, never observed, is still uncertain.
        let within = t + WINDOW_SECS + 1;
        let st = State::load_from(&p, within);
        assert!(!auto_would_send(&st.codex, within) && st.codex.last_error.is_some());
        assert!(!auto_would_send(&st.claude, within));
    }

    #[test]
    fn concurrent_writers_each_replace_with_their_own_file() {
        let p = test_file("concurrent");
        // The interleaving that broke a shared temp file: A has written its temp, B writes its
        // own before A renames. Each rename must publish exactly its writer's complete content.
        let a = write_temp(&p, "A").unwrap();
        let b = write_temp(&p, "B").unwrap();
        assert_ne!(a, b);
        std::fs::rename(&a, &p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "A");
        std::fs::rename(&b, &p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "B");

        // Threads racing on one file: every write lands whole, no temp file is left behind.
        std::thread::scope(|s| {
            for c in ["C", "D", "E", "F"] {
                let p = &p;
                s.spawn(move || (0..50).for_each(|_| _ = write_atomic(p, c)));
            }
        });
        assert!(["C", "D", "E", "F"].contains(&std::fs::read_to_string(&p).unwrap().as_str()));

        // A failed replacement keeps nothing but the original.
        let dir = p.with_file_name("dir");
        std::fs::create_dir(&dir).unwrap();
        assert!(write_atomic(&dir, "G").is_err());
        let files = std::fs::read_dir(p.parent().unwrap()).unwrap().count();
        assert_eq!(files, 2, "temp file left behind");
    }
}
