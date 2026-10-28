//! A server voice channel's own screen, as Discord shows it when you click one: everyone in it as a tile (their
//! camera, their stream's preview, or their avatar) with who's talking, muted or live, the call controls
//! below, and the channel's text chat alongside when wanted.

use eframe::egui::{self, Align, Align2, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::{Channel, VoiceState};

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::call::{camera_tile, round};
use crate::ui::emoji::Catalog;
use crate::ui::widgets;

const BG: Color32 = Color32::from_rgb(0x11, 0x12, 0x14);
const TILE: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);
const GAP: f32 = 8.0;

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &Catalog, channel: &Channel, actions: &mut Vec<Action>) {
    let joined = uis.voice.as_ref().is_some_and(|v| v.channel == channel.id);

    egui::Panel::top(("room-header", channel.id)).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(16, 10))).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::SPEAKER).font(theme::icon_font(18.0)).color(theme::MUTED()));
            ui.label(RichText::new(channel.label()).font(theme::font_semibold(16.0)).color(theme::HEADER()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = if uis.voice_room_chat { "Hide Chat" } else { "Show Chat" };
                if widgets::toggle_icon(ui, icon::CHAT, uis.voice_room_chat, tip).clicked() {
                    uis.voice_room_chat = !uis.voice_room_chat;
                }
            });
        });
    });

    if uis.voice_room_chat {
        egui::Panel::right(("room-chat", channel.id)).exact_size(400.0).resizable(false).frame(egui::Frame::new().fill(theme::CHAT())).show(ui, |ui| {
            crate::ui::chat::column(ui, s, uis, catalog, channel, actions);
        });
    }

    egui::Panel::bottom(("room-controls", channel.id)).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(16, 14))).show(ui, |ui| {
        controls(ui, uis, channel, joined, actions);
    });

    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(16)).show(ui, |ui| {
        let members = s.state.voice_members(channel);
        if members.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 60.0).max(0.0));
                ui.label(RichText::new(icon::SPEAKER).font(theme::icon_font(40.0)).color(theme::FAINT()));
                ui.add_space(8.0);
                ui.label(RichText::new("No one's here yet").font(theme::font_semibold(18.0)).color(theme::HEADER()));
                ui.label(RichText::new("Join to start talking.").color(theme::MUTED()));
            });
            return;
        }
        grid(ui, s, uis, channel, &members, joined, actions);
    });
}

/// Mute, deafen, camera, screen share and leave while in the call; otherwise a button to join it.
fn controls(ui: &mut Ui, uis: &UiState, channel: &Channel, joined: bool, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let buttons = if joined { 5.0 } else { 1.0 };
        ui.add_space(((ui.available_width() - buttons * 56.0) / 2.0).max(0.0));
        if !joined {
            if round(ui, icon::PHONE, Color32::WHITE, theme::GREEN(), "Join Voice").clicked() {
                actions.push(Action::JoinVoice { guild: channel.guild_id, channel: channel.id });
            }
            return;
        }
        let (mic, fg, bg) = if uis.self_mute { (icon::MIC_OFF, Color32::WHITE, theme::RED()) } else { (icon::MIC, theme::TEXT(), theme::SELECTED()) };
        if round(ui, mic, fg, bg, if uis.self_mute { "Unmute" } else { "Mute" }).clicked() {
            actions.push(Action::ToggleMute);
        }
        let (ear, fg, bg) = if uis.self_deaf { (icon::DEAFENED, Color32::WHITE, theme::RED()) } else { (icon::HEADPHONES, theme::TEXT(), theme::SELECTED()) };
        if round(ui, ear, fg, bg, if uis.self_deaf { "Undeafen" } else { "Deafen" }).clicked() {
            actions.push(Action::ToggleDeafen);
        }
        let (fg, bg, tip) = if uis.self_video { (Color32::WHITE, theme::GREEN(), "Turn Off Camera") } else { (theme::TEXT(), theme::SELECTED(), "Turn On Camera") };
        if round(ui, icon::CAMERA, fg, bg, tip).clicked() {
            actions.push(Action::ToggleCamera);
        }
        let (fg, bg, tip, action) = match uis.share {
            Some(_) => (Color32::WHITE, theme::GREEN(), "Stop Streaming", Action::StopShare),
            None => (theme::TEXT(), theme::SELECTED(), "Share Your Screen", Action::OpenSharePicker),
        };
        if round(ui, icon::SCREEN, fg, bg, tip).clicked() {
            actions.push(action);
        }
        if round(ui, icon::HANG_UP, Color32::WHITE, theme::RED(), "Disconnect").clicked() {
            actions.push(Action::LeaveVoice);
        }
    });
}

/// The tiles, 16:9, as large as fits: the column count that gives the biggest tiles, centred.
fn grid(ui: &mut Ui, s: &Session, uis: &UiState, channel: &Channel, members: &[&VoiceState], joined: bool, actions: &mut Vec<Action>) {
    let avail = ui.available_size();
    let n = members.len();
    let (columns, width) = (1..=n)
        .map(|cols| {
            let rows = n.div_ceil(cols);
            let w = ((avail.x - GAP * (cols - 1) as f32) / cols as f32).min((avail.y - GAP * (rows - 1) as f32) / rows as f32 * 16.0 / 9.0);
            (cols, w.max(1.0))
        })
        .fold((1, 0.0), |best, c| if c.1 > best.1 { c } else { best });
    let size = Vec2::new(width, width * 9.0 / 16.0);
    let rows = n.div_ceil(columns);
    let height = rows as f32 * size.y + GAP * (rows - 1) as f32;
    ui.add_space(((avail.y - height) / 2.0).max(0.0));
    for row in members.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let used = row.len() as f32 * (size.x + GAP) - GAP;
            ui.add_space(((ui.available_width() - used) / 2.0).max(0.0));
            for vs in row {
                let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                tile(ui, s, uis, channel, vs, rect, r, joined, actions);
            }
        });
        ui.add_space(GAP);
    }
}

#[allow(clippy::too_many_arguments)]
fn tile(ui: &Ui, s: &Session, uis: &UiState, channel: &Channel, vs: &VoiceState, rect: Rect, r: egui::Response, joined: bool, actions: &mut Vec<Action>) {
    let state = &s.state;
    let user = vs.user_id;
    let me = state.me.as_ref().is_some_and(|m| m.id == user);
    let speaking = joined && uis.speaking.contains(&user);
    let live = vs.self_stream;
    let painter = ui.painter();

    // Cameras arrive only while we're in the call.
    if joined && uis.cameras.contains_key(&user) {
        camera_tile(ui, s, uis, channel.guild_id, user, rect);
    } else {
        painter.rect_filled(rect, 8, TILE);
        // A live stream shows its preview behind the avatar, as Discord's tiles do.
        let key = crate::stream_glue::stream_key(channel.guild_id, channel.id, user);
        let preview = live.then(|| uis.stream_previews.get(&key)).flatten();
        if live && !me && preview.is_none_or(|(at, url)| url.is_some() && at.elapsed() > std::time::Duration::from_secs(60)) {
            actions.push(Action::LoadStreamPreview(key));
        }
        if let Some((_, Some(Some(url)))) = preview {
            egui::Image::from_uri(url.clone()).corner_radius(8).paint_at(ui, rect);
            painter.rect_filled(rect, 8, Color32::from_black_alpha(90));
        }
        let side = (rect.height() * 0.4).clamp(36.0, 112.0);
        let at = Rect::from_center_size(rect.center(), Vec2::splat(side));
        let url = state.user(user).map(|u| state.avatar_url(u, channel.guild_id, 128)).unwrap_or_default();
        egui::Image::from_uri(url).corner_radius(side / 2.0).show_loading_spinner(false).paint_at(ui, at);
        if speaking {
            painter.circle_stroke(at.center(), side / 2.0 + 3.0, egui::Stroke::new(3.0, theme::GREEN()));
        }
        name_label(ui, rect, &state.display_name(user, channel.guild_id));
    }
    if speaking {
        painter.rect_stroke(rect, 8, egui::Stroke::new(3.0, theme::GREEN()), egui::StrokeKind::Inside);
    }

    // Muted or deafened, in the top-right corner.
    let status = if vs.self_deaf || vs.deaf { Some(icon::DEAFENED) } else if vs.self_mute || vs.mute { Some(icon::MIC_OFF) } else { None };
    if let Some(glyph) = status {
        let c = rect.right_top() + Vec2::new(-18.0, 18.0);
        painter.circle_filled(c, 12.0, Color32::from_black_alpha(170));
        painter.text(c, Align2::CENTER_CENTER, glyph, theme::icon_font(12.0), theme::RED());
    }
    if live {
        let galley = painter.layout_no_wrap(" LIVE ".into(), theme::font_bold(11.0), Color32::WHITE);
        let badge = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 10.0), galley.size() + Vec2::new(4.0, 2.0));
        painter.rect_filled(badge, 4, theme::RED());
        painter.galley(badge.min + Vec2::new(2.0, 1.0), galley, Color32::WHITE);
        if !me {
            if r.hovered() {
                let button = Rect::from_center_size(rect.center_bottom() - Vec2::new(0.0, 40.0), Vec2::new(130.0, 32.0));
                painter.rect_filled(button, 6, Color32::from_black_alpha(200));
                painter.text(button.center(), Align2::CENTER_CENTER, "Watch Stream", theme::font_semibold(14.0), Color32::WHITE);
            }
            if r.clone().on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                actions.push(Action::WatchStream { guild: channel.guild_id, channel: channel.id, user });
            }
        }
    }
    r.context_menu(|ui| crate::ui::user_menu::voice_user(ui, state, uis, channel, user, &r, actions));
}

/// A name in a dark pill in a tile's bottom-left corner.
fn name_label(ui: &Ui, rect: Rect, name: &str) {
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(name.to_owned(), theme::font_semibold(12.5), Color32::WHITE);
    let label = Rect::from_min_size(rect.left_bottom() + Vec2::new(8.0, -galley.size().y - 12.0), galley.size() + Vec2::new(10.0, 4.0));
    painter.rect_filled(label.intersect(rect), 4, Color32::from_black_alpha(150));
    painter.with_clip_rect(rect).galley(label.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
}
