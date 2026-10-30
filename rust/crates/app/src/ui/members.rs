//! The member list beside server channels: Discord's lazily synced list of group headers (hoisted roles, then
//! Online and Offline) and members. Only the blocks of rows near the view are synced; the rest are placeholders.

use std::sync::Arc;

use egui::{self, Align2, Color32, FontId, Galley, Rect, Sense, Ui, Vec2};
use swiftcord_core::models::{Channel, PresenceStatus};
use swiftcord_core::state::{ListRow, State};
use swiftcord_core::Snowflake;

use crate::app::{Action, Session};
use crate::theme;
use crate::ui::widgets;

pub const WIDTH: f32 = 240.0;
const ROW: f32 = 44.0;
const AVATAR: f32 = 32.0;

pub fn panel(ui: &mut Ui, s: &Session, channel: &Channel, actions: &mut Vec<Action>) {
    let state = &s.state;
    let Some(guild) = channel.guild_id else { return };
    // Threads show their parent channel's list.
    let list_channel = if channel.is_thread() { channel.parent_id.unwrap_or(channel.id) } else { channel.id };
    let list = state.member_list(guild);
    let rows = list.map_or(0, |l| l.rows.len());

    ui.spacing_mut().item_spacing.y = 0.0;
    let out = egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW, rows, |ui, range| {
        for i in range.clone() {
            let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
            let rect = rect.shrink2(Vec2::new(8.0, 1.0));
            match list.and_then(|l| l.rows[i].as_ref()) {
                Some(ListRow::Group(id)) => group_header(ui, state, guild, id, list.map_or(0, |l| l.group_count(id)), rect),
                Some(ListRow::Member(user)) => member_row(ui, state, guild, *user, rect, &r, actions),
                None => placeholder(ui, rect),
            }
        }
        range
    });
    let range = out.inner;
    actions.push(Action::MemberListRange { guild, channel: list_channel, first: range.start, last: range.end.saturating_sub(1) });
    if list.is_none() {
        ui.put(ui.max_rect(), egui::Spinner::new());
    }
}

fn group_header(ui: &Ui, state: &State, guild: Snowflake, id: &str, count: u32, rect: Rect) {
    let name = match id {
        "online" => "Online".to_owned(),
        "offline" => "Offline".to_owned(),
        role => role
            .parse()
            .ok()
            .and_then(|r: u64| state.guild(guild)?.roles.iter().find(|x| x.id == Snowflake(r)).map(|x| x.name.clone()))
            .unwrap_or_else(|| "Members".to_owned()),
    };
    let text = format!("{} — {count}", name.to_uppercase());
    let galley = line(ui, &text, theme::font_semibold(12.0), theme::MUTED(), rect.width() - 8.0);
    ui.painter().galley(rect.left_bottom() + Vec2::new(8.0, -6.0 - galley.size().y), galley, theme::MUTED());
}

fn member_row(ui: &Ui, state: &State, guild: Snowflake, user_id: Snowflake, rect: Rect, r: &egui::Response, actions: &mut Vec<Action>) {
    let Some(user) = state.user(user_id) else { return placeholder(ui, rect) };
    let hovered = r.hovered();
    let bg = if hovered { theme::HOVER() } else { theme::SIDEBAR() };
    if hovered {
        ui.painter().rect_filled(rect, 4, bg);
    }
    let status = state.presence(user_id);
    let avatar = Rect::from_center_size(egui::pos2(rect.left() + 8.0 + AVATAR / 2.0, rect.center().y), Vec2::splat(AVATAR));
    widgets::paint_avatar_with_status(ui, state.avatar_url(user, Some(guild), widgets::avatar_px(AVATAR)), avatar, status, state.on_mobile(user_id), bg);

    // Offline members are dimmed, as in Discord, until hovered.
    let dim = |c: Color32| if status == PresenceStatus::Offline && !hovered { c.gamma_multiply(0.45) } else { c };
    let x = avatar.right() + 12.0;
    let width = rect.right() - x - 4.0;
    let tag = if user.bot { 30.0 } else { 0.0 };
    let color = dim(theme::role_color(state.role_color(guild, user_id)));
    let name = line(ui, &state.display_name(user_id, Some(guild)), theme::font_semibold(15.0), color, width - tag);
    let activity = state.activity(user_id).filter(|_| status != PresenceStatus::Offline);
    let activity = activity.map(|a| line(ui, a, FontId::proportional(12.0), dim(theme::MUTED()), width));

    let name_top = match &activity {
        Some(a) => rect.center().y - (name.size().y + a.size().y) / 2.0,
        None => rect.center().y - name.size().y / 2.0,
    };
    let name_rect = Rect::from_min_size(egui::pos2(x, name_top), name.size());
    if user.bot {
        let pill = Rect::from_min_size(egui::pos2(name_rect.right() + 4.0, name_rect.center().y - 7.5), Vec2::new(26.0, 15.0));
        ui.painter().rect_filled(pill, 3, dim(theme::ACCENT()));
        ui.painter().text(pill.center(), Align2::CENTER_CENTER, "APP", theme::font_semibold(10.0), Color32::WHITE);
    }
    ui.painter().galley(name_rect.min, name, color);
    if let Some(a) = activity {
        ui.painter().galley(egui::pos2(x, name_rect.bottom()), a, theme::MUTED());
    }

    let me = state.me.as_ref().is_some_and(|m| m.id == user_id);
    if r.clicked() {
        // Beside the list, which is at the window's right edge: open to its left.
        actions.push(Action::OpenProfile { user: user_id, guild: Some(guild), at: rect.left_top() - Vec2::new(300.0 + 16.0, 0.0) });
    }
    r.clone().on_hover_text(user.tag()).context_menu(|ui| {
        if !me && ui.button("Message").clicked() {
            actions.push(Action::OpenDm(user_id));
            ui.close();
        }
        if ui.button("Copy username").clicked() {
            actions.push(Action::Copy(user.tag()));
            ui.close();
        }
        if ui.button("Copy user ID").clicked() {
            actions.push(Action::Copy(user_id.to_string()));
            ui.close();
        }
    });
}

/// A row that hasn't been synced yet.
fn placeholder(ui: &Ui, rect: Rect) {
    let shade = theme::HOVER();
    let c = egui::pos2(rect.left() + 8.0 + AVATAR / 2.0, rect.center().y);
    ui.painter().circle_filled(c, AVATAR / 2.0, shade);
    let bar = Rect::from_min_size(egui::pos2(c.x + AVATAR / 2.0 + 12.0, c.y - 5.0), Vec2::new(rect.width() * 0.45, 10.0));
    ui.painter().rect_filled(bar, 5, shade);
}

/// One line of text, cut short with an ellipsis to fit `width`.
fn line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}
