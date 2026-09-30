//! Settings window. Runs as `clankshift --settings` in its own short-lived process, so the tray
//! process never loads any UI/GPU code. Saving writes config.toml; the tray reloads it on exit.

use eframe::egui;

use crate::config::Config;
use crate::{icon, platform};

struct Settings {
    cfg: Config,
    load_error: Option<String>,
    autostart: bool,
    daily_on: bool,
    hour: u8,
    minute: u8,
    error: Option<String>,
}

pub fn run() {
    let (cfg, load_error) = match Config::load() {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    };
    let daily = cfg.daily_time();
    let app = Settings {
        daily_on: daily.is_some(),
        hour: daily.map_or(7, |t| t.hour() as u8),
        minute: daily.map_or(0, |t| t.minute() as u8),
        autostart: platform::autostart_enabled(),
        cfg,
        load_error,
        error: None,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ClankShift Settings")
            .with_icon(egui::IconData {
                rgba: icon::rgba(64),
                width: 64,
                height: 64,
            })
            .with_inner_size([420.0, 450.0])
            .with_resizable(false)
            .with_maximize_button(false),
        centered: true,
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
        self.cfg.daily_at = self
            .daily_on
            .then(|| format!("{:02}:{:02}", self.hour, self.minute));
        if self.autostart != platform::autostart_enabled() || self.autostart {
            platform::set_autostart(self.autostart)?; // re-writing refreshes the path if the exe moved
        }
        self.cfg.save()
    }

    fn daily_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.daily_on, "Every day at");
            ui.add_enabled_ui(self.daily_on, |ui| {
                egui::ComboBox::from_id_salt("hour")
                    .width(44.0)
                    .selected_text(format!("{:02}", self.hour))
                    .show_ui(ui, |ui| {
                        for h in 0..24 {
                            ui.selectable_value(&mut self.hour, h, format!("{h:02}"));
                        }
                    });
                ui.label(":");
                egui::ComboBox::from_id_salt("minute")
                    .width(44.0)
                    .selected_text(format!("{:02}", self.minute))
                    .show_ui(ui, |ui| {
                        for m in (0..60).step_by(5) {
                            ui.selectable_value(&mut self.minute, m, format!("{m:02}"));
                        }
                    });
            });
        });
        if self.daily_on {
            ui.weak(format!(
                "→ those 5h limits reset at {:02}:{:02}",
                (self.hour + 5) % 24,
                self.minute
            ));
        }
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(e) = &self.load_error {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Could not read settings, showing defaults:\n{e}"),
                    );
                    ui.separator();
                }
                ui.label(
                    "Codex and Claude each have a 5h limit that starts with your first message and \
                     resets 5 hours later. ClankShift sends a tiny message at the times you choose, \
                     so your 5h limit starts earlier and resets sooner. It never does this while a \
                     5h limit is already running.",
                );

                ui.add_space(10.0);
                ui.heading("Providers");
                ui.checkbox(&mut self.cfg.codex.enabled, "OpenAI Codex");
                ui.checkbox(&mut self.cfg.claude.enabled, "Anthropic Claude");

                ui.add_space(10.0);
                ui.heading("Start 5h limits automatically");
                ui.checkbox(&mut self.cfg.auto_anchor, "On");
                ui.add_enabled_ui(self.cfg.auto_anchor, |ui| {
                    ui.indent("auto", |ui| {
                        ui.checkbox(
                            &mut self.cfg.anchor_on_start,
                            "When ClankShift starts (e.g. when you log in)",
                        );
                        self.daily_row(ui);
                    });
                });

                ui.add_space(10.0);
                ui.heading("System");
                ui.checkbox(&mut self.autostart, "Start ClankShift when I log in");

                ui.add_space(10.0);
                ui.collapsing("Advanced", |ui| {
                    ui.weak("Program paths. Leave empty to find them automatically.");
                    for (name, p) in [
                        ("codex", &mut self.cfg.codex),
                        ("claude", &mut self.cfg.claude),
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_sized([50.0, 20.0], egui::Label::new(name));
                            ui.text_edit_singleline(&mut p.command);
                        });
                    }
                });

                ui.add_space(12.0);
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
        });
    }
}
