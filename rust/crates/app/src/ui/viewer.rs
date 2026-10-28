//! The image viewer: a picture clicked in chat, large over a dark backdrop, as Discord shows it. A click
//! outside it or Escape closes it; "Open in Browser" is still there underneath.

use eframe::egui::{self, Color32, RichText, Ui, Vec2};

use crate::app::UiState;
use crate::ui::message::fit;

/// A picture to look at full size.
#[derive(Debug, Clone)]
pub struct Viewed {
    /// Discord's media proxy copy (resized to the screen when shown), or the original if there isn't one.
    pub proxy: String,
    /// What "Open in Browser" opens.
    pub original: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Animated: fetched as a GIF rather than a still WebP.
    pub gif: bool,
}

pub fn show(ui: &Ui, uis: &mut UiState) {
    let Some(v) = uis.viewer.clone() else { return };
    let ctx = ui.ctx();
    let screen = ctx.content_rect();
    // Room for the picture, leaving a margin and a line for the link below.
    let room = Vec2::new(screen.width() * 0.9, screen.height() * 0.9 - 36.0);
    let ppp = ctx.pixels_per_point();
    let known = v.width.zip(v.height).is_some();
    // Never larger than the picture itself; fetched at the size it's shown, so a big photo isn't decoded whole.
    let size = fit(v.width, v.height, room);
    let (pw, ph) = ((size.x * ppp) as u32, (size.y * ppp) as u32);
    let url = match (known, v.gif) {
        (false, _) => v.proxy.clone(),
        (true, true) => {
            let sep = if v.proxy.contains('?') { '&' } else { '?' };
            format!("{}{sep}width={pw}&height={ph}", v.proxy)
        }
        (true, false) => swiftcord_core::cdn::media_proxy(&v.proxy, pw, ph),
    };
    let modal = egui::Modal::new(egui::Id::unique("image-viewer")).backdrop_color(Color32::from_black_alpha(215)).frame(egui::Frame::NONE).show(ctx, |ui| {
        let image = egui::Image::from_uri(url).corner_radius(4).show_loading_spinner(true);
        let image = if known { image.fit_to_exact_size(size) } else { image.max_size(room).shrink_to_fit() };
        ui.add(image);
        ui.add_space(6.0);
        let link = ui.add(egui::Label::new(RichText::new("Open in Browser").size(14.0).color(Color32::from_gray(220))).sense(egui::Sense::click()));
        let link = link.on_hover_cursor(egui::CursorIcon::PointingHand);
        if link.hovered() {
            ui.painter().hline(link.rect.x_range(), link.rect.bottom(), egui::Stroke::new(1.0, Color32::from_gray(220)));
        }
        if link.clicked() {
            crate::platform::open_url(&v.original);
        }
    });
    if modal.should_close() {
        uis.viewer = None;
    }
}
