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

/// A VGUI icon button.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    icon_button_sized(ui, icon, tooltip, 30.0)
}

/// A VGUI icon button `side` points square.
pub fn icon_button_sized(ui: &mut Ui, icon: &str, tooltip: &str, side: f32) -> Response {
    vgui_button(ui, Vec2::splat(side), false, |ui, rect| glyph(ui, rect, icon, theme::HEADER())).on_hover_text(tooltip)
}

/// A VGUI icon button that stays pressed in while `active` (e.g. mute), its icon red then.
pub fn toggle_icon(ui: &mut Ui, icon: &str, active: bool, tooltip: &str) -> Response {
    toggle_icon_colored(ui, icon, active, theme::RED(), tooltip)
}

/// As [`toggle_icon`], with the icon in `active_color` while active.
pub fn toggle_icon_colored(ui: &mut Ui, icon: &str, active: bool, active_color: Color32, tooltip: &str) -> Response {
    let color = if active { active_color } else { theme::HEADER() };
    vgui_button(ui, Vec2::splat(30.0), active, |ui, rect| glyph(ui, rect, icon, color)).on_hover_text(tooltip)
}

fn glyph(ui: &Ui, rect: egui::Rect, icon: &str, color: Color32) {
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, theme::icon_font(15.0), color);
}

/// A VGUI button of `size`, laid out in place, its contents drawn by `paint` into the rect it's given.
/// Sunk while pressed (or while `held`, for toggles), outlined while it has keyboard focus.
pub fn vgui_button(ui: &mut Ui, size: Vec2, held: bool, paint: impl FnOnce(&Ui, egui::Rect)) -> Response {
    let (_, r) = ui.allocate_exact_size(size, Sense::click());
    paint_vgui_button(ui, &r, held, paint);
    r
}

/// A VGUI button placed at `rect` rather than laid out (see [`vgui_button`]).
pub fn vgui_button_at(ui: &Ui, id: egui::Id, rect: egui::Rect, held: bool, paint: impl FnOnce(&Ui, egui::Rect)) -> Response {
    let r = ui.interact(rect, id, Sense::click());
    paint_vgui_button(ui, &r, held, paint);
    r
}

/// Places `n` squares of `side` points in a row at the right of `area`, `gap` apart and `gap` in from its
/// right edge, centred down it. On whole pixels: laid out in points, the gaps would round to different widths.
pub fn pixel_row(ui: &Ui, area: egui::Rect, n: usize, side: f32, gap: f32) -> Vec<egui::Rect> {
    let ppp = ui.ctx().pixels_per_point();
    let (s, g) = ((side * ppp).round(), (gap * ppp).round());
    let right = (area.right() * ppp).round() - g;
    let top = (area.center().y * ppp - s / 2.0).round();
    (0..n)
        .map(|i| {
            let x1 = right - (n - 1 - i) as f32 * (s + g);
            egui::Rect::from_min_max(egui::pos2((x1 - s) / ppp, top / ppp), egui::pos2(x1 / ppp, (top + s) / ppp))
        })
        .collect()
}

/// Draws a VGUI button over `r` (see `theme::button_frame`). VGUI buttons keep the arrow cursor.
fn paint_vgui_button(ui: &Ui, r: &Response, held: bool, paint: impl FnOnce(&Ui, egui::Rect)) {
    plain_cursor(r);
    let ppp = ui.ctx().pixels_per_point();
    let look = if r.is_pointer_button_down_on() || held {
        theme::ButtonLook::Pressed
    } else if r.has_focus() {
        theme::ButtonLook::Focused
    } else {
        theme::ButtonLook::Default
    };
    ui.painter().add(theme::button_frame(r.rect, look, ppp));
    // Pressed, the contents move right as Source's labels do.
    let nudge = if look == theme::ButtonLook::Pressed { Vec2::new(2.0 * ppp.floor().max(1.0) / ppp, 0.0) } else { Vec2::ZERO };
    paint(ui, r.rect.translate(nudge));
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

/// A VGUI scrollbar for a vertical scroll area, drawn after it in the room it keeps on its right, pixel for
/// pixel as Source's: bevelled arrow buttons at both ends (scrolling while held), a bevelled thumb on a flat
/// track. Drawn at whole multiples of its size (1x up to 200% display scaling) so its 1 px edges stay
/// crisp, and it doesn't change when pointed at or pressed. Returns the new offset when it moved the area.
pub fn scrollbar<R>(ui: &Ui, out: &egui::scroll_area::ScrollAreaOutput<R>) -> Option<f32> {
    let inner = out.inner_rect;
    let max_offset = out.content_size.y - inner.height();
    let ppp = ui.ctx().pixels_per_point();
    // Laid out in physical pixels: the bar's design is 17 px wide, its buttons 18 px tall.
    let scale = ppp.floor().max(1.0);
    let (bar_w, button_h) = (17.0 * scale, 18.0 * scale);
    let right = (inner.right() * ppp).round();
    let (top, bottom) = ((inner.top() * ppp).round(), (inner.bottom() * ppp).round());
    let left = right - bar_w;
    if max_offset < 1.0 || bottom - top < button_h * 2.0 + 8.0 * scale {
        return None;
    }
    let track = (top + button_h, bottom - button_h);
    let track_len = track.1 - track.0;
    let thumb_len = (track_len * inner.height() / out.content_size.y).round().clamp(8.0 * scale, track_len);
    let travel = (track_len - thumb_len).max(1.0);
    let thumb_top = |offset: f32| track.0 + (travel * offset / max_offset).round();
    // Physical pixels to points.
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(egui::pos2(x0 / ppp, y0 / ppp), egui::pos2(x1 / ppp, y1 / ppp));

    let start = out.state.clamped_offset().y.clamp(0.0, max_offset);
    let id = out.id.with("vgui-scrollbar");
    // Registered back to front: the thumb and arrows take clicks over the track.
    let track_r = ui.interact(rect(left, track.0, right, track.1), id.with("track"), Sense::click());
    let thumb_r = ui.interact(rect(left, thumb_top(start), right, thumb_top(start) + thumb_len), id.with("thumb"), Sense::drag());
    let up_r = ui.interact(rect(left, top, right, top + button_h), id.with("up"), Sense::click());
    let down_r = ui.interact(rect(left, bottom - button_h, right, bottom), id.with("down"), Sense::click());

    let mut offset = start;
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    for (r, dir) in [(&up_r, -1.0), (&down_r, 1.0)] {
        if r.is_pointer_button_down_on() {
            offset += dir * 600.0 * dt;
            ui.ctx().request_repaint();
        }
    }
    if track_r.clicked() {
        // A page towards where it was clicked.
        if let Some(p) = track_r.interact_pointer_pos() {
            let y = p.y * ppp;
            if y < thumb_top(start) {
                offset -= inner.height();
            } else if y > thumb_top(start) + thumb_len {
                offset += inner.height();
            }
        }
    }
    offset += thumb_r.drag_delta().y * ppp * max_offset / travel;
    let offset = offset.clamp(0.0, max_offset);
    let moved = (offset - start).abs() > 0.01;
    if moved {
        set_scroll_offset(ui.ctx(), out.id, &out.state, offset);
        ui.ctx().request_repaint();
    }

    // A mesh of whole-pixel quads: egui's shapes are feathered, which would blur the 1 px edges.
    let mut mesh = egui::Mesh::default();
    let mut px = |x0: f32, y0: f32, x1: f32, y1: f32, c: u32| mesh.add_colored_rect(rect(x0, y0, x1, y1), rgb(c));
    // A bevelled box: light top and left edges, dark right and bottom ones, 1 design pixel wide. The buttons'
    // top edge runs the full width; the thumb's right edge takes its top-right pixel.
    let bevel = |px: &mut dyn FnMut(f32, f32, f32, f32, u32), y0: f32, y1: f32, fill: u32, light: (u32, u32), dark: (u32, u32), full_top: bool| {
        let s = scale;
        px(left, y0, right, y1, fill);
        px(left, y0, if full_top { right } else { right - s }, y0 + s, light.0);
        px(left, y0 + s, left + s, y1 - s, light.0);
        px(left, y0, left + s, y0 + s, light.1);
        px(right - s, if full_top { y0 + s } else { y0 }, right, y1 - s, dark.0);
        px(left, y1 - s, right - s, y1, dark.0);
        px(right - s, y1 - s, right, y1, dark.1);
    };
    px(left, track.0, right, track.1, 0x9b9b9b);
    let t = thumb_top(offset);
    bevel(&mut px, t, t + thumb_len, 0x9b9c9c, (0xc0c0c0, 0xc7c7c7), (0x545454, 0x333333), false);
    for (y0, fill, pointing_down) in [(top, 0x5c5e5e, false), (bottom - button_h, 0x5a5b5b, true)] {
        bevel(&mut px, y0, y0 + button_h, fill, (0xb7b7b7, 0xc5c5c5), (0x373838, 0x292929), true);
        // The arrow: five rows widening by a pixel each side, with Source's antialiased edge pixels.
        for k in 0..5 {
            let row = if pointing_down { 11 - k } else { 8 + k } as f32;
            let y = y0 + row * scale;
            let cell = |px: &mut dyn FnMut(f32, f32, f32, f32, u32), x: i32, c: u32| px(left + x as f32 * scale, y, left + (x + 1) as f32 * scale, y + scale, c);
            let k = k as i32;
            cell(&mut px, 8 - k, 0x878888);
            cell(&mut px, 10 + k, 0x878888);
            if k == 0 {
                cell(&mut px, 9, 0xebebeb);
            } else {
                cell(&mut px, 9 - k, 0xf5f5f5);
                cell(&mut px, 9 + k, 0xf5f5f5);
                for x in 10 - k..9 + k {
                    cell(&mut px, x, 0xffffff);
                }
            }
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    moved.then_some(offset)
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Moves a scroll area to `y` from the next frame. egui's scroll state has no setter, but its serde form has
/// the offset, so it goes out and back through that.
fn set_scroll_offset(ctx: &egui::Context, id: egui::Id, state: &egui::scroll_area::State, y: f32) {
    let Ok(mut value) = serde_json::to_value(state) else { return };
    let Some(offset) = value.get_mut("offset").and_then(|o| o.get_mut("y")) else { return };
    *offset = serde_json::json!(y);
    if let Ok(state) = serde_json::from_value::<egui::scroll_area::State>(value) {
        state.store(ctx, id);
    }
}

/// A one-pixel line down from `y.min` to `y.max`, in the pixel column starting at `x`: crisp, where egui's
/// lines are feathered across two.
pub fn pixel_vline(painter: &egui::Painter, x: f32, y: egui::Rangef, color: Color32) {
    let ppp = painter.ctx().pixels_per_point();
    let x = (x * ppp).round() / ppp;
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(egui::Rect::from_x_y_ranges(x..=x + 1.0 / ppp, y), color);
    painter.add(egui::Shape::mesh(mesh));
}

/// As [`pixel_vline`], across: the pixel row starting at `y`.
pub fn pixel_hline(painter: &egui::Painter, x: egui::Rangef, y: f32, color: Color32) {
    let ppp = painter.ctx().pixels_per_point();
    let y = (y * ppp).round() / ppp;
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(egui::Rect::from_x_y_ranges(x, y..=y + 1.0 / ppp), color);
    painter.add(egui::Shape::mesh(mesh));
}

/// One-pixel bevel lines just inside `rect` (`inset` pixels in): `top_left` along the top and left, `bottom_right`
/// along the bottom and right. Whole pixels (at whole multiples, as the scrollbar), so the edges stay crisp.
fn bevel(painter: &egui::Painter, rect: egui::Rect, top_left: Color32, bottom_right: Color32, inset: f32) {
    let ppp = painter.ctx().pixels_per_point();
    let s = ppp.floor().max(1.0);
    let i = inset * s;
    let (x0, x1) = ((rect.left() * ppp).round() + i, (rect.right() * ppp).round() - i);
    let (y0, y1) = ((rect.top() * ppp).round() + i, (rect.bottom() * ppp).round() - i);
    let r = |a: f32, b: f32, c: f32, d: f32| egui::Rect::from_min_max(egui::pos2(a / ppp, b / ppp), egui::pos2(c / ppp, d / ppp));
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(r(x0, y0, x1 - s, y0 + s), top_left);
    mesh.add_colored_rect(r(x0, y0 + s, x0 + s, y1 - s), top_left);
    mesh.add_colored_rect(r(x0, y1 - s, x1, y1), bottom_right);
    mesh.add_colored_rect(r(x1 - s, y0, x1, y1 - s), bottom_right);
    painter.add(egui::Shape::mesh(mesh));
}

/// VGUI's sunken edge, as around text fields: dark above and left, light below and right.
pub fn sunken_edge(painter: &egui::Painter, rect: egui::Rect) {
    bevel(painter, rect, rgb(0x353535), rgb(0xb7b7b7), 0.0);
}

/// A VGUI icon button placed at `rect` rather than laid out.
pub fn vgui_icon_button(ui: &Ui, id: egui::Id, rect: egui::Rect, icon: &str) -> Response {
    let r = ui.interact(rect, id, Sense::click());
    paint_vgui_button(ui, &r, false, |ui, rect| glyph(ui, rect, icon, theme::HEADER()));
    r
}
