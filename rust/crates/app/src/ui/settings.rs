//! The settings window: voice devices and sensitivity, notifications, account.

use eframe::egui::{self, RichText, Ui};

use crate::app::{Action, UiState};
use crate::settings::Settings;
use crate::theme;

/// An open settings window: the edited copy and the device lists, enumerated once when it opened.
pub struct Draft {
    pub settings: Settings,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Running while the user checks their mic; stops when the window closes.
    pub mic_test: Option<swiftcord_voice::MicTest>,
}

/// A level meter from -80 to 0 dBFS with a marker at the voice-activity threshold.
fn meter(ui: &mut Ui, level_db: f32, threshold_db: f32, open: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.0), egui::Sense::hover());
    let x = |db: f32| rect.left() + rect.width() * ((db + 80.0) / 80.0).clamp(0.0, 1.0);
    ui.painter().rect_filled(rect, 3, theme::RAIL);
    let fill = egui::Rect::from_min_max(rect.min, egui::pos2(x(level_db), rect.max.y));
    ui.painter().rect_filled(fill, 3, if open { theme::GREEN } else { theme::YELLOW });
    let tx = x(threshold_db);
    ui.painter().vline(tx, rect.y_range().expand(3.0), egui::Stroke::new(2.0, theme::HEADER));
}

pub fn show(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(draft) = &mut uis.settings_draft else { return };
    let mut open = true;
    let mut close = false;
    egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ui.ctx(), |ui| {
        ui.label(RichText::new("VOICE & AUDIO").font(theme::font_semibold(12.0)).color(theme::MUTED));
        ui.add_space(4.0);
        device_combo(ui, "Input device", &mut draft.settings.input_device, &draft.inputs);
        device_combo(ui, "Output device", &mut draft.settings.output_device, &draft.outputs);
        ui.add_space(6.0);
        ui.label("Input sensitivity");
        ui.add(egui::Slider::new(&mut draft.settings.vad_threshold_db, -80.0..=-10.0).suffix(" dB").text("transmit above"));
        ui.label(RichText::new("Lower values pick up quieter speech, and more background noise.").small().color(theme::FAINT));

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = if draft.mic_test.is_some() { "Stop Testing" } else { "Mic Test" };
            if ui.button(label).clicked() {
                draft.mic_test = match draft.mic_test.take() {
                    Some(_) => None,
                    None => {
                        let devices = swiftcord_voice::Devices { input: draft.settings.input_device.clone(), output: draft.settings.output_device.clone() };
                        Some(swiftcord_voice::MicTest::start(devices, draft.settings.vad_threshold_db))
                    }
                };
            }
            ui.label(RichText::new("Plays your mic back exactly as others would hear it in a call.").small().color(theme::FAINT));
        });
        if let Some(test) = &draft.mic_test {
            test.set_threshold(draft.settings.vad_threshold_db);
            meter(ui, test.level_db(), draft.settings.vad_threshold_db, test.transmitting());
            for e in test.errors() {
                actions.push(Action::Notice(e));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }

        ui.add_space(12.0);
        ui.label(RichText::new("NOTIFICATIONS").font(theme::font_semibold(12.0)).color(theme::MUTED));
        ui.checkbox(&mut draft.settings.notifications, "Desktop notifications for mentions and DMs");

        ui.add_space(12.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new("Save").color(egui::Color32::WHITE)).fill(theme::BLURPLE)).clicked() {
                actions.push(Action::SaveSettings(draft.settings.clone()));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Log out").color(theme::RED)).clicked() {
                    actions.push(Action::LogOut);
                    close = true;
                }
            });
        });
        ui.label(RichText::new("Device changes apply to the next call.").small().color(theme::FAINT));
    });
    if !open || close {
        uis.settings_draft = None;
    }
}

fn device_combo(ui: &mut Ui, label: &str, value: &mut Option<String>, devices: &[String]) {
    ui.horizontal(|ui| {
        ui.label(label);
        let text = value.clone().unwrap_or_else(|| "System default".into());
        egui::ComboBox::from_id_salt(label).width(240.0).selected_text(text).show_ui(ui, |ui| {
            ui.selectable_value(value, None, "System default");
            for d in devices {
                ui.selectable_value(value, Some(d.clone()), d);
            }
        });
    });
}
