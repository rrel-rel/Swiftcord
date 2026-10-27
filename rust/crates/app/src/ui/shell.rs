//! The signed-in window: server rail, channel sidebar and chat, plus overlays.

use eframe::egui::{self, Color32, RichText, Ui, Vec2};
use swiftcord_core::gateway::ConnectionState;
use swiftcord_core::rest::FileUpload;

use crate::app::{Action, Session, UiState, MAX_UPLOAD_BYTES};
use crate::theme;
use crate::ui::{chat, emoji, rail, sidebar};

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &mut emoji::Catalog, actions: &mut Vec<Action>) {
    if !s.ready {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::RAIL)).show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 2.0 - 40.0);
                    ui.spinner();
                    ui.label(RichText::new("Connecting to Discord…").color(theme::MUTED));
                });
            });
        });
        return;
    }

    if !matches!(s.conn, ConnectionState::Connected) {
        egui::Panel::top("connection").frame(egui::Frame::new().fill(theme::YELLOW.gamma_multiply(0.85)).inner_margin(4)).show(ui, |ui| {
            ui.vertical_centered(|ui| ui.label(RichText::new("Reconnecting…").color(Color32::BLACK)));
        });
    }

    // A fullscreen stream takes the whole window.
    if uis.stream.as_ref().is_some_and(|v| v.visible && v.fullscreen) {
        crate::ui::stream::show(ui, s, uis, actions);
        return;
    }

    egui::Panel::left("rail").exact_size(72.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(theme::RAIL)).show(ui, |ui| {
        rail::show(ui, &s.state, uis, actions);
    });
    egui::Panel::left("sidebar").exact_size(240.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(theme::SIDEBAR)).show(ui, |ui| {
        sidebar::show(ui, s, uis, actions);
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT)).show(ui, |ui| {
        if uis.stream.as_ref().is_some_and(|v| v.visible) {
            crate::ui::stream::show(ui, s, uis, actions);
        } else {
            crate::ui::stream::return_bar(ui, s, uis, actions);
            chat::show(ui, s, uis, catalog, actions);
        }
    });

    emoji::show(ui, &s.state, uis, catalog, actions);
    crate::ui::settings::show(ui, uis, actions);
    crate::ui::profile_edit::show(ui, s, uis, actions);
    crate::ui::profile::show(ui, s, uis, actions);
    crate::ui::gif::show(ui, uis, actions);
    crate::ui::sticker_picker::show(ui, s, uis, actions);
    crate::ui::share::picker(ui, uis, actions);
    crate::ui::threads::prompt(ui, uis, actions);
    crate::ui::call::incoming(ui, s, uis, actions);
    dropped_files(ui, uis, actions);
    banner(ui, uis);
}

fn dropped_files(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(channel) = uis.channel() else { return };
    let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
    if !dropped.is_empty() {
        let mut files = Vec::new();
        for path in dropped.iter().map(|f| f.path().to_path_buf()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::metadata(&path) {
                Ok(m) if m.len() as usize > MAX_UPLOAD_BYTES => actions.push(Action::Notice(format!("{name} is larger than 10 MB"))),
                Ok(m) if m.is_file() => {
                    if let Ok(bytes) = std::fs::read(&path) {
                        files.push(FileUpload { name, bytes, content_type: None });
                    }
                }
                _ => {}
            }
        }
        actions.push(Action::AddFiles { channel, files });
    }
    // Show a drop target while files hover over the window.
    if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
        let rect = ui.ctx().content_rect();
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop")));
        painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Drop files to upload", theme::font_bold(24.0), Color32::WHITE);
    }
}

fn banner(ui: &Ui, uis: &UiState) {
    let Some((text, _)) = &uis.banner else { return };
    let ctx = ui.ctx();
    egui::Area::new(egui::Id::unique("banner")).order(egui::Order::Tooltip).anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 12.0)).show(ctx, |ui| {
        egui::Frame::new().fill(theme::RED.gamma_multiply(0.9)).corner_radius(6).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.label(RichText::new(text).color(Color32::WHITE));
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}
