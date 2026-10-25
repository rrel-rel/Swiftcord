//! Emoji picker for reactions and the composer: recents, server emoji, then Unicode by category.

use eframe::egui::{self, Pos2, RichText, Ui, Vec2};
use serde::Deserialize;
use swiftcord_core::models::Emoji;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::theme;

const EMOJI_JSON: &str = include_str!("../../assets/emoji.json");
const RECENT_MAX: usize = 24;
const CELL: f32 = 38.0;
const COLUMNS: usize = 9;

#[derive(Debug, Clone, Copy)]
pub enum PickerTarget {
    Reaction { channel: Snowflake, message: Snowflake, at: Pos2 },
    Composer { channel: Snowflake, at: Pos2 },
}

impl PickerTarget {
    fn at(&self) -> Pos2 {
        match self {
            Self::Reaction { at, .. } | Self::Composer { at, .. } => *at,
        }
    }
}

struct Unicode {
    ch: String,
    name: String,
    terms: String,
    group: usize,
}

pub struct Catalog {
    unicode: Vec<Unicode>,
    groups: Vec<String>,
    recent: Vec<Emoji>,
}

#[derive(Deserialize)]
struct File {
    groups: Vec<String>,
    emoji: Vec<(String, String, String, usize)>,
}

fn recent_path() -> std::path::PathBuf {
    crate::platform::data_dir().join("recent-emoji.json")
}

impl Catalog {
    pub fn load() -> Catalog {
        let file: File = serde_json::from_str(EMOJI_JSON).unwrap_or(File { groups: vec![], emoji: vec![] });
        let unicode = file.emoji.into_iter().map(|(ch, name, terms, group)| Unicode { ch, name, terms, group }).collect();
        let recent = std::fs::read_to_string(recent_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<(Option<Snowflake>, Option<String>, bool)>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, name, animated)| Emoji { id, name, animated })
            .collect();
        Catalog { unicode, groups: file.groups, recent }
    }

    pub fn note_used(&mut self, e: &Emoji) {
        self.recent.retain(|r| !r.same_as(e));
        self.recent.insert(0, e.clone());
        self.recent.truncate(RECENT_MAX);
        let raw: Vec<_> = self.recent.iter().map(|e| (e.id, e.name.clone(), e.animated)).collect();
        let _ = std::fs::write(recent_path(), serde_json::to_string(&raw).unwrap_or_default());
    }
}

enum Item {
    Header(String),
    Emoji(Emoji),
}

fn url(e: &Emoji) -> String {
    match (e.id, &e.name) {
        (Some(id), _) => swiftcord_core::cdn::emoji(id, e.animated, 48),
        (None, Some(n)) => swiftcord_core::cdn::twemoji(n),
        _ => String::new(),
    }
}

/// Lays the picker out as rows (headers take a whole row) so only visible rows are drawn.
fn rows(catalog: &Catalog, state: &State, guild: Option<Snowflake>, query: &str) -> Vec<Vec<Item>> {
    let q = query.trim().to_lowercase();
    let mut sections: Vec<(String, Vec<Emoji>)> = Vec::new();
    if q.is_empty() && !catalog.recent.is_empty() {
        sections.push(("Frequently Used".into(), catalog.recent.clone()));
    }
    // Custom emoji: this server's first, then the rest.
    let mut guild_ids: Vec<Snowflake> = guild.into_iter().collect();
    guild_ids.extend(state.rail().into_iter().flat_map(|r| match r {
        swiftcord_core::state::RailItem::Guild(g) => vec![g],
        swiftcord_core::state::RailItem::Folder { guilds, .. } => guilds,
    }).filter(|g| Some(*g) != guild));
    for g in guild_ids {
        let Some(gd) = state.guild(g) else { continue };
        let list: Vec<Emoji> = gd
            .emojis
            .iter()
            .filter(|e| q.is_empty() || e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&q)))
            .cloned()
            .collect();
        if !list.is_empty() {
            sections.push((gd.name.clone(), list));
        }
    }
    for (gi, group) in catalog.groups.iter().enumerate() {
        let list: Vec<Emoji> = catalog
            .unicode
            .iter()
            .filter(|u| u.group == gi && (q.is_empty() || u.name.contains(&q) || u.terms.contains(&q)))
            .map(|u| Emoji { id: None, name: Some(u.ch.clone()), animated: false })
            .collect();
        if !list.is_empty() {
            sections.push((group.clone(), list));
        }
    }
    let mut out = Vec::new();
    for (title, list) in sections {
        out.push(vec![Item::Header(title)]);
        let mut it = list.into_iter().peekable();
        while it.peek().is_some() {
            out.push(it.by_ref().take(COLUMNS).map(Item::Emoji).collect());
        }
    }
    out
}

pub fn show(ui: &mut Ui, state: &State, uis: &mut UiState, catalog: &mut Catalog, actions: &mut Vec<Action>) {
    let Some(target) = uis.emoji_picker else { return };
    let ctx = ui.ctx().clone();
    let size = Vec2::new(COLUMNS as f32 * CELL + 24.0, 420.0);
    let screen = ctx.content_rect();
    let mut pos = target.at() - Vec2::new(size.x, size.y + 8.0);
    pos.x = pos.x.clamp(screen.left() + 8.0, screen.right() - size.x - 8.0);
    pos.y = pos.y.clamp(screen.top() + 8.0, screen.bottom() - size.y - 8.0);
    let guild = uis.channel().and_then(|c| state.channel(c)).and_then(|c| c.guild_id);

    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let area = egui::Area::new(egui::Id::unique("emoji-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR).inner_margin(10).show(ui, |ui| {
            ui.set_width(size.x - 20.0);
            ui.set_height(size.y - 20.0);
            let r = ui.add(egui::TextEdit::singleline(&mut uis.emoji_search).hint_text("Find the perfect emoji").desired_width(f32::INFINITY));
            if !r.has_focus() && uis.emoji_search.is_empty() {
                r.request_focus();
            }
            ui.add_space(4.0);
            let rows = rows(catalog, state, guild, &uis.emoji_search);
            let mut hovered: Option<String> = None;
            egui::ScrollArea::vertical().auto_shrink(false).max_height(size.y - 90.0).show_rows(ui, CELL, rows.len(), |ui, range| {
                for row in &rows[range] {
                    ui.horizontal(|ui| {
                        ui.set_height(CELL);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for item in row {
                            match item {
                                Item::Header(t) => {
                                    ui.label(RichText::new(t.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED));
                                }
                                Item::Emoji(e) => {
                                    let b = egui::Button::image(egui::Image::from_uri(url(e)).fit_to_exact_size(Vec2::splat(28.0)).show_loading_spinner(false))
                                        .frame_when_inactive(false)
                                        .min_size(Vec2::splat(CELL));
                                    let r = ui.add(b);
                                    if r.hovered() {
                                        hovered = e.name.clone();
                                    }
                                    if r.clicked() {
                                        match target {
                                            // The app records the reaction as recently used.
                                            PickerTarget::Reaction { channel, message, .. } => {
                                                actions.push(Action::React { channel, message, emoji: e.clone(), add: true });
                                            }
                                            PickerTarget::Composer { channel, .. } => {
                                                catalog.note_used(e);
                                                let text = match e.id {
                                                    Some(id) => format!("<{}:{}:{id}>", if e.animated { "a" } else { "" }, e.name.as_deref().unwrap_or("_")),
                                                    None => e.name.clone().unwrap_or_default(),
                                                };
                                                uis.drafts.entry(channel).or_default().text.push_str(&text);
                                                uis.focus_composer = true;
                                            }
                                        }
                                        // Shift-click picks several, as in Discord.
                                        if !ui.input(|i| i.modifiers.shift) {
                                            close = true;
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
            });
            ui.separator();
            let footer = hovered.map(|n| if n.chars().count() <= 2 { n } else { format!(":{n}:") }).unwrap_or_default();
            ui.label(RichText::new(footer).color(theme::MUTED));
        });
    });
    // Clicking anywhere else closes it.
    if ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && !ctx.is_being_dragged(area.response.id) {
        let just_opened = ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::unique("emoji-picker-open")).is_none());
        if !just_opened {
            close = true;
        }
    }
    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::unique("emoji-picker-open"), true));
    if close {
        uis.emoji_picker = None;
        uis.emoji_search.clear();
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::unique("emoji-picker-open")));
    }
}
