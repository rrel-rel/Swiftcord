//! The chat area: header, a virtualized message list that stays pinned to the bottom and keeps its place
//! when older history loads above, typing indicator and composer.

use std::collections::HashMap;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::mentions::{self, Token};
use swiftcord_core::models::{Channel, ChannelType, Message};
use swiftcord_core::permissions::perm;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::emoji::{Catalog, PickerTarget};
use crate::ui::message::{self, Ctx};
use crate::ui::suggest::{self, Suggestion};
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
    /// A message to scroll to, and how many frames we have tried.
    jump: Option<(Snowflake, u8)>,
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
        self.jump = Some((id, 0));
    }
}

struct Row<'a> {
    id: Snowflake,
    msg: Option<&'a Message>,
    compact: bool,
    divider: bool,
    /// The first unread message: the red NEW line goes above it.
    new_line: bool,
}

pub fn show(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &Catalog, actions: &mut Vec<Action>) {
    let state = &s.state;
    let Some(channel) = uis.channel().and_then(|c| state.channel(c)) else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(if s.ready { "No channel selected" } else { "Connecting…" }).color(theme::MUTED()));
        });
        return;
    };
    let perms = state.permissions(channel.id);

    // A voice channel has its own screen (its chat goes alongside it there).
    if channel.is_voice() {
        crate::ui::voice_room::show(ui, s, uis, catalog, channel, actions);
        return;
    }

    let in_call = uis.voice.as_ref().is_some_and(|v| v.channel == channel.id);
    header(ui, s, channel, in_call, &mut uis.search_box, actions);
    if channel.is_private() {
        crate::ui::call::panel(ui, s, uis, channel, actions);
    }

    // Forums list posts (threads) instead of messages.
    if channel.is_forum() {
        if !uis.hide_members {
            egui::Panel::right(("members", channel.id)).exact_size(crate::ui::members::WIDTH).frame(egui::Frame::new().fill(theme::SIDEBAR())).show(ui, |ui| {
                crate::ui::members::panel(ui, s, channel, actions);
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT())).show(ui, |ui| {
            crate::ui::threads::forum(ui, s, uis, channel, actions);
        });
        return;
    }

    egui::Panel::bottom(("composer", channel.id)).frame(egui::Frame::new().fill(theme::CHAT()).inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 20 })).show(ui, |ui| {
        typing_line(ui, s, channel);
        composer(ui, s, uis, catalog, channel, perms, actions);
    });

    let searching = uis.search.as_ref().is_some_and(|q| q.applies_to(channel.id, channel.guild_id));
    let pins = uis.pins.as_ref().filter(|(c, _)| *c == channel.id).map(|(_, p)| p.clone());
    if uis.inbox.is_some() {
        egui::Panel::right("inbox").exact_size(400.0).frame(egui::Frame::new().fill(theme::SIDEBAR()).inner_margin(8)).show(ui, |ui| {
            crate::ui::inbox::panel(ui, s, uis, actions);
        });
    } else if searching {
        egui::Panel::right("search").exact_size(400.0).frame(egui::Frame::new().fill(theme::SIDEBAR()).inner_margin(8)).show(ui, |ui| {
            crate::ui::search::panel(ui, s, uis, actions);
        });
    } else if let Some(pins) = pins {
        egui::Panel::right(("pins", channel.id)).exact_size(380.0).frame(egui::Frame::new().fill(theme::SIDEBAR()).inner_margin(8)).show(ui, |ui| {
            pins_panel(ui, s, uis, channel, pins.as_deref(), actions);
        });
    } else if uis.threads_panel == Some(channel.id) {
        egui::Panel::right(("threads", channel.id)).exact_size(340.0).frame(egui::Frame::new().fill(theme::SIDEBAR()).inner_margin(8)).show(ui, |ui| {
            crate::ui::threads::panel(ui, s, uis, channel, actions);
        });
    } else if channel.guild_id.is_some() && !channel.is_voice() && !uis.hide_members {
        egui::Panel::right(("members", channel.id)).exact_size(crate::ui::members::WIDTH).frame(egui::Frame::new().fill(theme::SIDEBAR())).show(ui, |ui| {
            crate::ui::members::panel(ui, s, channel, actions);
        });
    }

    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT())).show(ui, |ui| {
        message_list(ui, s, uis, channel, perms, actions);
    });
}

/// Just a channel's messages and composer, for a column beside something else (a voice channel's screen).
pub fn column(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &Catalog, channel: &Channel, actions: &mut Vec<Action>) {
    let perms = s.state.permissions(channel.id);
    egui::Panel::bottom(("composer", channel.id)).frame(egui::Frame::new().fill(theme::CHAT()).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 16 })).show(ui, |ui| {
        typing_line(ui, s, channel);
        composer(ui, s, uis, catalog, channel, perms, actions);
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHAT())).show(ui, |ui| {
        message_list(ui, s, uis, channel, perms, actions);
    });
}

fn header(ui: &mut Ui, s: &Session, c: &Channel, in_call: bool, search_box: &mut String, actions: &mut Vec<Action>) {
    egui::Panel::top(("chat-header", c.id)).frame(egui::Frame::new().fill(theme::CHAT()).inner_margin(egui::Margin::symmetric(16, 0))).show(ui, |ui| {
        ui.set_height(48.0);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            // In a thread: "# parent › thread", the parent leading back to its channel.
            let parent = c.parent_id.filter(|_| c.is_thread()).and_then(|p| s.state.channel(p));
            if let Some(p) = parent {
                ui.label(RichText::new(if p.is_forum() { "💬" } else { "#" }).font(theme::font(20.0)).color(theme::FAINT()));
                let r = ui.add(egui::Label::new(RichText::new(p.label()).font(theme::font_semibold(15.0)).color(theme::MUTED())).sense(Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    actions.push(Action::SelectChannel(p.id));
                }
                ui.label(RichText::new(icon::CHEVRON_RIGHT).font(theme::icon_font(10.0)).color(theme::FAINT()));
            }
            let glyph = match c.kind {
                ChannelType::Dm => "@",
                ChannelType::GroupDm => icon::PEOPLE,
                _ if c.is_voice() => icon::SPEAKER,
                _ if c.is_thread() => icon::CHAT,
                _ if c.is_forum() => "💬",
                _ => "#",
            };
            let font = if glyph == "#" || glyph == "@" { theme::font(22.0) } else if glyph == "💬" { theme::font(18.0) } else { theme::icon_font(18.0) };
            ui.label(RichText::new(glyph).font(font).color(theme::FAINT()));
            ui.label(RichText::new(c.label()).font(theme::font_semibold(16.0)).color(theme::HEADER()));
            if c.is_archived() {
                ui.label(RichText::new("Archived").size(12.0).color(theme::FAINT()));
            }
            if c.kind == ChannelType::Dm {
                if let Some(u) = c.recipients.first() {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    widgets::status_dot(ui.painter(), rect.center(), 5.0, s.state.presence(u.id), s.state.on_mobile(u.id), theme::CHAT());
                }
            }
            if let Some(topic) = c.topic.as_deref().filter(|t| !t.is_empty()) {
                ui.add(egui::Separator::default().vertical().spacing(16.0));
                let first_line = topic.lines().next().unwrap_or_default();
                // Leave room for the buttons and search box on the right, which are laid out after.
                let icons = [true, c.guild_id.is_some(), !c.is_forum(), c.has_threads() && !c.is_forum(), c.is_private()].iter().filter(|b| **b).count();
                let right = 230.0 + 34.0 * icons as f32 + if c.is_thread() { 110.0 } else { 0.0 };
                let room = (ui.available_width() - right).max(40.0);
                ui.scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(egui::Label::new(RichText::new(first_line).size(13.5).color(theme::MUTED())).truncate()).on_hover_text(topic);
                });
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::INBOX, "Inbox").clicked() {
                    actions.push(Action::ShowInbox);
                }
                search_field(ui, c, search_box, actions);
                if c.guild_id.is_some() && widgets::icon_button(ui, icon::PEOPLE, "Member List").clicked() {
                    actions.push(Action::ToggleMemberList);
                }
                if !c.is_forum() && widgets::icon_button(ui, icon::PIN, "Pinned Messages").clicked() {
                    actions.push(Action::ShowPins(c.id));
                }
                if c.has_threads() && !c.is_forum() && widgets::icon_button(ui, icon::CHAT, "Threads").clicked() {
                    actions.push(Action::ShowThreads(c.id));
                }
                if c.is_thread() {
                    let (label, join) = if c.member.is_some() { ("Leave Thread", false) } else { ("Join Thread", true) };
                    if ui.button(label).clicked() {
                        actions.push(Action::SetThreadJoined(c.id, join));
                    }
                }
                if c.is_private() && !in_call && s.state.call(c.id).is_none() && widgets::icon_button(ui, icon::PHONE, "Start Voice Call").clicked() {
                    actions.push(Action::JoinVoice { guild: None, channel: c.id });
                }
            });
        });
    });
    // Discord's subtle header shadow.
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::RAIL()));
}

fn message_list(ui: &mut Ui, s: &Session, uis: &mut UiState, channel: &Channel, perms: u64, actions: &mut Vec<Action>) {
    let state = &s.state;
    let Some(history) = state.messages(channel.id) else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    if perms & perm::READ_MESSAGE_HISTORY == 0 && channel.guild_id.is_some() {
        ui.centered_and_justified(|ui| ui.label(RichText::new("You don't have permission to read this channel's history.").color(theme::MUTED())));
        return;
    }

    let msgs: Vec<&Message> = history.list.values().collect();
    // The NEW line goes above the first message from someone else after where we'd read up to. If that's older
    // than the loaded history, there's no place for it yet.
    let me = state.me.as_ref().map(|u| u.id);
    let read_up_to = uis.unread_marker.get(&channel.id).copied();
    let unread: Vec<&Message> = read_up_to.map_or_else(Vec::new, |r| msgs.iter().copied().filter(|m| m.id > r && Some(m.author.id) != me).collect());
    let placed = read_up_to.is_some_and(|r| history.reached_start || msgs.first().is_some_and(|m| m.id <= r));
    let first_unread = unread.first().map(|m| m.id).filter(|_| placed);
    let mut rows = Vec::with_capacity(msgs.len() + 1);
    rows.push(Row { id: TOP_ROW, msg: None, compact: false, divider: false, new_line: false });
    for (i, m) in msgs.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| msgs[p]);
        let divider = prev.is_none_or(|p| !message::same_day(p.timestamp(), m.timestamp()));
        let new_line = first_unread == Some(m.id);
        rows.push(Row { id: m.id, msg: Some(m), compact: !divider && !new_line && message::is_compact(m, prev), divider, new_line });
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
            Some(m) => message::estimate_height(m, r.compact) + if r.divider || r.new_line { 34.0 } else { 0.0 },
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
    // A jump is retried for a few frames: the scroll area clamps offsets to the previous frame's size, so the
    // first attempt can fall short while row heights are still being measured.
    if let Some((target, tries)) = view.jump {
        match rows.iter().position(|r| r.id == target) {
            Some(i) => {
                scroll_to = Some((tops[i] - ui.available_height() / 3.0).max(0.0));
                view.at_bottom = false;
                if tries == 0 {
                    view.highlight = Some((target, Instant::now()));
                }
                view.jump = (tries < 10).then_some((target, tries + 1));
            }
            None => {
                view.jump = None;
                uis.banner = Some(("That message isn't loaded.".into(), Instant::now()));
            }
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

    let mut area = egui::ScrollArea::vertical().id_salt(("history", channel.id)).auto_shrink(false);
    // The scroll area clamps its stored offset, but the viewport it hands back uses the requested one, so an
    // offset past the end would hide every row. Clamp it here.
    let max_offset = (total - ui.available_height()).max(0.0);
    if let Some(y) = scroll_to {
        area = area.vertical_scroll_offset(y.clamp(0.0, max_offset));
    } else if view.at_bottom {
        area = area.vertical_scroll_offset(max_offset);
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
                            if r.divider || r.new_line {
                                day_divider(ui, m, r.divider, r.new_line);
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
    let at_end = out.state.clamped_offset().y >= max_scroll - 4.0;
    if view.at_bottom && scroll_to.is_none() {
        // Following the bottom: the offset can lag while sizes settle, so only the user scrolling or dragging
        // in the list stops it.
        let user_moved = ui.input(|i| {
            let over = i.pointer.hover_pos().is_some_and(|p| out.inner_rect.contains(p));
            over && (i.is_scrolling() || i.pointer.primary_down())
        });
        view.at_bottom = !user_moved || at_end;
    } else if view.jump.is_none() {
        view.at_bottom = at_end;
    }
    if view.jump.is_some() || (view.at_bottom && !at_end) {
        ui.ctx().request_repaint();
    }
    view.anchor = new_anchor;

    if viewport.min.y < 800.0 && !history.reached_start && history.loaded {
        actions.push(Action::LoadOlder(channel.id));
    }
    // After jumping to an old message, newer history loads as you approach the bottom.
    if history.has_newer && viewport.max.y > total - 800.0 {
        actions.push(Action::LoadNewer(channel.id));
    }

    // A "jump to present" pill when scrolled far up or viewing older history.
    if history.has_newer || (!view.at_bottom && max_scroll - out.state.clamped_offset().y > 1500.0) {
        let rect = Rect::from_center_size(out.inner_rect.center_bottom() - Vec2::new(0.0, 24.0), Vec2::new(150.0, 30.0));
        let r = ui.put(rect, egui::Button::new(RichText::new("Jump to present").color(Color32::WHITE)).fill(theme::BLURPLE()).corner_radius(15));
        if r.clicked() {
            actions.push(Action::JumpToPresent(channel.id));
            ui.ctx().request_repaint();
        }
    }

    // While the NEW line is above what's shown (or not loaded yet): "N new messages since …" across the top.
    // Clicking it goes to the line; Mark As Read drops it.
    if let (Some(read), false) = (read_up_to, unread.is_empty()) {
        let line_above = match first_unread.and_then(|id| rows.iter().position(|r| r.id == id)) {
            Some(i) => tops[i] < viewport.min.y,
            None => true,
        };
        if line_above {
            let since = if placed { unread[0].timestamp() } else { read.created_at() };
            let local = since.with_timezone(&chrono::Local);
            let when = if local.date_naive() == chrono::Local::now().date_naive() {
                local.format("%-I:%M %p").to_string()
            } else {
                local.format("%-I:%M %p on %B %-d, %Y").to_string()
            };
            let count = format!("{}{}", unread.len(), if placed { "" } else { "+" });
            let noun = if unread.len() == 1 && placed { "message" } else { "messages" };
            let rect = Rect::from_min_size(out.inner_rect.left_top() + Vec2::new(16.0, 0.0), Vec2::new(out.inner_rect.width() - 32.0, 26.0));
            let bar = ui.interact(rect, ui.scope_id().with(("new-messages", channel.id)), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, egui::CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }, if bar.hovered() { theme::BLURPLE().gamma_multiply(0.85) } else { theme::BLURPLE() });
            p.text(rect.left_center() + Vec2::new(12.0, 0.0), egui::Align2::LEFT_CENTER, format!("{count} new {noun} since {when}"), theme::font_semibold(13.0), Color32::WHITE);
            let mark = Rect::from_min_max(egui::pos2(rect.right() - 130.0, rect.top()), rect.right_bottom());
            let mark_r = ui.interact(mark, ui.scope_id().with(("mark-read", channel.id)), Sense::click());
            let label = if mark_r.hovered() { RichText::new("Mark As Read").underline() } else { RichText::new("Mark As Read") };
            ui.put(mark.shrink2(Vec2::new(10.0, 0.0)), egui::Label::new(label.font(theme::font_semibold(13.0)).color(Color32::WHITE)).selectable(false));
            if mark_r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                uis.unread_marker.remove(&channel.id);
            } else if bar.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                actions.push(Action::JumpTo { channel: channel.id, message: Some(first_unread.unwrap_or(read)) });
            }
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
                    ui.painter().circle_filled(rect.center(), 34.0, theme::SELECTED());
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "#", theme::font(42.0), theme::HEADER());
                }
            }
            let title = if c.is_private() || c.is_thread() { c.label() } else { format!("Welcome to #{}!", c.label()) };
            ui.label(RichText::new(title).font(theme::font_bold(28.0)).color(theme::HEADER()));
            let sub = if c.is_private() {
                format!("This is the beginning of your direct message history with {}.", c.label())
            } else if c.is_thread() {
                match c.owner_id {
                    Some(o) => format!("Started by {}.", s.state.display_name(o, c.guild_id)),
                    None => "This is the start of the thread.".to_owned(),
                }
            } else {
                format!("This is the start of the #{} channel.", c.label())
            };
            ui.label(RichText::new(sub).color(theme::MUTED()));
            let _ = s;
        });
    });
    ui.add_space(8.0);
}

/// The line above a message starting a new day (with the date) and/or the first unread one (red, with a NEW
/// tag at the end), as Discord draws them.
fn day_divider(ui: &mut Ui, m: &Message, date: bool, new: bool) {
    ui.add_space(14.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
    let (line, text_color) = if new { (theme::RED(), theme::RED()) } else { (theme::SELECTED(), theme::FAINT()) };
    let stroke = egui::Stroke::new(1.0, line);
    let c = rect.center();
    let mut right = rect.right() - 16.0;
    if new {
        // The NEW tag: a red label with a pointed left end, flush with the line's right end.
        let galley = ui.painter().layout_no_wrap("NEW".into(), theme::font_bold(10.0), Color32::WHITE);
        let tag = Rect::from_min_max(egui::pos2(right - galley.size().x - 8.0, c.y - 7.0), egui::pos2(right, c.y + 7.0));
        let point = egui::pos2(tag.left() - 5.0, c.y);
        ui.painter().add(egui::Shape::convex_polygon(vec![point, tag.left_top(), tag.right_top(), tag.right_bottom(), tag.left_bottom()], theme::RED(), egui::Stroke::NONE));
        ui.painter().galley(egui::pos2(tag.left() + 4.0, c.y - galley.size().y / 2.0), galley, Color32::WHITE);
        right = point.x;
    }
    if date {
        let text = m.timestamp().with_timezone(&chrono::Local).format("%B %-d, %Y").to_string();
        let galley = ui.painter().layout_no_wrap(text, theme::font_semibold(12.0), text_color);
        let half = galley.size().x / 2.0 + 8.0;
        ui.painter().hline((rect.left() + 16.0)..=(c.x - half), c.y, stroke);
        ui.painter().hline((c.x + half)..=right, c.y, stroke);
        ui.painter().galley(c - galley.size() / 2.0, galley, text_color);
    } else {
        ui.painter().hline((rect.left() + 16.0)..=right, c.y, stroke);
    }
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
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, text, theme::font_semibold(12.5), theme::TEXT());
        // Typing indicators expire on a timer, so keep repainting while one is shown.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
}

/// Byte offset of a character index.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// Takes a picked suggestion: a slash command opens its form; anything else replaces the token being typed.
fn choose(ctx: &egui::Context, edit_id: egui::Id, draft: &mut crate::app::Draft, token: Option<&Token>, item: &Suggestion, commands: Option<&swiftcord_core::commands::Index>) {
    if let Suggestion::Command { index, app, .. } = item {
        if let Some(cmd) = commands.and_then(|i| i.commands.get(*index)) {
            draft.command = Some(crate::ui::commands::CommandDraft::new(cmd.clone(), app.clone()));
            draft.text.clear();
            draft.caret = 0;
            draft.selected = 0;
        }
        return;
    }
    if let Some(t) = token {
        accept(ctx, edit_id, draft, t, item);
    }
}

/// Replaces the token being typed with a picked suggestion and moves the caret after it.
fn accept(ctx: &egui::Context, edit_id: egui::Id, draft: &mut crate::app::Draft, token: &Token, item: &Suggestion) {
    let caret = byte_at(&draft.text, draft.caret);
    let (text, raw) = item.insertion();
    let inserted = format!("{text} ");
    draft.text.replace_range(token.start..caret, &inserted);
    if let Some(raw) = raw {
        draft.mentions.retain(|(d, _)| *d != text);
        draft.mentions.push((text, raw));
    }
    draft.caret = draft.text[..token.start].chars().count() + inserted.chars().count();
    draft.selected = 0;
    if let Some(mut st) = egui::TextEdit::load_state(ctx, edit_id) {
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(draft.caret))));
        st.store(ctx, edit_id);
    }
}

#[allow(clippy::too_many_arguments)]
fn composer(ui: &mut Ui, s: &Session, uis: &mut UiState, catalog: &Catalog, c: &Channel, perms: u64, actions: &mut Vec<Action>) {
    let state = &s.state;
    if perms & perm::SEND_MESSAGES == 0 {
        egui::Frame::new().fill(theme::INPUT()).corner_radius(8).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("You do not have permission to send messages in this channel.").color(theme::FAINT()));
        });
        return;
    }
    let edit_id = egui::Id::unique(("composer", c.id));
    // Clicking a suggestion takes focus from the text box first; the list stays while it's pointed at.
    let focused = ui.ctx().memory(|m| m.has_focus(edit_id)) || suggest::pointed_at(ui.ctx());
    // egui drops focus as soon as Escape is pressed, before we look: it was ours if we had it last frame.
    let had_focus = focused || ui.ctx().memory(|m| m.had_focus_last_frame(edit_id));
    let draft = uis.drafts.entry(c.id).or_default();

    // Autocomplete for the @/#/: token at the caret.
    let at_caret = focused.then(|| mentions::token_at(&draft.text, byte_at(&draft.text, draft.caret))).flatten();
    // Escape hides suggestions for that token only; they come back for the next one.
    if at_caret.as_ref().map(|t| t.start) != draft.dismissed {
        draft.dismissed = None;
    }
    let token = at_caret.filter(|t| draft.dismissed != Some(t.start));
    let mut items = token.as_ref().map(|t| suggest::suggestions(s, catalog, c, t, perms)).unwrap_or_default();
    // Slash commands: `/` at the very start lists the apps' commands (fetched the first time).
    let command_key = c.guild_id.unwrap_or(c.id);
    let slash = focused
        && perms & perm::USE_APPLICATION_COMMANDS != 0
        && draft.command.is_none()
        && draft.text.starts_with('/')
        && !draft.text.contains(char::is_whitespace);
    let index = match uis.commands.get(&command_key) {
        Some(crate::ui::commands::Loaded::Ready(i)) => Some(i),
        _ => None,
    };
    if slash {
        match (uis.commands.get(&command_key), index) {
            (None, _) => actions.push(Action::LoadCommands { guild: c.guild_id, channel: c.id }),
            (_, Some(index)) => {
                items = crate::ui::commands::matching(index, &draft.text[1..], 8)
                    .into_iter()
                    .map(|(index, name, description, app)| Suggestion::Command { index, name, description, app })
                    .collect();
            }
            _ => {}
        }
    }
    draft.selected = draft.selected.min(items.len().saturating_sub(1));

    // Server members aren't all known locally, so ask Discord as the user types (once they pause).
    if let (Some(t), Some(g)) = (token.as_ref().filter(|t| t.trigger == '@' && !t.query.is_empty()), c.guild_id) {
        let changed = uis.member_query.as_ref().is_none_or(|(qg, q, ..)| *qg != g || *q != t.query);
        if changed {
            uis.member_query = Some((g, t.query.clone(), Instant::now(), false));
        }
        if let Some((g, q, at, sent)) = &mut uis.member_query {
            if !*sent && at.elapsed().as_millis() > 250 {
                *sent = true;
                actions.push(Action::SearchMembers { guild: *g, query: q.clone() });
            } else if !*sent {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(260));
            }
        }
    }

    // Keys handled before the text box sees them. With suggestions open: arrows choose, Tab/Enter insert,
    // Escape closes. Otherwise Enter sends (Shift+Enter is a newline), Escape cancels a reply and Up edits
    // your last message.
    let suggesting = !items.is_empty();
    let (enter, esc, up, down, tab) = ui.input_mut(|i| {
        let enter = focused && !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        let esc = had_focus && i.key_pressed(egui::Key::Escape);
        let up = focused && (suggesting || draft.text.is_empty()) && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
        let down = focused && suggesting && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
        // Tab is always ours while typing (no tab characters in messages); it only completes suggestions.
        let tab = focused && i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) && suggesting;
        (enter, esc, up, down, tab)
    });
    if suggesting {
        let n = items.len();
        if up {
            draft.selected = (draft.selected + n - 1) % n;
        }
        if down {
            draft.selected = (draft.selected + 1) % n;
        }
        if enter || tab {
            let item = items[draft.selected].clone();
            choose(ui.ctx(), edit_id, draft, token.as_ref(), &item, index);
        }
        if esc {
            if slash {
                // The command list goes with what was typed for it.
                draft.text.clear();
            }
            draft.dismissed = token.as_ref().map(|t| t.start);
        }
    } else if esc {
        // Escape cancels a reply; with none, it marks the channel read (drops the NEW line), as in Discord.
        if draft.reply.take().is_none() {
            uis.unread_marker.remove(&c.id);
        }
    }
    if esc {
        // Typing carries on in the text box.
        ui.ctx().memory_mut(|m| m.request_focus(edit_id));
    }
    let (enter, up) = if suggesting { (false, false) } else { (enter, up) };

    let reply = draft.reply;
    let files: Vec<(String, usize)> = draft.files.iter().map(|f| (f.name.clone(), f.bytes.len())).collect();

    let frame = egui::Frame::new().fill(theme::INPUT()).corner_radius(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if let Some((rid, ping)) = reply {
            egui::Frame::new().fill(theme::SIDEBAR()).corner_radius(egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(14, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let name = state.message(c.id, rid).map(|m| state.display_name(m.author.id, c.guild_id)).unwrap_or_default();
                    ui.label(RichText::new("Replying to ").color(theme::MUTED()));
                    ui.label(RichText::new(name).font(theme::font_semibold(14.0)).color(theme::HEADER()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, icon::CLOSE, "Cancel reply").clicked() {
                            uis.drafts.entry(c.id).or_default().reply = None;
                        }
                        let label = RichText::new(if ping { "@ ON" } else { "@ OFF" }).font(theme::font_bold(12.0)).color(if ping { theme::BLURPLE() } else { theme::MUTED() });
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
                    egui::Frame::new().fill(theme::SIDEBAR()).corner_radius(6).inner_margin(8).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::FILE).font(theme::icon_font(18.0)).color(theme::BLURPLE()));
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(RichText::new(name).size(13.0)).truncate());
                                ui.label(RichText::new(widgets::human_size(*size as u64)).size(11.0).color(theme::MUTED()));
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

        // A slash command being filled in takes the text box's place.
        if let Some(cmd) = uis.drafts.entry(c.id).or_default().command.as_mut() {
            match crate::ui::commands::form(ui, state, c, cmd) {
                crate::ui::commands::FormResult::Run => actions.push(Action::RunCommand(c.id)),
                crate::ui::commands::FormResult::Cancel => {
                    uis.drafts.entry(c.id).or_default().command = None;
                    uis.focus_composer = true;
                }
                crate::ui::commands::FormResult::Nothing => {}
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if perms & perm::ATTACH_FILES != 0 && widgets::icon_button(ui, icon::ADD, "Upload a file").clicked() {
                actions.push(Action::PickFiles(c.id));
            }
            let emoji_w = 108.0;
            let draft = uis.drafts.entry(c.id).or_default();
            let hint = match c.kind {
                ChannelType::Dm | ChannelType::GroupDm => format!("Message @{}", c.label()),
                _ => format!("Message #{}", c.label()),
            };
            let before = draft.text.len();
            let out = egui::TextEdit::multiline(&mut draft.text)
                .id(edit_id)
                .hint_text(RichText::new(hint).color(theme::FAINT()))
                .font(theme::font(15.5))
                .frame(egui::Frame::NONE)
                .desired_rows(1)
                .desired_width(ui.available_width() - emoji_w - 8.0)
                .margin(egui::Margin::symmetric(4, 12))
                // Tab completes suggestions rather than moving focus.
                .lock_focus(true)
                .show(ui);
            if let Some(range) = out.cursor_range {
                if draft.caret != range.primary.index.0 {
                    // Suggestions are worked out from the caret before drawing, so take another look now.
                    draft.caret = range.primary.index.0;
                    ui.ctx().request_repaint();
                }
            }
            if draft.text.is_empty() {
                draft.mentions.clear();
            }
            let r = out.response.response;
            if draft.text.len() != before && !draft.text.is_empty() {
                actions.push(Action::Typing(c.id));
            }
            // An image on the clipboard (a screenshot, say) is pasted as an attachment.
            if r.has_focus() {
                let pasted = ui.input(|i| i.events.iter().find_map(|e| if let egui::Event::PasteImage(image) = e { Some(image.clone()) } else { None }));
                if let Some(image) = pasted {
                    if perms & perm::ATTACH_FILES != 0 {
                        actions.push(Action::PasteImage { channel: c.id, image });
                    } else {
                        actions.push(Action::Notice("You can't upload files in this channel".into()));
                    }
                }
            }
            if uis.focus_composer {
                r.request_focus();
                uis.focus_composer = false;
            }
            let gr = ui.add(egui::Button::new(RichText::new("GIF").font(theme::font_bold(11.5)).color(theme::MUTED())).frame_when_inactive(false).min_size(Vec2::new(34.0, 28.0)));
            let gif_at = gr.rect.right_top();
            if gr.on_hover_text("GIFs").clicked() {
                actions.push(Action::OpenGifPicker { channel: c.id, at: gif_at });
            }
            let sr = sticker_button(ui);
            let sticker_at = sr.rect.right_top();
            if sr.on_hover_text("Stickers").clicked() {
                actions.push(Action::OpenStickerPicker { channel: c.id, at: sticker_at });
            }
            let er = widgets::icon_button(ui, icon::EMOJI, "Emoji");
            if er.clicked() {
                uis.emoji_picker = Some(PickerTarget::Composer { channel: c.id, at: er.rect.left_top() });
            }
        });
    });

    // Suggestions float above the composer.
    if !items.is_empty() && (token.is_some() || slash) {
        let title = match token.as_ref().map(|t| t.trigger) {
            Some('@') => "MEMBERS",
            Some('#') => "TEXT CHANNELS",
            Some(_) => "EMOJI",
            None => "COMMANDS",
        };
        let index = match uis.commands.get(&command_key) {
            Some(crate::ui::commands::Loaded::Ready(i)) => Some(i),
            _ => None,
        };
        let draft = uis.drafts.entry(c.id).or_default();
        if let Some(i) = suggest::popup(ui, frame.response.rect, &items, draft.selected, title) {
            choose(ui.ctx(), edit_id, draft, token.as_ref(), &items[i], index);
            suggest::hidden(ui.ctx());
            ui.ctx().memory_mut(|m| m.request_focus(edit_id));
        }
    } else {
        suggest::hidden(ui.ctx());
    }
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

/// The header's search box: Enter searches the whole server (or this DM); filters like `from:` and `in:`
/// narrow it.
fn search_field(ui: &mut Ui, c: &Channel, text: &mut String, actions: &mut Vec<Action>) {
    egui::Frame::new().fill(theme::RAIL()).corner_radius(4).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::SEARCH).font(theme::icon_font(12.0)).color(theme::MUTED()));
            let hint = if c.guild_id.is_some() { "Search" } else { "Search this chat" };
            let r = ui.add(egui::TextEdit::singleline(text).hint_text(hint).frame(egui::Frame::NONE).desired_width(170.0));
            let r = r.on_hover_text("Search messages. Filters: from:name, in:channel, has:image, mentions:name, before:2024-01-31, after:, during:");
            if r.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                    actions.push(Action::Search { channel: c.id, text: text.trim().to_owned() });
                } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    text.clear();
                }
            }
        });
    });
}

/// The composer's sticker button: a rounded square with a peeled corner (there's no sticker icon in the font).
fn sticker_button(ui: &mut Ui) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(32.0, 28.0), Sense::click());
    let color = if r.hovered() { theme::HEADER() } else { theme::MUTED() };
    if r.hovered() {
        ui.painter().rect_filled(rect, 4, theme::SELECTED());
    }
    let b = Rect::from_center_size(rect.center(), Vec2::splat(17.0));
    let fold = 6.0;
    let stroke = egui::Stroke::new(1.6, color);
    let p = |x: f32, y: f32| egui::pos2(x, y);
    let outline = vec![
        p(b.left() + 3.0, b.top()),
        p(b.right() - 3.0, b.top()),
        p(b.right(), b.top() + 3.0),
        p(b.right(), b.bottom() - fold),
        p(b.right() - fold, b.bottom()),
        p(b.left() + 3.0, b.bottom()),
        p(b.left(), b.bottom() - 3.0),
        p(b.left(), b.top() + 3.0),
    ];
    ui.painter().add(egui::Shape::closed_line(outline, stroke));
    ui.painter().line_segment([p(b.right(), b.bottom() - fold), p(b.right() - fold, b.bottom() - fold)], stroke);
    ui.painter().line_segment([p(b.right() - fold, b.bottom() - fold), p(b.right() - fold, b.bottom())], stroke);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn pins_panel(ui: &mut Ui, s: &Session, uis: &mut UiState, c: &Channel, pins: Option<&[Message]>, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Pinned Messages").font(theme::font_semibold(16.0)).color(theme::HEADER()));
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
        ui.label(RichText::new("This channel doesn't have any pinned messages… yet.").color(theme::MUTED()));
        return;
    }
    let perms = s.state.permissions(c.id);
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt(("pins-list", c.id)).auto_shrink(false).show(ui, |ui| {
        // Messages lay out within the panel rather than at their natural width.
        ui.set_max_width(ui.available_width());
        let mut cx = Ctx { state: &s.state, uis, actions, channel: c.id, guild: c.guild_id, perms, highlight: None, read_only: true };
        for m in pins {
            let r = egui::Frame::new().fill(theme::CHAT()).corner_radius(6).inner_margin(egui::Margin { left: 0, right: 8, top: 0, bottom: 8 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                message::show(ui, m, false, &mut cx);
            });
            // "Jump" in the corner while hovered, as in Discord.
            let rect = r.response.rect;
            if widgets::jump_button(ui, rect, ("pin-jump", m.id)) {
                jump = Some(m.id);
            }
            ui.add_space(6.0);
        }
    });
    if let Some(message) = jump {
        actions.push(Action::JumpTo { channel: c.id, message: Some(message) });
    }
}
