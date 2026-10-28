//! The mentions inbox panel: recent messages that mention us, newest first, each under the server and
//! channel it was said in, with Jump and a button to take it off the list.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::message::{self, Ctx};
use crate::ui::widgets;

pub fn panel(ui: &mut Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(inbox) = &uis.inbox else { return };
    let (loading, error, reached_end) = (inbox.loading, inbox.error.clone(), inbox.reached_end);
    let (mut only_here, mut everyone, mut roles) = (inbox.guild.is_some(), inbox.everyone, inbox.roles);
    let messages = inbox.messages.clone();
    // "This server" is the one open now, or the one already chosen.
    let here = inbox.guild.or(uis.guild);
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::INBOX).font(theme::icon_font(16.0)).color(theme::HEADER()));
        ui.label(RichText::new("Mentions").font(theme::font_semibold(16.0)).color(theme::HEADER()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, icon::CLOSE, "Close").clicked() {
                actions.push(Action::ShowInbox);
            }
            ui.menu_button(RichText::new("Filter").size(13.0).color(theme::MUTED()), |ui| {
                let mut changed = false;
                let server = here.and_then(|g| s.state.guild(g)).map(|g| g.name.clone());
                ui.add_enabled_ui(server.is_some(), |ui| {
                    let label = server.as_deref().map_or("This server only".to_owned(), |n| format!("Only {n}"));
                    changed |= ui.checkbox(&mut only_here, label).changed();
                });
                changed |= ui.checkbox(&mut everyone, "Include @everyone and @here").changed();
                changed |= ui.checkbox(&mut roles, "Include role mentions").changed();
                if changed {
                    actions.push(Action::InboxFilters { guild: here.filter(|_| only_here), everyone, roles });
                }
            });
        });
    });
    ui.separator();
    if let Some(e) = &error {
        ui.label(RichText::new(format!("Couldn't load mentions: {e}")).color(theme::RED()));
    }
    if messages.is_empty() {
        if loading {
            ui.centered_and_justified(|ui| ui.spinner());
        } else if error.is_none() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("You're all caught up").font(theme::font_semibold(15.0)).color(theme::HEADER()));
                ui.label(RichText::new("Messages that mention you will show up here.").color(theme::MUTED()));
            });
        }
        return;
    }
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt("inbox-mentions").auto_shrink(false).show(ui, |ui| {
        ui.set_max_width(ui.available_width());
        for m in &messages {
            let channel = s.state.channel(m.channel_id);
            let guild = m.guild_id.or_else(|| channel.and_then(|c| c.guild_id));
            // Where it was said, as Discord heads each mention.
            let place = match (guild.and_then(|g| s.state.guild(g)), channel) {
                (Some(g), Some(c)) => format!("{} › {}{}", g.name, if c.is_thread() { "💬 " } else { "#" }, c.label()),
                (None, Some(c)) if c.recipients.len() == 1 => format!("@{}", c.label()),
                (None, Some(c)) => c.label(),
                (Some(g), None) => format!("{} › unknown channel", g.name),
                (None, None) => "Unknown channel".to_owned(),
            };
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, icon::CLOSE, "Remove from Inbox").clicked() {
                        actions.push(Action::DismissMention(m.id));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(place).font(theme::font_semibold(12.5)).color(theme::MUTED())).truncate());
                    });
                });
            });
            let perms = s.state.permissions(m.channel_id);
            let mut cx = Ctx { state: &s.state, uis, actions, channel: m.channel_id, guild, perms, highlight: None, read_only: true };
            let r = egui::Frame::new().fill(theme::CHAT()).corner_radius(6).inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                message::show(ui, m, false, &mut cx);
            });
            if widgets::jump_button(ui, r.response.rect, ("inbox-jump", m.id)) {
                jump = Some((m.channel_id, m.id));
            }
            ui.add_space(8.0);
        }
        if !reached_end {
            ui.vertical_centered(|ui| {
                if loading {
                    ui.spinner();
                } else if ui.button("Load more").clicked() {
                    actions.push(Action::InboxMore);
                }
            });
        }
        ui.add_space(8.0);
    });
    if let Some((channel, message)) = jump {
        actions.push(Action::JumpTo { channel, message: Some(message) });
    }
}
