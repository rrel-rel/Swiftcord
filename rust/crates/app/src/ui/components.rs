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
                ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::SELECTED));
            }
            ui.add_space(gap / 2.0);
        }
        kind::ACTION_ROW => {
            ui.horizontal_wrapped(|ui| {
                for child in &c.components {
                    if child.kind == kind::BUTTON {
                        button(ui, child, cx);
                    } else if kind::is_select(child.kind) {
                        select(ui, child);
                    }
                }
            });
        }
        kind::BUTTON => {
            button(ui, c, cx);
        }
        _ => {}
    }
}

/// A box like an embed's, with the accent colour down its left edge.
fn container(ui: &mut Ui, c: &Component, msg: &Message, n: &mut u16, cx: &mut Ctx) {
    let r = egui::Frame::new()
        .fill(theme::EMBED_BG)
        .corner_radius(8)
        .stroke(egui::Stroke::new(1.0, theme::RAIL))
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
                kind::BUTTON => button(ui, a, cx),
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
        color: theme::TEXT,
        jumbo: false,
    };
    md::blocks(ui, &blocks, &mut mcx);
}

fn button(ui: &mut Ui, c: &Component, cx: &mut Ctx) {
    let style = c.style.unwrap_or(2);
    let fill = match style {
        1 | 6 => theme::BLURPLE,
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
    let r = ui.add_enabled(!c.disabled, button.fill(fill).corner_radius(4).min_size(Vec2::new(60.0, 32.0)));
    match (&c.url, style) {
        (Some(url), 5) => {
            if r.on_hover_text(url).clicked() {
                cx.actions.push(Action::OpenLink(url.clone()));
            }
        }
        _ => {
            if r.clicked() {
                cx.actions.push(Action::Notice("App buttons aren't supported in Swiftcord yet.".into()));
            }
        }
    }
}

/// Select menus are shown but can't be used yet.
/// Painted at a fixed size: it sits in a wrapping row, where there's no width to fill.
fn select(ui: &mut Ui, c: &Component) {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(260.0, 36.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4, theme::INPUT);
    let prompt = c.placeholder.as_deref().unwrap_or("Make a selection");
    p.text(rect.left_center() + Vec2::new(10.0, 0.0), egui::Align2::LEFT_CENTER, prompt, theme::font(14.0), theme::MUTED);
    p.text(rect.right_center() - Vec2::new(12.0, 0.0), egui::Align2::RIGHT_CENTER, icon::CHEVRON_DOWN, theme::icon_font(12.0), theme::MUTED);
    r.on_hover_text("Menus from apps aren't supported in Swiftcord yet.");
}
