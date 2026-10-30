//! The server rail: Home, then servers and folders in the user's saved order.

use std::collections::HashMap;

use egui::{self, Color32, CornerRadius, Rect, Sense, Ui, Vec2};
use swiftcord_core::state::{RailItem, State};
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::theme;
use crate::ui::widgets;

const ICON: f32 = 48.0;
const WIDTH: f32 = 72.0;

pub fn show(ui: &mut Ui, state: &State, uis: &mut UiState, actions: &mut Vec<Action>) {
    let badges = state.guild_badges();
    egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        ui.set_width(WIDTH);
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            let home = uis.guild.is_none();
            let r = item(ui, home, false, state.dm_mentions(), "Direct Messages", |ui, rect, hovered| {
                let fill = if home || hovered { theme::ACCENT() } else { theme::SELECTED() };
                ui.painter().rect_filled(rect, radius(home || hovered), fill);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, theme::icon::HOME, theme::icon_font(22.0), Color32::WHITE);
            });
            if r.clicked() {
                actions.push(Action::SelectGuild(None));
            }

            ui.add_space(4.0);
            let (line, _) = ui.allocate_exact_size(Vec2::new(32.0, 2.0), Sense::hover());
            ui.painter().rect_filled(line, 1, theme::SELECTED());
            ui.add_space(4.0);

            for entry in state.rail() {
                match entry {
                    RailItem::Guild(id) => guild(ui, state, uis, &badges, id, actions),
                    RailItem::Folder { id, name, color, guilds } => folder(ui, state, uis, &badges, id, name, color, &guilds, actions),
                }
            }
            ui.add_space(12.0);
        });
    });
}

fn radius(active: bool) -> CornerRadius {
    // Squircle when selected or hovered, circle otherwise, as in Discord.
    CornerRadius::same(if active { 16 } else { 24 })
}

/// A rail slot: the icon (drawn by `paint`), the left selection pill and a mention badge.
fn item(
    ui: &mut Ui,
    selected: bool,
    unread: bool,
    mentions: u32,
    tooltip: &str,
    paint: impl FnOnce(&mut Ui, Rect, bool),
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(ICON), Sense::click());
    let hovered = r.hovered();
    paint(ui, rect, hovered);

    let pill_h = if selected { 40.0 } else if hovered { 20.0 } else if unread { 8.0 } else { 0.0 };
    if pill_h > 0.0 {
        let x = ui.max_rect().left();
        let pill = Rect::from_min_size(egui::pos2(x - 4.0, rect.center().y - pill_h / 2.0), Vec2::new(8.0, pill_h));
        ui.painter().rect_filled(pill, 4, theme::HEADER());
    }
    if mentions > 0 {
        widgets::badge(ui, rect.right_bottom() - Vec2::new(6.0, 6.0), mentions, theme::RAIL());
    }
    ui.add_space(8.0);
    r.on_hover_text(tooltip)
}

fn acronym(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(4).collect()
}

fn guild_icon(ui: &mut Ui, state: &State, id: Snowflake, rect: Rect, rounding: CornerRadius, hovered: bool) {
    let Some(g) = state.guild(id) else { return };
    match swiftcord_core::cdn::guild_icon(g, 96) {
        Some(url) => {
            egui::Image::from_uri(url).corner_radius(rounding).show_loading_spinner(false).paint_at(ui, rect);
        }
        None => {
            let fill = if hovered { theme::ACCENT() } else { theme::SELECTED() };
            ui.painter().rect_filled(rect, rounding, fill);
            let size = (rect.width() * 0.33).clamp(9.0, 16.0);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, acronym(&g.name), theme::font_semibold(size), theme::HEADER());
        }
    }
}

/// Marks an icon whose server has people in voice (a green speaker) or someone screensharing (a red screen),
/// in its bottom-left corner (the mention badge has the bottom-right).
fn voice_badge(ui: &Ui, rect: Rect, (people, live): (usize, usize)) {
    if people == 0 {
        return;
    }
    let (fill, glyph) = if live > 0 { (theme::RED(), theme::icon::SCREEN) } else { (theme::GREEN(), theme::icon::SPEAKER) };
    let center = rect.left_bottom() + Vec2::new(7.0, -7.0);
    ui.painter().circle_filled(center, 9.5, theme::RAIL());
    ui.painter().circle_filled(center, 7.5, fill);
    ui.painter().text(center, egui::Align2::CENTER_CENTER, glyph, theme::icon_font(8.5), Color32::WHITE);
}

/// "3 in voice · 1 live", under a server's name in its tooltip.
fn voice_summary((people, live): (usize, usize)) -> Option<String> {
    match (people, live) {
        (0, _) => None,
        (p, 0) => Some(format!("{p} in voice")),
        (p, l) => Some(format!("{p} in voice · {l} live")),
    }
}

fn guild(ui: &mut Ui, state: &State, uis: &UiState, badges: &HashMap<Snowflake, (bool, u32)>, id: Snowflake, actions: &mut Vec<Action>) {
    let Some(g) = state.guild(id) else { return };
    let selected = uis.guild == Some(id);
    let (unread, mentions) = badges.get(&id).copied().unwrap_or_default();
    let unread = unread && !state.is_guild_muted(id);
    let activity = state.voice_activity(id);
    let tooltip = match voice_summary(activity) {
        Some(v) => format!("{}\n{v}", g.name),
        None => g.name.clone(),
    };
    let r = item(ui, selected, unread, mentions, &tooltip, |ui, rect, hovered| {
        guild_icon(ui, state, id, rect, radius(selected || hovered), hovered);
        voice_badge(ui, rect, activity);
    });
    if r.clicked() {
        actions.push(Action::SelectGuild(Some(id)));
    }
    r.context_menu(|ui| {
        if ui.button("Edit Server Profile").clicked() {
            actions.push(Action::EditProfile { guild: Some(id), server: true });
            ui.close();
        }
        if ui.button("Copy Server ID").clicked() {
            actions.push(Action::Copy(id.to_string()));
            ui.close();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn folder(
    ui: &mut Ui,
    state: &State,
    uis: &mut UiState,
    badges: &HashMap<Snowflake, (bool, u32)>,
    id: i64,
    name: Option<String>,
    color: Option<u32>,
    guilds: &[Snowflake],
    actions: &mut Vec<Action>,
) {
    let open = uis.open_folders.contains(&id);
    let tint = color.map_or(theme::ACCENT(), |c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
    let unread = guilds.iter().any(|g| badges.get(g).is_some_and(|b| b.0) && !state.is_guild_muted(*g));
    let mentions = if open { 0 } else { guilds.iter().map(|g| badges.get(g).map_or(0, |b| b.1)).sum() };
    let selected = !open && uis.guild.is_some_and(|g| guilds.contains(&g));
    let label = name.unwrap_or_else(|| {
        guilds.iter().filter_map(|g| state.guild(*g)).map(|g| g.name.as_str()).take(3).collect::<Vec<_>>().join(", ")
    });

    if open {
        // The open folder is a tinted column behind its servers; its height is known only afterwards, so a
        // shape slot is reserved first.
        let top = ui.cursor().top();
        let bg_slot = ui.painter().add(egui::Shape::Noop);
        let r = item(ui, false, false, 0, &label, |ui, rect, _| {
            ui.painter().rect_filled(rect, radius(true), tint.gamma_multiply(0.35));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, theme::icon::FOLDER, theme::icon_font(22.0), tint);
        });
        if r.clicked() {
            uis.open_folders.remove(&id);
        }
        for &g in guilds {
            guild(ui, state, uis, badges, g, actions);
        }
        let bottom = ui.cursor().top() - 8.0;
        let bg = Rect::from_x_y_ranges(ui.max_rect().center().x - ICON / 2.0..=ui.max_rect().center().x + ICON / 2.0, top..=bottom);
        ui.painter().set(bg_slot, egui::Shape::rect_filled(bg, 16, tint.gamma_multiply(0.12)));
        ui.add_space(4.0);
    } else {
        let r = item(ui, selected, unread, mentions, &label, |ui, rect, hovered| {
            ui.painter().rect_filled(rect, radius(hovered), tint.gamma_multiply(if hovered { 0.45 } else { 0.3 }));
            // A 2×2 preview of the first servers.
            let cell = (ICON - 12.0) / 2.0;
            for (i, &g) in guilds.iter().take(4).enumerate() {
                let min = rect.min + Vec2::new(4.0 + (i % 2) as f32 * (cell + 4.0), 4.0 + (i / 2) as f32 * (cell + 4.0));
                guild_icon(ui, state, g, Rect::from_min_size(min, Vec2::splat(cell)), CornerRadius::same(7), false);
            }
            // Voice in any server inside shows on the closed folder.
            let activity = guilds.iter().map(|g| state.voice_activity(*g)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
            voice_badge(ui, rect, activity);
        });
        if r.clicked() {
            uis.open_folders.insert(id);
        }
    }
}
