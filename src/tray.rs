//! The resident tray process. Sleeps in the OS event loop until a menu click, a finished provider
//! operation, or the next scheduled moment — no polling, no provider processes while idle.

use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::config::Config;
use crate::providers::{Observation, Outcome, Provider, now};
use crate::schedule::{DAILY_GRACE_SECS, known_active, next_daily};
use crate::state::{State, log};

/// Longest single sleep. OS wait timers pause during system sleep, so re-check the wall clock
/// at least this often; each wake-up is a few microseconds of work.
const MAX_SLEEP: Duration = Duration::from_secs(10 * 60);

pub enum UserEvent {
    Menu(MenuEvent),
    Done(Provider, Result<(Outcome, Observation), String>),
    SettingsClosed,
}

struct Items {
    status: [MenuItem; 2],
    anchor: [MenuItem; 2],
    auto: CheckMenuItem,
    settings: MenuItem,
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
        state: State::load(),
        rules_since: now(),
        busy: [false; 2],
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
                // Unreadable config: stay conservative, no automatic anchoring.
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
            anchor: Provider::ALL
                .map(|p| MenuItem::new(format!("Anchor {} now", p.name()), true, None)),
            auto: CheckMenuItem::new("Automatic anchoring", true, self.config.auto_anchor, None),
            settings: MenuItem::new("Settings…", true, None),
            quit: MenuItem::new("Quit", true, None),
        };
        let sep = PredefinedMenuItem::separator;
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::new("ClankShift", false, None),
            &items.status[0],
            &items.status[1],
            &sep(),
            &items.anchor[0],
            &items.anchor[1],
            &sep(),
            &items.auto,
            &items.settings,
            &sep(),
            &items.quit,
        ])
        .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_tooltip("ClankShift")
            .with_icon(icon())
            .build()
            .map_err(|e| e.to_string())?;
        self.tray = Some((tray, items));
        Ok(())
    }

    /// Start an anchor unless one is running or the provider already reported an active window.
    fn anchor(&mut self, p: Provider) {
        let i = idx(p);
        if self.busy[i] || known_active(p.state(&self.state).resets_at, now()) {
            return;
        }
        self.busy[i] = true;
        let (cfg, proxy) = (p.config(&self.config).clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let _ = proxy.send_event(UserEvent::Done(p, p.anchor(&cfg)));
        });
    }

    fn anchor_enabled(&mut self, why: &str) {
        log(&format!("automatic trigger: {why}"));
        for p in Provider::ALL {
            if p.config(&self.config).enabled {
                self.anchor(p);
            }
        }
    }

    fn next_daily(&self) -> Option<Timestamp> {
        let t = self
            .config
            .daily_time()
            .filter(|_| self.config.auto_anchor)?;
        let base = self.state.last_daily_run.unwrap_or(0).max(self.rules_since);
        Some(next_daily(
            Timestamp::from_second(base).ok()?,
            t,
            &TimeZone::system(),
        ))
    }

    fn check_daily(&mut self) {
        let Some(due) = self.next_daily() else { return };
        let now = now();
        if now < due.as_second() {
            return;
        }
        self.state.last_daily_run = Some(now);
        self.state.save();
        if now - due.as_second() <= DAILY_GRACE_SECS {
            self.anchor_enabled("daily time");
        } else {
            log("daily trigger missed (computer asleep or off); skipped");
        }
    }

    fn finish(&mut self, p: Provider, result: Result<(Outcome, Observation), String>) {
        self.busy[idx(p)] = false;
        let st = p.state_mut(&mut self.state);
        match result {
            Ok((outcome, obs)) => {
                let what = match outcome {
                    Outcome::Anchored => "anchored a new window",
                    Outcome::AlreadyActive => "window already active",
                };
                log(&format!(
                    "{}: {what}, resets {}",
                    p.name(),
                    obs.resets_at.map_or("unknown".into(), |r| r.to_string())
                ));
                (st.resets_at, st.used_percent, st.checked_at, st.last_error) =
                    (obs.resets_at, obs.used_percent, Some(now()), None);
            }
            Err(e) => {
                log(&format!("{}: error: {e}", p.name()));
                st.last_error = Some(e);
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

    fn refresh_menu(&self) {
        let Some((tray, items)) = &self.tray else {
            return;
        };
        let now = now();
        for p in Provider::ALL {
            let i = idx(p);
            let st = p.state(&self.state);
            let enabled = p.config(&self.config).enabled;
            let active = known_active(st.resets_at, now);
            let status = if !enabled {
                "disabled".to_string()
            } else if self.busy[i] {
                "checking…".to_string()
            } else if let Some(e) = &st.last_error {
                format!("error: {}", truncate(e, 70))
            } else if active {
                let used = st
                    .used_percent
                    .map(|u| format!(" ({u:.0}% used)"))
                    .unwrap_or_default();
                format!("resets {}{used}", fmt_time(st.resets_at.unwrap(), now))
            } else {
                "no known active window".to_string()
            };
            items.status[i].set_text(format!("{}: {status}", p.name()));
            items.anchor[i].set_enabled(enabled && !self.busy[i] && !active);
            let suffix = if active { " (window active)" } else { "" };
            items.anchor[i].set_text(format!("Anchor {} now{suffix}", p.name()));
        }
        items.auto.set_checked(self.config.auto_anchor);
        let auto = match (&self.config_error, self.next_daily()) {
            (Some(_), _) => "Automatic anchoring (config.toml error)".to_string(),
            (None, Some(t)) => format!(
                "Automatic anchoring (next {})",
                fmt_time(t.as_second(), now)
            ),
            (None, None) => "Automatic anchoring".to_string(),
        };
        items.auto.set_text(auto);
        items.settings.set_enabled(!self.settings_open);
        let _ = tray.set_tooltip(Some(if self.busy.contains(&true) {
            "ClankShift – working…"
        } else {
            "ClankShift"
        }));
    }

    /// Earliest moment something can change: the daily trigger, or a known window ending.
    fn next_wake(&self) -> Instant {
        let now = now();
        let mut at = self.next_daily().map(|t| t.as_second());
        for p in Provider::ALL {
            if let Some(r) = p.state(&self.state).resets_at.filter(|&r| r > now) {
                at = Some(at.map_or(r, |a| a.min(r)));
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
                if self.config.auto_anchor && self.config.anchor_on_start {
                    self.anchor_enabled("ClankShift started");
                }
                self.refresh_menu();
            }
            StartCause::ResumeTimeReached { .. } => {
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
                } else if id == items.auto.id() {
                    self.config.auto_anchor = !self.config.auto_anchor;
                    // Never overwrite a config file we failed to read.
                    if self.config_error.is_none()
                        && let Err(e) = self.config.save()
                    {
                        log(&e);
                    }
                    self.rules_since = now();
                } else if let Some(p) = Provider::ALL
                    .into_iter()
                    .find(|&p| id == items.anchor[idx(p)].id())
                {
                    log(&format!("{}: manual anchor", p.name()));
                    self.anchor(p);
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

fn truncate(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    match line.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// 32×32 amber disc with a dark clock hand, drawn in code so there is no asset to ship.
fn icon() -> Icon {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let alpha = (c + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            let hand = (dx.abs() < 1.6 && dy < 0.5 && dy > -10.0)
                || (dy.abs() < 1.6 && dx > -0.5 && dx < 7.0);
            let px = if hand { [40, 30, 20] } else { [240, 160, 40] };
            rgba[(y * N + x) * 4..][..4].copy_from_slice(&[
                px[0],
                px[1],
                px[2],
                (alpha * 255.0) as u8,
            ]);
        }
    }
    Icon::from_rgba(rgba, N as u32, N as u32).expect("valid icon")
}
