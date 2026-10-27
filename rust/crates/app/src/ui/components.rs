//! Message components: buttons and select menus under a message, and Components V2 layouts, where apps (embed
//! bots especially) build the whole message from containers, sections, text, thumbnails, galleries, files and
//! separators instead of content and embeds.

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Ui, Vec2};
use swiftcord_core::models::{component_type as kind, Component, Message};

use crate::app::Action;
use crate::theme::{self, icon};
use crate::ui::markdown::{self as md, Cx};
use crate::ui::message::{self, Ctx, GalleryMedia, GALLERY_SPOILERS};

const MAX_WIDTH: f32 = 520.0;
const THUMBNAIL: f32 = 85.0;

pub fn show(ui: &mut Ui, components: &[Component], msg: &Message, cx: &mut Ctx) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut n = 0;
        for c in components {
            component(ui, c, msg, &mut n, cx);
        }
    });
}

/// `n` numbers galleries, so each keeps its own revealed spoilers.
fn component(ui: &mut Ui, c: &Component, msg: &Message, n: &mut u16, cx: &mut Ctx) {
    match c.kind {
        kind::CONTAINER => container(ui, c, msg, n, cx),
        kind::SECTION => section(ui, c, msg, n, cx),
        kind::TEXT_DISPLAY => text(ui, c.content.as_deref().unwrap_or_default(), msg, cx),
        kind::THUMBNAIL => thumbnail(ui, c, msg, n, cx),
        kind::MEDIA_GALLERY => {
            let items: Vec<GalleryMedia> = c
                .items
                .iter()
                .map(|i| GalleryMedia {
                    url: &i.media.url,
                    proxy_url: i.media.proxy_url.as_deref(),
                    width: i.media.width,
                    height: i.media.height,
                    video: i.media.is_video(),
                    spoiler: i.spoiler,
                    alt: i.description.as_deref(),
                })
                .collect();
            *n += 1;
            message::gallery(ui, &items, msg.id, GALLERY_SPOILERS + *n * 16, cx);
        }
        kind::FILE => {
            // `attachment://name`, or the CDN link Discord resolved it to.
            let Some(f) = &c.file else { return };
            let name = f.url.strip_prefix("attachment://");
            let path = f.url.split('?').next().unwrap_or_default();
            let a = msg.attachments.iter().find(|a| Some(a.filename.as_str()) == name || a.url.split('?').next() == Some(path));
            if let Some(a) = a {
                message::attachment(ui, a, cx);
            }
        }
        kind::SEPARATOR => {
            let gap = if c.spacing == Some(2) { 16.0 } else { 6.0 };
            ui.add_space(gap / 2.0);
            if c.divider != Some(false) {
                let y = ui.cursor().top();
                ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::SELECTED()));
            }
            ui.add_space(gap / 2.0);
        }
        kind::ACTION_ROW => {
            ui.horizontal_wrapped(|ui| {
                for child in &c.components {
                    if child.kind == kind::BUTTON {
                        button(ui, child, msg, cx);
                    } else if kind::is_select(child.kind) {
                        select(ui, child, msg, cx);
                    }
                }
            });
        }
        kind::BUTTON => {
            button(ui, c, msg, cx);
        }
        _ => {}
    }
}

/// A box like an embed's, with the accent colour down its left edge.
fn container(ui: &mut Ui, c: &Component, msg: &Message, n: &mut u16, cx: &mut Ctx) {
    let r = egui::Frame::new()
        .fill(theme::EMBED_BG())
        .corner_radius(8)
        .stroke(egui::Stroke::new(1.0, theme::RAIL()))
        .inner_margin(egui::Margin { left: 16, right: 16, top: 14, bottom: 14 })
        .show(ui, |ui| {
            ui.set_max_width(MAX_WIDTH.min(ui.available_width() - 8.0));
            ui.spacing_mut().item_spacing.y = 8.0;
            for child in &c.components {
                component(ui, child, msg, n, cx);
            }
        });
    if let Some(color) = c.accent_color {
        let rect = r.response.rect;
        let bar = Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height()));
        ui.painter().rect_filled(bar, egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, theme::role_color(Some(color)));
    }
}

/// Text beside a thumbnail or button.
fn section(ui: &mut Ui, c: &Component, msg: &Message, n: &mut u16, cx: &mut Ctx) {
    let accessory = c.accessory.as_deref();
    ui.horizontal_top(|ui| {
        let reserve = match accessory.map(|a| a.kind) {
            Some(kind::THUMBNAIL) => THUMBNAIL + 16.0,
            Some(kind::BUTTON) => 120.0,
            _ => 0.0,
        };
        let text_w = (ui.available_width() - reserve).max(80.0);
        ui.vertical(|ui| {
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y = 4.0;
            for child in &c.components {
                component(ui, child, msg, n, cx);
            }
        });
        if let Some(a) = accessory {
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| match a.kind {
                kind::THUMBNAIL => thumbnail(ui, a, msg, n, cx),
                kind::BUTTON => button(ui, a, msg, cx),
                _ => {}
            });
        }
    });
}

fn thumbnail(ui: &mut Ui, c: &Component, msg: &Message, n: &mut u16, cx: &mut Ctx) {
    let Some(m) = &c.media else { return };
    let item = GalleryMedia {
        url: &m.url,
        proxy_url: m.proxy_url.as_deref(),
        width: m.width,
        height: m.height,
        video: m.is_video(),
        spoiler: c.spoiler,
        alt: c.description.as_deref(),
    };
    ui.allocate_ui(Vec2::splat(THUMBNAIL), |ui| {
        *n += 1;
        // A single picture, cropped square like Discord's thumbnails.
        let square = GalleryMedia { width: Some(m.width.unwrap_or(1).min(m.height.unwrap_or(1))), height: Some(m.width.unwrap_or(1).min(m.height.unwrap_or(1))), ..item };
        message::gallery(ui, &[square], msg.id, GALLERY_SPOILERS + *n * 16, cx);
    });
}

fn text(ui: &mut Ui, content: &str, msg: &Message, cx: &mut Ctx) {
    if content.is_empty() {
        return;
    }
    let blocks = message::parsed(msg.id, content);
    let mut mcx = Cx {
        state: cx.state,
        guild: cx.guild,
        message: msg.id,
        revealed: &mut cx.uis.revealed,
        actions: cx.actions,
        size: 15.0,
        color: theme::TEXT(),
        jumbo: false,
    };
    md::blocks(ui, &blocks, &mut mcx);
}

fn button(ui: &mut Ui, c: &Component, msg: &Message, cx: &mut Ctx) {
    let style = c.style.unwrap_or(2);
    let waiting = c.custom_id.as_deref().is_some_and(|id| cx.uis.interactions.is_pending(msg.id, id));
    let fill = match style {
        1 | 6 => theme::BLURPLE(),
        3 => Color32::from_rgb(0x24, 0x80, 0x46),
        4 => Color32::from_rgb(0xda, 0x37, 0x3c),
        _ => Color32::from_rgb(0x4e, 0x50, 0x58),
    };
    let mut label = c.label.clone().unwrap_or_default();
    if style == 5 {
        label.push_str(" ↗");
    }
    let text = RichText::new(label).font(theme::font_semibold(14.0)).color(Color32::WHITE);
    let button = match &c.emoji {
        Some(e) if e.id.is_some() => {
            let url = swiftcord_core::cdn::emoji(e.id.unwrap_or_default(), e.animated, 48);
            egui::Button::image_and_text(egui::Image::from_uri(url).fit_to_exact_size(Vec2::splat(18.0)), text)
        }
        Some(e) => egui::Button::new(RichText::new(format!("{} {}", e.name.as_deref().unwrap_or_default(), c.label.as_deref().unwrap_or_default())).font(theme::font_semibold(14.0)).color(Color32::WHITE)),
        None => egui::Button::new(text),
    };
    let r = ui.add_enabled(!c.disabled && !waiting && !cx.read_only, button.fill(fill).corner_radius(4).min_size(Vec2::new(60.0, 32.0)));
    if waiting {
        // The app is working on it.
        egui::Spinner::new().size(14.0).color(Color32::WHITE).paint_at(ui, Rect::from_center_size(r.rect.right_center() - Vec2::new(12.0, 0.0), Vec2::splat(14.0)));
    }
    match (&c.url, style) {
        (Some(url), 5) => {
            if r.on_hover_text(url).clicked() {
                cx.actions.push(Action::OpenLink(url.clone()));
            }
        }
        // Premium buttons open a store page, which Swiftcord doesn't have.
        (_, 6) => {
            r.on_hover_text("Purchases aren't available in Swiftcord.");
        }
        _ => {
            if r.clicked() {
                cx.actions.push(Action::UseComponent { msg: Box::new(msg.clone()), component: Box::new(c.clone()), values: None });
            }
        }
    }
}

/// A choice in a select menu: the value sent, its label, and an optional description.
struct Choice {
    value: String,
    label: String,
    description: Option<String>,
}

/// What a select menu offers: a string select's options, or the server's people, roles or channels.
fn choices(c: &Component, query: &str, cx: &Ctx) -> Vec<Choice> {
    let roles = || -> Vec<Choice> {
        let Some(g) = cx.guild.and_then(|g| cx.state.guild(g)) else { return Vec::new() };
        let q = query.to_lowercase();
        let mut roles: Vec<_> = g.roles.iter().filter(|r| r.id != g.id && r.name.to_lowercase().contains(&q)).collect();
        roles.sort_by_key(|r| -r.position);
        roles.into_iter().take(25).map(|r| Choice { value: r.id.to_string(), label: format!("@{}", r.name), description: None }).collect()
    };
    let users = || -> Vec<Choice> {
        let Some(ch) = cx.state.channel(cx.channel) else { return Vec::new() };
        cx.state
            .search_people(ch, query, 25)
            .into_iter()
            .map(|u| Choice { value: u.id.to_string(), label: cx.state.display_name(u.id, cx.guild), description: Some(u.tag()) })
            .collect()
    };
    match c.kind {
        kind::STRING_SELECT => c.options.iter().map(|o| Choice { value: o.value.clone(), label: o.label.clone(), description: o.description.clone() }).collect(),
        kind::ROLE_SELECT => roles(),
        kind::USER_SELECT => users(),
        kind::MENTIONABLE_SELECT => users().into_iter().chain(roles()).collect(),
        kind::CHANNEL_SELECT => {
            let Some(g) = cx.guild else { return Vec::new() };
            let q = query.to_lowercase();
            cx.state
                .channel_tree(g)
                .into_iter()
                .flat_map(|(_, chans)| chans)
                .filter(|ch| ch.label().to_lowercase().contains(&q))
                .take(25)
                .map(|ch| Choice { value: ch.id.to_string(), label: format!("#{}", ch.label()), description: None })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// An app's select menu. One choice is sent as soon as it's picked; for several, they're ticked and then
/// applied. Painted at a fixed size: it sits in a wrapping row, where there's no width to fill.
fn select(ui: &mut Ui, c: &Component, msg: &Message, cx: &mut Ctx) {
    let custom_id = c.custom_id.clone().unwrap_or_default();
    let waiting = cx.uis.interactions.is_pending(msg.id, &custom_id);
    let enabled = !c.disabled && !waiting && !cx.read_only;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(260.0, 36.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let p = ui.painter();
    p.rect_filled(rect, 4, if r.hovered() && enabled { theme::SELECTED() } else { theme::INPUT() });
    let defaults: Vec<&str> = c.options.iter().filter(|o| o.default).map(|o| o.label.as_str()).collect();
    let (text, color) = if defaults.is_empty() { (c.placeholder.clone().unwrap_or_else(|| "Make a selection".into()), theme::MUTED()) } else { (defaults.join(", "), theme::TEXT()) };
    let galley = p.layout(text, theme::font(14.0), if enabled { color } else { color.gamma_multiply(0.5) }, rect.width() - 44.0);
    p.galley(rect.left_center() + Vec2::new(10.0, -galley.size().y / 2.0), galley, color);
    if waiting {
        egui::Spinner::new().size(14.0).paint_at(ui, Rect::from_center_size(rect.right_center() - Vec2::new(16.0, 0.0), Vec2::splat(14.0)));
    } else {
        p.text(rect.right_center() - Vec2::new(12.0, 0.0), egui::Align2::RIGHT_CENTER, icon::CHEVRON_DOWN, theme::icon_font(12.0), theme::MUTED());
    }
    if !enabled {
        return;
    }
    let many = c.max_values.unwrap_or(1) > 1;
    let min = c.min_values.unwrap_or(1) as usize;
    let max = c.max_values.unwrap_or(1) as usize;
    let searchable = c.kind != kind::STRING_SELECT;
    let key = (msg.id, custom_id.clone());
    let query_id = ui.scope_id().with(("select-query", msg.id, &custom_id));
    egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(260.0);
        ui.set_max_height(320.0);
        let mut query: String = ui.ctx().data(|d| d.get_temp(query_id)).unwrap_or_default();
        if searchable && ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search").desired_width(f32::INFINITY)).changed() {
            ui.ctx().data_mut(|d| d.insert_temp(query_id, query.clone()));
        }
        let list = choices(c, &query, cx);
        if list.is_empty() {
            ui.label(RichText::new("Nothing to choose from.").color(theme::MUTED()));
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for ch in &list {
                if many {
                    let chosen = cx.uis.interactions.choosing.entry(key.clone()).or_default();
                    let mut on = chosen.contains(&ch.value);
                    if ui.checkbox(&mut on, &ch.label).changed() {
                        chosen.retain(|v| v != &ch.value);
                        if on && chosen.len() < max {
                            chosen.push(ch.value.clone());
                        }
                    }
                } else if ui.selectable_label(false, &ch.label).clicked() {
                    cx.actions.push(Action::UseComponent { msg: Box::new(msg.clone()), component: Box::new(c.clone()), values: Some(vec![ch.value.clone()]) });
                    ui.close();
                }
                if let Some(d) = &ch.description {
                    ui.label(RichText::new(d).size(11.5).color(theme::MUTED()));
                }
            }
        });
        if many {
            ui.separator();
            let chosen = cx.uis.interactions.choosing.get(&key).cloned().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} chosen (pick {min}–{max})", chosen.len())).size(12.0).color(theme::MUTED()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = (min..=max).contains(&chosen.len());
                    if ui.add_enabled(ok, egui::Button::new("Apply")).clicked() {
                        cx.uis.interactions.choosing.remove(&key);
                        cx.actions.push(Action::UseComponent { msg: Box::new(msg.clone()), component: Box::new(c.clone()), values: Some(chosen) });
                        ui.close();
                    }
                });
            });
        }
    });
}
