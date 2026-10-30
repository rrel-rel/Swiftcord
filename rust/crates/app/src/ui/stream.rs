//! The Go Live viewer: the stream fitted to the main area, with controls.

use egui::{self, Align, Color32, Layout, RichText, Sense, Ui, Vec2};

use crate::app::{Action, Session, UiState};
use crate::stream_glue::WatchStatus;
use crate::theme;
use crate::ui::widgets;

/// A LIVE badge for someone's stream: hovering it shows the stream's latest preview picture (fetched when
/// hovered, at most once a minute). Returns the badge's response, for clicking to watch.
pub fn live_badge(ui: &mut Ui, uis: &UiState, key: String, text: &str, size: f32, hint: &str, actions: &mut Vec<Action>) -> egui::Response {
    let badge = ui.add(egui::Label::new(RichText::new(text).font(theme::font_bold(size)).color(Color32::WHITE).background_color(theme::RED())).sense(Sense::click()));
    if !badge.hovered() {
        return badge;
    }
    let preview = uis.stream_previews.get(&key);
    if preview.is_none_or(|(at, url)| url.is_some() && at.elapsed() > std::time::Duration::from_secs(60)) {
        actions.push(Action::LoadStreamPreview(key));
    }
    let picture = preview.and_then(|(_, url)| url.clone());
    badge.on_hover_ui(|ui| {
        let size = Vec2::new(320.0, 180.0);
        match picture {
            Some(Some(url)) => {
                ui.add(egui::Image::from_uri(url).fit_to_exact_size(size).corner_radius(4));
            }
            Some(None) => {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                ui.painter().rect_filled(rect, 4, theme::RAIL());
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No preview yet", theme::font(13.0), theme::MUTED());
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                ui.painter().rect_filled(rect, 4, theme::RAIL());
                ui.put(egui::Rect::from_center_size(rect.center(), Vec2::splat(24.0)), egui::Spinner::new());
            }
        }
        if !hint.is_empty() {
            ui.label(RichText::new(hint).color(theme::MUTED()));
        }
    })
}

pub fn show(ui: &mut Ui, s: &Session, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(view) = &uis.stream else { return };
    let name = s.state.display_name(view.owner, view.guild);

    if !view.fullscreen {
        egui::Panel::top("stream-bar").frame(egui::Frame::new().fill(theme::RAIL()).inner_margin(egui::Margin::symmetric(12, 6))).show(ui, |ui| {
            // Buttons first, from the right, so a long name is cut short rather than pushing them out of view.
            ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Stop Watching").color(Color32::WHITE)).fill(theme::RED())).clicked() {
                    actions.push(Action::StopWatching);
                }
                if ui.button("Fullscreen").clicked() {
                    actions.push(Action::ToggleFullscreen);
                }
                sound_menu(ui, uis, actions);
                if ui.button("Show Chat").clicked() {
                    actions.push(Action::ShowStream(false));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(" LIVE ").font(theme::font_bold(11.0)).color(Color32::WHITE).background_color(theme::RED()));
                    ui.add(egui::Label::new(RichText::new(format!("{name}'s screen")).font(theme::font_semibold(15.0)).color(theme::HEADER())).truncate());
                });
            }));
        });
    }

    egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
        let area = ui.max_rect();
        let r = ui.allocate_rect(area, Sense::click());
        crate::ui::widgets::plain_cursor(&r);
        match &view.texture {
            Some(tex) => {
                // Fit to the area, keeping the aspect ratio.
                let size = tex.size_vec2();
                let scale = (area.width() / size.x).min(area.height() / size.y);
                let rect = egui::Rect::from_center_size(area.center(), size * scale);
                egui::Image::new(tex).paint_at(ui, rect);
            }
            None => {
                let text = match view.status {
                    WatchStatus::Waiting => "Joining the stream…",
                    WatchStatus::Connecting => "Connecting to the stream…",
                    WatchStatus::Live => "Waiting for video…",
                };
                ui.put(egui::Rect::from_center_size(area.center() - Vec2::new(0.0, 20.0), Vec2::splat(32.0)), egui::Spinner::new().size(32.0));
                ui.painter().text(area.center() + Vec2::new(0.0, 20.0), egui::Align2::CENTER_CENTER, text, theme::font(15.0), theme::MUTED());
            }
        }
        // Double-click toggles fullscreen; Escape leaves it.
        if r.double_clicked() || (view.fullscreen && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            actions.push(Action::ToggleFullscreen);
        }
        if view.fullscreen {
            ui.painter().text(area.right_top() + Vec2::new(-12.0, 12.0), egui::Align2::RIGHT_TOP, "Esc to exit fullscreen", theme::font(12.0), Color32::from_white_alpha(90));
        }
    });
}

/// The stream's sound: a speaker button (showing the volume) opening a slider, and whether it keeps playing
/// while you're deafened. Separate from the call's own volume.
fn sound_menu(ui: &mut Ui, uis: &UiState, actions: &mut Vec<Action>) {
    let (volume, when_deafened) = uis.stream_audio;
    let glyph = if volume <= 0.0 || (uis.self_deaf && !when_deafened) { theme::icon::MUTED } else { theme::icon::VOLUME };
    let label = RichText::new(format!("{glyph}  {}%", (volume * 100.0).round() as i32));
    ui.menu_button(label.font(theme::icon_font(13.0)), |ui| {
        ui.set_min_width(240.0);
        ui.label(RichText::new("Stream Volume").font(theme::font_semibold(13.0)).color(theme::HEADER()));
        let mut v = volume * 100.0;
        let r = ui.add(egui::Slider::new(&mut v, 0.0..=200.0).suffix("%").step_by(5.0));
        if r.changed() {
            actions.push(Action::SetStreamAudio { volume: v / 100.0, when_deafened });
        }
        ui.add_space(4.0);
        let mut keep = when_deafened;
        if ui.checkbox(&mut keep, "Keep stream sound when deafened").changed() {
            actions.push(Action::SetStreamAudio { volume, when_deafened: keep });
        }
        ui.label(RichText::new("Deafening still silences the call itself.").size(12.0).color(theme::MUTED()));
    })
    .response
    .on_hover_text("Stream sound");
}

/// A bar above the chat while a stream is being watched but not shown.
pub fn return_bar(ui: &mut Ui, s: &Session, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(view) = uis.stream.as_ref().filter(|v| !v.visible) else { return };
    let name = s.state.display_name(view.owner, view.guild);
    egui::Panel::top("stream-return").frame(egui::Frame::new().fill(theme::BLURPLE().gamma_multiply(0.5)).inner_margin(egui::Margin::symmetric(12, 4))).show(ui, |ui| {
        // Buttons first, from the right, so a long name is cut short rather than pushing them out of view.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, theme::icon::CLOSE, "Stop watching").clicked() {
                    actions.push(Action::StopWatching);
                }
                if ui.button("Return to Stream").clicked() {
                    actions.push(Action::ShowStream(true));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(format!("Watching {name}'s stream")).color(Color32::WHITE)).truncate());
                });
            });
        });
    });
}
