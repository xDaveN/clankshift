//! Settings window. Runs as `clankshift --settings` in its own short-lived process, so the tray
//! process never loads any UI/GPU code. Saving writes config.toml; the tray reloads it on exit.
//!
//! Look: Windows 11 "Fluent" settings. Follows the system light/dark theme, white cards on a
//! grey background, toggle switches and the Windows accent blue.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Align, Color32, FontFamily, FontId, Layout, RichText, Stroke, vec2};

use crate::config::{Config, KeepStarting};
use crate::{icon, platform, state};

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
            .with_title("ClankShift")
            .with_icon(egui::IconData {
                rgba: icon::rgba(64),
                width: 64,
                height: 64,
            })
            .with_inner_size([480.0, 680.0])
            .with_resizable(false)
            .with_maximize_button(false),
        centered: true,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "ClankShift Settings",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            for dark in [false, true] {
                let theme = if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                };
                cc.egui_ctx
                    .style_mut_of(theme, |s| apply_style(s, &Palette::new(dark)));
            }
            Ok(Box::new(app))
        }),
    ) {
        state::log(&format!("settings window failed: {e}"));
    }
}

struct Palette {
    bg: Color32,
    card: Color32,
    control: Color32,
    stroke: Color32,
    text: Color32,
    weak: Color32,
    accent: Color32,
    on_accent: Color32,
}

impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: Color32::from_rgb(0x20, 0x20, 0x20),
                card: Color32::from_rgb(0x2b, 0x2b, 0x2b),
                control: Color32::from_rgb(0x37, 0x37, 0x37),
                stroke: Color32::from_rgb(0x3a, 0x3a, 0x3a),
                text: Color32::from_rgb(0xff, 0xff, 0xff),
                weak: Color32::from_rgb(0xc5, 0xc5, 0xc5),
                accent: accent(true),
                on_accent: on(accent(true)),
            }
        } else {
            Self {
                bg: Color32::from_rgb(0xf3, 0xf3, 0xf3),
                card: Color32::WHITE,
                control: Color32::from_rgb(0xfb, 0xfb, 0xfb),
                stroke: Color32::from_rgb(0xe5, 0xe5, 0xe5),
                text: Color32::from_rgb(0x1b, 0x1b, 0x1b),
                weak: Color32::from_rgb(0x5f, 0x5f, 0x5f),
                accent: accent(false),
                on_accent: on(accent(false)),
            }
        }
    }

    fn of(ui: &egui::Ui) -> Self {
        Self::new(ui.visuals().dark_mode)
    }
}

/// The Windows accent color (read once), else the Windows 11 default blue.
fn accent(dark: bool) -> Color32 {
    static SYSTEM: OnceLock<Option<platform::Accent>> = OnceLock::new();
    let [r, g, b] = match SYSTEM.get_or_init(platform::accent) {
        Some(a) if dark => a.dark,
        Some(a) => a.light,
        None if dark => [0x60, 0xcd, 0xff],
        None => [0x00, 0x5f, 0xb8],
    };
    Color32::from_rgb(r, g, b)
}

/// Black or white, whichever reads better on `bg`.
fn on(bg: Color32) -> Color32 {
    let luma = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if luma > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Segoe UI (the Windows UI font) when present; egui's built-in font otherwise.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut semibold = fonts.families[&FontFamily::Proportional].clone();
    if let Some(dir) = std::env::var_os("WINDIR").map(|w| std::path::Path::new(&w).join("Fonts"))
        && let (Ok(regular), Ok(bold)) = (
            std::fs::read(dir.join("segoeui.ttf")),
            std::fs::read(dir.join("seguisb.ttf")),
        )
    {
        let data = |b| Arc::new(egui::FontData::from_owned(b));
        fonts.font_data.insert("segoe".into(), data(regular));
        fonts.font_data.insert("segoe-sb".into(), data(bold));
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "segoe".into());
        semibold.insert(0, "segoe-sb".into());
    }
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semibold);
    ctx.set_fonts(fonts);
}

/// Every card row is this tall, whatever its controls. Rows are `ROW_GAP` apart (the item
/// spacing) with the divider in the middle of the gap, and cards pad by half of it, so each row's
/// band between hairlines or card edges is the same height.
const ROW_HEIGHT: f32 = 44.0;
const ROW_GAP: f32 = 6.0;

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn apply_style(s: &mut egui::Style, p: &Palette) {
    use egui::TextStyle::*;
    s.text_styles = [
        (Heading, semibold(24.0)),
        (Body, FontId::proportional(14.0)),
        (Button, FontId::proportional(14.0)),
        (Small, FontId::proportional(12.0)),
        (Monospace, FontId::monospace(13.0)),
    ]
    .into();
    s.spacing.item_spacing = vec2(8.0, ROW_GAP);
    s.spacing.button_padding = vec2(12.0, 5.0);
    s.spacing.interact_size.y = 28.0;
    let v = &mut s.visuals;
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.stroke);
    v.extreme_bg_color = p.control;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent;
    v.widgets.noninteractive.fg_stroke.color = p.text;
    v.widgets.noninteractive.bg_stroke.color = p.stroke;
    for (w, fill) in [
        (&mut v.widgets.inactive, p.control),
        (&mut v.widgets.hovered, p.card.lerp_to_gamma(p.stroke, 0.6)),
        (&mut v.widgets.active, p.stroke),
        (&mut v.widgets.open, p.control),
    ] {
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, p.stroke);
        w.fg_stroke.color = p.text;
        w.corner_radius = 4.into();
        w.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke.color = p.weak.gamma_multiply(0.5);
}

/// Windows-style toggle switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let p = Palette::of(ui);
    let (rect, mut r) = ui.allocate_exact_size(vec2(40.0, 20.0), egui::Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label)
    });
    let t = ui.ctx().animate_bool_responsive(r.id, *on);
    let radius = rect.height() / 2.0;
    let painter = ui.painter();
    if *on {
        painter.rect_filled(rect, radius, p.accent);
    } else {
        let stroke = Stroke::new(1.0, p.weak);
        painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    }
    let knob = if r.hovered() { 7.0 } else { 6.0 };
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    let color = if *on { p.on_accent } else { p.weak };
    painter.circle_filled(egui::pos2(x, rect.center().y), knob, color);
    r
}

/// A settings card: rounded panel with rows separated by hairlines.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = Palette::of(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(16, ROW_GAP as i8 / 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn divider(ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    let rect = ui.available_rect_before_wrap();
    // The cursor is one item spacing below the previous row.
    let y = ui.cursor().top() - ROW_GAP / 2.0;
    let x = (rect.left() - 16.0)..=(rect.right() + 16.0);
    ui.painter().hline(x, y, Stroke::new(1.0, p.stroke));
}

/// Title on the left, controls added right-to-left on the right.
fn row(ui: &mut egui::Ui, title: &str, controls: impl FnOnce(&mut egui::Ui)) {
    let size = vec2(ui.available_width(), ROW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let layout = Layout::right_to_left(Align::Center);
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    controls(&mut ui);
    ui.add_space(12.0);
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.label(title));
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).font(semibold(14.0)));
    ui.add_space(2.0);
}

fn keep_starting_picker(ui: &mut egui::Ui, value: &mut KeepStarting) {
    let text = |k: KeepStarting| match k {
        KeepStarting::Off => "Off",
        KeepStarting::For(_) => "Count",
        KeepStarting::UntilStopped => "Until stopped",
    };
    // Right-to-left: the mode first, then its count.
    egui::ComboBox::from_id_salt("keep_starting")
        .width(120.0)
        .selected_text(text(*value))
        .show_ui(ui, |ui| {
            let count = match *value {
                KeepStarting::For(n) => KeepStarting::For(n),
                _ => KeepStarting::For(3),
            };
            for k in [KeepStarting::Off, count, KeepStarting::UntilStopped] {
                ui.selectable_value(value, k, text(k));
            }
        });
    if let KeepStarting::For(n) = value {
        ui.add(egui::DragValue::new(n).range(1..=u32::MAX).prefix("× "));
    }
}

impl Settings {
    fn save(&mut self) -> Result<(), String> {
        self.cfg.daily_at = self
            .daily_on
            .then(|| format!("{:02}:{:02}", self.hour, self.minute));
        // Config first: if it fails, nothing has changed.
        self.cfg.save()?;
        if self.autostart || platform::autostart_enabled() {
            // Re-writing refreshes the path if the exe moved.
            platform::set_autostart(self.autostart)
                .map_err(|e| format!("Settings saved, but Start with Windows failed: {e}"))?;
        }
        Ok(())
    }

    fn time_picker(&mut self, ui: &mut egui::Ui) {
        // Right-to-left: minute first.
        egui::ComboBox::from_id_salt("minute")
            .width(52.0)
            .selected_text(format!("{:02}", self.minute))
            .show_ui(ui, |ui| {
                for m in (0..60).step_by(5) {
                    ui.selectable_value(&mut self.minute, m, format!("{m:02}"));
                }
            });
        ui.label(":");
        egui::ComboBox::from_id_salt("hour")
            .width(52.0)
            .selected_text(format!("{:02}", self.hour))
            .show_ui(ui, |ui| {
                for h in 0..24 {
                    ui.selectable_value(&mut self.hour, h, format!("{h:02}"));
                }
            });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let p = Palette::of(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(egui::Button::new("Cancel").min_size(vec2(96.0, 32.0)))
                .clicked()
            {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let save = egui::Button::new(RichText::new("Save").color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(96.0, 32.0));
            if ui.add(save).clicked() {
                match self.save() {
                    Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.error = Some(e),
                }
            }
            if let Some(e) = &self.error {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                });
            }
        });
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = Palette::of(ui);
        egui::Panel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(p.card.lerp_to_gamma(p.bg, 0.5))
                    .inner_margin(egui::Margin::symmetric(24, 16)),
            )
            .show_separator_line(true)
            .show(ui, |ui| self.footer(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Settings");

                    if let Some(e) = &self.load_error {
                        ui.add_space(12.0);
                        egui::Frame::new()
                            .fill(ui.visuals().error_fg_color.gamma_multiply(0.12))
                            .corner_radius(6)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    format!("Could not read settings, showing defaults:\n{e}"),
                                );
                            });
                    }

                    section(ui, "Providers");
                    card(ui, |ui| {
                        row(ui, "Codex", |ui| {
                            toggle(ui, &mut self.cfg.codex.enabled, "Codex");
                        });
                        divider(ui);
                        row(ui, "Claude", |ui| {
                            toggle(ui, &mut self.cfg.claude.enabled, "Claude");
                        });
                    });

                    section(ui, "Automatic starts");
                    card(ui, |ui| {
                        let title = "Master switch";
                        row(ui, title, |ui| {
                            toggle(ui, &mut self.cfg.auto_anchor, title);
                        });
                        if self.cfg.auto_anchor {
                            divider(ui);
                            let title = "On launch";
                            row(ui, title, |ui| {
                                toggle(ui, &mut self.cfg.anchor_on_start, title);
                            });
                            divider(ui);
                            row(ui, "Every day at", |ui| {
                                toggle(ui, &mut self.daily_on, "Every day at");
                                ui.add_space(8.0);
                                ui.add_enabled_ui(self.daily_on, |ui| self.time_picker(ui));
                            });
                            divider(ui);
                            row(ui, "Repeat", |ui| {
                                keep_starting_picker(ui, &mut self.cfg.keep_starting);
                            });
                        }
                    });
                    if self.cfg.auto_anchor
                        && matches!(self.cfg.keep_starting, KeepStarting::For(_))
                    {
                        ui.label(
                            RichText::new(
                                "Repeat counts the first 5h limit, including one already running.",
                            )
                            .small()
                            .color(p.weak),
                        );
                    }

                    section(ui, "General");
                    card(ui, |ui| {
                        let title = if cfg!(windows) {
                            "Start with Windows"
                        } else {
                            "Start at login"
                        };
                        row(ui, title, |ui| {
                            toggle(ui, &mut self.autostart, title);
                        });
                    });

                    ui.add_space(14.0);
                    card(ui, |ui| {
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    "Only needed if ClankShift can't find Codex or Claude.",
                                )
                                .small()
                                .color(p.weak),
                            );
                            for (name, pc) in [
                                ("Codex", &mut self.cfg.codex),
                                ("Claude", &mut self.cfg.claude),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.add_sized([56.0, 28.0], egui::Label::new(name));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut pc.command)
                                            .hint_text("Auto-detect")
                                            .desired_width(f32::INFINITY)
                                            .margin(vec2(8.0, 6.0)),
                                    );
                                });
                            }
                        });
                        ui.add_space(4.0);
                    });
                });
            });
    }
}
