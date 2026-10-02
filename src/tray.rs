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

use crate::config::Config;
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
    Done(Provider, Result<(Outcome, Option<i64>), String>),
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
        self.rules_since = now();
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
            &MenuItem::new("ClankShift", false, None),
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
                "{}: automatic start skipped, saved state unreadable",
                p.name()
            ));
            return;
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
        for p in Provider::ALL {
            if p.config(&self.config).enabled {
                self.anchor(p, Some(since));
            }
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
            if !self.config.auto_anchor || !p.config(&self.config).enabled {
                continue;
            }
            if now - since > GRACE_SECS {
                log(&format!(
                    "{}: retry missed (computer asleep or off); skipped",
                    p.name()
                ));
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
        let now = now();
        let due = Timestamp::from_second(now)
            .ok()
            .and_then(|n| latest_daily(base, n, t, &TimeZone::system()));
        let Some(due) = due else { return };
        self.state.last_daily_run = Some(now);
        self.state.save();
        match daily_trigger(due.as_second(), now) {
            Some(since) => self.anchor_enabled("daily time", since),
            None => log("daily trigger missed (computer asleep or off); skipped"),
        }
    }

    fn finish(&mut self, p: Provider, result: Result<(Outcome, Option<i64>), String>) {
        let i = idx(p);
        self.busy[i] = false;
        let auto_since = self.auto_since[i].take();
        match &result {
            Ok((outcome, resets_at)) => {
                let what = match outcome {
                    Outcome::Anchored => "started a new 5h limit",
                    Outcome::AlreadyActive => "5h limit was already running",
                };
                log(&format!(
                    "{}: {what}, resets {}",
                    p.name(),
                    resets_at.map_or("unknown".into(), |r| r.to_string())
                ));
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
            "Not found, set path in Settings".to_string()
        } else if st.last_error.is_some() {
            // The reason is in the log.
            match self.retry[idx(p)] {
                Some((_, at)) => format!("Error, retrying {}", fmt_time(at, now)),
                None => "Error, see logs".to_string(),
            }
        } else {
            match st.resets_at {
                Some(r) if r > now => format!("Resets {}", fmt_time(r, now)),
                Some(r) => format!("Ended {}", fmt_time(r, now)),
                None => "Not checked yet".to_string(),
            }
        };
        format!("{} · {text}", p.name())
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
        items.auto.set_enabled(self.config_error.is_none());
        items
            .auto
            .set_text(match (&self.config_error, self.next_daily()) {
                (Some(_), _) => "Automatic starts · settings file error".to_string(),
                (None, Some(t)) => {
                    format!("Automatic starts · next {}", fmt_time(t.as_second(), now))
                }
                (None, None) => "Automatic starts".to_string(),
            });
        items.settings.set_enabled(!self.settings_open);
        // Windows truncates tray tooltips at 127 characters.
        let _ = tray.set_tooltip(Some(truncate_chars(&tooltip, 127)));
    }

    /// Earliest moment something can change: the daily trigger, a retry, or a known window ending.
    fn next_wake(&self) -> Instant {
        let now = now();
        let mut at = self.next_daily().map(|t| t.as_second());
        for p in Provider::ALL {
            let reset = p.state(&self.state).resets_at.filter(|&r| r > now);
            for t in [reset, self.retry[idx(p)].map(|(_, at)| at)]
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
                self.refresh_menu();
            }
            StartCause::ResumeTimeReached { .. } => {
                self.check_retries();
                self.check_daily();
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
                    if toggle_auto(&mut self.config, &self.config_error)
                        && let Err(e) = self.config.save()
                    {
                        log(&e);
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
/// reach the provider, so an automatic start may be retried.
pub(crate) fn apply(
    st: &mut ProviderState,
    result: Result<(Outcome, Option<i64>), String>,
    now: i64,
) -> bool {
    match result {
        Ok((_, resets_at)) => {
            st.resets_at = resets_at;
            st.checked_at = Some(now);
            st.last_error = None;
            st.unknown_until = None;
            false
        }
        Err(e) => {
            let retryable = !maybe_sent(&e);
            st.last_error = Some(e);
            retryable
        }
    }
}

/// Flip automatic starts; true if the change should be saved. With an unreadable config the
/// in-memory defaults are only a stand-in, so nothing changes until Settings recovers the file.
fn toggle_auto(config: &mut Config, config_error: &Option<String>) -> bool {
    if config_error.is_some() {
        return false;
    }
    config.auto_anchor = !config.auto_anchor;
    true
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

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max.saturating_sub(1)) {
        Some((i, _)) if s.chars().count() > max => format!("{}…", &s[..i]),
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_blocks_auto_toggle() {
        let mut config = Config {
            auto_anchor: false,
            ..Config::default()
        };
        assert!(!toggle_auto(&mut config, &Some("bad config".into())));
        assert!(!config.auto_anchor);
        assert!(toggle_auto(&mut config, &None));
        assert!(config.auto_anchor);
    }
}
