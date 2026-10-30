//! Small shared widgets.

use egui::{self, Color32, CornerRadius, Response, Sense, Ui, Vec2};
use swiftcord_core::models::{PresenceStatus, User};

use crate::theme;

/// A "Jump" button in the top-right corner of `rect` (a message in a side panel) while it's hovered. It's
/// painted over the message rather than laid out, so it doesn't push the next one down. True when clicked.
pub fn jump_button(ui: &Ui, rect: egui::Rect, id: impl std::hash::Hash + std::fmt::Debug) -> bool {
    if !ui.rect_contains_pointer(rect) {
        return false;
    }
    let b = egui::Rect::from_min_size(rect.right_top() + Vec2::new(-56.0, 6.0), Vec2::new(48.0, 22.0));
    let r = ui.interact(b, egui::Id::unique(id), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect_filled(b, 4, if r.hovered() { theme::SELECTED() } else { theme::SIDEBAR() });
    ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, "Jump", theme::font(12.0), theme::HEADER());
    r.clicked()
}

/// Lines of text (a name, then maybe a subtitle) as a block centred on the row's height, truncated to fit,
/// leaving `reserve` on the right (for a badge laid out after it).
pub fn centered_lines(ui: &mut Ui, reserve: f32, lines: &[(String, egui::FontId, Color32)]) {
    let avail = ui.available_rect_before_wrap();
    let width = (avail.width() - reserve).max(0.0);
    let gap = ui.spacing().item_spacing.y;
    let heights: Vec<f32> = lines.iter().map(|(_, font, _)| ui.ctx().fonts_mut(|f| f.row_height(font))).collect();
    let height = heights.iter().sum::<f32>() + gap * lines.len().saturating_sub(1) as f32;
    let top = avail.center().y - height / 2.0;
    let block = egui::Rect::from_min_size(egui::pos2(avail.left(), top), Vec2::new(width, height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(block).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
        for (text, font, color) in lines {
            ui.add(egui::Label::new(egui::RichText::new(text).font(font.clone()).color(*color)).truncate().selectable(false));
        }
    });
}

pub fn avatar(ui: &mut Ui, url: &str, size: f32) -> Response {
    ui.add(
        egui::Image::from_uri(url.to_owned())
            .fit_to_exact_size(Vec2::splat(size))
            .corner_radius(CornerRadius::same((size / 2.0) as u8))
            .show_loading_spinner(false)
            .sense(Sense::click()),
    )
}

pub fn user_avatar(ui: &mut Ui, user: &User, size: f32) -> Response {
    avatar(ui, &swiftcord_core::cdn::avatar(user, avatar_px(size)), size)
}

/// Someone's avatar as shown in `guild`: their server profile's, if they have one.
pub fn member_avatar(ui: &mut Ui, state: &swiftcord_core::state::State, user: &User, guild: Option<swiftcord_core::Snowflake>, size: f32) -> Response {
    avatar(ui, &state.avatar_url(user, guild, avatar_px(size)), size)
}

/// The image size to fetch for an avatar shown `size` points wide.
pub fn avatar_px(size: f32) -> u32 {
    if size <= 32.0 { 64 } else { 128 }
}

pub fn status_color(s: PresenceStatus) -> Color32 {
    match s {
        PresenceStatus::Online => theme::GREEN(),
        PresenceStatus::Idle => theme::YELLOW(),
        PresenceStatus::Dnd => theme::RED(),
        PresenceStatus::Offline => theme::OFFLINE(),
    }
}

/// A status dot in Discord's shapes, cut out with `bg`: a moon for idle, a bar for do not disturb and a ring
/// for offline, so they read without colour too. Someone online only from a phone (`mobile`) gets a phone.
pub fn status_dot(painter: &egui::Painter, c: egui::Pos2, r: f32, status: PresenceStatus, mobile: bool, bg: Color32) {
    if mobile && status == PresenceStatus::Online {
        // Taller than the dot, rising from the same bottom edge, with its own cut-out ring.
        let body = egui::Rect::from_min_max(egui::pos2(c.x - r * 0.75, c.y - r * 1.6), egui::pos2(c.x + r * 0.75, c.y + r));
        painter.rect_filled(body.expand(r * 0.35), r * 0.55, bg);
        painter.rect_filled(body, r * 0.3, status_color(status));
        let screen = egui::Rect::from_min_max(body.min + Vec2::splat(r * 0.28), egui::pos2(body.right() - r * 0.28, body.bottom() - r * 0.6));
        painter.rect_filled(screen, 0.0, bg);
        return;
    }
    painter.circle_filled(c, r, status_color(status));
    match status {
        PresenceStatus::Online => {}
        PresenceStatus::Idle => {
            // The cut-out reaches past the dot; keep it off the avatar.
            let clipped = painter.with_clip_rect(egui::Rect::from_center_size(c, Vec2::splat(r * 2.0)).intersect(painter.clip_rect()));
            clipped.circle_filled(c - Vec2::splat(r * 0.5), r * 0.75, bg);
        }
        PresenceStatus::Dnd => {
            let bar = egui::Rect::from_center_size(c, Vec2::new(r * 1.5, r * 0.5));
            painter.rect_filled(bar, r * 0.25, bg);
        }
        PresenceStatus::Offline => {
            painter.circle_filled(c, r * 0.5, bg);
        }
    }
}

/// Paints an avatar (`url`) into `rect` with a status dot in its bottom-right corner, ringed with `bg`.
pub fn paint_avatar_with_status(ui: &Ui, url: String, rect: egui::Rect, status: PresenceStatus, mobile: bool, bg: Color32) {
    let size = rect.width();
    egui::Image::from_uri(url)
        .corner_radius(CornerRadius::same((size / 2.0) as u8))
        .show_loading_spinner(false)
        .paint_at(ui, rect);
    corner_status(ui, rect, status, mobile, bg);
}

/// An avatar with a presence dot in its bottom-right corner.
pub fn avatar_with_status(ui: &mut Ui, user: &User, size: f32, status: PresenceStatus, mobile: bool, bg: Color32) -> Response {
    let r = user_avatar(ui, user, size);
    corner_status(ui, r.rect, status, mobile, bg);
    r
}

fn corner_status(ui: &Ui, avatar: egui::Rect, status: PresenceStatus, mobile: bool, bg: Color32) {
    let dot = avatar.width() * 0.3;
    let c = avatar.right_bottom() - Vec2::splat(dot * 0.5);
    ui.painter().circle_filled(c, dot * 0.5 + 2.0, bg);
    status_dot(ui.painter(), c, dot * 0.5, status, mobile, bg);
}

/// A red pill with a count, e.g. unread mentions.
pub fn badge(ui: &Ui, center: egui::Pos2, count: u32, outline: Color32) {
    let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
    let galley = ui.painter().layout_no_wrap(text, theme::font_bold(11.0), Color32::WHITE);
    let w = (galley.size().x + 9.0).max(16.0);
    let rect = egui::Rect::from_center_size(center, egui::vec2(w, 16.0));
    ui.painter().rect_filled(rect.expand(3.0), 11, outline);
    ui.painter().rect_filled(rect, 8, theme::RED());
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// A flat icon button that highlights on hover.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let r = ui.add(
        egui::Button::new(egui::RichText::new(icon).font(theme::icon_font(16.0)).color(theme::MUTED()))
            .frame(false)
            .min_size(Vec2::splat(30.0)),
    );
    if r.hovered() {
        ui.painter().rect_filled(r.rect, 4, theme::HOVER().gamma_multiply(0.6));
    }
    r.on_hover_text(tooltip)
}

/// A toggle-style icon button (e.g. mute) that turns red when active.
pub fn toggle_icon(ui: &mut Ui, icon: &str, active: bool, tooltip: &str) -> Response {
    let color = if active { theme::RED() } else { theme::MUTED() };
    let r = ui.add(egui::Button::new(egui::RichText::new(icon).font(theme::icon_font(16.0)).color(color)).frame(false).min_size(Vec2::splat(30.0)));
    if r.hovered() {
        ui.painter().rect_filled(r.rect, 4, theme::HOVER().gamma_multiply(0.6));
    }
    r.on_hover_text(tooltip)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// Marks a response as clickable only for a context menu (or a backdrop that closes something), so it keeps
/// the arrow cursor; see [`pointer_cursor`].
pub fn plain_cursor(r: &Response) {
    r.ctx.data_mut(|d| d.get_temp_mut_or_default::<PlainCursor>(egui::Id::NULL).0.insert(r.id));
}

#[derive(Clone, Default)]
struct PlainCursor(std::collections::HashSet<egui::Id>);

/// Run after the frame's UI: shows the pointing hand over whatever a click would hit, unless the widget
/// chose a cursor of its own or was marked with [`plain_cursor`]. Widgets that can also be dragged (text,
/// sliders, modal backdrops) aren't buttons, so they're left alone; give them a cursor where it fits.
pub fn pointer_cursor(ctx: &egui::Context) {
    let plain = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<PlainCursor>(egui::Id::NULL).0));
    if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Default {
        return;
    }
    let Some(pos) = ctx.pointer_hover_pos() else { return };
    let Some(hit) = ctx.viewport(|vp| vp.hits.click) else { return };
    // An area's own background (popups and menus take clicks to come to the front) isn't a control.
    let background = hit.id == hit.layer_id.id.with("move");
    let button = hit.sense.senses_click() && !hit.sense.senses_drag();
    if hit.enabled && button && hit.interact_rect.contains(pos) && !background && !plain.contains(&hit.id) {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
