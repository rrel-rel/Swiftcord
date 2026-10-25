//! The chat area: header, a virtualized message list that stays pinned to the bottom and keeps its place
//! when older history loads above, typing indicator and composer.

use std::collections::HashMap;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::{Channel, ChannelType, Message};
use swiftcord_core::permissions::perm;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::emoji::PickerTarget;
use crate::ui::message::{self, Ctx};
use crate::ui::widgets;

/// Space around the list that's laid out even though it's off-screen, so short scrolls don't show gaps.
const OVERSCAN: f32 = 600.0;
const TOP_ROW: Snowflake = Snowflake(0);

pub struct ChatView {
    heights: HashMap<Snowflake, f32>,
    /// The first row in view and its offset from the top of the content, to keep it still when rows above
    /// change height or are inserted.
    anchor: Option<(Snowflake, f32)>,
    at_bottom: bool,
    scroll_y: f32,
    jump: Option<Snowflake>,
    highlight: Option<(Snowflake, Instant)>,
}

impl Default for ChatView {
    fn default() -> Self {
        Self { heights: HashMap::new(), anchor: None, at_bottom: true, scroll_y: 0.0, jump: None, highlight: None }
    }
}

impl ChatView {
    pub fn scroll_to_bottom(&mut self) {
        self.at_bottom = true;
        self.anchor = None;
    }

    pub fn jump_to(&mut self, id: Snowflake) {
        self.jump = Some(id);
    }
}

struct Row<'a> {
    id: Snowflake,
    msg: Option<&'a Message>,
    compact: bool,
    divider: bool,
}

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let state = &s.state;
    let Some(channel) = uis.channel().and_then(|c| state.channel(c)) else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(if s.ready { "No channel selected" } else { "Connecting…" }).color(theme::MUTED));
        });
        return;
    };
    let perms = state.permissions(channel.id);

    header(ui, s, channel, actions);

    egui::Panel::bottom(("composer", channel.id)).frame(egui::Frame::new().fill(theme::CHAT).inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 20 })).show(ui, |ui| {
        typing_line(ui, s, channel);
        composer(ui, s, uis, channel, perms, actions);
    });

    if let Some((pin_channel, pins)) = &uis.pins {
        if *pin_channel == channel.id {
            let pins = pins.clone();
            egui::Panel::right(("pins", channel.id)).exact_size(380.0).frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(8)).show(ui, |ui| {
                pins_panel(ui, s, uis, channel, pins.as_deref(), actions);
            });
        }
    }

    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT)).show(ui, |ui| {
        message_list(ui, s, uis, channel, perms, actions);
    });
}

fn header(ui: &mut Ui, s: &Session, c: &Channel, actions: &mut Vec<Action>) {
    egui::Panel::top(("chat-header", c.id)).frame(egui::Frame::new().fill(theme::CHAT).inner_margin(egui::Margin::symmetric(16, 0))).show(ui, |ui| {
        ui.set_height(48.0);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let glyph = match c.kind {
                ChannelType::Dm => "@",
                ChannelType::GroupDm => icon::PEOPLE,
                _ if c.is_voice() => icon::SPEAKER,
                _ => "#",
            };
            let font = if glyph == "#" || glyph == "@" { theme::font(22.0) } else { theme::icon_font(18.0) };
            ui.label(RichText::new(glyph).font(font).color(theme::FAINT));
            ui.label(RichText::new(c.label()).font(theme::font_semibold(16.0)).color(theme::HEADER));
            if c.kind == ChannelType::Dm {
                if let Some(u) = c.recipients.first() {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, widgets::status_color(s.state.presence(u.id)));
                }
            }
            if let Some(topic) = c.topic.as_deref().filter(|t| !t.is_empty()) {
                ui.add(egui::Separator::default().vertical().spacing(16.0));
                let first_line = topic.lines().next().unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(first_line).size(13.5).color(theme::MUTED)).truncate()).on_hover_text(topic);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::PIN, "Pinned Messages").clicked() {
                    actions.push(Action::ShowPins(c.id));
                }
                if c.is_private() && widgets::icon_button(ui, crate::theme::icon::SIGNAL, "Start Voice Call").clicked() {
                    actions.push(Action::JoinVoice { guild: None, channel: c.id });
                }
            });
        });
    });
    // Discord's subtle header shadow.
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::RAIL));
}

fn message_list(ui: &mut Ui, s: &Session, uis: &mut UiState, channel: &Channel, perms: u64, actions: &mut Vec<Action>) {
    let state = &s.state;
    let Some(history) = state.messages(channel.id) else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    if perms & perm::READ_MESSAGE_HISTORY == 0 && channel.guild_id.is_some() {
        ui.centered_and_justified(|ui| ui.label(RichText::new("You don't have permission to read this channel's history.").color(theme::MUTED)));
        return;
    }

    let msgs: Vec<&Message> = history.list.values().collect();
    let read_up_to = None::<Snowflake>;
    let mut rows = Vec::with_capacity(msgs.len() + 1);
    rows.push(Row { id: TOP_ROW, msg: None, compact: false, divider: false });
    for (i, m) in msgs.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| msgs[p]);
        let divider = prev.is_none_or(|p| !message::same_day(p.timestamp(), m.timestamp()));
        rows.push(Row { id: m.id, msg: Some(m), compact: !divider && message::is_compact(m, prev) && read_up_to != Some(m.id), divider });
    }

    let mut view = uis.chat.remove(&channel.id).unwrap_or_default();
    if view.highlight.is_some_and(|(_, at)| at.elapsed().as_secs_f32() > 2.0) {
        view.highlight = None;
    }
    let height_of = |view: &ChatView, r: &Row| -> f32 {
        if let Some(h) = view.heights.get(&r.id) {
            return *h;
        }
        match r.msg {
            None => if history.reached_start { 150.0 } else { 120.0 },
            Some(m) => message::estimate_height(m, r.compact) + if r.divider { 34.0 } else { 0.0 },
        }
    };
    let mut tops = Vec::with_capacity(rows.len());
    let mut y = 0.0;
    for r in &rows {
        tops.push(y);
        y += height_of(&view, r);
    }
    let total = y + 16.0;

    // Keep the anchor row still if anything above it changed.
    let mut scroll_to = None;
    if let Some(target) = view.jump.take() {
        match rows.iter().position(|r| r.id == target) {
            Some(i) => {
                scroll_to = Some((tops[i] - ui.available_height() / 3.0).max(0.0));
                view.at_bottom = false;
                view.highlight = Some((target, Instant::now()));
            }
            None => uis.banner = Some(("That message isn't loaded.".into(), Instant::now())),
        }
    } else if !view.at_bottom {
        if let Some((id, old_top)) = view.anchor {
            if let Some(i) = rows.iter().position(|r| r.id == id) {
                let delta = tops[i] - old_top;
                if delta.abs() > 0.5 {
                    scroll_to = Some(view.scroll_y + delta);
                }
            }
        }
    }

    let mut area = egui::ScrollArea::vertical().id_salt(("history", channel.id)).auto_shrink(false).stick_to_bottom(true);
    if let Some(y) = scroll_to {
        area = area.vertical_scroll_offset(y);
    } else if view.at_bottom {
        // Clamped to the real maximum by the scroll area.
        area = area.vertical_scroll_offset(total);
    }

    let highlight = view.highlight.map(|(id, _)| id);
    let guild = channel.guild_id;
    let mut new_anchor = None;
    let mut measured = Vec::new();
    let out = area.show_viewport(ui, |ui, viewport| {
        ui.set_height(total);
        let origin = ui.min_rect().min;
        let width = ui.available_width();
        let visible = (viewport.min.y - OVERSCAN)..(viewport.max.y + OVERSCAN);
        let mut cx = Ctx { state, uis: &mut *uis, actions: &mut *actions, channel: channel.id, guild, perms, highlight, read_only: false };
        for (i, r) in rows.iter().enumerate() {
            let top = tops[i];
            let h = height_of(&view, r);
            if top + h < visible.start || top > visible.end {
                continue;
            }
            if new_anchor.is_none() && top + h > viewport.min.y {
                new_anchor = Some((r.id, top));
            }
            let rect = Rect::from_min_size(origin + Vec2::new(0.0, top), Vec2::new(width, 10_000.0));
            let used = ui
                .scope_builder(egui::UiBuilder::new().max_rect(rect).id_salt(("row", r.id)), |ui| {
                    let start = ui.cursor().top();
                    match r.msg {
                        None => top_row(ui, s, channel, history.reached_start),
                        Some(m) => {
                            if r.divider {
                                day_divider(ui, m);
                            }
                            message::show(ui, m, r.compact, &mut cx);
                        }
                    }
                    ui.cursor().top() - start
                })
                .inner;
            if (used - h).abs() > 0.5 {
                measured.push((r.id, used));
            }
        }
        viewport
    });

    let viewport = out.inner;
    if !measured.is_empty() {
        for (id, h) in measured {
            view.heights.insert(id, h);
        }
        ui.ctx().request_repaint();
    }
    view.scroll_y = out.state.clamped_offset().y;
    let max_scroll = (out.content_size.y - out.inner_rect.height()).max(0.0);
    view.at_bottom = out.state.clamped_offset().y >= max_scroll - 4.0;
    view.anchor = new_anchor;

    if viewport.min.y < 800.0 && !history.reached_start && history.loaded {
        actions.push(Action::LoadOlder(channel.id));
    }

    // A "jump to present" pill when scrolled far up.
    if !view.at_bottom && max_scroll - out.state.clamped_offset().y > 1500.0 {
        let rect = Rect::from_center_size(out.inner_rect.center_bottom() - Vec2::new(0.0, 24.0), Vec2::new(150.0, 30.0));
        let r = ui.put(rect, egui::Button::new(RichText::new("Jump to present").color(Color32::WHITE)).fill(theme::BLURPLE).corner_radius(15));
        if r.clicked() {
            view.scroll_to_bottom();
            ui.ctx().request_repaint();
        }
    }
    uis.chat.insert(channel.id, view);
}

fn top_row(ui: &mut Ui, s: &Session, c: &Channel, reached_start: bool) {
    if !reached_start {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| ui.spinner());
        ui.add_space(64.0);
        return;
    }
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.vertical(|ui| {
            match c.kind {
                ChannelType::Dm => {
                    if let Some(u) = c.recipients.first() {
                        widgets::user_avatar(ui, u, 80.0);
                    }
                }
                _ => {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(68.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 34.0, theme::SELECTED);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "#", theme::font(42.0), theme::HEADER);
                }
            }
            let title = if c.is_private() { c.label() } else { format!("Welcome to #{}!", c.label()) };
            ui.label(RichText::new(title).font(theme::font_bold(28.0)).color(theme::HEADER));
            let sub = if c.is_private() {
                format!("This is the beginning of your direct message history with {}.", c.label())
            } else {
                format!("This is the start of the #{} channel.", c.label())
            };
            ui.label(RichText::new(sub).color(theme::MUTED));
            let _ = s;
        });
    });
    ui.add_space(8.0);
}

fn day_divider(ui: &mut Ui, m: &Message) {
    ui.add_space(14.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
    let text = m.timestamp().with_timezone(&chrono::Local).format("%B %-d, %Y").to_string();
    let galley = ui.painter().layout_no_wrap(text, theme::font_semibold(12.0), theme::FAINT);
    let c = rect.center();
    let half = galley.size().x / 2.0 + 8.0;
    let stroke = egui::Stroke::new(1.0, theme::SELECTED);
    ui.painter().hline((rect.left() + 16.0)..=(c.x - half), c.y, stroke);
    ui.painter().hline((c.x + half)..=(rect.right() - 16.0), c.y, stroke);
    ui.painter().galley(c - galley.size() / 2.0, galley, theme::FAINT);
    ui.add_space(4.0);
}

fn typing_line(ui: &mut Ui, s: &Session, c: &Channel) {
    let users = s.state.typing_users(c.id);
    let text = match users.len() {
        0 => String::new(),
        1 => format!("{} is typing…", s.state.display_name(users[0], c.guild_id)),
        2 => format!("{} and {} are typing…", s.state.display_name(users[0], c.guild_id), s.state.display_name(users[1], c.guild_id)),
        3 => format!(
            "{}, {} and {} are typing…",
            s.state.display_name(users[0], c.guild_id),
            s.state.display_name(users[1], c.guild_id),
            s.state.display_name(users[2], c.guild_id)
        ),
        _ => "Several people are typing…".to_owned(),
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    if !text.is_empty() {
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, text, theme::font_semibold(12.5), theme::TEXT);
        // Typing indicators expire on a timer, so keep repainting while one is shown.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
}

fn composer(ui: &mut Ui, s: &Session, uis: &mut UiState, c: &Channel, perms: u64, actions: &mut Vec<Action>) {
    let state = &s.state;
    if perms & perm::SEND_MESSAGES == 0 {
        egui::Frame::new().fill(theme::INPUT).corner_radius(8).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("You do not have permission to send messages in this channel.").color(theme::FAINT));
        });
        return;
    }
    let edit_id = egui::Id::unique(("composer", c.id));
    let focused = ui.ctx().memory(|m| m.has_focus(edit_id));
    let draft = uis.drafts.entry(c.id).or_default();

    // Keys handled before the text box sees them: Enter sends (Shift+Enter is a newline), Escape cancels a
    // reply, Up edits your last message.
    let (enter, esc, up) = ui.input_mut(|i| {
        let enter = focused && !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        let esc = focused && i.key_pressed(egui::Key::Escape);
        let up = focused && draft.text.is_empty() && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
        (enter, esc, up)
    });
    if esc {
        draft.reply = None;
    }

    let reply = draft.reply;
    let files: Vec<(String, usize)> = draft.files.iter().map(|f| (f.name.clone(), f.bytes.len())).collect();

    egui::Frame::new().fill(theme::INPUT).corner_radius(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if let Some((rid, ping)) = reply {
            egui::Frame::new().fill(theme::SIDEBAR).corner_radius(egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(14, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let name = state.message(c.id, rid).map(|m| state.display_name(m.author.id, c.guild_id)).unwrap_or_default();
                    ui.label(RichText::new("Replying to ").color(theme::MUTED));
                    ui.label(RichText::new(name).font(theme::font_semibold(14.0)).color(theme::HEADER));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, icon::CLOSE, "Cancel reply").clicked() {
                            uis.drafts.entry(c.id).or_default().reply = None;
                        }
                        let label = RichText::new(if ping { "@ ON" } else { "@ OFF" }).font(theme::font_bold(12.0)).color(if ping { theme::BLURPLE } else { theme::MUTED });
                        if ui.add(egui::Button::new(label).frame(false)).on_hover_text("Mention the author").clicked() {
                            if let Some(r) = &mut uis.drafts.entry(c.id).or_default().reply {
                                r.1 = !r.1;
                            }
                        }
                    });
                });
            });
        }
        if !files.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                for (i, (name, size)) in files.iter().enumerate() {
                    egui::Frame::new().fill(theme::SIDEBAR).corner_radius(6).inner_margin(8).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::FILE).font(theme::icon_font(18.0)).color(theme::BLURPLE));
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(RichText::new(name).size(13.0)).truncate());
                                ui.label(RichText::new(widgets::human_size(*size as u64)).size(11.0).color(theme::MUTED));
                            });
                            if widgets::icon_button(ui, icon::DELETE, "Remove").clicked() {
                                let d = uis.drafts.entry(c.id).or_default();
                                if i < d.files.len() {
                                    d.files.remove(i);
                                }
                            }
                        });
                    });
                }
            });
        }

        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if perms & perm::ATTACH_FILES != 0 && widgets::icon_button(ui, icon::ADD, "Upload a file").clicked() {
                actions.push(Action::PickFiles(c.id));
            }
            let emoji_w = 36.0;
            let draft = uis.drafts.entry(c.id).or_default();
            let hint = match c.kind {
                ChannelType::Dm | ChannelType::GroupDm => format!("Message @{}", c.label()),
                _ => format!("Message #{}", c.label()),
            };
            let before = draft.text.len();
            let r = ui.add(
                egui::TextEdit::multiline(&mut draft.text)
                    .id(edit_id)
                    .hint_text(RichText::new(hint).color(theme::FAINT))
                    .font(theme::font(15.5))
                    .frame(egui::Frame::NONE)
                    .desired_rows(1)
                    .desired_width(ui.available_width() - emoji_w - 8.0)
                    .margin(egui::Margin::symmetric(4, 12)),
            );
            if draft.text.len() != before && !draft.text.is_empty() {
                actions.push(Action::Typing(c.id));
            }
            if uis.focus_composer {
                r.request_focus();
                uis.focus_composer = false;
            }
            let er = widgets::icon_button(ui, icon::EMOJI, "Emoji");
            if er.clicked() {
                uis.emoji_picker = Some(PickerTarget::Composer { channel: c.id, at: er.rect.left_top() });
            }
        });
    });

    if enter {
        actions.push(Action::Send(c.id));
    }
    if up {
        let me = state.me.as_ref().map(|m| m.id);
        if let Some(last) = state.messages(c.id).and_then(|h| h.list.values().rev().find(|m| Some(m.author.id) == me && m.pending.is_none() && m.is_regular())) {
            uis.editing = Some((last.id, last.content.clone()));
        }
    }
}

fn pins_panel(ui: &mut Ui, s: &Session, uis: &mut UiState, c: &Channel, pins: Option<&[Message]>, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Pinned Messages").font(theme::font_semibold(16.0)).color(theme::HEADER));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, icon::CLOSE, "Close").clicked() {
                actions.push(Action::ShowPins(c.id));
            }
        });
    });
    ui.separator();
    let Some(pins) = pins else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    if pins.is_empty() {
        ui.label(RichText::new("This channel doesn't have any pinned messages… yet.").color(theme::MUTED));
        return;
    }
    let perms = s.state.permissions(c.id);
    egui::ScrollArea::vertical().id_salt(("pins-list", c.id)).show(ui, |ui| {
        let mut cx = Ctx { state: &s.state, uis, actions, channel: c.id, guild: c.guild_id, perms, highlight: None, read_only: true };
        for m in pins {
            egui::Frame::new().fill(theme::CHAT).corner_radius(6).inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
                message::show(ui, m, false, &mut cx);
            });
            ui.add_space(6.0);
        }
    });
}
