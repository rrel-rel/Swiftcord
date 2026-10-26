//! Small shared widgets.

use eframe::egui::{self, Color32, CornerRadius, Response, Sense, Ui, Vec2};
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
    ui.painter().rect_filled(b, 4, if r.hovered() { theme::SELECTED } else { theme::SIDEBAR });
    ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, "Jump", theme::font(12.0), theme::HEADER);
    r.clicked()
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
    let px = if size <= 32.0 { 64 } else { 128 };
    avatar(ui, &swiftcord_core::cdn::avatar(user, px), size)
}

pub fn status_color(s: PresenceStatus) -> Color32 {
    match s {
        PresenceStatus::Online => theme::GREEN,
        PresenceStatus::Idle => theme::YELLOW,
        PresenceStatus::Dnd => theme::RED,
        PresenceStatus::Offline => theme::OFFLINE,
    }
}

/// A status dot in Discord's shapes, cut out with `bg`: a moon for idle, a bar for do not disturb and a ring
/// for offline, so they read without colour too.
pub fn status_dot(painter: &egui::Painter, c: egui::Pos2, r: f32, status: PresenceStatus, bg: Color32) {
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

/// Paints an avatar into `rect` with a status dot in its bottom-right corner, ringed with `bg`.
pub fn paint_avatar_with_status(ui: &Ui, user: &User, rect: egui::Rect, status: PresenceStatus, bg: Color32) {
    let size = rect.width();
    let px = if size <= 32.0 { 64 } else { 128 };
    egui::Image::from_uri(swiftcord_core::cdn::avatar(user, px))
        .corner_radius(CornerRadius::same((size / 2.0) as u8))
        .show_loading_spinner(false)
        .paint_at(ui, rect);
    let dot = size * 0.3;
    let c = rect.right_bottom() - Vec2::splat(dot * 0.5);
    ui.painter().circle_filled(c, dot * 0.5 + 2.0, bg);
    status_dot(ui.painter(), c, dot * 0.5, status, bg);
}

/// An avatar with a presence dot in its bottom-right corner.
pub fn avatar_with_status(ui: &mut Ui, user: &User, size: f32, status: PresenceStatus, bg: Color32) -> Response {
    let r = user_avatar(ui, user, size);
    let dot = size * 0.3;
    let c = r.rect.right_bottom() - Vec2::splat(dot * 0.5);
    ui.painter().circle_filled(c, dot * 0.5 + 2.0, bg);
    status_dot(ui.painter(), c, dot * 0.5, status, bg);
    r
}

/// A red pill with a count, e.g. unread mentions.
pub fn badge(ui: &Ui, center: egui::Pos2, count: u32, outline: Color32) {
    let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
    let galley = ui.painter().layout_no_wrap(text, theme::font_bold(11.0), Color32::WHITE);
    let w = (galley.size().x + 9.0).max(16.0);
    let rect = egui::Rect::from_center_size(center, egui::vec2(w, 16.0));
    ui.painter().rect_filled(rect.expand(3.0), 11, outline);
    ui.painter().rect_filled(rect, 8, theme::RED);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// A flat icon button that highlights on hover.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let r = ui.add(
        egui::Button::new(egui::RichText::new(icon).font(theme::icon_font(16.0)).color(theme::MUTED))
            .frame(false)
            .min_size(Vec2::splat(28.0)),
    );
    if r.hovered() {
        ui.painter().rect_filled(r.rect, 4, theme::HOVER.gamma_multiply(0.6));
    }
    r.on_hover_text(tooltip)
}

/// A toggle-style icon button (e.g. mute) that turns red when active.
pub fn toggle_icon(ui: &mut Ui, icon: &str, active: bool, tooltip: &str) -> Response {
    let color = if active { theme::RED } else { theme::MUTED };
    let r = ui.add(egui::Button::new(egui::RichText::new(icon).font(theme::icon_font(16.0)).color(color)).frame(false).min_size(Vec2::splat(30.0)));
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
