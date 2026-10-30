//! The signed-in window: server rail, channel sidebar and chat, plus overlays.

use egui::{self, Color32, RichText, Ui, Vec2};
use swiftcord_core::gateway::ConnectionState;
use swiftcord_core::permissions::perm;
use swiftcord_core::rest::FileUpload;

use crate::app::{Action, Session, UiState, MAX_UPLOAD_BYTES};
use crate::theme;
use crate::ui::{chat, emoji, rail, sidebar};

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &mut emoji::Catalog, actions: &mut Vec<Action>) {
    if !s.ready {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::RAIL())).show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 2.0 - 40.0);
                    ui.spinner();
                    ui.label(RichText::new("Connecting to Discord…").color(theme::MUTED()));
                });
            });
        });
        return;
    }

    if !matches!(s.conn, ConnectionState::Connected) {
        egui::Panel::top("connection").frame(egui::Frame::new().fill(theme::YELLOW().gamma_multiply(0.85)).inner_margin(4)).show(ui, |ui| {
            ui.vertical_centered(|ui| ui.label(RichText::new("Reconnecting…").color(Color32::BLACK)));
        });
    }

    // A fullscreen stream takes the whole window.
    if uis.stream.as_ref().is_some_and(|v| v.visible && v.fullscreen) {
        crate::ui::stream::show(ui, s, uis, actions);
        return;
    }

    egui::Panel::left("rail").exact_size(72.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(theme::RAIL())).show(ui, |ui| {
        rail::show(ui, &s.state, uis, actions);
    });
    egui::Panel::left("sidebar").exact_size(240.0).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(theme::SIDEBAR())).show(ui, |ui| {
        sidebar::show(ui, s, uis, actions);
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT())).show(ui, |ui| {
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
    crate::interactions_glue::modal_window(ui, uis, actions);
    crate::ui::profile::show(ui, s, uis, actions);
    crate::ui::gif::show(ui, uis, actions);
    crate::ui::sticker_picker::show(ui, s, uis, actions);
    crate::ui::share::picker(ui, uis, actions);
    crate::ui::threads::prompt(ui, uis, actions);
    crate::ui::call::incoming(ui, s, uis, actions);
    dropped_files(ui, s, uis, actions);
    delete_prompt(ui, s, uis, actions);
    crate::ui::viewer::show(ui, uis);
    crate::ui::forward::show(ui, s, uis, actions);
    banner(ui, uis);
}

/// "Delete Message": the message, and Cancel/Delete. Enter deletes; Escape or a click outside cancels.
fn delete_prompt(ui: &Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some((channel, message)) = uis.confirm_delete else { return };
    let Some(msg) = s.state.message(channel, message).cloned() else {
        uis.confirm_delete = None;
        return;
    };
    let guild = s.state.channel(channel).and_then(|c| c.guild_id);
    let perms = s.state.permissions(channel);
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::unique("delete-message")).show(ui.ctx(), |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new("Delete Message").font(theme::font_bold(20.0)).color(theme::HEADER()));
        ui.label(RichText::new("Are you sure you want to delete this message?").color(theme::MUTED()));
        ui.add_space(8.0);
        egui::Frame::new().fill(theme::CHAT()).corner_radius(6).inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 10 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut cx = crate::ui::message::Ctx { state: &s.state, uis: &mut *uis, actions: &mut *actions, channel, guild, perms, highlight: None, read_only: true };
            crate::ui::message::show(ui, &msg, false, &mut cx);
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Tip: hold Shift when deleting to skip this.").size(12.0).color(theme::FAINT()));
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(theme::RED()).min_size(Vec2::new(90.0, 32.0))).clicked() {
                choice = Some(true);
            }
            if ui.add(egui::Button::new("Cancel").frame_when_inactive(false).min_size(Vec2::new(80.0, 32.0))).clicked() {
                choice = Some(false);
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            choice = Some(true);
        }
    });
    if modal.should_close() && choice.is_none() {
        choice = Some(false);
    }
    if let Some(delete) = choice {
        uis.confirm_delete = None;
        uis.focus_composer = true;
        if delete {
            actions.push(Action::Delete { channel, message });
        }
    }
}

fn dropped_files(ui: &Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    // Files only go to an open channel; with none open there's no drop target.
    let Some(channel) = uis.channel() else { return };
    let allowed = s.state.permissions(channel) & perm::ATTACH_FILES != 0;
    let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
    if !dropped.is_empty() {
        if !allowed {
            actions.push(Action::Notice("You can't upload files in this channel".into()));
            return;
        }
        let mut files = Vec::new();
        for path in dropped.iter().map(|f| f.path().to_path_buf()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::metadata(&path) {
                Ok(m) if m.is_dir() => actions.push(Action::Notice(format!("{name} is a folder: drop the files inside it instead"))),
                Ok(m) if m.len() as usize > MAX_UPLOAD_BYTES => actions.push(Action::Notice(format!("{name} is larger than 10 MB"))),
                Ok(_) => match std::fs::read(&path) {
                    Ok(bytes) => files.push(FileUpload { name, bytes, content_type: None }),
                    Err(e) => actions.push(Action::Notice(format!("Couldn't read {name}: {e}"))),
                },
                Err(e) => actions.push(Action::Notice(format!("Couldn't read {name}: {e}"))),
            }
        }
        if !files.is_empty() {
            actions.push(Action::AddFiles { channel, files });
        }
    }
    // Show a drop target while files hover over the window.
    if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
        let rect = ui.ctx().content_rect();
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop")));
        painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
        let text = if allowed { "Drop files to upload" } else { "You can't upload files in this channel" };
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::font_bold(24.0), Color32::WHITE);
    }
}

fn banner(ui: &Ui, uis: &UiState) {
    let Some((text, _)) = &uis.banner else { return };
    let ctx = ui.ctx();
    egui::Area::new(egui::Id::unique("banner")).order(egui::Order::Tooltip).anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 12.0)).show(ctx, |ui| {
        egui::Frame::new().fill(theme::RED().gamma_multiply(0.9)).corner_radius(6).inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.label(RichText::new(text).color(Color32::WHITE));
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}
