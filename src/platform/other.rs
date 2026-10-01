//! Non-Windows placeholders so the portable core builds everywhere. Not a supported desktop target yet.

use std::process::{Child, Command};

pub const EXE_SUFFIXES: &[&str] = &[""];

pub fn hide_window(_cmd: &mut Command) {}

pub struct ProcessGuard;

pub fn contain(_child: &Child) -> ProcessGuard {
    ProcessGuard
}

pub fn single_instance() -> bool {
    true
}

pub fn autostart_enabled() -> bool {
    false
}

pub fn set_autostart(_on: bool) -> Result<(), String> {
    Err("Start at login is not supported on this platform yet".into())
}

pub fn notify(_tray: &tray_icon::TrayIcon, _text: &str) -> bool {
    true
}

pub fn open_folder(path: &std::path::Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener).arg(path).spawn();
}
