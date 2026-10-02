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
    /// Until then a 5h limit may be running unseen, so automatic starts wait. Set when state.json
    /// could not be read, or a request may have reached the provider without a usable report.
    /// Cleared by the next provider report.
    pub unknown_until: Option<i64>,
    /// A Claude request is (or was, if ClankShift then stopped) launched and its result not yet
    /// recorded. Nothing bounds when it reaches Claude while its process runs (the PC may sleep at
    /// any point), so automatic starts wait for as long as this is set. Saved before launch.
    pub request_pending: bool,
}

impl ProviderState {
    /// Automatic starts wait while a 5h limit may be running unseen.
    pub fn auto_paused(&self, now: i64) -> bool {
        self.request_pending || self.unknown_until.is_some_and(|u| u > now)
    }

    /// A request may have reached the provider until `sent_by`: pause automatic starts until the
    /// latest a 5h limit it started could reset, keeping any later pause.
    pub fn sent_by(&mut self, sent_by: i64) {
        let until = sent_by + MAX_RESET_AFTER;
        self.unknown_until = Some(self.unknown_until.map_or(until, |u| u.max(until)));
    }
}

/// Latest a 5h limit can reset after a moment by which it was observed, or its request reached
/// the provider: Claude's bound (Codex's, skew plus 1 s rounding, is smaller).
pub const MAX_RESET_AFTER: i64 = crate::schedule::WINDOW_SECS + claude::MAX_RESET_EXTRA_SECS;
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
    /// Called once at startup, so no earlier ClankShift is running (single instance) and the
    /// processes of its requests have ended with it: a request it left pending reached Claude, if
    /// at all, before `now`.
    ///
    /// Missing state means "nothing known yet". An unreadable file is not the same: everything it
    /// held came from before its modified time (a reset observed, or a request ended, by then),
    /// except a pending Claude request, which may have gone out until `now`.
    pub fn load(now: i64) -> Self {
        Self::load_from(&path(), now)
    }

    fn load_from(path: &Path, now: i64) -> Self {
        let err = match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Self>(&s) {
                Ok(mut st) => {
                    for (name, p) in [("Codex", &mut st.codex), ("Claude", &mut st.claude)] {
                        if std::mem::take(&mut p.request_pending) {
                            p.sent_by(now);
                            log(&format!(
                                "{name}: request left unfinished by the last run; automatic \
                                 starts paused until {}",
                                local_time(p.unknown_until.unwrap_or(now))
                            ));
                        }
                    }
                    return st;
                }
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
        let mut st = Self::default();
        // Only Claude requests are left pending (Codex checks before sending).
        for (name, p, sent_by) in [
            ("Codex", &mut st.codex, written),
            ("Claude", &mut st.claude, written.max(now)),
        ] {
            let until = sent_by + MAX_RESET_AFTER;
            if until > now {
                let msg = format!(
                    "state.json unreadable ({err}); automatic {name} starts paused until {}, \
                     the latest a 5h limit recorded there could reset",
                    local_time(until)
                );
                log(&msg);
                p.last_error = Some(msg);
                p.unknown_until = Some(until);
            } else {
                log(&format!(
                    "state.json unreadable ({err}); any {name} 5h limit it held has reset"
                ));
            }
        }
        st
    }

    /// False (logged) if it could not be written.
    pub fn save(&self) -> bool {
        let _ = std::fs::create_dir_all(data_dir());
        self.save_to(&path())
            .inspect_err(|e| log(&format!("failed to save state: {e}")))
            .is_ok()
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, &serde_json::to_string_pretty(self).unwrap())
    }
}

fn local_time(t: i64) -> String {
    jiff::Timestamp::from_second(t).map_or(t.to_string(), |t| {
        t.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%F %T")
            .to_string()
    })
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
        // Codex: the latest reset a reply observed by the last write could carry.
        let latest = t + MAX_RESET_AFTER;
        assert_eq!(MAX_RESET_AFTER, WINDOW_SECS + claude::MAX_RESET_EXTRA_SECS);
        for now in [t + 60, t + WINDOW_SECS + 1, latest - 1] {
            let st = State::load_from(&p, now);
            assert!(!auto_would_send(&st.codex, now) && st.codex.last_error.is_some());
        }
        assert!(auto_would_send(&State::load_from(&p, latest).codex, latest));
        // Claude: the file may have held a pending request, which may have gone out until the
        // load, however long after the write that is.
        for now in [t + 60, latest, t + 7 * 86400] {
            let st = State::load_from(&p, now);
            assert_eq!(st.claude.unknown_until, Some(now + MAX_RESET_AFTER));
            assert!(st.claude.last_error.is_some());
        }
    }

    /// Request-side decisions for Claude, from the state the tray saves before launching it to
    /// a restart: (saved before launch, result recorded at `done`, or `None` = never recorded).
    fn claude_request(p: &Path, before: ProviderState, result: Option<(Result, i64)>) -> State {
        let mut st = State {
            claude: before,
            ..State::default()
        };
        st.claude.request_pending = true;
        st.save_to(p).unwrap();
        if let Some((result, done)) = result {
            crate::tray::apply(&mut st.claude, result, done);
            st.save_to(p).unwrap();
        }
        st
    }
    type Result = std::result::Result<(Outcome, Option<i64>), String>;
    const MAYBE: &str = "stream ended (may have been sent; not retried)";

    #[test]
    fn unresolved_claude_request_pauses_until_it_ends() {
        use crate::providers::{Provider, maybe_sent};
        let p = test_file("unresolved");
        let t = 1_790_000_000;
        assert!(maybe_sent(MAYBE));
        assert!(Provider::Claude.sends_unchecked() && !Provider::Codex.sends_unchecked());

        // Launched at t, then the PC sleeps or the worker is delayed for any length of time:
        // automatic starts wait while it is pending, in memory and after a restart.
        let st = claude_request(&p, ProviderState::default(), None);
        for now in [t, t + MAX_RESET_AFTER, t + 30 * 86400] {
            assert!(!auto_would_send(&st.claude, now));
        }
        assert!(auto_would_send(&st.codex, t));

        // Quit/crash (or a failed final save) leaves it pending on disk; its process ended with
        // that run, so the restart counts from its own start, wherever that is.
        let restart = t + 8 * 3600;
        let st = State::load_from(&p, restart);
        assert!(!st.claude.request_pending);
        assert_eq!(st.claude.unknown_until, Some(restart + MAX_RESET_AFTER));
        assert!(!auto_would_send(&st.claude, restart + MAX_RESET_AFTER - 1));
        assert!(auto_would_send(&st.claude, restart + MAX_RESET_AFTER));
        assert!(auto_would_send(&st.codex, restart));

        // Possibly sent, recorded at `done` after preflight and request were delayed (sleep):
        // the pause counts from when its process ended, not from the launch; it is kept for an
        // independent trigger in this run and after a restart, up to the exact boundary.
        let done = t + 9 * 3600;
        let st = claude_request(
            &p,
            ProviderState::default(),
            Some((Err(MAYBE.into()), done)),
        );
        let until = done + MAX_RESET_AFTER;
        assert_eq!(st.claude.unknown_until, Some(until));
        for now in [done, until - 1] {
            assert!(!auto_would_send(&st.claude, now));
            assert!(!auto_would_send(&State::load_from(&p, now).claude, now));
        }
        assert!(auto_would_send(&State::load_from(&p, until).claude, until));

        // A later pause (unreadable state) is kept.
        let later = ProviderState {
            unknown_until: Some(until + 600),
            ..ProviderState::default()
        };
        let st = claude_request(&p, later, Some((Err(MAYBE.into()), done)));
        assert_eq!(st.claude.unknown_until, Some(until + 600));

        // A failure known to precede the request keeps the pause from before, retryable.
        for prior in [None, Some(t + 600)] {
            let before = ProviderState {
                unknown_until: prior,
                ..ProviderState::default()
            };
            let mut st = before.clone();
            st.request_pending = true;
            assert!(crate::tray::apply(
                &mut st,
                Err("claude CLI not found".into()),
                t + 60
            ));
            assert!(!st.request_pending && st.unknown_until == prior);
        }

        // A usable report resolves it.
        let reset = t + WINDOW_SECS;
        let report = Ok((Outcome::Anchored, Some(reset)));
        let st = claude_request(&p, ProviderState::default(), Some((report, t + 60)));
        let st2 = State::load_from(&p, t + 60);
        for st in [&st, &st2] {
            assert!(!st.claude.request_pending && st.claude.unknown_until.is_none());
            assert_eq!(st.claude.resets_at, Some(reset));
        }
    }

    #[test]
    fn unreadable_pending_state_keeps_its_protection() {
        let p = test_file("unreadable-pending");
        let t = 1_790_000_000;
        // A pending request saved at t; the file then breaks, keeping that modified time.
        claude_request(&p, ProviderState::default(), None);
        corrupt(&p, t);
        for now in [t + 60, t + MAX_RESET_AFTER, t + 3 * 86400] {
            let st = State::load_from(&p, now);
            assert!(!auto_would_send(&st.claude, now + MAX_RESET_AFTER - 1));
            assert!(auto_would_send(&st.claude, now + MAX_RESET_AFTER));
        }
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
