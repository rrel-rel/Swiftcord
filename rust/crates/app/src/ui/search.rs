//! The message search results panel: newest first, each with the channel it's in and a Jump button, a page
//! at a time.

use egui::{self, Align, Layout, RichText, Ui, Vec2};

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::message::{self, Ctx};
use crate::ui::widgets;

pub fn panel(ui: &mut Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(search) = &uis.search else { return };
    let (guild, loading, total, error, unknown) = (search.guild, search.loading, search.total, search.error.clone(), search.unknown.clone());
    let results = search.results.clone();
    let has_more = search.has_more();
    let query = search.text.clone();
    ui.horizontal(|ui| {
        let title = if loading && results.is_empty() {
            "Searching…".to_owned()
        } else {
            match total {
                1 => "1 Result".to_owned(),
                n => format!("{n} Results"),
            }
        };
        ui.label(RichText::new(title).font(theme::font_semibold(16.0)).color(theme::HEADER()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, icon::CLOSE, "Close").clicked() {
                actions.push(Action::CloseSearch);
            }
        });
    });
    ui.label(RichText::new(query).size(12.5).color(theme::FAINT()));
    ui.separator();
    if !unknown.is_empty() {
        ui.label(RichText::new(format!("Couldn't find {} (left out of the search).", unknown.join(", "))).size(12.5).color(theme::YELLOW()));
    }
    if let Some(e) = &error {
        ui.label(RichText::new(e).color(theme::RED()));
    }
    if results.is_empty() {
        if loading {
            ui.centered_and_justified(|ui| ui.spinner());
        } else if error.is_none() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No results found.").color(theme::MUTED())));
        }
        return;
    }
    let mut jump = None;
    let scroll = egui::ScrollArea::vertical().id_salt("search-results").auto_shrink(false).show(ui, |ui| {
        ui.set_max_width(ui.available_width());
        for m in &results {
            let channel = s.state.channel(m.channel_id);
            // Where it was said, as Discord labels results.
            if guild.is_some() {
                let name = channel.map_or_else(|| "unknown-channel".to_owned(), |c| c.label());
                let glyph = if channel.is_some_and(|c| c.is_thread()) { "💬 " } else { "# " };
                ui.label(RichText::new(format!("{glyph}{name}")).font(theme::font_semibold(12.5)).color(theme::MUTED()));
            }
            let perms = s.state.permissions(m.channel_id);
            let mut cx = Ctx { state: &s.state, uis, actions, channel: m.channel_id, guild, perms, highlight: None, read_only: true };
            let r = egui::Frame::new().fill(theme::CHAT()).corner_radius(6).inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                message::show(ui, m, false, &mut cx);
            });
            if widgets::jump_button(ui, r.response.rect, ("search-jump", m.id)) {
                jump = Some((m.channel_id, m.id));
            }
            ui.add_space(8.0);
        }
        if has_more {
            ui.vertical_centered(|ui| {
                if loading {
                    ui.spinner();
                } else if ui.button("Load more").clicked() {
                    actions.push(Action::SearchMore);
                }
            });
        }
        ui.add_space(Vec2::splat(8.0).y);
    });
    crate::ui::widgets::scrollbar(ui, &scroll);
    if let Some((channel, message)) = jump {
        actions.push(Action::JumpTo { channel, message: Some(message) });
    }
}
