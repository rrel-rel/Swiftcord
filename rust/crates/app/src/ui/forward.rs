//! "Forward To": picking up to five DMs or channels to forward a message to, with an optional note, as in
//! Discord. DMs come first (most recent first), then the open server's channels; typing searches them all.

use egui::{self, Color32, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::Channel;
use swiftcord_core::permissions::perm;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme;

const MAX_PICKS: usize = 5;

/// A message being forwarded, and where to.
#[derive(Debug, Default)]
pub struct Forward {
    pub channel: Snowflake,
    pub message: Snowflake,
    pub query: String,
    pub picked: Vec<Snowflake>,
    pub note: String,
}

/// Where a message can be forwarded: DMs, and text channels we can post in.
fn destinations<'a>(state: &'a State, guild: Option<Snowflake>, query: &str) -> Vec<&'a Channel> {
    let q = query.trim().to_lowercase();
    let can_post = |c: &Channel| state.permissions(c.id) & perm::SEND_MESSAGES != 0;
    let dms = state.private_channels().into_iter().filter(|c| c.is_text_based());
    if q.is_empty() {
        let here = guild.map(|g| state.channel_tree(g).into_iter().flat_map(|(_, cs)| cs).filter(|c| c.is_text_based() && !c.is_voice() && can_post(c)).collect::<Vec<_>>());
        return dms.take(12).chain(here.unwrap_or_default()).collect();
    }
    let channels = state.guilds().flat_map(|g| state.channel_tree(g.id).into_iter().flat_map(|(_, cs)| cs)).filter(|c| c.is_text_based() && !c.is_voice() && can_post(c));
    dms.chain(channels).filter(|c| c.label().to_lowercase().contains(&q)).take(40).collect()
}

pub fn show(ui: &Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let guild = uis.guild;
    let Some(fwd) = &mut uis.forward else { return };
    let state = &s.state;
    let Some(msg) = state.message(fwd.channel, fwd.message) else {
        uis.forward = None;
        return;
    };
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::unique("forward-message")).show(ui.ctx(), |ui| {
        ui.set_width(460.0);
        ui.label(RichText::new("Forward To").font(theme::font_bold(20.0)).color(theme::HEADER()));
        ui.label(RichText::new(format!("Select where you want to share this message (up to {MAX_PICKS}).")).color(theme::MUTED()));
        ui.add_space(8.0);
        let search = ui.add(egui::TextEdit::singleline(&mut fwd.query).hint_text("Search").desired_width(f32::INFINITY));
        if search.gained_focus() || ui.memory(|m| m.focused().is_none()) {
            search.request_focus();
        }
        ui.add_space(6.0);
        let scroll = egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            let list = destinations(state, guild, &fwd.query);
            if list.is_empty() {
                ui.label(RichText::new("No matches.").color(theme::MUTED()));
            }
            for c in list {
                let picked = fwd.picked.contains(&c.id);
                if destination_row(ui, state, c, picked).clicked() {
                    if picked {
                        fwd.picked.retain(|&p| p != c.id);
                    } else if fwd.picked.len() < MAX_PICKS {
                        fwd.picked.push(c.id);
                    }
                }
            }
        });
        crate::ui::widgets::scrollbar(ui, &scroll);
        ui.add_space(8.0);
        // What's being forwarded.
        egui::Frame::new().fill(theme::CHAT()).corner_radius(6).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let author = state.display_name(msg.author.id, state.channel(msg.channel_id).and_then(|c| c.guild_id));
            ui.label(RichText::new(author).font(theme::font_semibold(13.0)).color(theme::HEADER()));
            let text = if msg.content.is_empty() {
                match msg.attachments.len() {
                    0 => "A message".to_owned(),
                    1 => "1 attachment".to_owned(),
                    n => format!("{n} attachments"),
                }
            } else {
                msg.content.lines().next().unwrap_or_default().to_owned()
            };
            ui.add(egui::Label::new(RichText::new(text).color(theme::TEXT())).truncate());
        });
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut fwd.note).hint_text("Add an optional message…").desired_width(f32::INFINITY));
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if fwd.picked.len() > 1 { format!("Send ({})", fwd.picked.len()) } else { "Send".to_owned() };
            let send = egui::Button::new(RichText::new(label).color(Color32::WHITE)).min_size(Vec2::new(90.0, 32.0));
            if ui.add_enabled(!fwd.picked.is_empty(), send).clicked() {
                actions.push(Action::Forward { channel: fwd.channel, message: fwd.message, to: fwd.picked.clone(), note: fwd.note.trim().to_owned() });
                close = true;
            }
            if ui.add(egui::Button::new("Cancel").min_size(Vec2::new(80.0, 32.0))).clicked() {
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        uis.forward = None;
    }
}

/// Text on one line, ending in "…" if it's wider than `width`.
fn one_line(p: &egui::Painter, text: String, font: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    p.layout_job(job)
}

/// One destination: a tick circle, its picture or symbol, its name and (for a server channel) the server.
fn destination_row(ui: &mut Ui, state: &State, c: &Channel, picked: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
    let p = ui.painter();
    if r.hovered() || picked {
        p.rect_filled(rect, 4, if picked { theme::SELECTED() } else { theme::HOVER() });
    }
    let icon = egui::Rect::from_center_size(rect.left_center() + Vec2::new(26.0, 0.0), Vec2::splat(28.0));
    match c.recipients.first().filter(|_| c.recipients.len() == 1) {
        Some(u) => {
            egui::Image::from_uri(swiftcord_core::cdn::avatar(u, 64)).corner_radius(14).show_loading_spinner(false).paint_at(ui, icon);
        }
        None => {
            p.circle_filled(icon.center(), 14.0, theme::RAIL());
            let glyph = if c.is_private() { theme::icon::PEOPLE } else { "#" };
            let font = if c.is_private() { theme::icon_font(13.0) } else { theme::font_semibold(16.0) };
            p.text(icon.center(), egui::Align2::CENTER_CENTER, glyph, font, theme::MUTED());
        }
    }
    // The server's name on the right, then the channel's name cut short to the room left.
    let mut right = rect.right() - 36.0;
    if let Some(g) = c.guild_id.and_then(|g| state.guild(g)) {
        let server = one_line(p, g.name.clone(), theme::font(12.0), theme::FAINT(), 140.0);
        right -= server.size().x + 12.0;
        p.galley(egui::pos2(right + 12.0, rect.center().y - server.size().y / 2.0), server, theme::FAINT());
    }
    let name = if c.is_private() { c.label() } else { format!("#{}", c.label()) };
    let left = icon.right() + 10.0;
    let name = one_line(p, name, theme::font_semibold(14.0), theme::HEADER(), (right - left - 8.0).max(20.0));
    p.galley(egui::pos2(left, rect.center().y - name.size().y / 2.0), name, theme::HEADER());
    // The tick circle on the right.
    let tick = rect.right_center() - Vec2::new(18.0, 0.0);
    if picked {
        p.circle_filled(tick, 9.0, theme::ACCENT());
        p.text(tick, egui::Align2::CENTER_CENTER, "✔", theme::font_bold(11.0), Color32::WHITE);
    } else {
        p.circle_stroke(tick, 9.0, egui::Stroke::new(1.5, theme::MUTED()));
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}
