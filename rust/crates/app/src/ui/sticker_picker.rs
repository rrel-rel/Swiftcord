//! Sticker picker: this server's stickers first, other servers' with Nitro (which sending them needs), then
//! Discord's standard packs. Picking one sends it.

use std::time::{Duration, Instant};

use egui::{self, Pos2, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::Sticker;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme;

const SIZE: Vec2 = Vec2::new(440.0, 480.0);
const CELL: f32 = 100.0;

pub struct StickerPicker {
    pub channel: Snowflake,
    at: Pos2,
    query: String,
    opened: Instant,
}

impl StickerPicker {
    pub fn new(channel: Snowflake, at: Pos2) -> Self {
        Self { channel, at, query: String::new(), opened: Instant::now() }
    }
}

fn matches(s: &Sticker, q: &str) -> bool {
    q.is_empty() || s.name.to_lowercase().contains(q) || s.tags.to_lowercase().contains(q)
}

pub fn show(ui: &Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let UiState { sticker_picker, stickers, sticker_packs, .. } = uis;
    let Some(p) = sticker_picker else { return };
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let mut pos = p.at - Vec2::new(SIZE.x, SIZE.y + 8.0);
    pos.x = pos.x.clamp(screen.left() + 8.0, (screen.right() - SIZE.x - 8.0).max(screen.left() + 8.0));
    pos.y = pos.y.clamp(screen.top() + 8.0, (screen.bottom() - SIZE.y - 8.0).max(screen.top() + 8.0));
    if sticker_packs.is_none() {
        // Marks them as on the way, so they're asked for once.
        *sticker_packs = Some(None);
        actions.push(Action::LoadStickerPacks);
    }

    // Sections: this server, other servers (with Nitro), then the standard packs.
    let state = &s.state;
    let here = state.channel(p.channel).and_then(|c| c.guild_id);
    let nitro = state.me.as_ref().and_then(|m| m.premium_type).is_some_and(|t| t == 2 || t == 3);
    let q = p.query.trim().to_lowercase();
    let mut sections: Vec<(String, Vec<&Sticker>)> = Vec::new();
    let mut guilds: Vec<Snowflake> = here.into_iter().collect();
    if nitro {
        guilds.extend(state.rail().into_iter().flat_map(|r| match r {
            swiftcord_core::state::RailItem::Guild(g) => vec![g],
            swiftcord_core::state::RailItem::Folder { guilds, .. } => guilds,
        }).filter(|g| Some(*g) != here));
    }
    for g in guilds {
        let Some(gd) = state.guild(g) else { continue };
        let list: Vec<&Sticker> = gd.stickers.iter().filter(|st| st.available && matches(st, &q)).collect();
        if !list.is_empty() {
            sections.push((gd.name.clone(), list));
        }
    }
    let packs_loading = matches!(sticker_packs, Some(None));
    if let Some(Some(packs)) = sticker_packs.as_ref() {
        for pack in packs {
            let mut list: Vec<&Sticker> = pack.stickers.iter().filter(|st| st.available && matches(st, &q)).collect();
            list.sort_by_key(|st| st.sort_value.unwrap_or(0));
            if !list.is_empty() {
                sections.push((pack.name.clone(), list));
            }
        }
    }

    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut picked = None;
    let area = egui::Area::new(egui::Id::unique("sticker-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR()).inner_margin(10).show(ui, |ui| {
            ui.set_width(SIZE.x - 20.0);
            ui.set_height(SIZE.y - 20.0);
            let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text("Find stickers").desired_width(f32::INFINITY));
            if p.opened.elapsed() < Duration::from_millis(300) {
                r.request_focus();
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let columns = ((ui.available_width() / CELL).floor() as usize).max(1);
                for (title, list) in &sections {
                    ui.label(RichText::new(title.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED()));
                    for row in list.chunks(columns) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for st in row {
                                let (rect, r) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                if r.hovered() {
                                    ui.painter().rect_filled(rect, 6, theme::SELECTED());
                                }
                                stickers.paint(ui, rect.shrink(8.0), &st.item(), r.hovered());
                                let r = r.on_hover_text(&st.name).on_hover_cursor(egui::CursorIcon::PointingHand);
                                if r.clicked() {
                                    picked = Some(st.item());
                                }
                            }
                        });
                    }
                    ui.add_space(6.0);
                }
                if packs_loading {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| ui.spinner());
                } else if sections.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("No stickers found.").color(theme::MUTED())));
                }
            });
        });
    });
    if let Some(sticker) = picked {
        actions.push(Action::SendSticker { channel: p.channel, sticker });
        // Shift-click sends several, as with emoji.
        if !ctx.input(|i| i.modifiers.shift) {
            close = true;
        }
    }
    // Clicking outside closes it (but not the click that opened it).
    if p.opened.elapsed() > Duration::from_millis(200) && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() {
        close = true;
    }
    if close {
        *sticker_picker = None;
    }
}
