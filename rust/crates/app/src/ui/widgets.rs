//! Small shared widgets.

use eframe::egui::{self, Color32, CornerRadius, Response, Sense, Ui, Vec2};
use swiftcord_core::models::{PresenceStatus, User};

use crate::theme;

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
        PresenceStatus::Offline => theme::FAINT,
    }
}

/// An avatar with a presence dot in its bottom-right corner.
pub fn avatar_with_status(ui: &mut Ui, user: &User, size: f32, status: PresenceStatus, bg: Color32) -> Response {
    let r = user_avatar(ui, user, size);
    let dot = size * 0.3;
    let c = r.rect.right_bottom() - Vec2::splat(dot * 0.5);
    ui.painter().circle_filled(c, dot * 0.5 + 2.0, bg);
    ui.painter().circle_filled(c, dot * 0.5, status_color(status));
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
