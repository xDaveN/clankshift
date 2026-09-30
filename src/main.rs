// No console window for the tray or settings process.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod config;
mod platform;
mod providers;
mod schedule;
mod settings;
mod state;
mod tray;

fn main() {
    if std::env::args().any(|a| a == "--settings") {
        settings::run();
        return;
    }
    if !platform::single_instance() {
        return; // already running
    }
    if let Err(e) = tray::run() {
        state::log(&format!("fatal: {e}"));
    }
}
