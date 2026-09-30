//! Settings window. Runs as `clankshift --settings` in its own short-lived process, so the tray
//! process never loads any UI/GPU code. Saving writes config.toml; the tray reloads it on exit.

use eframe::egui;

use crate::config::{Config, parse_hhmm};
use crate::platform;

struct Settings {
    cfg: Config,
    load_error: Option<String>,
    autostart: bool,
    daily_on: bool,
    daily_text: String,
    error: Option<String>,
}

pub fn run() {
    let (cfg, load_error) = match Config::load() {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    };
    let app = Settings {
        daily_on: cfg.daily_at.is_some(),
        daily_text: cfg.daily_at.clone().unwrap_or_else(|| "07:00".into()),
        autostart: platform::autostart_enabled(),
        cfg,
        load_error,
        error: None,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ClankShift Settings")
            .with_inner_size([380.0, 360.0])
            .with_resizable(false),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "ClankShift Settings",
        options,
        Box::new(|_| Ok(Box::new(app))),
    );
}

impl Settings {
    fn save(&mut self) -> Result<(), String> {
        self.cfg.daily_at = if self.daily_on {
            let t = parse_hhmm(&self.daily_text).ok_or("Daily time must be HH:MM, e.g. 07:00")?;
            Some(t.strftime("%H:%M").to_string())
        } else {
            None
        };
        if self.autostart != platform::autostart_enabled() || self.autostart {
            platform::set_autostart(self.autostart)?; // re-writing refreshes the path if the exe moved
        }
        self.cfg.save()
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(e) = &self.load_error {
                ui.colored_label(ui.visuals().error_fg_color, format!("Could not read settings, showing defaults:\n{e}"));
                ui.separator();
            }

            ui.heading("Providers");
            ui.checkbox(&mut self.cfg.codex.enabled, "OpenAI Codex");
            ui.checkbox(&mut self.cfg.claude.enabled, "Anthropic Claude");

            ui.add_space(8.0);
            ui.heading("Automatic anchoring");
            ui.checkbox(&mut self.cfg.auto_anchor, "Anchor automatically");
            ui.add_enabled_ui(self.cfg.auto_anchor, |ui| {
                ui.checkbox(&mut self.cfg.anchor_on_start, "When ClankShift starts");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.daily_on, "Every day at");
                    ui.add_enabled(self.daily_on, egui::TextEdit::singleline(&mut self.daily_text).desired_width(50.0));
                });
            });
            ui.label("A window is only started when none is running. Resets come 5 hours after the anchor.");

            ui.add_space(8.0);
            ui.heading("System");
            ui.checkbox(&mut self.autostart, "Start ClankShift when I log in");

            ui.add_space(8.0);
            ui.collapsing("Advanced", |ui| {
                ui.label("CLI paths (leave empty to find them on PATH)");
                for (name, p) in [("codex", &mut self.cfg.codex), ("claude", &mut self.cfg.claude)] {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        ui.text_edit_singleline(&mut p.command);
                    });
                }
            });

            ui.add_space(8.0);
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    match self.save() {
                        Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error = Some(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}
