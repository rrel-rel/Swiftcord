//! Composer autocomplete: @people/roles, #channels and :emoji:, shown above the text box.

use egui::{self, RichText, Sense, Ui, Vec2};
use swiftcord_core::mentions::Token;
use swiftcord_core::models::{Channel, ChannelType};
use swiftcord_core::permissions::perm;
use swiftcord_core::Snowflake;

use crate::app::Session;
use crate::theme;
use crate::ui::emoji::Catalog;
use crate::ui::widgets;

const LIMIT: usize = 8;

#[derive(Debug, Clone)]
pub enum Suggestion {
    User { id: Snowflake, display: String, username: String, avatar: String },
    Role { id: Snowflake, name: String, color: u32 },
    Everyone(&'static str),
    Channel { id: Snowflake, name: String },
    /// A Unicode emoji (inserted as itself, and drawn from the emoji sheet) or a custom one (`image`).
    Emoji { text: String, name: String, image: String, custom: Option<(Snowflake, bool)> },
    /// An app's slash command (`index` into the place's commands).
    Command { index: usize, name: String, description: String, app: String },
}

impl Suggestion {
    /// What goes into the text box, and how it's rewritten when sending (if at all).
    pub fn insertion(&self) -> (String, Option<String>) {
        match self {
            Suggestion::User { id, username, .. } => (format!("@{username}"), Some(format!("<@{id}>"))),
            Suggestion::Role { id, name, .. } => (format!("@{name}"), Some(format!("<@&{id}>"))),
            Suggestion::Everyone(word) => (word.to_string(), None),
            Suggestion::Channel { id, name } => (format!("#{name}"), Some(format!("<#{id}>"))),
            Suggestion::Emoji { text, custom: None, .. } => (text.clone(), None),
            Suggestion::Emoji { name, custom: Some((id, animated)), .. } => {
                (format!(":{name}:"), Some(format!("<{}:{name}:{id}>", if *animated { "a" } else { "" })))
            }
            // Picking a command opens its form instead of inserting text.
            Suggestion::Command { name, .. } => (format!("/{name}"), None),
        }
    }
}

pub fn suggestions(s: &Session, catalog: &Catalog, channel: &Channel, token: &Token, perms: u64) -> Vec<Suggestion> {
    let state = &s.state;
    let q = token.query.to_lowercase();
    let mut out = Vec::new();
    match token.trigger {
        '@' => {
            for u in state.search_people(channel, &token.query, LIMIT) {
                out.push(Suggestion::User {
                    id: u.id,
                    display: state.display_name(u.id, channel.guild_id),
                    username: u.username.clone(),
                    avatar: state.avatar_url(u, channel.guild_id, 64),
                });
            }
            if let Some(g) = channel.guild_id.and_then(|g| state.guild(g)) {
                if perms & perm::MENTION_EVERYONE != 0 {
                    for word in ["@everyone", "@here"] {
                        if word[1..].starts_with(&q) {
                            out.push(Suggestion::Everyone(word));
                        }
                    }
                }
                let mut roles: Vec<_> = g.roles.iter().filter(|r| r.id != g.id && r.name.to_lowercase().starts_with(&q)).collect();
                roles.sort_by_key(|r| std::cmp::Reverse(r.position));
                for r in roles.into_iter().take(3) {
                    out.push(Suggestion::Role { id: r.id, name: r.name.clone(), color: r.color });
                }
            }
        }
        '#' => {
            if let Some(g) = channel.guild_id {
                let mut chans: Vec<&Channel> = state
                    .channel_tree(g)
                    .into_iter()
                    .flat_map(|(_, cs)| cs)
                    .filter(|c| !matches!(c.kind, ChannelType::Category) && c.label().to_lowercase().contains(&q))
                    .collect();
                chans.sort_by_key(|c| !c.label().to_lowercase().starts_with(&q));
                out.extend(chans.into_iter().take(LIMIT).map(|c| Suggestion::Channel { id: c.id, name: c.label() }));
            }
        }
        ':' => {
            // This server's emoji first, then Unicode.
            if let Some(g) = channel.guild_id.and_then(|g| state.guild(g)) {
                for e in g.emojis.iter().filter(|e| e.name.as_deref().is_some_and(|n| n.to_lowercase().starts_with(&q))).take(4) {
                    let (Some(id), Some(name)) = (e.id, e.name.clone()) else { continue };
                    out.push(Suggestion::Emoji {
                        text: String::new(),
                        image: swiftcord_core::cdn::emoji(id, e.animated, 48),
                        name,
                        custom: Some((id, e.animated)),
                    });
                }
            }
            for (ch, name) in catalog.search(&q, LIMIT - out.len().min(LIMIT)) {
                out.push(Suggestion::Emoji { image: String::new(), text: ch, name, custom: None });
            }
        }
        _ => {}
    }
    out.truncate(LIMIT);
    out
}

/// Draws the list above `anchor` (the composer's rect); returns a clicked suggestion's index.
pub fn popup(ui: &Ui, anchor: egui::Rect, items: &[Suggestion], selected: usize, title: &str) -> Option<usize> {
    const ROW: f32 = 34.0;
    // Rows are separated by the usual item spacing, so the list sits clear of the composer.
    let height = items.len() as f32 * (ROW + ui.spacing().item_spacing.y) + 34.0;
    let pos = anchor.left_top() - Vec2::new(0.0, height + 6.0);
    let mut clicked = None;
    let area = egui::Area::new(egui::Id::unique("composer-suggestions")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR()).corner_radius(8).inner_margin(6).show(ui, |ui| {
            ui.set_width(anchor.width() - 12.0);
            ui.label(RichText::new(title).font(theme::font_semibold(11.5)).color(theme::MUTED()));
            for (i, item) in items.iter().enumerate() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
                if i == selected || ui.rect_contains_pointer(rect) {
                    ui.painter().rect_filled(rect, 4, theme::SELECTED());
                }
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                match item {
                    Suggestion::User { display, username, avatar, .. } => {
                        widgets::avatar(&mut row, avatar, 24.0);
                        row.label(RichText::new(display).color(theme::HEADER()));
                        row.label(RichText::new(username).color(theme::MUTED()));
                    }
                    Suggestion::Role { name, color, .. } => {
                        row.label(RichText::new(format!("@{name}")).color(theme::role_color(Some(*color).filter(|&c| c != 0))));
                    }
                    Suggestion::Everyone(word) => {
                        row.label(RichText::new(*word).color(theme::HEADER()));
                        let note = if *word == "@everyone" { "Notify everyone who can see this channel" } else { "Notify everyone online" };
                        row.label(RichText::new(note).small().color(theme::MUTED()));
                    }
                    Suggestion::Channel { name, .. } => {
                        row.label(RichText::new("#").size(18.0).color(theme::FAINT()));
                        row.label(RichText::new(name).color(theme::HEADER()));
                    }
                    Suggestion::Emoji { image, name, text, custom } => {
                        let size = Vec2::splat(22.0);
                        let img = match custom {
                            Some(_) => egui::Image::from_uri(image.clone()).fit_to_exact_size(size).show_loading_spinner(false),
                            None => crate::emoji_sheet::image(row.ctx(), text, size),
                        };
                        row.add(img);
                        row.label(RichText::new(format!(":{name}:")).color(theme::HEADER()));
                    }
                    Suggestion::Command { name, description, app, .. } => {
                        row.label(RichText::new(format!("/{name}")).font(theme::font_semibold(14.0)).color(theme::HEADER()));
                        row.add(egui::Label::new(RichText::new(description).size(12.5).color(theme::MUTED())).truncate());
                        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(app).size(12.0).color(theme::FAINT()));
                        });
                    }
                }
                // Registered after the row's contents so it's on top of them: the labels (selectable) and
                // avatars would otherwise take the click.
                let r = ui.interact(rect, ui.scope_id().with(("suggestion", i)), Sense::click());
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(rect_id(), area.response.rect));
    clicked
}

fn rect_id() -> egui::Id {
    egui::Id::unique("composer-suggestions-rect")
}

/// Whether the pointer is over the list as last drawn. Pressing on it takes focus from the text box, so the
/// composer keeps showing it while it's pointed at, or the click would be lost.
pub fn pointed_at(ctx: &egui::Context) -> bool {
    let rect = ctx.memory(|m| m.data.get_temp::<egui::Rect>(rect_id()));
    rect.zip(ctx.pointer_latest_pos()).is_some_and(|(r, p)| r.contains(p))
}

/// Forgets the list's position once it's no longer shown.
pub fn hidden(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.data.remove::<egui::Rect>(rect_id()));
}
