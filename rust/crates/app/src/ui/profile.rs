//! The profile card shown when clicking someone's avatar or name: banner, Nitro profile colours, avatar
//! decoration, pronouns, bio, badges, roles and member-since dates. The profile is fetched when a card is
//! first opened and kept for the session.

use std::collections::HashMap;

use chrono::{DateTime, Local, Utc};
use eframe::egui::{self, Color32, CornerRadius, Margin, Rect, RichText, Sense, Vec2};
use swiftcord_core::cdn;
use swiftcord_core::models::Profile;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme;
use crate::ui::markdown::{self as md, Cx};
use crate::ui::widgets;

const WIDTH: f32 = 300.0;
const AVATAR: f32 = 80.0;
/// Banners are 5:2.
const BANNER: f32 = WIDTH * 2.0 / 5.0;
const NO_BANNER: f32 = 60.0;

/// Profiles by user and server (`None` outside servers): `None` while loading.
pub type Profiles = HashMap<(Snowflake, Option<Snowflake>), Option<Result<Profile, String>>>;

pub struct Card {
    pub user: Snowflake,
    pub guild: Option<Snowflake>,
    /// Where it opens (beside what was clicked); it's kept on screen.
    pub at: egui::Pos2,
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn date(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%b %-d, %Y").to_string()
}

pub fn show(ui: &egui::Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(card) = &uis.profile_card else { return };
    let state = &s.state;
    let (user_id, guild) = (card.user, card.guild);
    let Some(user) = state.user(user_id).cloned() else {
        uis.profile_card = None;
        return;
    };
    let loaded = uis.profiles.get(&(user_id, guild));
    let profile = loaded.and_then(|p| p.as_ref()).and_then(|r| r.as_ref().ok());
    let error = loaded.and_then(|p| p.as_ref()).and_then(|r| r.as_ref().err()).cloned();
    let loading = matches!(loaded, Some(None));
    let view = profile.map(Profile::view).unwrap_or_default();
    let badges = profile.map(|p| p.badges.clone()).unwrap_or_default();
    let joined = guild.and_then(|g| state.member(g, user_id).and_then(|m| m.joined_at)).or_else(|| profile.and_then(|p| p.guild_member.as_ref()?.joined_at));
    let me = state.me.as_ref().is_some_and(|m| m.id == user_id);
    let ctx = ui.ctx().clone();

    let id = egui::Id::unique("profile-card");
    let hovered = ctx.memory(|m| m.data.get_temp::<Rect>(id)).is_some_and(|r| ctx.pointer_hover_pos().is_some_and(|p| r.contains(p)));
    let (bg, theme_colors) = match view.theme_colors {
        // Darkened so the usual light text stays readable on any colour.
        Some([a, b]) => (rgb(a).lerp_to_gamma(Color32::BLACK, 0.45), Some((rgb(a).lerp_to_gamma(Color32::BLACK, 0.45), rgb(b).lerp_to_gamma(Color32::BLACK, 0.55)))),
        None => (theme::SIDEBAR, None),
    };

    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(card.at).constrain(true).show(&ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 4.0;
        let background = ui.painter().add(egui::Shape::Noop);
        let top = ui.cursor().min;

        // Banner: their picture, or a band of their accent colour.
        let height = if view.banner.is_some() { BANNER } else { NO_BANNER };
        let (banner, _) = ui.allocate_exact_size(Vec2::new(WIDTH, height), Sense::hover());
        let round_top = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
        match &view.banner {
            // Animated banners only play while the card is hovered.
            Some((hash, server)) => {
                egui::Image::from_uri(cdn::banner(user_id, guild.filter(|_| *server), hash, hovered))
                    .corner_radius(round_top)
                    .show_loading_spinner(false)
                    .paint_at(ui, banner);
            }
            None => {
                let band = view.accent_color.map(rgb).or(theme_colors.map(|(a, _)| a.lerp_to_gamma(Color32::WHITE, 0.15))).unwrap_or(theme::SELECTED);
                ui.painter().rect_filled(banner, round_top, band);
            }
        }

        // Avatar, overlapping the banner, ringed with the card colour, with its decoration over it.
        let center = egui::pos2(banner.left() + 16.0 + AVATAR / 2.0, banner.bottom());
        let avatar = Rect::from_center_size(center, Vec2::splat(AVATAR));
        ui.painter().circle_filled(center, AVATAR / 2.0 + 6.0, bg);
        widgets::paint_avatar_with_status(ui, state.avatar_url(&user, guild, 128), avatar, state.presence(user_id), state.on_mobile(user_id), bg);
        if let Some(asset) = &view.decoration {
            egui::Image::from_uri(cdn::avatar_decoration(asset)).show_loading_spinner(false).paint_at(ui, avatar.expand(AVATAR * 0.1));
        }
        // Badges sit to the right of the avatar, below the banner.
        if !badges.is_empty() {
            let row = Rect::from_min_size(egui::pos2(avatar.right() + 16.0, banner.bottom() + 8.0), Vec2::new(WIDTH - AVATAR - 48.0, 24.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                egui::Frame::new().fill(Color32::from_black_alpha(80)).corner_radius(6).inner_margin(Margin::symmetric(4, 2)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for b in badges.iter().rev() {
                        ui.add(egui::Image::from_uri(cdn::badge_icon(&b.icon)).fit_to_exact_size(Vec2::splat(20.0)).show_loading_spinner(false).sense(Sense::hover()))
                            .on_hover_text(&b.description);
                    }
                });
            });
        }
        ui.add_space(AVATAR / 2.0 + 10.0);

        egui::Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
            ui.set_width(WIDTH - 32.0);
            ui.label(RichText::new(user.display_name()).font(theme::font_bold(20.0)).color(theme::HEADER));
            let nick = guild.map(|g| state.display_name(user_id, Some(g))).filter(|n| n != user.display_name());
            let mut line = user.tag();
            if !view.pronouns.is_empty() {
                line.push_str(&format!(" • {}", view.pronouns));
            }
            ui.label(RichText::new(line).size(13.5).color(theme::TEXT));
            if let Some(n) = nick {
                ui.label(RichText::new(format!("Known here as {n}")).size(12.0).color(theme::MUTED));
            }
            ui.add_space(8.0);

            egui::Frame::new().fill(Color32::from_black_alpha(70)).corner_radius(8).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !view.bio.is_empty() {
                    let blocks = crate::ui::message::parsed(user_id, &view.bio);
                    let mut mcx = Cx {
                        state,
                        guild,
                        message: user_id,
                        revealed: &mut uis.revealed,
                        actions: &mut *actions,
                        size: 13.5,
                        color: theme::TEXT,
                        jumbo: false,
                    };
                    md::blocks(ui, &blocks, &mut mcx);
                    ui.add_space(8.0);
                }
                heading(ui, "Member Since");
                ui.label(RichText::new(format!("Discord: {}", date(user_id.created_at()))).size(12.5).color(theme::TEXT));
                if let Some(j) = joined {
                    ui.label(RichText::new(format!("This server: {}", date(j))).size(12.5).color(theme::TEXT));
                }
                if let Some(g) = guild.and_then(|g| state.guild(g)) {
                    let mut roles: Vec<_> = state
                        .member(g.id, user_id)
                        .map(|m| g.roles.iter().filter(|r| r.id != g.id && m.roles.contains(&r.id)).collect())
                        .unwrap_or_default();
                    roles.sort_by_key(|r| -r.position);
                    if !roles.is_empty() {
                        ui.add_space(8.0);
                        heading(ui, if roles.len() == 1 { "Role" } else { "Roles" });
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                            for r in roles {
                                role_chip(ui, &r.name, theme::role_color(Some(r.color).filter(|&c| c != 0)));
                            }
                        });
                    }
                }
                if loading {
                    ui.add_space(4.0);
                    ui.add(egui::Spinner::new().size(14.0));
                } else if let Some(e) = &error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Couldn't load the full profile: {e}")).size(12.0).color(theme::MUTED));
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if !me && ui.add(egui::Button::new(RichText::new("Message").color(Color32::WHITE)).fill(theme::BLURPLE)).clicked() {
                    actions.push(Action::OpenDm(user_id));
                    actions.push(Action::CloseProfile);
                }
                if me && ui.button("Edit Profile").clicked() {
                    actions.push(Action::EditProfile { guild, server: false });
                    actions.push(Action::CloseProfile);
                }
                if let (true, Some(g)) = (me, guild) {
                    if ui.button("Edit Server Profile").clicked() {
                        actions.push(Action::EditProfile { guild: Some(g), server: true });
                        actions.push(Action::CloseProfile);
                    }
                }
            });
        });

        // The card's background, now that its size is known: a gradient of their theme colours, or plain.
        let rect = Rect::from_min_max(top, egui::pos2(top.x + WIDTH, ui.min_rect().bottom()));
        let shapes = match theme_colors {
            Some((a, b)) => {
                let mut mesh = egui::Mesh::default();
                let middle = Rect::from_min_max(rect.min + Vec2::new(0.0, 8.0), rect.max - Vec2::new(0.0, 8.0));
                mesh.colored_vertex(middle.left_top(), a);
                mesh.colored_vertex(middle.right_top(), a);
                mesh.colored_vertex(middle.right_bottom(), b);
                mesh.colored_vertex(middle.left_bottom(), b);
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                // Rounded ends in each colour, with the gradient between them.
                vec![
                    egui::Shape::rect_filled(rect, 8, a),
                    egui::Shape::rect_filled(Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 16.0), rect.max), 8, b),
                    egui::Shape::mesh(mesh),
                ]
            }
            None => vec![egui::Shape::rect_filled(rect, 8, bg)],
        };
        let mut all = vec![egui::Shape::rect_filled(rect.expand(1.0), 9, Color32::from_black_alpha(120))];
        all.extend(shapes);
        ui.painter().set(background, egui::Shape::Vec(all));
        rect
    });
    let rect = area.inner;
    ctx.memory_mut(|m| m.data.insert_temp(id, rect));

    // Closed with Escape or a click elsewhere (done here, before this frame's other clicks are applied, so a
    // click on another avatar opens that card instead).
    let clicked_away = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p)));
    if clicked_away || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        uis.profile_card = None;
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).font(theme::font_bold(11.5)).color(theme::HEADER));
}

/// One widget (not nested layouts), so the row wraps.
fn role_chip(ui: &mut egui::Ui, name: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(name.to_owned(), theme::font(12.0), theme::TEXT);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 26.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 4, Color32::from_black_alpha(90));
    ui.painter().circle_filled(egui::pos2(rect.left() + 10.0, rect.center().y), 5.0, color);
    ui.painter().galley(egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
}

/// Where a card opened from `r` goes: just right of it, top-aligned.
pub fn beside(r: &egui::Response) -> egui::Pos2 {
    r.rect.right_top() + Vec2::new(8.0, 0.0)
}

pub fn open(user: Snowflake, guild: Option<Snowflake>, r: &egui::Response) -> Action {
    Action::OpenProfile { user, guild, at: beside(r) }
}
