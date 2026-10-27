//! The channel sidebar: a guild's channel tree or the DM list, then voice status and the user panel.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::{Channel, ChannelType, PresenceStatus};
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::widgets;

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let state = &s.state;
    let title = match uis.guild.and_then(|g| state.guild(g)) {
        Some(g) => g.name.clone(),
        None => "Direct Messages".to_owned(),
    };

    // Header.
    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.set_height(48.0);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(title).font(theme::font_semibold(15.0)).color(theme::HEADER())).truncate());
        });
    });
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, theme::RAIL()));

    // Bottom panels first so the list takes the remaining space.
    egui::Panel::bottom("user-panel").frame(egui::Frame::new().fill(Color32::from_rgb(0x23, 0x24, 0x28))).show(ui, |ui| {
        crate::voice_glue::status_panel(ui, s, uis, actions);
        user_panel(ui, state, uis, actions);
    });

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(8.0);
        match uis.guild {
            Some(g) => guild_channels(ui, state, uis, g, actions),
            None => dm_list(ui, state, uis, actions),
        }
        ui.add_space(8.0);
    });
}

fn row(ui: &mut Ui, selected: bool, height: f32, add: impl FnOnce(&mut Ui, bool)) -> egui::Response {
    let width = ui.available_width() - 8.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let rect = rect.translate(Vec2::new(4.0, 0.0));
    // contains_pointer, not hovered: buttons inside the row would otherwise un-hover it and hide themselves.
    let hovered = r.contains_pointer();
    if selected || hovered {
        ui.painter().rect_filled(rect, 4, if selected { theme::SELECTED() } else { theme::HOVER() });
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    add(&mut child, hovered);
    ui.add_space(2.0);
    r
}

fn guild_channels(ui: &mut Ui, state: &State, uis: &mut UiState, guild: Snowflake, actions: &mut Vec<Action>) {
    let selected = uis.channel();
    let my_voice = state.my_voice_state().and_then(|v| v.channel_id);
    for (category, channels) in state.channel_tree(guild) {
        let collapsed = category.is_some_and(|c| uis.collapsed.contains(&c.id));
        if let Some(cat) = category {
            ui.add_space(12.0);
            let r = row(ui, false, 22.0, |ui, hovered| {
                let color = if hovered { theme::TEXT() } else { theme::MUTED() };
                ui.label(RichText::new(if collapsed { icon::CHEVRON_RIGHT } else { icon::CHEVRON_DOWN }).font(theme::icon_font(9.0)).color(color));
                ui.add(egui::Label::new(RichText::new(cat.label().to_uppercase()).font(theme::font_semibold(12.0)).color(color)).truncate().selectable(false));
            });
            if r.clicked() {
                if collapsed {
                    uis.collapsed.remove(&cat.id);
                } else {
                    uis.collapsed.insert(cat.id);
                }
            }
        }
        for c in channels {
            let is_selected = selected == Some(c.id);
            let unread = state.is_unread(c.id) && !state.is_channel_muted(c);
            // Collapsed categories still show the open channel, unread channels and the voice channel you're in.
            if collapsed && !is_selected && !unread && my_voice != Some(c.id) {
                continue;
            }
            channel_row(ui, state, c, is_selected, unread, actions);
            if c.is_voice() {
                voice_members(ui, state, uis, c, actions);
            }
            if c.has_threads() {
                thread_rows(ui, state, c, selected, collapsed, actions);
            }
        }
    }
}

/// Joined threads under their channel, and the open thread even if not joined, joined to it by a line.
fn thread_rows(ui: &mut Ui, state: &State, parent: &Channel, selected: Option<Snowflake>, collapsed: bool, actions: &mut Vec<Action>) {
    let mut threads = state.joined_threads(parent.id);
    if let Some(open) = selected.and_then(|id| state.channel(id)).filter(|t| t.is_thread() && t.parent_id == Some(parent.id)) {
        if !threads.iter().any(|t| t.id == open.id) {
            threads.push(open);
        }
    }
    let top = ui.cursor().top() - 4.0;
    for t in threads {
        let is_selected = selected == Some(t.id);
        let unread = state.is_unread(t.id);
        if collapsed && !is_selected && !unread {
            continue;
        }
        let r = row(ui, is_selected, 30.0, |ui, hovered| {
            ui.add_space(22.0);
            let color = if is_selected || unread { theme::HEADER() } else if hovered { theme::TEXT() } else { theme::MUTED() };
            let font = if unread || is_selected { theme::font_semibold(14.5) } else { theme::font(14.5) };
            ui.add(egui::Label::new(RichText::new(t.label()).font(font).color(color)).truncate().selectable(false));
            let mentions = state.mention_count(t.id);
            if mentions > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 16.0), Sense::hover());
                    widgets::badge(ui, rect.center(), mentions, Color32::TRANSPARENT);
                });
            }
        });
        // The connector: down from the channel, then across to this thread.
        let x = r.rect.left() + 18.0;
        let y = r.rect.center().y;
        let stroke = egui::Stroke::new(1.5, Color32::from_rgb(0x4e, 0x50, 0x58));
        ui.painter().line_segment([egui::pos2(x, top), egui::pos2(x, y - 6.0)], stroke);
        ui.painter().add(egui::Shape::QuadraticBezier(egui::epaint::QuadraticBezierShape::from_points_stroke(
            [egui::pos2(x, y - 6.0), egui::pos2(x, y), egui::pos2(x + 8.0, y)],
            false,
            Color32::TRANSPARENT,
            stroke,
        )));
        if r.clicked() {
            actions.push(Action::SelectChannel(t.id));
        }
        r.context_menu(|ui| {
            if ui.button("Leave Thread").clicked() {
                actions.push(Action::SetThreadJoined(t.id, false));
                ui.close();
            }
            if ui.button("Copy channel ID").clicked() {
                actions.push(Action::Copy(t.id.to_string()));
                ui.close();
            }
        });
    }
}

fn channel_row(ui: &mut Ui, state: &State, c: &Channel, selected: bool, unread: bool, actions: &mut Vec<Action>) {
    let mentions = state.mention_count(c.id);
    let muted = state.is_channel_muted(c);
    let mut open_chat = false;
    let r = row(ui, selected, 32.0, |ui, hovered| {
        let color = if selected || unread { theme::HEADER() } else if hovered { theme::TEXT() } else if muted { theme::FAINT() } else { theme::MUTED() };
        let glyph = match c.kind {
            ChannelType::Voice | ChannelType::Stage => RichText::new(icon::SPEAKER).font(theme::icon_font(15.0)),
            ChannelType::Announcement => RichText::new("📣").size(14.0),
            ChannelType::Forum | ChannelType::Media => RichText::new("💬").size(14.0),
            _ => RichText::new("#").font(theme::font(20.0)),
        };
        ui.add_sized(Vec2::new(20.0, 24.0), egui::Label::new(glyph.color(theme::FAINT())).selectable(false));
        let font = if unread || selected { theme::font_semibold(15.0) } else { theme::font(15.0) };
        ui.add(egui::Label::new(RichText::new(c.label()).font(font).color(color)).truncate().selectable(false));
        if mentions > 0 || (c.is_voice() && (hovered || selected)) {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Voice channels have a text chat too; this opens it (clicking the row joins the call).
                if c.is_voice() && (hovered || selected) && widgets::icon_button(ui, icon::CHAT, "Open Chat").clicked() {
                    open_chat = true;
                }
                if mentions > 0 {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 16.0), Sense::hover());
                    widgets::badge(ui, rect.center(), mentions, Color32::TRANSPARENT);
                }
            });
        }
    });
    if unread && !selected {
        let x = ui.max_rect().left();
        let y = r.rect.center().y;
        ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x - 4.0, y - 4.0), Vec2::new(8.0, 8.0)), 4, theme::HEADER());
    }
    let r = r.on_hover_text(c.topic.clone().unwrap_or_default());
    if open_chat {
        actions.push(Action::SelectChannel(c.id));
    } else if r.clicked() {
        if c.is_voice() {
            actions.push(Action::JoinVoice { guild: c.guild_id, channel: c.id });
        } else {
            actions.push(Action::SelectChannel(c.id));
        }
    }
    if c.is_voice() && r.double_clicked() {
        actions.push(Action::SelectChannel(c.id));
    }
    r.context_menu(|ui| {
        if ui.button("Copy channel ID").clicked() {
            actions.push(Action::Copy(c.id.to_string()));
            ui.close();
        }
        if !c.is_voice() && ui.button("Mark as read").clicked() {
            actions.push(Action::Ack(c.id));
            ui.close();
        }
    });
}

fn voice_members(ui: &mut Ui, state: &State, uis: &UiState, c: &Channel, actions: &mut Vec<Action>) {
    for vs in state.voice_members(c) {
        let name = state.display_name(vs.user_id, c.guild_id);
        let speaking = crate::voice_glue::is_speaking(uis, vs.user_id);
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            let r = match state.user(vs.user_id) {
                Some(u) => widgets::member_avatar(ui, state, u, c.guild_id, 22.0),
                None => ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover()).1,
            };
            if speaking {
                ui.painter().circle_stroke(r.rect.center(), 12.0, egui::Stroke::new(2.0, theme::GREEN()));
            }
            // The badges are laid out from the right first, so a long name is cut short instead of pushing
            // the LIVE button out of view.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                if vs.self_deaf || vs.deaf {
                    ui.label(RichText::new(icon::DEAFENED).font(theme::icon_font(12.0)).color(theme::RED()));
                } else if vs.self_mute || vs.mute {
                    ui.label(RichText::new(icon::MIC_OFF).font(theme::icon_font(12.0)).color(theme::RED()));
                }
                if vs.self_video {
                    ui.label(RichText::new(icon::CAMERA).font(theme::icon_font(12.0)).color(theme::MUTED())).on_hover_text("Camera on");
                }
                if vs.self_stream {
                    let mine = state.me.as_ref().is_some_and(|m| m.id == vs.user_id);
                    let key = crate::stream_glue::stream_key(c.guild_id, c.id, vs.user_id);
                    let live = crate::ui::stream::live_badge(ui, uis, key, " LIVE ", 10.0, if mine { "" } else { "Click to watch" }, actions);
                    if !mine && live.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        actions.push(Action::WatchStream { guild: c.guild_id, channel: c.id, user: vs.user_id });
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(name).size(14.0).color(theme::MUTED())).truncate().selectable(false));
                });
            });
        });
    }
}

fn dm_list(ui: &mut Ui, state: &State, uis: &mut UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new("DIRECT MESSAGES").font(theme::font_semibold(12.0)).color(theme::MUTED()));
    });
    ui.add_space(4.0);
    let selected = uis.channel();
    for c in state.private_channels() {
        let is_selected = selected == Some(c.id);
        let unread = state.is_unread(c.id);
        let mentions = state.mention_count(c.id);
        let r = row(ui, is_selected, 42.0, |ui, hovered| {
            if c.kind == ChannelType::Dm {
                if let Some(u) = c.recipients.first() {
                    widgets::avatar_with_status(ui, u, 32.0, state.presence(u.id), state.on_mobile(u.id), if is_selected { theme::SELECTED() } else if hovered { theme::HOVER() } else { theme::SIDEBAR() });
                }
            } else {
                match swiftcord_core::cdn::group_dm_icon(c, 64) {
                    Some(url) => {
                        widgets::avatar(ui, &url, 32.0);
                    }
                    None => {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                        ui.painter().circle_filled(rect.center(), 16.0, theme::BLURPLE());
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::PEOPLE, theme::icon_font(15.0), Color32::WHITE);
                    }
                }
            }
            let color = if is_selected || unread { theme::HEADER() } else if hovered { theme::TEXT() } else { theme::MUTED() };
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(c.label()).size(15.0).color(color)).truncate().selectable(false));
                if c.kind == ChannelType::GroupDm {
                    ui.label(RichText::new(format!("{} Members", c.recipients.len() + 1)).small().color(theme::FAINT()));
                }
            });
            if mentions > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(22.0, 16.0), Sense::hover());
                    widgets::badge(ui, rect.center(), mentions, Color32::TRANSPARENT);
                });
            }
        });
        if r.clicked() {
            actions.push(Action::SelectChannel(c.id));
        }
        r.context_menu(|ui| {
            if ui.button("Copy channel ID").clicked() {
                actions.push(Action::Copy(c.id.to_string()));
                ui.close();
            }
        });
    }
}

const STATUSES: [(PresenceStatus, &str, Option<&str>); 4] = [
    (PresenceStatus::Online, "Online", None),
    (PresenceStatus::Idle, "Idle", None),
    (PresenceStatus::Dnd, "Do Not Disturb", Some("You will not receive desktop notifications")),
    (PresenceStatus::Offline, "Invisible", Some("You will appear offline, but have full access")),
];

/// Choosing our status, from the avatar in the user panel.
fn status_menu(ui: &mut Ui, actions: &mut Vec<Action>) {
    let bg = ui.visuals().window_fill;
    for (status, label, detail) in STATUSES {
        let height = if detail.is_some() { 40.0 } else { 30.0 };
        let (rect, r) = ui.allocate_exact_size(Vec2::new(290.0, height), Sense::click());
        let fill = if r.hovered() { theme::BLURPLE() } else { bg };
        if r.hovered() {
            ui.painter().rect_filled(rect, 3, fill);
        }
        let top = rect.top() + 15.0;
        widgets::status_dot(ui.painter(), egui::pos2(rect.left() + 14.0, top), 5.0, status, false, fill);
        let text = if r.hovered() { Color32::WHITE } else { theme::TEXT() };
        ui.painter().text(egui::pos2(rect.left() + 30.0, top), egui::Align2::LEFT_CENTER, label, theme::font_semibold(14.0), text);
        if let Some(d) = detail {
            let muted = if r.hovered() { Color32::WHITE } else { theme::MUTED() };
            ui.painter().text(egui::pos2(rect.left() + 30.0, top + 16.0), egui::Align2::LEFT_CENTER, d, theme::font(12.0), muted);
        }
        if r.clicked() {
            actions.push(Action::SetStatus(status));
            ui.close();
        }
    }
}

fn user_panel(ui: &mut Ui, state: &State, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(me) = &state.me else { return };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let avatar = widgets::avatar_with_status(ui, me, 32.0, state.own_status(), false, Color32::from_rgb(0x23, 0x24, 0x28));
        egui::Popup::menu(&avatar).show(|ui| status_menu(ui, actions));
        avatar.on_hover_text("Set Status");
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 100.0);
            ui.add(egui::Label::new(RichText::new(me.display_name()).font(theme::font_semibold(14.0)).color(theme::HEADER())).truncate());
            let subtitle = state.activity(me.id).map_or_else(|| me.tag(), str::to_owned);
            ui.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(theme::MUTED())).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(4.0);
            let settings = widgets::icon_button(ui, icon::SETTINGS, "Settings");
            egui::Popup::menu(&settings).show(|ui| {
                ui.set_min_width(160.0);
                if ui.button("Settings…").clicked() {
                    actions.push(Action::OpenSettings);
                }
                if ui.button("Edit Profile…").clicked() {
                    // With the server being viewed, so its tab is one click away.
                    actions.push(Action::EditProfile { guild: uis.guild, server: false });
                }
                if ui.button("Copy user ID").clicked() {
                    actions.push(Action::Copy(me.id.to_string()));
                }
                ui.separator();
                if ui.button(RichText::new("Log out").color(theme::RED())).clicked() {
                    actions.push(Action::LogOut);
                }
            });
            if widgets::toggle_icon(ui, if uis.self_deaf { icon::DEAFENED } else { icon::HEADPHONES }, uis.self_deaf, "Deafen").clicked() {
                actions.push(Action::ToggleDeafen);
            }
            if widgets::toggle_icon(ui, if uis.self_mute { icon::MIC_OFF } else { icon::MIC }, uis.self_mute, "Mute").clicked() {
                actions.push(Action::ToggleMute);
            }
        });
    });
    ui.add_space(6.0);
}
