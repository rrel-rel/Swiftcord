//! DM calls: the call screen above a DM's messages, and the incoming-call popup.

use egui::{self, Align2, Color32, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::{Channel, User};
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::widgets;
use crate::voice_glue::Status;

/// A camera in the DM call screen.
const CAMERA_TILE: Vec2 = Vec2::new(256.0, 144.0);

/// Shown when the DM has a call going on, or we're in one there.
pub fn panel(ui: &mut Ui, s: &Session, uis: &UiState, channel: &Channel, actions: &mut Vec<Action>) {
    let state = &s.state;
    let call = state.call(channel.id);
    let mine = uis.voice.as_ref().filter(|v| v.channel == channel.id);
    if call.is_none() && mine.is_none() {
        return;
    }
    let Some(me) = state.me.as_ref() else { return };
    let in_call: Vec<Snowflake> = state.voice_members(channel).iter().map(|v| v.user_id).collect();
    let ringing: &[Snowflake] = call.map_or(&[], |c| c.ringing.as_slice());
    let ringing_me = ringing.contains(&me.id) && mine.is_none();

    // Everyone in the conversation who's in the call or being rung; in a 1:1 DM, always both of you.
    let mut people: Vec<&User> = std::iter::once(me).chain(channel.recipients.iter()).collect();
    if channel.recipients.len() > 1 {
        people.retain(|u| in_call.contains(&u.id) || ringing.contains(&u.id));
    }

    egui::Panel::top(("call", channel.id)).frame(egui::Frame::new().fill(theme::RAIL()).inner_margin(egui::Margin::symmetric(16, 18))).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                let width: f32 = people.iter().map(|u| if in_call.contains(&u.id) && uis.cameras.contains_key(&u.id) { CAMERA_TILE.x } else { 96.0 } + 24.0).sum();
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                for u in &people {
                    participant(ui, s, uis, channel, u, in_call.contains(&u.id), ringing.contains(&u.id), actions);
                }
            });
            ui.add_space(10.0);
            let status = match mine.map(|v| v.status) {
                Some(Status::Connected { .. }) => {
                    let others = in_call.iter().filter(|&&u| u != me.id).count();
                    if others == 0 { "Waiting for others to join…".to_owned() } else { "In call".to_owned() }
                }
                Some(_) => "Connecting…".to_owned(),
                None if ringing_me => format!("{} is calling you…", channel.label()),
                None => "Call in progress".to_owned(),
            };
            ui.label(RichText::new(status).color(theme::MUTED()));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let buttons = if mine.is_some() { 4.0 } else if ringing_me { 2.0 } else { 1.0 };
                ui.add_space(((ui.available_width() - buttons * 56.0) / 2.0).max(0.0));
                if mine.is_some() {
                    if round(ui, if uis.self_mute { icon::MIC_OFF } else { icon::MIC }, if uis.self_mute { Color32::WHITE } else { theme::TEXT() }, if uis.self_mute { theme::RED() } else { theme::SELECTED() }, "Mute").clicked() {
                        actions.push(Action::ToggleMute);
                    }
                    if round(ui, if uis.self_deaf { icon::DEAFENED } else { icon::HEADPHONES }, if uis.self_deaf { Color32::WHITE } else { theme::TEXT() }, if uis.self_deaf { theme::RED() } else { theme::SELECTED() }, "Deafen").clicked() {
                        actions.push(Action::ToggleDeafen);
                    }
                    let (fg, bg, tip) = if uis.self_video { (Color32::WHITE, theme::GREEN(), "Turn Off Camera") } else { (theme::TEXT(), theme::SELECTED(), "Turn On Camera") };
                    if round(ui, icon::CAMERA, fg, bg, tip).clicked() {
                        actions.push(Action::ToggleCamera);
                    }
                    if round(ui, icon::HANG_UP, Color32::WHITE, theme::RED(), "Leave call").clicked() {
                        actions.push(Action::LeaveVoice);
                    }
                } else {
                    if round(ui, icon::PHONE, Color32::WHITE, theme::GREEN(), if ringing_me { "Accept" } else { "Join call" }).clicked() {
                        actions.push(Action::AcceptCall(channel.id));
                    }
                    if ringing_me && round(ui, icon::HANG_UP, Color32::WHITE, theme::RED(), "Decline").clicked() {
                        actions.push(Action::DeclineCall(channel.id));
                    }
                }
            });
        });
    });
}

/// One person's camera in `rect`: the picture filling it (cropped, not stretched), their name, and a green
/// ring while they talk. A phone held upright in a wide tile is shown whole, with bars, rather than cropped
/// to a sliver. Our own is mirrored, like a mirror.
pub fn camera_tile(ui: &Ui, s: &Session, uis: &UiState, guild: Option<Snowflake>, user: Snowflake, rect: egui::Rect) {
    let Some(tex) = uis.cameras.get(&user) else { return };
    let mine = s.state.me.as_ref().is_some_and(|m| m.id == user);
    let (pic, tile) = (tex.size_vec2(), rect.size());
    let (pa, ta) = (pic.x / pic.y.max(1.0), tile.x / tile.y.max(1.0));
    let (mut u0, mut u1, mut v0, mut v1) = (0.0, 1.0, 0.0, 1.0);
    let mut at = rect;
    if pa < 1.0 && ta > 1.0 {
        ui.painter().rect_filled(rect, 8, Color32::BLACK);
        at = egui::Rect::from_center_size(rect.center(), Vec2::new(tile.y * pa, tile.y));
    } else if pa > ta {
        // Crop the picture to the tile's shape around its centre.
        let keep = ta / pa;
        (u0, u1) = ((1.0 - keep) / 2.0, (1.0 + keep) / 2.0);
    } else {
        let keep = pa / ta;
        (v0, v1) = ((1.0 - keep) / 2.0, (1.0 + keep) / 2.0);
    }
    if mine {
        (u0, u1) = (u1, u0);
    }
    egui::Image::new(tex).uv(egui::Rect::from_min_max(egui::pos2(u0, v0), egui::pos2(u1, v1))).corner_radius(8).paint_at(ui, at);
    if uis.speaking.contains(&user) {
        ui.painter().rect_stroke(rect, 8, egui::Stroke::new(3.0, theme::GREEN()), egui::StrokeKind::Inside);
    }
    let name = s.state.display_name(user, guild);
    let galley = ui.painter().layout_no_wrap(name, theme::font_semibold(12.5), Color32::WHITE);
    let label = egui::Rect::from_min_size(rect.left_bottom() + Vec2::new(8.0, -galley.size().y - 12.0), galley.size() + Vec2::new(10.0, 4.0));
    ui.painter().rect_filled(label.intersect(rect), 4, Color32::from_black_alpha(150));
    ui.painter().with_clip_rect(rect).galley(label.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
}

#[allow(clippy::too_many_arguments)]
fn participant(ui: &mut Ui, s: &Session, uis: &UiState, channel: &Channel, u: &User, joined: bool, ringing: bool, actions: &mut Vec<Action>) {
    let vs = s.state.voice_state(None, u.id).filter(|v| v.channel_id == Some(channel.id));
    let camera = joined && uis.cameras.contains_key(&u.id);
    let width = if camera { CAMERA_TILE.x } else { 96.0 };
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.vertical_centered(|ui| {
            // Their camera takes the place of their avatar and name (the tile shows the name itself).
            let r = if camera {
                let (rect, r) = ui.allocate_exact_size(CAMERA_TILE, Sense::hover());
                camera_tile(ui, s, uis, None, u.id, rect);
                r
            } else {
                let r = widgets::user_avatar(ui, u, 80.0);
                if !joined {
                    // Not in the call (yet): dimmed.
                    ui.painter().circle_filled(r.rect.center(), 40.0, theme::RAIL().gamma_multiply(0.6));
                }
                if uis.speaking.contains(&u.id) && joined {
                    ui.painter().circle_stroke(r.rect.center(), 42.0, egui::Stroke::new(3.0, theme::GREEN()));
                }
                r
            };
            if let Some(v) = vs {
                let badge = if v.self_deaf || v.deaf { Some(icon::DEAFENED) } else if v.self_mute || v.mute { Some(icon::MIC_OFF) } else { None };
                if let Some(b) = badge {
                    let c = r.rect.right_bottom() - Vec2::splat(10.0);
                    ui.painter().circle_filled(c, 12.0, theme::RED());
                    ui.painter().text(c, Align2::CENTER_CENTER, b, theme::icon_font(11.0), Color32::WHITE);
                }
            }
            if !camera {
                ui.add(egui::Label::new(RichText::new(u.display_name()).color(theme::HEADER())).truncate());
            }
            if ringing && !joined {
                ui.label(RichText::new("Ringing…").small().color(theme::MUTED()));
            }
            if vs.is_some_and(|v| v.self_stream) && s.state.me.as_ref().is_some_and(|m| m.id != u.id) {
                let key = crate::stream_glue::stream_key(None, channel.id, u.id);
                let live = crate::ui::stream::live_badge(ui, uis, key, " LIVE · Watch ", 10.5, "Click to watch", actions);
                if live.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    actions.push(Action::WatchStream { guild: None, channel: channel.id, user: u.id });
                }
            }
        });
    });
    ui.add_space(24.0);
}

/// A round call-control button.
pub(crate) fn round(ui: &mut Ui, glyph: &str, fg: Color32, bg: Color32, tip: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    let fill = if r.hovered() { bg.gamma_multiply(1.2) } else { bg };
    ui.painter().circle_filled(rect.center(), 24.0, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, theme::icon_font(20.0), fg);
    ui.add_space(8.0);
    r.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A popup for calls ringing you that aren't on screen.
pub fn incoming(ui: &Ui, s: &Session, uis: &UiState, actions: &mut Vec<Action>) {
    let showing = uis.channel().filter(|_| uis.guild.is_none() && uis.stream.as_ref().is_none_or(|v| !v.visible));
    let in_call = uis.voice.as_ref().map(|v| v.channel);
    for (i, channel) in s.state.ringing_me().into_iter().enumerate() {
        if showing == Some(channel) || in_call == Some(channel) {
            continue;
        }
        let Some(c) = s.state.channel(channel) else { continue };
        egui::Area::new(egui::Id::unique(("incoming-call", channel)))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 60.0 + i as f32 * 110.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).fill(theme::RAIL()).corner_radius(10).inner_margin(14).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        match c.recipients.first().filter(|_| c.recipients.len() == 1) {
                            Some(u) => {
                                widgets::user_avatar(ui, u, 48.0);
                            }
                            None => {
                                let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                                ui.painter().circle_filled(rect.center(), 24.0, theme::ACCENT());
                                ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon::PEOPLE, theme::icon_font(20.0), Color32::WHITE);
                            }
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(c.label()).font(theme::font_semibold(16.0)).color(theme::HEADER()));
                            ui.label(RichText::new("Incoming call…").color(theme::MUTED()));
                        });
                        ui.add_space(16.0);
                        if round(ui, icon::PHONE, Color32::WHITE, theme::GREEN(), "Accept").clicked() {
                            actions.push(Action::AcceptCall(channel));
                        }
                        if round(ui, icon::HANG_UP, Color32::WHITE, theme::RED(), "Decline").clicked() {
                            actions.push(Action::DeclineCall(channel));
                        }
                    });
                });
            });
        // Keep it pulsing in the user's attention; also lets the ring end promptly.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}
