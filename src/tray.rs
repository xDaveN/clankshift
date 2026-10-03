//! The resident tray process. Sleeps in the OS event loop until a menu click, a finished provider
//! operation, or the next scheduled moment — no polling, no provider processes while idle.
//!
//! User-facing wording says "start a 5h limit"; internally that operation is called "anchor".

use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::civil::Time;
use jiff::tz::TimeZone;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::config::{Config, KeepStarting};
use crate::providers::{Outcome, Provider, is_not_found, maybe_sent, now};
use crate::schedule::{
    GRACE_SECS, daily_trigger, known_active, latest_daily, next_daily, next_retry,
};
use crate::state::{ProviderState, State, data_dir, log};
use crate::{icon, platform};

/// Longest single sleep. OS wait timers pause during system sleep, so re-check the wall clock
/// at least this often; each wake-up is a few microseconds of work.
const MAX_SLEEP: Duration = Duration::from_secs(10 * 60);

pub enum UserEvent {
    Menu(MenuEvent),
    Done(Provider, Result<(Outcome, i64), String>),
    SettingsClosed,
}

struct Items {
    status: [MenuItem; 2],
    start: [MenuItem; 2],
    auto: CheckMenuItem,
    settings: MenuItem,
    logs: MenuItem,
    quit: MenuItem,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    config: Config,
    config_error: Option<String>,
    state: State,
    /// Daily triggers before this moment are never run (app start, or last settings change).
    rules_since: i64,
    busy: [bool; 2],
    /// Trigger time of the running operation, if it is an automatic start (retried on failure).
    auto_since: [Option<i64>; 2],
    /// Failed automatic start: (trigger time, next attempt).
    retry: [Option<(i64, i64)>; 2],
    settings_open: bool,
    tray: Option<(TrayIcon, Items)>,
}

pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));
    let mut app = App {
        proxy,
        config: Config::default(),
        config_error: None,
        state: State::load(now()),
        rules_since: now(),
        busy: [false; 2],
        auto_since: [None; 2],
        retry: [None; 2],
        settings_open: false,
        tray: None,
    };
    app.reload_config();
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

fn idx(p: Provider) -> usize {
    p as usize
}

impl App {
    fn reload_config(&mut self) {
        let old = self.config.clone();
        match Config::load() {
            Ok(c) => (self.config, self.config_error) = (c, None),
            Err(e) => {
                // Unreadable config: stay conservative, nothing automatic.
                log(&e);
                self.config = Config {
                    auto_anchor: false,
                    ..Config::default()
                };
                self.config_error = Some(e);
            }
        }
        if schedule_changed(&old, &self.config) {
            self.cancel_retries();
        }
        self.rules_since = now();
    }

    /// Could a "keep starting" sequence run for `p` under the current settings?
    fn keeps_starting(&self, p: Provider) -> bool {
        self.config.auto_anchor
            && p.config(&self.config).enabled
            && self.config.keep_starting != KeepStarting::Off
    }

    /// Has a sequence that counted `n` 5h limits reached its count?
    fn sequence_full(&self, n: u32) -> bool {
        matches!(self.config.keep_starting, KeepStarting::For(max) if n > max)
    }

    fn end_sequence(&mut self, p: Provider, why: &str) {
        if p.state_mut(&mut self.state).sequence.take().is_some() {
            log(&format!("{}: stopped starting 5h limits ({why})", p.name()));
        }
    }

    /// Retries belong to the automatic rules that triggered them: after those change, neither a
    /// pending retry nor a still-running automatic start may retry. The running start still
    /// records what the provider reports, but no longer counts for a sequence that ended here.
    fn cancel_retries(&mut self) {
        self.retry = [None; 2];
        self.auto_since = [None; 2];
        for p in Provider::ALL {
            // A first start belongs to its trigger; a sequence ends once it may no longer run.
            if p.state(&self.state).sequence == Some(0) || !self.keeps_starting(p) {
                self.end_sequence(p, "settings changed");
            }
        }
        self.state.save();
    }

    fn build_tray(&mut self) -> Result<(), String> {
        let items = Items {
            status: Provider::ALL.map(|_| MenuItem::new("", false, None)),
            start: Provider::ALL.map(|_| MenuItem::new("", true, None)),
            auto: CheckMenuItem::new("", true, self.config.auto_anchor, None),
            settings: MenuItem::new("Settings…", true, None),
            logs: MenuItem::new("Open logs", true, None),
            quit: MenuItem::new("Quit", true, None),
        };
        let sep = PredefinedMenuItem::separator;
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::new(
                concat!("ClankShift v", env!("CARGO_PKG_VERSION")),
                false,
                None,
            ),
            &items.status[0],
            &items.status[1],
            &sep(),
            &items.start[0],
            &items.start[1],
            &sep(),
            &items.auto,
            &items.settings,
            &items.logs,
            &sep(),
            &items.quit,
        ])
        .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_tooltip("ClankShift")
            .with_icon(Icon::from_rgba(icon::rgba(32), 32, 32).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| e.to_string())?;
        self.tray = Some((tray, items));
        Ok(())
    }

    /// Start a window unless a call is running or the provider already reported an active window.
    /// `auto_since` is the trigger time for automatic starts, `None` for manual ones.
    fn anchor(&mut self, p: Provider, auto_since: Option<i64>) {
        let i = idx(p);
        if self.busy[i] || known_active(p.state(&self.state).resets_at, now()) {
            return;
        }
        if auto_since.is_some() && p.state(&self.state).auto_paused(now()) {
            log(&format!(
                "{}: automatic start skipped, 5h limit state uncertain",
                p.name()
            ));
            return;
        }
        if p.sends_unchecked() {
            // Only the reply tells whether the request started a 5h limit, and nothing bounds
            // when it goes out while its process runs: until the result is recorded, neither
            // another trigger nor a restart may send automatically.
            p.state_mut(&mut self.state).request_pending = true;
            if !self.state.save() {
                // Nothing was sent: finish it as a failure before the request, so an automatic
                // start is retried from its original trigger like any other.
                self.auto_since[i] = auto_since;
                self.finish(p, Err("could not save state; nothing sent".into()));
                return;
            }
        }
        self.busy[i] = true;
        self.auto_since[i] = auto_since;
        self.retry[i] = None;
        let (cfg, proxy) = (p.config(&self.config).clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let _ = proxy.send_event(UserEvent::Done(p, p.anchor(&cfg)));
        });
    }

    /// `since` is when the trigger was due; retries stay within `GRACE_SECS` of it.
    fn anchor_enabled(&mut self, why: &str, since: i64) {
        log(&format!("automatic trigger: {why}"));
        // End sequences that are over or were missed (e.g. while closed) before the trigger.
        self.check_sequences();
        let now = now();
        for p in Provider::ALL {
            if !p.config(&self.config).enabled {
                continue;
            }
            let i = idx(p);
            let st = p.state(&self.state);
            // A sequence still present owns its starts, deadlines and retries, even a full one
            // waiting out its last limit's reset slack: a trigger neither restarts it nor adds to
            // its count. `check_sequences` alone decides when it has ended.
            if st.sequence.is_some() {
                continue;
            }
            let active = known_active(st.resets_at, now);
            if self.keeps_starting(p) && !self.busy[i] {
                log(&format!("{}: keep starting 5h limits", p.name()));
                p.state_mut(&mut self.state).sequence = Some(active.into());
            }
            self.anchor(p, Some(since));
            if p.state(&self.state).sequence == Some(0) && !self.busy[i] && self.retry[i].is_none()
            {
                self.end_sequence(p, "first start skipped");
            }
        }
        self.state.save();
    }

    /// Start the next 5h limit of each ongoing sequence once the last one has surely ended
    /// (its reported reset plus the provider's rounding), within the usual grace and retries.
    /// A later one is never made up: the sequence ends instead.
    fn check_sequences(&mut self) {
        let now = now();
        for p in Provider::ALL {
            let i = idx(p);
            let st = p.state(&self.state);
            let Some(n) = st.sequence else { continue };
            if !self.keeps_starting(p) {
                // Turned off while ClankShift was closed.
                self.end_sequence(p, "settings changed");
                self.state.save();
                continue;
            }
            let due = st.resets_at.map(|r| r + p.reset_slack());
            if self.busy[i] || self.retry[i].is_some() || due.is_some_and(|d| d > now) {
                continue;
            }
            let why = if self.sequence_full(n) {
                "done"
            } else if due.is_some_and(|d| now - d <= GRACE_SECS) {
                log(&format!("{}: starting next 5h limit", p.name()));
                self.anchor(p, due);
                if self.busy[i] || self.retry[i].is_some() {
                    continue;
                }
                "next start skipped"
            } else {
                "next start missed (computer asleep or off)"
            };
            self.end_sequence(p, why);
            self.state.save();
        }
    }

    fn check_retries(&mut self) {
        let now = now();
        for p in Provider::ALL {
            let i = idx(p);
            let Some((since, _)) = self.retry[i].filter(|&(_, at)| at <= now) else {
                continue;
            };
            self.retry[i] = None;
            if now - since > GRACE_SECS {
                log(&format!(
                    "{}: retry missed (computer asleep or off); skipped",
                    p.name()
                ));
                self.end_sequence(p, "retry missed");
                self.state.save();
            } else {
                log(&format!("{}: retrying automatic start", p.name()));
                self.anchor(p, Some(since));
            }
        }
    }

    /// Daily time and the moment after which its occurrences count, if a daily start is on.
    fn daily_rule(&self) -> Option<(Time, Timestamp)> {
        let t = self
            .config
            .daily_time()
            .filter(|_| self.config.auto_anchor)?;
        let base = self.state.last_daily_run.unwrap_or(0).max(self.rules_since);
        Some((t, Timestamp::from_second(base).ok()?))
    }

    fn next_daily(&self) -> Option<Timestamp> {
        let (t, base) = self.daily_rule()?;
        Some(next_daily(base, t, &TimeZone::system()))
    }

    fn check_daily(&mut self) {
        let Some((t, base)) = self.daily_rule() else {
            return;
        };
        let timestamp = Timestamp::now();
        let now = timestamp.as_second();
        let Some(due) = latest_daily(base, timestamp, t, &TimeZone::system()) else {
            return;
        };
        self.state.last_daily_run = Some(now);
        self.state.save();
        match daily_trigger(due.as_second(), now) {
            Some(since) => self.anchor_enabled("daily time", since),
            None => log("daily trigger missed (computer asleep or off); skipped"),
        }
    }

    fn finish(&mut self, p: Provider, result: Result<(Outcome, i64), String>) {
        let i = idx(p);
        self.busy[i] = false;
        let auto_since = self.auto_since[i].take();
        let (prev, reported) = (
            p.state(&self.state).resets_at,
            result.as_ref().ok().map(|&(_, r)| r),
        );
        match &result {
            Ok((outcome, resets_at)) => {
                let what = match outcome {
                    Outcome::Anchored => "started a new 5h limit",
                    Outcome::AlreadyActive => "5h limit was already running",
                };
                log(&format!("{}: {what}, resets {resets_at}", p.name()));
            }
            Err(e) => log(&format!("{}: error: {e}", p.name())),
        }
        let retryable = apply(p.state_mut(&mut self.state), result, now());
        if let Some(since) = auto_since.filter(|_| retryable) {
            let now = now();
            self.retry[i] = next_retry(since, now).map(|at| (since, at));
            match self.retry[i] {
                Some((_, at)) => log(&format!(
                    "{}: will retry at {}",
                    p.name(),
                    fmt_time(at, now)
                )),
                None => log(&format!(
                    "{}: automatic start gave up after 1 hour",
                    p.name()
                )),
            }
        }
        if let Some(n) = p.state(&self.state).sequence {
            match reported {
                // Each 5h limit counts once: a repeated report of the last one does not.
                Some(r) if prev.is_none_or(|prev| r > prev) && !self.sequence_full(n) => {
                    p.state_mut(&mut self.state).sequence = Some(n + 1);
                }
                Some(_) => {}
                None if self.retry[i].is_none() => self.end_sequence(p, "start failed"),
                None => {}
            }
        }
        self.state.save();
    }

    fn open_settings(&mut self) {
        if self.settings_open {
            return;
        }
        let child = std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn());
        match child {
            Ok(mut child) => {
                self.settings_open = true;
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let _ = child.wait();
                    let _ = proxy.send_event(UserEvent::SettingsClosed);
                });
            }
            Err(e) => log(&format!("could not open settings: {e}")),
        }
    }

    /// One line per provider, containing only what the provider actually reported.
    fn status(&self, p: Provider, now: i64) -> String {
        let st = p.state(&self.state);
        let text = if !p.config(&self.config).enabled {
            "Off".to_string()
        } else if self.busy[idx(p)] {
            "Checking…".to_string()
        } else if st.last_error.as_deref().is_some_and(is_not_found) {
            "Not found | set path in Settings".to_string()
        } else if st.last_error.is_some() {
            // The reason is in the log.
            match self.retry[idx(p)] {
                Some((_, at)) => format!("Error | retrying {}", fmt_time(at, now)),
                None => "Error | see logs".to_string(),
            }
        } else {
            match st.resets_at {
                Some(r) if r > now => fmt_time(r, now),
                Some(r) => format!("Ended {}", fmt_time(r, now)),
                None => "Not checked yet".to_string(),
            }
        };
        let progress = match (st.sequence, self.config.keep_starting) {
            // One segment per 5h limit of the sequence, filled once counted.
            (Some(n @ 1..), KeepStarting::For(max)) => format!(
                " | {}{}",
                "▰".repeat(n as usize),
                "▱".repeat(max.saturating_add(1).saturating_sub(n) as usize)
            ),
            (Some(1..), _) => " | ↻".to_string(),
            _ => String::new(),
        };
        // Menu text is proportional: a three-per-em space makes "Codex" as wide as "Claude" in
        // Segoe UI, so what follows the name lines up.
        let name = match p {
            Provider::Codex => "Codex\u{2004}",
            Provider::Claude => "Claude",
        };
        format!("{name} | {text}{progress}")
    }

    fn refresh_menu(&self) {
        let Some((tray, items)) = &self.tray else {
            return;
        };
        let now = now();
        let mut tooltip = String::from("ClankShift");
        for p in Provider::ALL {
            let i = idx(p);
            let status = self.status(p, now);
            items.status[i].set_text(&status);
            if p.config(&self.config).enabled {
                tooltip.push('\n');
                tooltip.push_str(&status);
            }
            let active = known_active(p.state(&self.state).resets_at, now);
            items.start[i].set_enabled(p.config(&self.config).enabled && !self.busy[i] && !active);
            items.start[i].set_text(format!("Start {} 5h limit", p.name()));
        }
        items.auto.set_checked(self.config.auto_anchor);
        // Settings holds its own copy of the config and saves all of it, so the tray must not
        // write the file while it is open.
        items
            .auto
            .set_enabled(self.config_error.is_none() && !self.settings_open);
        items
            .auto
            .set_text(match (&self.config_error, self.next_daily()) {
                (Some(_), _) => "Automatic starts | settings file error".to_string(),
                (None, Some(t)) => {
                    format!("Automatic starts | next {}", fmt_time(t.as_second(), now))
                }
                (None, None) => "Automatic starts".to_string(),
            });
        items.settings.set_enabled(!self.settings_open);
        let _ = tray.set_tooltip(Some(&tooltip));
    }

    /// Earliest moment something can change: the daily trigger, a retry, or a known window ending.
    fn next_wake(&self) -> Instant {
        let now = now();
        let mut at = self.next_daily().map(|t| t.as_second());
        for p in Provider::ALL {
            let st = p.state(&self.state);
            let reset = st.resets_at.filter(|&r| r > now);
            let next = st.sequence.and(st.resets_at).map(|r| r + p.reset_slack());
            for t in [
                reset,
                next.filter(|&t| t > now),
                self.retry[idx(p)].map(|(_, at)| at),
            ]
            .into_iter()
            .flatten()
            {
                at = Some(at.map_or(t, |a| a.min(t)));
            }
        }
        let wait = at.map_or(MAX_SLEEP, |a| {
            Duration::from_secs((a - now).max(0) as u64 + 1).min(MAX_SLEEP)
        });
        Instant::now() + wait
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        match cause {
            StartCause::Init => {
                if let Err(e) = self.build_tray() {
                    log(&format!("could not create tray icon: {e}"));
                    event_loop.exit();
                    return;
                }
                if let Some((tray, _)) = &self.tray
                    && !platform::notify(tray, "ClankShift is running in the system tray.")
                {
                    log("could not show the startup notification");
                }
                if self.config.auto_anchor && self.config.anchor_on_start {
                    self.anchor_enabled("ClankShift started", now());
                }
                self.check_sequences();
                self.refresh_menu();
            }
            StartCause::ResumeTimeReached { .. } => {
                self.check_retries();
                self.check_daily();
                self.check_sequences();
                self.refresh_menu();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Done(p, result) => self.finish(p, result),
            UserEvent::SettingsClosed => {
                self.settings_open = false;
                self.reload_config();
            }
            UserEvent::Menu(e) => {
                let Some((_, items)) = &self.tray else { return };
                let id = e.id();
                if id == items.quit.id() {
                    event_loop.exit();
                } else if id == items.settings.id() {
                    self.open_settings();
                } else if id == items.logs.id() {
                    let _ = std::fs::create_dir_all(data_dir());
                    platform::open_folder(&data_dir());
                } else if id == items.auto.id() {
                    match toggle_auto(
                        &mut self.config,
                        self.config_error.is_some() || self.settings_open,
                        Config::save,
                    ) {
                        Ok(true) => self.cancel_retries(),
                        Ok(false) => {}
                        Err(e) => {
                            log(&e);
                            if let Some((tray, _)) = &self.tray {
                                platform::notify(
                                    tray,
                                    "Could not save the automatic starts setting; it is unchanged.",
                                );
                            }
                        }
                    }
                    self.rules_since = now();
                } else if let Some(p) = Provider::ALL
                    .into_iter()
                    .find(|&p| id == items.start[idx(p)].id())
                {
                    log(&format!("{}: manual start", p.name()));
                    self.anchor(p, None);
                }
            }
        }
        self.refresh_menu();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_wake()));
    }

    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

/// Record a finished start in the provider's state. True if it failed before anything could
/// reach the provider, so an automatic start may be retried. `now` is after the operation's
/// processes ended, so a request it launched reached the provider, if at all, before then.
pub(crate) fn apply(
    st: &mut ProviderState,
    result: Result<(Outcome, i64), String>,
    now: i64,
) -> bool {
    st.request_pending = false;
    match result {
        Ok((_, resets_at)) => {
            st.resets_at = Some(resets_at);
            st.checked_at = Some(now);
            st.last_error = None;
            st.unknown_until = None;
            false
        }
        Err(e) => {
            let retryable = !maybe_sent(&e);
            if !retryable {
                st.sent_by(now);
            }
            st.last_error = Some(e);
            retryable
        }
    }
}

/// Flip automatic starts and save; true if it changed. The live config changes only after the
/// save succeeds, so the tray never runs a setting that a restart would not load. Blocked while
/// the config is unreadable (the in-memory defaults are only a stand-in) or while Settings is
/// open (its save would overwrite the change, or this save would overwrite Settings' before the
/// tray reloads).
fn toggle_auto(
    config: &mut Config,
    blocked: bool,
    save: impl FnOnce(&Config) -> Result<(), String>,
) -> Result<bool, String> {
    if blocked {
        return Ok(false);
    }
    let next = Config {
        auto_anchor: !config.auto_anchor,
        ..config.clone()
    };
    save(&next)?;
    *config = next;
    Ok(true)
}

/// Did the settings that decide when automatic starts run change? CLI paths and models do not.
fn schedule_changed(a: &Config, b: &Config) -> bool {
    let rules = |c: &Config| {
        (
            c.auto_anchor,
            c.anchor_on_start,
            c.daily_time(),
            c.keep_starting,
            c.codex.enabled,
            c.claude.enabled,
        )
    };
    rules(a) != rules(b)
}

fn fmt_time(epoch: i64, now: i64) -> String {
    let tz = TimeZone::system();
    let (Ok(t), Ok(n)) = (Timestamp::from_second(epoch), Timestamp::from_second(now)) else {
        return "?".into();
    };
    let (t, n) = (t.to_zoned(tz.clone()), n.to_zoned(tz));
    if t.date() == n.date() {
        t.strftime("%H:%M").to_string()
    } else {
        t.strftime("%a %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_auto_toggle_changes_nothing() {
        let mut config = Config {
            auto_anchor: false,
            ..Config::default()
        };
        let mut saved = None;
        assert_eq!(
            toggle_auto(&mut config, true, |_| unreachable!()),
            Ok(false)
        );
        assert!(!config.auto_anchor);
        let save = |c: &Config| {
            saved = Some(c.auto_anchor);
            Ok(())
        };
        assert_eq!(toggle_auto(&mut config, false, save), Ok(true));
        assert!(config.auto_anchor);
        assert_eq!(saved, Some(true));
    }

    #[test]
    fn failed_auto_toggle_save_changes_nothing() {
        let mut config = Config::default();
        let failed = toggle_auto(&mut config, false, |c| {
            assert!(!c.auto_anchor);
            Err("config.toml: Access is denied.".into())
        });
        assert_eq!(failed, Err("config.toml: Access is denied.".into()));
        assert_eq!(config, Config::default());
    }

    #[test]
    fn only_scheduling_edits_cancel_retries() {
        let old = Config {
            daily_at: Some("7:00".into()),
            ..Config::default()
        };
        let changed = |edit: fn(&mut Config)| {
            let mut new = old.clone();
            edit(&mut new);
            schedule_changed(&old, &new)
        };
        assert!(!changed(|_| {}));
        assert!(!changed(|c| c.daily_at = Some("07:00".into())));
        assert!(!changed(|c| c.claude.command = "claude.exe".into()));
        assert!(!changed(|c| c.codex.model = "m".into()));
        assert!(changed(|c| c.auto_anchor = false));
        assert!(changed(|c| c.anchor_on_start = false));
        assert!(changed(|c| c.daily_at = Some("08:00".into())));
        assert!(changed(|c| c.daily_at = None));
        assert!(changed(|c| c.keep_starting = KeepStarting::UntilStopped));
        assert!(changed(|c| c.codex.enabled = false));
        assert!(changed(|c| c.claude.enabled = false));
    }
}
