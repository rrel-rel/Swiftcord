//! Going live: the "share your screen" picker and the panel shown while we stream.

use egui::{self, Align, Color32, Layout, RichText, Sense, Ui, Vec2};
use swiftcord_voice::capture::Source;

use crate::app::{Action, UiState};
use crate::share_glue::{Quality, ShareStatus};
use crate::theme::{self, icon};
use crate::ui::widgets;

const RESOLUTIONS: [(u32, &str); 4] = [(480, "480p"), (720, "720p"), (1080, "1080p"), (0, "Source")];
const FRAME_RATES: [u32; 3] = [15, 30, 60];

pub fn picker(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(p) = &mut uis.share_picker else { return };
    let mut open = true;
    egui::Window::new("Share Your Screen")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                let mut heading = None;
                for (i, s) in p.sources.iter().enumerate() {
                    let screen = matches!(s.source, Source::Monitor { .. });
                    let section = if screen { "SCREENS" } else { "APPLICATIONS" };
                    if heading != Some(section) {
                        heading = Some(section);
                        ui.add_space(if i == 0 { 0.0 } else { 8.0 });
                        ui.label(RichText::new(section).font(theme::font_semibold(12.0)).color(theme::MUTED()));
                    }
                    let selected = p.selected == Some(i);
                    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                    if selected || r.hovered() {
                        ui.painter().rect_filled(rect, 4, if selected { theme::SELECTED() } else { theme::HOVER() });
                    }
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
                    let glyph = if screen { icon::SCREEN } else { icon::WINDOW };
                    child.label(RichText::new(glyph).font(theme::icon_font(14.0)).color(if selected { theme::HEADER() } else { theme::MUTED() }));
                    child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}×{}", s.width, s.height)).small().color(theme::FAINT()));
                        ui.add(egui::Label::new(RichText::new(&s.label).color(if selected { theme::HEADER() } else { theme::TEXT() })).truncate().selectable(false));
                    });
                    if r.clicked() {
                        p.selected = Some(i);
                    }
                    if r.double_clicked() {
                        actions.push(Action::StartShare);
                    }
                }
                if p.sources.is_empty() {
                    ui.label(RichText::new("Nothing to share was found.").color(theme::MUTED()));
                }
            });

            ui.add_space(8.0);
            ui.separator();
            quality(ui, &mut p.quality);
            if p.quality.height > 720 || p.quality.height == 0 || p.quality.fps > 30 {
                ui.label(RichText::new("Discord limits streams above 720p / 30 fps to Nitro subscribers.").small().color(theme::FAINT()));
            }
            ui.label(RichText::new("Only video is shared; your microphone carries on as normal.").small().color(theme::FAINT()));
            let screen = p.selected.and_then(|i| p.sources.get(i)).is_some_and(|s| matches!(s.source, swiftcord_voice::capture::Source::Monitor { .. }));
            if screen {
                ui.add_space(4.0);
                ui.checkbox(&mut p.fullscreen_games, "Capture fullscreen games (experimental)");
                ui.label(
                    RichText::new("Uses another way of capturing that can see games in exclusive fullscreen. If it shows nothing, sharing switches back by itself after a few seconds.")
                        .small()
                        .color(theme::FAINT()),
                );
            }

            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = ui.add_enabled(p.selected.is_some(), egui::Button::new(RichText::new("Go Live").color(Color32::WHITE)).fill(theme::BLURPLE()));
                if go.clicked() {
                    actions.push(Action::StartShare);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CloseSharePicker);
                }
            });
        });
    if !open {
        actions.push(Action::CloseSharePicker);
    }
}

fn quality(ui: &mut Ui, q: &mut Quality) {
    egui::Grid::new("share-quality").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Resolution").color(theme::MUTED()));
        ui.horizontal(|ui| {
            for (h, label) in RESOLUTIONS {
                ui.selectable_value(&mut q.height, h, label);
            }
        });
        ui.end_row();
        ui.label(RichText::new("Frame rate").color(theme::MUTED()));
        ui.horizontal(|ui| {
            for f in FRAME_RATES {
                ui.selectable_value(&mut q.fps, f, format!("{f} fps"));
            }
        });
        ui.end_row();
    });
}

/// Under the voice status while we stream: what's live, how it's going, a preview and a stop button.
pub fn live_panel(ui: &mut Ui, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(view) = &uis.share else { return };
    egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 0, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (badge, color) = match view.status {
                ShareStatus::Live => (" LIVE ", theme::RED()),
                ShareStatus::Starting => (" STARTING ", theme::FAINT()),
            };
            ui.label(RichText::new(badge).font(theme::font_bold(10.0)).color(Color32::WHITE).background_color(color));
            let detail = match view.stats {
                Some(s) => format!("{} · {}p {:.0} fps", view.label, s.height, s.fps),
                None => view.label.clone(),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::CLOSE, "Stop Streaming").clicked() {
                    actions.push(Action::StopShare);
                }
                ui.add(egui::Label::new(RichText::new(detail).size(12.0).color(theme::MUTED())).truncate());
            });
        });
        if let Some(t) = &view.preview {
            let size = t.size_vec2();
            let w = ui.available_width();
            let h = (w * size.y / size.x.max(1.0)).min(140.0);
            ui.add(egui::Image::new((t.id(), Vec2::new(h * size.x / size.y.max(1.0), h))).corner_radius(4));
        }
    });
}
