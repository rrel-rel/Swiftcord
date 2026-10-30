//! One message: author header (or compact continuation), reply preview, content, media, embeds, reactions,
//! hover toolbar and context menu.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Local, Utc};
use egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::markdown::{self, Block};
use swiftcord_core::models::{message_type, Attachment, Embed, EmbedMedia, Message, PendingState};
use swiftcord_core::permissions::perm;
use swiftcord_core::proto::FavoriteGif;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::theme::{self, icon};
use crate::ui::emoji::PickerTarget;
use crate::ui::markdown::{self as md, Cx};
use crate::ui::viewer::Viewed;
use crate::ui::widgets;

pub const GUTTER: f32 = 72.0;
const MAX_MEDIA: Vec2 = Vec2::new(420.0, 320.0);
/// Revealed-spoiler keys for gallery items start here, clear of the markdown spoilers counted from 0.
pub(crate) const GALLERY_SPOILERS: u16 = 0x8000;

thread_local! {
    static PARSED: RefCell<HashMap<(Snowflake, u64), Rc<Vec<Block>>>> = RefCell::new(HashMap::new());
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Parsed markdown, cached by message and content so scrolling doesn't re-parse.
pub fn parsed(id: Snowflake, content: &str) -> Rc<Vec<Block>> {
    PARSED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 4000 {
            c.clear();
        }
        c.entry((id, hash(content))).or_insert_with(|| Rc::new(markdown::parse(content))).clone()
    })
}

/// Whether `msg` continues `prev`'s group (no avatar/name header).
pub fn is_compact(msg: &Message, prev: Option<&Message>) -> bool {
    let Some(p) = prev else { return false };
    msg.is_regular()
        && !msg.is_reply()
        && p.is_regular()
        && p.author.id == msg.author.id
        && p.webhook_id == msg.webhook_id
        && (msg.timestamp() - p.timestamp()).num_minutes() < 7
        && same_day(msg.timestamp(), p.timestamp())
}

pub fn same_day(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.with_timezone(&Local).date_naive() == b.with_timezone(&Local).date_naive()
}

fn time_label(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    let today = Local::now().date_naive();
    match (today - local.date_naive()).num_days() {
        0 => format!("Today at {}", local.format("%-I:%M %p")),
        1 => format!("Yesterday at {}", local.format("%-I:%M %p")),
        _ => local.format("%m/%d/%Y %-I:%M %p").to_string(),
    }
}

pub struct Ctx<'a> {
    pub state: &'a State,
    pub uis: &'a mut UiState,
    pub actions: &'a mut Vec<Action>,
    pub channel: Snowflake,
    pub guild: Option<Snowflake>,
    pub perms: u64,
    pub highlight: Option<Snowflake>,
    /// Pinned-messages list: no toolbar.
    pub read_only: bool,
}

/// Draws a message inside `rect` (whose height is the cached estimate) and returns the height used.
pub fn show(ui: &mut Ui, msg: &Message, compact: bool, cx: &mut Ctx) -> f32 {
    // A thread opens with the message it was started from.
    if msg.kind == message_type::THREAD_STARTER_MESSAGE {
        if let Some(original) = &msg.referenced_message {
            return show(ui, original, false, cx);
        }
    }
    let me = cx.state.me.as_ref().map(|m| m.id);
    // Highlighted like Discord, including when you mention yourself.
    let mentioned = me.is_some() && cx.state.mentions_me(msg);
    let row_id = egui::Id::unique(("msg", msg.id, cx.read_only));

    // Background slot, filled once we know the height and hover state.
    let bg = ui.painter().add(egui::Shape::Noop);
    let top = ui.cursor().top();
    let width = ui.available_width();
    let hover_rect_prev = ui.ctx().memory(|m| m.data.get_temp::<Rect>(row_id));
    let hovered = !cx.read_only
        && hover_rect_prev.is_some_and(|r| ui.rect_contains_pointer(r))
        && ui.ctx().memory(|m| m.focused().is_none_or(|_| true));

    // Context menu target registered before the content, so links and buttons inside still get clicks.
    let menu_resp = hover_rect_prev.map(|r| ui.interact(r, row_id.with("ctx"), Sense::click()));
    if let Some(r) = &menu_resp {
        widgets::plain_cursor(r);
    }

    ui.add_space(if compact { 1.0 } else { 14.0 });
    let alpha = match msg.pending {
        Some(PendingState::Sending) => 0.5,
        _ => 1.0,
    };

    if msg.is_reply() {
        reply_preview(ui, msg, cx);
    }

    if !msg.is_regular() {
        system_line(ui, msg, cx);
    } else {
        ui.horizontal_top(|ui| {
            ui.add_space(16.0);
            if compact {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 20.0), Sense::hover());
                if hovered {
                    let t = msg.timestamp().with_timezone(&Local).format("%-I:%M %p").to_string();
                    ui.painter().text(rect.right_center() + Vec2::new(4.0, 1.0), egui::Align2::RIGHT_CENTER, t, theme::font(10.5), theme::FAINT());
                }
            } else {
                let r = widgets::member_avatar(ui, cx.state, &msg.author, cx.guild, 40.0);
                // Webhook "authors" aren't real users and have no profile.
                if msg.webhook_id.is_none() && r.clone().on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.actions.push(crate::ui::profile::open(msg.author.id, cx.guild, &r));
                }
            }
            // The avatar's item spacing counts towards the 16 pt, so the text starts at GUTTER exactly.
            ui.add_space(16.0 - ui.spacing().item_spacing.x);
            ui.vertical(|ui| {
                ui.style_mut().interaction.selectable_labels = true;
                ui.set_max_width(width - GUTTER - 24.0);
                ui.multiply_opacity(alpha);
                if !compact {
                    header(ui, msg, cx);
                }
                body(ui, msg, cx);
            });
        });
    }
    if msg.pending == Some(PendingState::Failed) {
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.colored_label(theme::RED(), "Message failed to send.");
            if ui.link("Retry").clicked() {
                cx.actions.push(Action::Retry { channel: msg.channel_id, nonce: msg.id });
            }
            if ui.link("Delete").clicked() {
                cx.actions.push(Action::Discard { channel: msg.channel_id, nonce: msg.id });
            }
        });
    }
    ui.add_space(2.0);

    let rect = Rect::from_min_max(egui::pos2(ui.max_rect().left(), top), egui::pos2(ui.max_rect().left() + width, ui.cursor().top()));
    ui.ctx().memory_mut(|m| m.data.insert_temp(row_id, rect));
    // The message being replied to stays marked while the reply is written.
    let replying = !cx.read_only && cx.uis.drafts.get(&cx.channel).and_then(|d| d.reply).is_some_and(|(id, _)| id == msg.id);
    let fill = if cx.highlight == Some(msg.id) || replying {
        Some(theme::ACCENT().gamma_multiply(0.18))
    } else if mentioned {
        Some(if hovered { theme::MENTIONED_ROW_HOVER() } else { theme::MENTIONED_ROW() })
    } else if hovered {
        Some(theme::MESSAGE_HOVER())
    } else {
        None
    };
    if let Some(f) = fill {
        ui.painter().set(bg, egui::Shape::rect_filled(rect, 0, f));
    }
    if replying {
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0, theme::ACCENT());
    } else if mentioned {
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0, theme::YELLOW());
    }

    if !cx.read_only && msg.pending.is_none() {
        if hovered {
            toolbar(ui, msg, rect, cx);
        }
        if let Some(r) = menu_resp {
            // Alt-click marks it unread, as in Discord.
            if r.clicked() && ui.input(|i| i.modifiers.alt) {
                cx.actions.push(Action::MarkUnread { channel: msg.channel_id, message: msg.id });
            }
            r.context_menu(|ui| context_menu(ui, msg, cx));
        }
    }
    rect.height()
}

fn header(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    // Wrapped, so a long name in a narrow column (the inbox, search) puts the time on the next line rather
    // than widening the column.
    ui.horizontal_wrapped(|ui| {
        let name = match &msg.member {
            Some(m) if m.nick.as_deref().is_some_and(|n| !n.is_empty()) => m.nick.clone().unwrap(),
            _ if msg.webhook_id.is_some() => msg.author.display_name().to_owned(),
            _ => cx.state.display_name(msg.author.id, cx.guild),
        };
        let color = cx.guild.and_then(|g| cx.state.role_color(g, msg.author.id));
        let r = ui.add(egui::Label::new(RichText::new(name).font(theme::font_semibold(15.5)).color(theme::role_color(color))).selectable(false).sense(Sense::click()));
        if r.clicked() && msg.webhook_id.is_none() {
            cx.actions.push(crate::ui::profile::open(msg.author.id, cx.guild, &r));
        }
        r.on_hover_text(msg.author.tag());
        if msg.author.bot || msg.webhook_id.is_some() {
            let text = if msg.webhook_id.is_some() { " APP " } else { " BOT " };
            ui.label(RichText::new(text).font(theme::font_semibold(10.0)).color(Color32::WHITE).background_color(theme::ACCENT()));
        }
        ui.label(RichText::new(time_label(msg.timestamp())).size(12.0).color(theme::FAINT()))
            .on_hover_text(msg.timestamp().with_timezone(&Local).format("%A, %B %-d, %Y %-I:%M %p").to_string());
    });
}

fn body(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    use swiftcord_core::models::message_flags::{EPHEMERAL, LOADING};
    // An app that deferred its reply: "… is thinking" until the reply replaces this message.
    if msg.flags & LOADING != 0 && msg.content.is_empty() && msg.embeds.is_empty() && msg.components.is_empty() {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.0));
            ui.label(RichText::new(format!("{} is thinking…", msg.author.display_name())).italics().color(theme::MUTED()));
        });
    }
    if msg.flags & EPHEMERAL != 0 {
        // Above an app's private reply.
        ui.label(RichText::new("Only you can see this").size(12.0).italics().color(theme::MUTED()));
    }
    body_content(ui, msg, cx);
}

fn body_content(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    let editing = cx.uis.editing.as_ref().is_some_and(|(id, _)| *id == msg.id);
    if editing {
        edit_box(ui, msg, cx);
    } else if !msg.content.is_empty() && !is_lone_media_link(msg) {
        let blocks = parsed(msg.id, &msg.content);
        let jumbo = markdown::is_emoji_only(&blocks);
        let mut mcx = Cx {
            state: cx.state,
            guild: cx.guild,
            message: msg.id,
            revealed: &mut cx.uis.revealed,
            actions: cx.actions,
            size: 15.5,
            color: theme::TEXT(),
            jumbo,
        };
        md::blocks(ui, &blocks, &mut mcx);
        if msg.edited_timestamp.is_some() {
            ui.label(RichText::new("(edited)").size(10.5).color(theme::FAINT()));
        }
    }

    if msg.is_forward() {
        for snap in &msg.message_snapshots {
            let r = egui::Frame::new().inner_margin(egui::Margin { left: 12, ..Default::default() }).show(ui, |ui| {
                ui.label(RichText::new("↪ Forwarded").italics().size(12.5).color(theme::MUTED()));
                if !snap.message.content.is_empty() {
                    let blocks = parsed(Snowflake(msg.id.0 ^ 1), &snap.message.content);
                    let mut mcx = Cx { state: cx.state, guild: cx.guild, message: msg.id, revealed: &mut cx.uis.revealed, actions: cx.actions, size: 15.5, color: theme::TEXT(), jumbo: false };
                    md::blocks(ui, &blocks, &mut mcx);
                }
                for a in &snap.message.attachments {
                    attachment(ui, a, cx);
                }
                for e in &snap.message.embeds {
                    embed(ui, e, &[], msg.id, cx);
                }
            });
            let rect = r.response.rect;
            ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())), 2, theme::FAINT());
        }
    }

    // Components V2 messages place their attachments themselves (file components, galleries).
    let v2 = msg.flags & swiftcord_core::models::message_flags::IS_COMPONENTS_V2 != 0;
    if !v2 {
        for a in &msg.attachments {
            attachment(ui, a, cx);
        }
    }
    if msg.flags & swiftcord_core::models::message_flags::SUPPRESS_EMBEDS == 0 {
        let mut i = 0;
        while i < msg.embeds.len() {
            let e = &msg.embeds[i];
            let extra = gallery_extras(e, &msg.embeds[i + 1..]);
            embed(ui, e, &extra, msg.id, cx);
            i += 1 + extra.len();
        }
    }
    if !msg.components.is_empty() {
        crate::ui::components::show(ui, &msg.components, msg, cx);
    }
    // The thread this message started (not shown inside that thread itself).
    if let Some(t) = msg.thread.as_deref().filter(|t| t.id != cx.channel) {
        // The state's copy is kept current (name, message count); the message's may be from when it loaded.
        let t = cx.state.channel(t.id).unwrap_or(t);
        crate::ui::threads::chip(ui, t, cx.actions);
    }
    for s in &msg.sticker_items {
        cx.uis.stickers.show(ui, s, 160.0);
    }
    if !msg.reactions.is_empty() {
        reactions(ui, msg, cx);
    }
}

fn edit_box(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    let Some((_, text)) = &mut cx.uis.editing else { return };
    let id = egui::Id::unique(("edit", msg.id));
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    // egui drops focus as soon as Escape is pressed, before we look: it was ours if we had it last frame.
    let had_focus = focused || ui.ctx().memory(|m| m.had_focus_last_frame(id));
    let (enter, esc) = ui.input_mut(|i| {
        let enter = focused && !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
        (enter, had_focus && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    });
    egui::Frame::new().fill(theme::INPUT()).corner_radius(6).inner_margin(8).show(ui, |ui| {
        let r = ui.add(egui::TextEdit::multiline(text).id(id).desired_rows(1).frame(egui::Frame::NONE).desired_width(f32::INFINITY));
        if !focused && !r.lost_focus() {
            r.request_focus();
        }
    });
    ui.label(RichText::new("escape to cancel • enter to save").size(11.5).color(theme::MUTED()));
    if esc {
        cx.uis.editing = None;
        cx.uis.focus_composer = true;
    } else if enter {
        let content = text.trim().to_owned();
        if content.is_empty() {
            // Saving nothing deletes the message, after asking.
            cx.uis.confirm_delete = Some((msg.channel_id, msg.id));
            cx.uis.editing = None;
        } else {
            cx.actions.push(Action::SaveEdit { channel: msg.channel_id, message: msg.id, content });
            cx.uis.focus_composer = true;
        }
    }
}

/// Deleting asks first, unless Shift is held, as in Discord.
fn ask_delete(ui: &Ui, msg: &Message, cx: &mut Ctx) {
    if ui.input(|i| i.modifiers.shift) {
        cx.actions.push(Action::Delete { channel: msg.channel_id, message: msg.id });
    } else {
        cx.uis.confirm_delete = Some((msg.channel_id, msg.id));
    }
}

/// A message's text for a one-line preview: mentions, roles, channels and custom emoji as their names rather
/// than their `<@123>` codes, on one line.
fn plain_preview(content: &str, cx: &Ctx) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let inner = &tail[1..end];
        let id = |s: &str| s.parse::<u64>().ok().map(Snowflake);
        let named = if let Some(u) = inner.strip_prefix("@&").and_then(id) {
            cx.guild.and_then(|g| cx.state.guild(g)).and_then(|g| g.roles.iter().find(|r| r.id == u)).map(|r| format!("@{}", r.name))
        } else if let Some(u) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')).and_then(id) {
            Some(format!("@{}", cx.state.display_name(u, cx.guild)))
        } else if let Some(c) = inner.strip_prefix('#').and_then(id) {
            Some(format!("#{}", cx.state.channel(c).map_or_else(|| "unknown".to_owned(), |c| c.label())))
        } else if inner.starts_with(':') || inner.starts_with("a:") {
            inner.trim_start_matches('a').split(':').nth(1).map(|n| format!(":{n}:"))
        } else {
            None
        };
        match named {
            Some(n) => out.push_str(&n),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out.replace('\n', " ")
}

fn reply_preview(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    ui.horizontal(|ui| {
        ui.add_space(GUTTER - 40.0);
        // The elbow connecting the reply to its parent.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0 - ui.spacing().item_spacing.x, 14.0), Sense::hover());
        let stroke = egui::Stroke::new(2.0, theme::FAINT());
        let p = ui.painter();
        p.line_segment([rect.left_bottom() + Vec2::new(4.0, 2.0), rect.left_top() + Vec2::new(4.0, 7.0)], stroke);
        p.line_segment([rect.left_top() + Vec2::new(4.0, 7.0), rect.right_top() + Vec2::new(-4.0, 7.0)], stroke);
        let reference = msg.message_reference.as_ref().and_then(|r| Some((r.channel_id.unwrap_or(msg.channel_id), r.message_id?)));
        // Lists from elsewhere (recent mentions) leave the replied-to message out: use ours if it's loaded.
        let state = cx.state;
        let referenced = msg.referenced_message.as_deref().or_else(|| reference.and_then(|(c, id)| state.message(c, id)));
        match referenced {
            Some(r) => {
                widgets::member_avatar(ui, cx.state, &r.author, cx.guild, 16.0);
                let color = cx.guild.and_then(|g| cx.state.role_color(g, r.author.id));
                ui.label(RichText::new(cx.state.display_name(r.author.id, cx.guild)).font(theme::font_semibold(13.0)).color(theme::role_color(color)));
                let preview: String = if r.content.is_empty() { "Click to see attachment".into() } else { plain_preview(&r.content, cx) };
                let resp = ui.add(egui::Label::new(RichText::new(preview).size(13.0).color(theme::MUTED())).truncate().selectable(false).sense(Sense::click()));
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.actions.push(Action::JumpTo { channel: cx.channel, message: Some(r.id) });
                }
            }
            // In those lists it's only missing, not necessarily deleted, so offer to go and see it.
            None if cx.read_only && reference.is_some() => {
                let resp = ui.add(egui::Label::new(RichText::new("Click to see original message").italics().size(13.0).color(theme::MUTED())).selectable(false).sense(Sense::click()));
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    let (channel, id) = reference.unwrap_or_default();
                    cx.actions.push(Action::JumpTo { channel, message: Some(id) });
                }
            }
            None => {
                ui.label(RichText::new("Original message was deleted").italics().size(13.0).color(theme::MUTED()));
            }
        }
    });
}

fn system_line(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    let author = cx.state.display_name(msg.author.id, cx.guild);
    let (glyph, text) = match msg.kind {
        message_type::RECIPIENT_ADD => ("→", format!("{author} added {} to the group.", msg.mentions.first().map_or("someone", |u| u.display_name()))),
        message_type::RECIPIENT_REMOVE => ("←", format!("{author} left the group.")),
        message_type::CALL => ("📞", format!("{author} started a call.")),
        message_type::CHANNEL_NAME_CHANGE => ("✎", format!("{author} changed the channel name: {}", msg.content)),
        message_type::CHANNEL_PINNED_MESSAGE => ("📌", format!("{author} pinned a message to this channel.")),
        message_type::USER_JOIN => ("→", format!("{author} joined the server.")),
        message_type::GUILD_BOOST | 9..=11 => ("💎", format!("{author} just boosted the server!")),
        message_type::THREAD_CREATED => ("#", format!("{author} started a thread: {}", msg.content)),
        _ => ("•", if msg.content.is_empty() { format!("{author} did something.") } else { msg.content.clone() }),
    };
    // "Started a thread" points at the thread; clicking opens it.
    let thread = (msg.kind == message_type::THREAD_CREATED).then(|| msg.message_reference.as_ref().and_then(|r| r.channel_id)).flatten();
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.add_sized(Vec2::new(24.0, 18.0), egui::Label::new(RichText::new(glyph).color(theme::GREEN())));
        ui.add_space(GUTTER - 48.0 - 2.0 * ui.spacing().item_spacing.x);
        ui.label(RichText::new(text).color(theme::MUTED()));
        if let Some(t) = thread {
            if ui.link(RichText::new("See Thread ›").size(13.0)).clicked() {
                cx.actions.push(Action::OpenThread(t));
            }
        }
        ui.label(RichText::new(time_label(msg.timestamp())).size(11.5).color(theme::FAINT()));
    });
}

/// A media size limit narrowed to the space there is (a narrow window, or the pins panel).
fn within(ui: &Ui, max: Vec2) -> Vec2 {
    Vec2::new(max.x.min(ui.available_width().max(80.0)), max.y)
}

pub(crate) fn fit(w: Option<u32>, h: Option<u32>, max: Vec2) -> Vec2 {
    let (w, h) = (w.unwrap_or(300).max(1) as f32, h.unwrap_or(200).max(1) as f32);
    let scale = (max.x / w).min(max.y / h).min(1.0);
    Vec2::new((w * scale).round(), (h * scale).round())
}

/// Shows an image, opening it in the viewer when clicked; a video's poster opens `original` (a page) instead.
#[allow(clippy::too_many_arguments)]
fn media(ui: &mut Ui, proxy_url: &str, original: &str, size: Vec2, dims: (Option<u32>, Option<u32>), video: bool, alt: Option<&str>, cx: &mut Ctx) {
    if media_response(ui, proxy_url, size, false, video, alt).clicked() {
        if video {
            crate::platform::open_url(original);
        } else {
            cx.uis.viewer = Some(viewed(proxy_url, original, dims, false));
        }
    }
}

fn viewed(proxy_url: &str, original: &str, (width, height): (Option<u32>, Option<u32>), gif: bool) -> Viewed {
    Viewed { proxy: proxy_url.to_owned(), original: original.to_owned(), width, height, gif }
}

/// A GIF with a favourite star in its corner: clicking the star toggles the favourite, anywhere else opens it
/// in the viewer (`view`), or `original` in the browser for a GIF that's really a video clip.
fn gif_media(ui: &mut Ui, r: egui::Response, original: &str, gif: FavoriteGif, view: Option<Viewed>, cx: &mut Ctx) {
    let over_star = favorite_star(ui, &r, gif, cx);
    if r.clicked() && !over_star {
        match view {
            Some(v) => cx.uis.viewer = Some(v),
            None => crate::platform::open_url(original),
        }
    }
}

/// Draws the star while the GIF is hovered or already a favourite, and toggles it on click. Returns whether the
/// pointer is over the star, so the click isn't also taken as opening the GIF.
fn favorite_star(ui: &Ui, r: &egui::Response, gif: FavoriteGif, cx: &mut Ctx) -> bool {
    let starred = cx.uis.favorite_gifs.iter().any(|f| f.url == gif.url);
    let star = Rect::from_min_size(r.rect.right_top() + Vec2::new(-32.0, 6.0), Vec2::splat(26.0));
    let over = ui.rect_contains_pointer(star) && r.hovered();
    if r.hovered() || starred {
        let p = ui.painter_at(r.rect);
        p.circle_filled(star.center(), 13.0, Color32::from_black_alpha(if over { 200 } else { 140 }));
        let (glyph, color) = if starred { (icon::STAR_FILLED, theme::YELLOW()) } else { (icon::STAR, Color32::WHITE) };
        p.text(star.center(), egui::Align2::CENTER_CENTER, glyph, theme::icon_font(14.0), color);
    }
    if over {
        r.clone().on_hover_text(if starred { "Remove from Favourites" } else { "Add to Favourites" });
        if r.clicked() {
            cx.actions.push(Action::ToggleFavoriteGif(gif));
        }
    }
    over
}

fn media_response(ui: &mut Ui, proxy_url: &str, size: Vec2, gif: bool, video: bool, alt: Option<&str>) -> egui::Response {
    let ppp = ui.ctx().pixels_per_point();
    let (pw, ph) = ((size.x * ppp) as u32, (size.y * ppp) as u32);
    // Animated GIFs must stay GIFs; everything else is fetched resized as WebP to save memory.
    let url = if gif {
        let sep = if proxy_url.contains('?') { '&' } else { '?' };
        format!("{proxy_url}{sep}width={pw}&height={ph}")
    } else {
        swiftcord_core::cdn::media_proxy(proxy_url, pw, ph)
    };
    let r = ui.add(egui::Image::from_uri(url).fit_to_exact_size(size).corner_radius(6).sense(Sense::click()));
    if video {
        ui.painter().circle_filled(r.rect.center(), 24.0, Color32::from_black_alpha(160));
        ui.painter().text(r.rect.center(), egui::Align2::CENTER_CENTER, icon::PLAY, theme::icon_font(20.0), Color32::WHITE);
    }
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    match alt {
        Some(a) if !a.is_empty() => r.on_hover_text(a),
        _ => r,
    }
}

pub(crate) fn attachment(ui: &mut Ui, a: &Attachment, cx: &mut Ctx) {
    if a.url.is_empty() {
        // Still uploading.
        ui.label(RichText::new(format!("{} ({})", a.filename, widgets::human_size(a.size))).color(theme::MUTED()));
        return;
    }
    if a.is_voice_message() {
        let (output, volume) = (cx.uis.videos.output.clone(), cx.uis.videos.volume.unwrap_or(1.0));
        cx.uis.audio.voice_message(ui, a, output, volume);
        return;
    }
    if a.is_audio() {
        let (output, volume) = (cx.uis.videos.output.clone(), cx.uis.videos.volume.unwrap_or(1.0));
        cx.uis.audio.file(ui, a, output, volume);
        return;
    }
    if a.is_video() {
        let size = fit(a.width.or(Some(480)), a.height.or(Some(270)), within(ui, MAX_MEDIA));
        let ppp = ui.ctx().pixels_per_point();
        let poster = a.width.map(|_| swiftcord_core::cdn::media_proxy(&a.proxy_url, (size.x * ppp) as u32, (size.y * ppp) as u32));
        let video = crate::player::Video { url: &a.url, link: &a.url, poster, width: a.width, height: a.height };
        cx.uis.videos.show(ui, &video, size);
        return;
    }
    if a.is_image() {
        let size = fit(a.width, a.height, within(ui, MAX_MEDIA));
        let gif = a.content_type.as_deref() == Some("image/gif");
        let r = media_response(ui, &a.proxy_url, size, gif, a.is_video(), a.description.as_deref());
        let view = viewed(&a.proxy_url, &a.url, (a.width, a.height), gif);
        if gif {
            let fav = FavoriteGif { url: a.url.clone(), src: a.url.clone(), format: 1, width: a.width.unwrap_or(0), height: a.height.unwrap_or(0), order: 0 };
            gif_media(ui, r, &a.url, fav, Some(view), cx);
        } else if r.clicked() {
            cx.uis.viewer = Some(view);
        }
        return;
    }
    egui::Frame::new().fill(theme::EMBED_BG()).corner_radius(6).inner_margin(10).stroke(egui::Stroke::new(1.0, theme::RAIL())).show(ui, |ui| {
        ui.set_width(380.0_f32.min(ui.available_width()));
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::FILE).font(theme::icon_font(28.0)).color(theme::ACCENT()));
            ui.vertical(|ui| {
                if ui.link(&a.filename).clicked() {
                    crate::platform::open_url(&a.url);
                }
                ui.label(RichText::new(widgets::human_size(a.size)).size(12.0).color(theme::MUTED()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::DOWNLOAD, "Download").clicked() {
                    crate::platform::open_url(&a.url);
                }
            });
        });
    });
}

fn embed_text(ui: &mut Ui, e: &Embed, msg_id: Snowflake, cx: &mut Ctx) {
    if let Some(p) = e.provider.as_ref().filter(|p| !p.name.is_empty()) {
        ui.label(RichText::new(&p.name).size(12.0).color(theme::MUTED()));
    }
    if let Some(a) = e.author.as_ref().filter(|a| !a.name.is_empty()) {
        ui.horizontal(|ui| {
            if let Some(icon) = &a.icon_url {
                widgets::avatar(ui, icon, 20.0);
            }
            // Wrapped: a long author line in a narrow column mustn't widen it.
            let r = ui.add(egui::Label::new(RichText::new(&a.name).font(theme::font_semibold(13.5)).color(theme::HEADER())).wrap().selectable(false).sense(Sense::click()));
            if let Some(u) = &a.url {
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.actions.push(Action::OpenLink(u.clone()));
                }
            }
        });
    }
    if let Some(t) = &e.title {
        let rt = RichText::new(t).font(theme::font_semibold(15.5));
        match &e.url {
            Some(u) => {
                if ui.add(egui::Label::new(rt.color(theme::LINK())).selectable(false).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.actions.push(Action::OpenLink(u.clone()));
                }
            }
            None => {
                ui.label(rt.color(theme::HEADER()));
            }
        }
    }
    let mut mcx = Cx { state: cx.state, guild: cx.guild, message: msg_id, revealed: &mut cx.uis.revealed, actions: cx.actions, size: 14.0, color: theme::TEXT(), jumbo: false };
    if let Some(d) = e.description.as_deref().filter(|d| !d.is_empty()) {
        md::blocks(ui, &parsed(msg_id, d), &mut mcx);
    }
    // Fields: inline ones share rows of up to three.
    let mut i = 0;
    while i < e.fields.len() {
        let n = if e.fields[i].inline { e.fields[i..].iter().take(3).take_while(|f| f.inline).count().max(1) } else { 1 };
        ui.columns(n, |cols| {
            for (j, col) in cols.iter_mut().enumerate() {
                let f = &e.fields[i + j];
                // Names take markdown too (emoji, timestamps, links), shown bold and brighter, as in Discord.
                let (size, color) = (mcx.size, mcx.color);
                (mcx.size, mcx.color) = (13.5, theme::HEADER());
                md::blocks(col, &parsed(msg_id, &format!("**{}**", f.name.trim())), &mut mcx);
                (mcx.size, mcx.color) = (size, color);
                md::blocks(col, &parsed(msg_id, &f.value), &mut mcx);
            }
        });
        i += n;
    }
}

fn is_gif_url(url: &str) -> bool {
    url.split(['?', '#']).next().is_some_and(|path| path.to_ascii_lowercase().ends_with(".gif"))
}

/// An animated GIF for a GIF-as-video embed, where the site offers one at a predictable URL.
fn animated_gif(e: &Embed) -> Option<String> {
    let v = &e.video.as_ref()?.url;
    // Tenor names formats by a suffix on the media ID: AAAPo is MP4, AAAAC the full GIF.
    if v.starts_with("https://media.tenor.com/") && v.ends_with(".mp4") {
        let (dir, file) = v.rsplit_once('/')?;
        let dir = dir.strip_suffix("AAAPo")?;
        return Some(format!("{dir}AAAAC/{}.gif", file.strip_suffix(".mp4")?));
    }
    if v.contains("giphy.com/media/") && v.ends_with("/giphy.mp4") {
        return Some(v.replace("/giphy.mp4", "/giphy.gif"));
    }
    None
}

/// A post with several pictures (as embed bots and Twitter links send them) arrives as one embed followed by
/// embeds with the same URL holding only an image; Discord shows those images together in the first one.
fn gallery_extras<'a>(first: &Embed, rest: &'a [Embed]) -> Vec<&'a EmbedMedia> {
    if first.url.is_none() || !matches!(first.kind.as_deref(), None | Some("rich" | "article" | "link")) {
        return Vec::new();
    }
    rest.iter()
        .take_while(|x| x.url == first.url && x.title.is_none() && x.description.is_none() && x.image.is_some())
        .filter_map(|x| x.image.as_ref())
        .take(3)
        .collect()
}

/// A picture or video in a gallery.
pub(crate) struct GalleryMedia<'a> {
    pub url: &'a str,
    pub proxy_url: Option<&'a str>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video: bool,
    pub spoiler: bool,
    pub alt: Option<&'a str>,
}

impl<'a> From<&'a EmbedMedia> for GalleryMedia<'a> {
    fn from(m: &'a EmbedMedia) -> Self {
        GalleryMedia { url: &m.url, proxy_url: m.proxy_url.as_deref(), width: m.width, height: m.height, video: false, spoiler: false, alt: None }
    }
}

/// Pictures in a grid, cropped to fill their cells: one alone at its own shape, two side by side, three as
/// one tall and two stacked, four as two rows, and more in rows of three. Spoilers stay hidden until clicked
/// (remembered per message under `key`).
pub(crate) fn gallery(ui: &mut Ui, items: &[GalleryMedia], msg_id: Snowflake, key: u16, cx: &mut Ctx) {
    const GAP: f32 = 4.0;
    let width = 480.0_f32.min(ui.available_width());
    // Cells are laid out from the origin, then moved to wherever the gallery is allocated (the cursor can't
    // be used: in right-to-left layouts its left edge is unbounded).
    let mut cells: Vec<Rect> = Vec::new();
    let cell = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, h));
    let half_w = (width - GAP) / 2.0;
    let height = match items.len() {
        0 => return,
        1 => {
            let size = fit(items[0].width, items[0].height, Vec2::new(width, 480.0));
            cells.push(cell(0.0, 0.0, size.x, size.y));
            size.y
        }
        2 => {
            cells.extend([cell(0.0, 0.0, half_w, 240.0), cell(half_w + GAP, 0.0, half_w, 240.0)]);
            240.0
        }
        3 => {
            let half_h = (320.0 - GAP) / 2.0;
            cells.extend([cell(0.0, 0.0, half_w, 320.0), cell(half_w + GAP, 0.0, half_w, half_h), cell(half_w + GAP, half_h + GAP, half_w, half_h)]);
            320.0
        }
        4 => {
            let half_h = (320.0 - GAP) / 2.0;
            for (x, y) in [(0.0, 0.0), (half_w + GAP, 0.0), (0.0, half_h + GAP), (half_w + GAP, half_h + GAP)] {
                cells.push(cell(x, y, half_w, half_h));
            }
            320.0
        }
        n => {
            let side = (width - 2.0 * GAP) / 3.0;
            for i in 0..n {
                let (col, row) = ((i % 3) as f32, (i / 3) as f32);
                cells.push(cell(col * (side + GAP), row * (side + GAP), side, side));
            }
            n.div_ceil(3) as f32 * (side + GAP) - GAP
        }
    };
    // A lone picture may be narrower than the gallery.
    let used_w = cells.iter().map(|c| c.right()).fold(0.0, f32::max);
    let (area, _) = ui.allocate_exact_size(Vec2::new(used_w, height), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    for (i, (m, rect)) in items.iter().zip(cells).enumerate() {
        let rect = rect.translate(area.min.to_vec2());
        let reveal_key = (msg_id, key.wrapping_add(i as u16));
        if m.spoiler && !cx.uis.revealed.contains(&reveal_key) {
            let r = ui.interact(rect, egui::Id::unique(("gallery-spoiler", reveal_key, cx.read_only)), Sense::click());
            ui.painter().rect_filled(rect, 4, theme::RAIL());
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "SPOILER", theme::font_bold(14.0), Color32::WHITE);
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                cx.uis.revealed.insert(reveal_key);
            }
            continue;
        }
        // Crop to cover the cell, keeping the middle.
        let (iw, ih) = (m.width.unwrap_or(1).max(1) as f32, m.height.unwrap_or(1).max(1) as f32);
        let scale = (rect.width() / iw).max(rect.height() / ih);
        let (u, v) = ((rect.width() / (iw * scale)).min(1.0), (rect.height() / (ih * scale)).min(1.0));
        let uv = Rect::from_min_max(egui::pos2((1.0 - u) / 2.0, (1.0 - v) / 2.0), egui::pos2((1.0 + u) / 2.0, (1.0 + v) / 2.0));
        let (pw, ph) = ((iw * scale * ppp) as u32, (ih * scale * ppp) as u32);
        let url = swiftcord_core::cdn::media_proxy(m.proxy_url.unwrap_or(m.url), pw, ph);
        // A started video plays in its cell, letterboxed.
        if m.video && cx.uis.videos.is_open(m.url) {
            let video = crate::player::Video { url: m.url, link: m.url, poster: Some(url), width: m.width, height: m.height };
            cx.uis.videos.show_at(ui, rect, &video);
            continue;
        }
        let r = ui.put(rect, egui::Image::from_uri(url).uv(uv).maintain_aspect_ratio(false).fit_to_exact_size(rect.size()).corner_radius(4).sense(Sense::click()));
        if m.video {
            ui.painter().circle_filled(rect.center(), 24.0, Color32::from_black_alpha(160));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::PLAY, theme::icon_font(20.0), Color32::WHITE);
        }
        let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
        let r = match m.alt {
            Some(a) if !a.is_empty() => r.on_hover_text(a),
            _ => r,
        };
        if r.clicked() {
            if m.video {
                cx.uis.videos.play(ui.ctx(), m.url, rect.width());
            } else {
                cx.uis.viewer = Some(viewed(m.proxy_url.unwrap_or(m.url), m.url, (m.width, m.height), false));
            }
        }
    }
}

/// Whether an embed's video is a file we can play, rather than a page to embed (YouTube and the like). Discord
/// proxies the files it can play itself.
fn playable(v: &EmbedMedia) -> bool {
    let path = v.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    v.proxy_url.is_some() || [".mp4", ".mov", ".m4v", ".webm"].iter().any(|ext| path.ends_with(ext))
}

/// A video that's a web player (YouTube and the like): its poster (or a blank tile) with a play button, which
/// plays it right there, as Discord does (in lists like pins and search, in the video window instead).
fn video_poster(ui: &mut Ui, poster: Option<&EmbedMedia>, video: &EmbedMedia, title: Option<&str>, cx: &mut Ctx) {
    let size = match poster {
        Some(p) => fit(p.width.or(video.width), p.height.or(video.height), within(ui, Vec2::new(480.0, 270.0))),
        None => fit(video.width.or(Some(480)), video.height.or(Some(270)), within(ui, Vec2::new(480.0, 270.0))),
    };
    let playing = !cx.read_only && cx.uis.web_player.as_ref().is_some_and(|(u, c)| *u == video.url && *c == cx.channel);
    if playing {
        // The player (a window of its own) goes over this spot; black shows while it loads or is hidden.
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(rect, 6, Color32::BLACK);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Loading…", theme::font(13.0), theme::FAINT());
        cx.uis.web_player_slot = Some((rect, ui.clip_rect()));
        if ui.add(egui::Label::new(RichText::new("Stop video").size(12.0).color(theme::LINK())).selectable(false).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            cx.actions.push(Action::PlayWeb(None));
        }
        return;
    }
    let r = match poster {
        Some(p) => media_response(ui, p.proxy_url.as_deref().unwrap_or(&p.url), size, false, true, None),
        None => {
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            ui.painter().rect_filled(rect, 6, Color32::BLACK);
            ui.painter().circle_filled(rect.center(), 24.0, Color32::from_white_alpha(40));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::PLAY, theme::icon_font(20.0), Color32::WHITE);
            r.on_hover_cursor(egui::CursorIcon::PointingHand)
        }
    };
    let r = r.on_hover_text("Play");
    if r.clicked() {
        if cx.read_only {
            crate::platform::video_window::open(&video.url, title.unwrap_or("Video"));
        } else {
            cx.actions.push(Action::PlayWeb(Some((video.url.clone(), cx.channel))));
        }
    }
    r.context_menu(|ui| {
        if ui.button("Open in Window").clicked() {
            crate::platform::video_window::open(&video.url, title.unwrap_or("Video"));
            ui.close();
        }
        if ui.button("Open in Browser").clicked() {
            crate::platform::open_url(&video.url);
            ui.close();
        }
    });
}

fn embed(ui: &mut Ui, e: &Embed, extra_images: &[&EmbedMedia], msg_id: Snowflake, cx: &mut Ctx) {
    let kind = e.kind.as_deref().unwrap_or("rich");
    // Bare media links (images, GIFs) render as the media alone.
    if matches!(kind, "image" | "gifv") {
        let Some(m) = e.thumbnail.as_ref().or(e.image.as_ref()).or(e.video.as_ref()) else { return };
        let size = fit(m.width, m.height, within(ui, MAX_MEDIA));
        let target = e.url.as_deref().unwrap_or(&m.url).to_owned();
        // GIFs are usually MP4s now: play them as looping clips, with the still until the first frame.
        if let Some(video) = e.video.as_ref().filter(|_| kind == "gifv" && !e.video.as_ref().unwrap().url.is_empty()) {
            let still = e.thumbnail.as_ref().map(|t| swiftcord_core::cdn::media_proxy(t.proxy_url.as_deref().unwrap_or(&t.url), size.x as u32, size.y as u32));
            let r = cx.uis.clips.show(ui, &video.url, still.as_deref(), size).on_hover_cursor(egui::CursorIcon::PointingHand);
            let (width, height) = (video.width.or(m.width).unwrap_or(0), video.height.or(m.height).unwrap_or(0));
            let fav = FavoriteGif { url: target.clone(), src: video.url.clone(), format: 2, width, height, order: 0 };
            gif_media(ui, r, &target, fav, None, cx);
            return;
        }
        let gif = animated_gif(e).or_else(|| is_gif_url(&m.url).then(|| m.proxy_url.clone().unwrap_or_else(|| m.url.clone())));
        match gif {
            Some(gif) => {
                let r = media_response(ui, &gif, size, true, false, None);
                let view = viewed(&gif, &target, (m.width, m.height), true);
                let fav = FavoriteGif { url: target.clone(), src: gif, format: 1, width: m.width.unwrap_or(0), height: m.height.unwrap_or(0), order: 0 };
                gif_media(ui, r, &target, fav, Some(view), cx);
            }
            None => media(ui, m.proxy_url.as_deref().unwrap_or(&m.url), &target, size, (m.width, m.height), kind == "gifv", None, cx),
        }
        return;
    }
    let bar = e.color.map_or(theme::RAIL(), |c| theme::role_color(Some(c)));
    // Videos and images are shown large below the text; thumbnails sit beside it, unless they're a video's
    // poster. Link previews only have room for one picture, so a large image hides their thumbnail.
    let video = e.video.as_ref().filter(|v| !v.url.is_empty());
    let side_thumb = e.thumbnail.as_ref().filter(|_| video.is_none() && (e.image.is_none() || kind == "rich"));
    let r = egui::Frame::new().fill(theme::EMBED_BG()).corner_radius(4).inner_margin(egui::Margin { left: 16, right: 14, top: 10, bottom: 12 }).show(ui, |ui| {
        ui.set_max_width(520.0_f32.min(ui.available_width() - 8.0));
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal_top(|ui| {
            let text_w = ui.available_width() - if side_thumb.is_some() { 96.0 } else { 0.0 };
            ui.vertical(|ui| {
                ui.set_max_width(text_w);
                embed_text(ui, e, msg_id, cx);
            });
            if let Some(t) = side_thumb {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let size = fit(t.width, t.height, Vec2::splat(80.0));
                    media(ui, t.proxy_url.as_deref().unwrap_or(&t.url), &t.url, size, (t.width, t.height), false, None, cx);
                });
            }
        });
        if let Some(v) = video {
            let target = e.url.as_deref().unwrap_or(&v.url);
            let poster = e.thumbnail.as_ref().or(e.image.as_ref());
            if playable(v) {
                let size = fit(poster.and_then(|p| p.width).or(v.width), poster.and_then(|p| p.height).or(v.height), within(ui, Vec2::new(480.0, 270.0)));
                let ppp = ui.ctx().pixels_per_point();
                let still = poster.map(|p| swiftcord_core::cdn::media_proxy(p.proxy_url.as_deref().unwrap_or(&p.url), (size.x * ppp) as u32, (size.y * ppp) as u32));
                let video = crate::player::Video { url: &v.url, link: target, poster: still, width: v.width, height: v.height };
                cx.uis.videos.show(ui, &video, size);
            } else {
                video_poster(ui, poster, v, e.title.as_deref(), cx);
            }
        } else if let Some(img) = &e.image {
            if extra_images.is_empty() {
                let size = fit(img.width, img.height, within(ui, Vec2::new(480.0, 300.0)));
                media(ui, img.proxy_url.as_deref().unwrap_or(&img.url), &img.url, size, (img.width, img.height), false, None, cx);
            } else {
                let images: Vec<GalleryMedia> = std::iter::once(img).chain(extra_images.iter().copied()).map(GalleryMedia::from).collect();
                gallery(ui, &images, msg_id, GALLERY_SPOILERS, cx);
            }
        }
        if let Some(f) = &e.footer {
            ui.horizontal(|ui| {
                if let Some(icon) = &f.icon_url {
                    widgets::avatar(ui, icon, 16.0);
                }
                let mut text = f.text.clone();
                if let Some(ts) = e.timestamp {
                    text = format!("{text} • {}", time_label(ts));
                }
                ui.label(RichText::new(text).size(11.5).color(theme::MUTED()));
            });
        }
    });
    let rect = r.response.rect;
    ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())), egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, bar);
}

fn reactions(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for r in &msg.reactions {
            // A frame's width isn't known before it's drawn, so wrapping wouldn't happen until one had overflowed:
            // measure the chip (margins, emoji, gap, count, stroke) and start a new row if it won't fit.
            let count_w = ui.painter().layout_no_wrap(r.count.to_string(), theme::font_semibold(13.0), Color32::WHITE).size().x;
            if ui.available_size_before_wrap().x < 6.0 + 18.0 + 4.0 + count_w + 6.0 + 2.0 {
                ui.end_row();
            }
            let (fill, stroke) = if r.me { (theme::ACCENT().gamma_multiply(0.25), theme::ACCENT()) } else { (theme::SIDEBAR(), theme::SIDEBAR()) };
            let resp = egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, stroke)).corner_radius(8).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add(crate::emoji_sheet::any(ui.ctx(), &r.emoji, Vec2::splat(18.0)));
                    ui.label(RichText::new(r.count.to_string()).font(theme::font_semibold(13.0)).color(if r.me { theme::MENTION_TEXT() } else { theme::MUTED() }));
                });
            });
            let resp = ui.interact(resp.response.rect, egui::Id::unique(("react", msg.id, r.emoji.reaction_key(), cx.read_only)), Sense::click());
            let name = r.emoji.name.clone().unwrap_or_default();
            if resp.on_hover_text(format!(":{name}:")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && cx.perms & perm::ADD_REACTIONS != 0 {
                cx.actions.push(Action::React { channel: msg.channel_id, message: msg.id, emoji: r.emoji.clone(), add: !r.me });
            }
        }
        if cx.perms & perm::ADD_REACTIONS != 0 {
            let add = ui.add(egui::Button::new(RichText::new(icon::EMOJI).font(theme::icon_font(14.0)).color(theme::MUTED())).fill(theme::SIDEBAR()).corner_radius(8));
            if add.clicked() {
                cx.uis.emoji_picker = Some(PickerTarget::Reaction { channel: msg.channel_id, message: msg.id, at: add.rect.left_bottom() });
            }
        }
    });
}

fn toolbar(ui: &mut Ui, msg: &Message, row: Rect, cx: &mut Ctx) {
    let mine = cx.state.me.as_ref().is_some_and(|m| m.id == msg.author.id);
    // In DMs (where every permission is granted) only your own messages can be deleted.
    let can_delete = mine || (cx.guild.is_some() && cx.perms & perm::MANAGE_MESSAGES != 0);
    let mut buttons: Vec<(&str, &str, u8)> = Vec::new();
    if cx.perms & perm::ADD_REACTIONS != 0 {
        buttons.push((icon::EMOJI, "Add Reaction", 0));
    }
    if cx.perms & perm::SEND_MESSAGES != 0 && msg.is_regular() {
        buttons.push((icon::REPLY, "Reply", 1));
    }
    if mine && msg.is_regular() {
        buttons.push((icon::EDIT, "Edit", 2));
    }
    if can_delete {
        buttons.push((icon::DELETE, "Delete", 3));
    }
    // The reactions used most, one click each, as in the official client's hover bar.
    let quick: Vec<swiftcord_core::models::Emoji> =
        if cx.perms & perm::ADD_REACTIONS != 0 { cx.uis.quick_reactions.1.iter().take(3).cloned().collect() } else { Vec::new() };
    let divider = if quick.is_empty() { 0.0 } else { 9.0 };
    let w = (buttons.len() + quick.len()) as f32 * 32.0 + divider + 8.0;
    let rect = Rect::from_min_size(egui::pos2(row.right() - w - 16.0, row.top() - 14.0), Vec2::new(w, 32.0));
    let rect = rect.translate(Vec2::new(0.0, (ui.clip_rect().top() - rect.top()).max(0.0)));
    ui.painter().rect(rect, 6, theme::SIDEBAR(), egui::Stroke::new(1.0, theme::RAIL()), egui::StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(4.0)).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().item_spacing.x = 4.0;
    for e in &quick {
        quick_reaction(&mut child, msg, e, cx);
    }
    if !quick.is_empty() {
        let x = child.cursor().left() + 2.0;
        child.painter().vline(x, rect.y_range().shrink(8.0), egui::Stroke::new(1.0, theme::RAIL()));
        child.add_space(5.0);
    }
    for (glyph, tip, kind) in buttons {
        let color = if kind == 3 { theme::RED() } else { theme::MUTED() };
        let r = child.add(egui::Button::new(RichText::new(glyph).font(theme::icon_font(15.0)).color(color)).frame_when_inactive(false).min_size(Vec2::splat(28.0)));
        let r = r.on_hover_text(tip);
        if r.clicked() {
            match kind {
                0 => cx.uis.emoji_picker = Some(PickerTarget::Reaction { channel: msg.channel_id, message: msg.id, at: r.rect.left_bottom() }),
                1 => start_reply(msg, cx),
                2 => cx.uis.editing = Some((msg.id, msg.content.clone())),
                _ => ask_delete(ui, msg, cx),
            }
        }
    }
}

/// A one-click reaction: adds it, or takes ours back if we've already reacted with it.
fn quick_reaction(ui: &mut Ui, msg: &Message, e: &swiftcord_core::models::Emoji, cx: &mut Ctx) -> bool {
    let mine = msg.reactions.iter().any(|r| r.me && r.emoji.same_as(e));
    // Drawn by hand at exactly the size of the toolbar's icon buttons (an image button pads itself wider).
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if r.hovered() || mine {
        ui.painter().rect_filled(rect, 4, if mine { theme::ACCENT().gamma_multiply(0.3) } else { theme::HOVER() });
    }
    crate::emoji_sheet::any(ui.ctx(), e, Vec2::splat(20.0)).paint_at(ui, rect.shrink(4.0));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    let name = e.name.clone().unwrap_or_default();
    let shown = if e.id.is_some() { format!(":{name}:") } else { name };
    if r.on_hover_text(shown).clicked() {
        cx.actions.push(Action::React { channel: msg.channel_id, message: msg.id, emoji: e.clone(), add: !mine });
        return true;
    }
    false
}

fn start_reply(msg: &Message, cx: &mut Ctx) {
    let mine = cx.state.me.as_ref().is_some_and(|m| m.id == msg.author.id);
    cx.uis.drafts.entry(cx.channel).or_default().reply = Some((msg.id, !mine));
    cx.uis.focus_composer = true;
}

fn context_menu(ui: &mut Ui, msg: &Message, cx: &mut Ctx) {
    ui.set_min_width(180.0);
    let mine = cx.state.me.as_ref().is_some_and(|m| m.id == msg.author.id);
    // The reactions used most, then the full picker, as at the top of the official client's menu.
    if cx.perms & perm::ADD_REACTIONS != 0 && msg.pending.is_none() {
        let quick = cx.uis.quick_reactions.1.clone();
        let mut picked = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for e in &quick {
                picked |= quick_reaction(ui, msg, e, cx);
            }
            let more = ui.add(egui::Button::new(RichText::new(icon::EMOJI).font(theme::icon_font(15.0)).color(theme::MUTED())).frame_when_inactive(false).min_size(Vec2::splat(28.0)));
            if more.on_hover_text("Add Reaction").clicked() {
                cx.uis.emoji_picker = Some(PickerTarget::Reaction { channel: msg.channel_id, message: msg.id, at: ui.ctx().pointer_latest_pos().unwrap_or_default() });
                picked = true;
            }
        });
        if picked {
            ui.close();
        }
        ui.separator();
    }
    if cx.perms & perm::SEND_MESSAGES != 0 && ui.button("Reply").clicked() {
        start_reply(msg, cx);
        ui.close();
    }
    if mine && msg.is_regular() && ui.button("Edit Message").clicked() {
        cx.uis.editing = Some((msg.id, msg.content.clone()));
        ui.close();
    }
    if msg.is_regular() && msg.pending.is_none() && ui.button("Forward").clicked() {
        cx.uis.forward = Some(crate::ui::forward::Forward { channel: msg.channel_id, message: msg.id, ..Default::default() });
        ui.close();
    }
    if cx.perms & perm::ADD_REACTIONS != 0 && ui.button("Add Reaction").clicked() {
        let at = ui.ctx().pointer_latest_pos().unwrap_or_default();
        cx.uis.emoji_picker = Some(PickerTarget::Reaction { channel: msg.channel_id, message: msg.id, at });
        ui.close();
    }
    // Threads start from messages in text and announcement channels, once per message.
    let in_channel = cx.state.channel(msg.channel_id).is_some_and(|c| c.has_threads() && !c.is_forum());
    if in_channel && msg.thread.is_none() && msg.is_regular() && cx.perms & perm::CREATE_PUBLIC_THREADS != 0 && ui.button("Create Thread").clicked() {
        // Named after the message, as Discord suggests.
        let name: String = msg.content.lines().next().unwrap_or_default().chars().take(40).collect();
        cx.actions.push(Action::StartThread { channel: msg.channel_id, message: Some(msg.id), name });
        ui.close();
    }
    ui.separator();
    if !msg.content.is_empty() && ui.button("Copy Text").clicked() {
        cx.actions.push(Action::Copy(msg.content.clone()));
        ui.close();
    }
    if ui.button("Mark Unread").clicked() {
        cx.actions.push(Action::MarkUnread { channel: msg.channel_id, message: msg.id });
        ui.close();
    }
    if ui.button("Copy Message Link").clicked() {
        let g = cx.guild.map_or("@me".to_owned(), |g| g.to_string());
        cx.actions.push(Action::Copy(format!("https://discord.com/channels/{g}/{}/{}", msg.channel_id, msg.id)));
        ui.close();
    }
    if ui.button("Copy Message ID").clicked() {
        cx.actions.push(Action::Copy(msg.id.to_string()));
        ui.close();
    }
    if mine || (cx.guild.is_some() && cx.perms & perm::MANAGE_MESSAGES != 0) {
        ui.separator();
        if ui.button(RichText::new("Delete Message").color(theme::RED())).clicked() {
            ask_delete(ui, msg, cx);
            ui.close();
        }
    }
}

/// A rough height before a message has been laid out, so the scroll bar is about right.
pub fn estimate_height(msg: &Message, compact: bool) -> f32 {
    let mut h = if compact { 24.0 } else { 46.0 };
    let lines = msg.content.lines().map(|l| 1 + l.len() / 90).sum::<usize>().max(1);
    h += (lines.saturating_sub(1)) as f32 * 21.0;
    for a in &msg.attachments {
        h += if a.is_voice_message() {
            58.0
        } else if a.is_audio() {
            104.0
        } else if a.is_image() || a.is_video() { fit(a.width.or(Some(480)), a.height.or(Some(270)), MAX_MEDIA).y + 6.0 } else { 64.0 };
    }
    h += msg.embeds.len() as f32 * 120.0;
    h += msg.components.len() as f32 * 160.0;
    if !msg.reactions.is_empty() {
        h += 30.0;
    }
    if msg.is_reply() {
        h += 22.0;
    }
    h
}

/// A message that's nothing but a link to a GIF or image shows just the media, as in Discord.
fn is_lone_media_link(msg: &Message) -> bool {
    let text = msg.content.trim();
    (text.starts_with("https://") || text.starts_with("http://"))
        && !text.contains(char::is_whitespace)
        && msg.embeds.iter().any(|e| matches!(e.kind.as_deref(), Some("gifv" | "image")) && e.url.as_deref() == Some(text))
}
