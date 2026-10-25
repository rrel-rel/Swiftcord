//! The GIF picker: trending categories, then Tenor search results in two columns. Results show still
//! previews and only animate on hover, and animated GIFs are freed when the picker closes, to keep memory low.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, RichText, Sense, Ui, Vec2};
use swiftcord_core::rest::Gif;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::theme;

const SIZE: Vec2 = Vec2::new(440.0, 480.0);
const DEBOUNCE: Duration = Duration::from_millis(350);

pub struct GifPicker {
    pub channel: Snowflake,
    at: Pos2,
    query: String,
    edited: Instant,
    /// The query whose results are showing or loading ("" is trending); `None` shows the categories.
    pub requested: Option<String>,
    pub results: Option<Vec<Gif>>,
    pub categories: Option<Vec<String>>,
    categories_requested: bool,
    /// Image URIs loaded so far (animations and stills), freed on close.
    pub animated: HashSet<String>,
    opened: Instant,
}

impl GifPicker {
    pub fn new(channel: Snowflake, at: Pos2) -> Self {
        Self {
            channel,
            at,
            query: String::new(),
            edited: Instant::now(),
            requested: None,
            results: None,
            categories: None,
            categories_requested: false,
            animated: HashSet::new(),
            opened: Instant::now(),
        }
    }

    fn open_results(&mut self, query: String, actions: &mut Vec<Action>) {
        self.results = None;
        self.requested = Some(query.clone());
        actions.push(Action::GifSearch(query));
    }
}

/// Category tiles get a colour from their name, so the grid isn't uniform.
fn tile_color(name: &str) -> Color32 {
    let h = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let palette = [
        Color32::from_rgb(0x5d, 0x3f, 0x8e),
        Color32::from_rgb(0x2f, 0x6a, 0x8f),
        Color32::from_rgb(0x8e, 0x3f, 0x5d),
        Color32::from_rgb(0x3f, 0x7e, 0x5a),
        Color32::from_rgb(0x8a, 0x5a, 0x2c),
        Color32::from_rgb(0x4a, 0x4f, 0x9a),
    ];
    palette[h as usize % palette.len()]
}

pub fn show(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(p) = &mut uis.gif_picker else { return };
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let mut pos = p.at - Vec2::new(SIZE.x, SIZE.y + 8.0);
    pos.x = pos.x.clamp(screen.left() + 8.0, (screen.right() - SIZE.x - 8.0).max(screen.left() + 8.0));
    pos.y = pos.y.clamp(screen.top() + 8.0, (screen.bottom() - SIZE.y - 8.0).max(screen.top() + 8.0));

    // Search as the user pauses typing.
    let typed = p.query.trim().to_owned();
    if !typed.is_empty() && p.requested.as_deref() != Some(typed.as_str()) && p.edited.elapsed() >= DEBOUNCE {
        p.open_results(typed, actions);
    } else if !typed.is_empty() && p.requested.as_deref() != Some(typed.as_str()) {
        ctx.request_repaint_after(DEBOUNCE);
    }
    if p.requested.is_none() && !p.categories_requested {
        p.categories_requested = true;
        actions.push(Action::GifCategories);
    }

    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let area = egui::Area::new(egui::Id::unique("gif-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR).inner_margin(10).show(ui, |ui| {
            ui.set_width(SIZE.x - 20.0);
            ui.set_height(SIZE.y - 20.0);
            ui.horizontal(|ui| {
                if p.requested.is_some() && ui.button("←").on_hover_text("Back to categories").clicked() {
                    p.requested = None;
                    p.results = None;
                    p.query.clear();
                }
                let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text("Search Tenor").desired_width(f32::INFINITY));
                if r.changed() {
                    p.edited = Instant::now();
                    if p.query.trim().is_empty() {
                        p.requested = None;
                        p.results = None;
                    }
                }
                if p.opened.elapsed() < Duration::from_millis(300) {
                    r.request_focus();
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let width = ui.available_width();
                let col_w = (width - 6.0) / 2.0;
                match (&p.requested, &p.results) {
                    (None, _) => {
                        // Categories, with "Trending" first.
                        let mut names = vec!["Trending GIFs".to_owned()];
                        names.extend(p.categories.clone().unwrap_or_default());
                        let mut picked = None;
                        for row in names.chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                for name in row {
                                    let (rect, r) = ui.allocate_exact_size(Vec2::new(col_w, 64.0), Sense::click());
                                    let fill = if r.hovered() { tile_color(name).gamma_multiply(1.25) } else { tile_color(name) };
                                    ui.painter().rect_filled(rect, 6, fill);
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, name, theme::font_bold(15.0), Color32::WHITE);
                                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        picked = Some(name.clone());
                                    }
                                }
                            });
                        }
                        if p.categories.is_none() {
                            ui.add_space(8.0);
                            ui.vertical_centered(|ui| ui.spinner());
                        }
                        if let Some(name) = picked {
                            if name == "Trending GIFs" {
                                p.open_results(String::new(), actions);
                            } else {
                                p.query = name.clone();
                                p.open_results(name, actions);
                            }
                        }
                    }
                    (Some(_), None) => {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| ui.spinner());
                    }
                    (Some(_), Some(gifs)) if gifs.is_empty() => {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("No GIFs found.").color(theme::MUTED)));
                    }
                    (Some(_), Some(gifs)) => {
                        // Masonry: each GIF goes into the shorter column.
                        let mut heights = [0.0f32; 2];
                        let mut cols: [Vec<&Gif>; 2] = [Vec::new(), Vec::new()];
                        for g in gifs {
                            let h = col_w * g.height.max(1) as f32 / g.width.max(1) as f32;
                            let c = if heights[0] <= heights[1] { 0 } else { 1 };
                            heights[c] += h + 6.0;
                            cols[c].push(g);
                        }
                        let mut send = None;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                            for col in &cols {
                                ui.vertical(|ui| {
                                    ui.set_width(col_w);
                                    for g in col {
                                        let h = (col_w * g.height.max(1) as f32 / g.width.max(1) as f32).clamp(40.0, 400.0);
                                        let (rect, r) = ui.allocate_exact_size(Vec2::new(col_w, h), Sense::click());
                                        if !ui.is_rect_visible(rect) {
                                            continue;
                                        }
                                        ui.painter().rect_filled(rect, 4, theme::RAIL);
                                        // Still previews, animating only under the pointer.
                                        let uri = if r.hovered() && !g.gif_src.is_empty() {
                                            p.animated.insert(g.gif_src.clone());
                                            g.gif_src.clone()
                                        } else if is_still(&g.preview) {
                                            g.preview.clone()
                                        } else {
                                            // The "preview" is often a video; show the animation's first frame instead.
                                            format!("{}{}", crate::media::STILL, g.gif_src)
                                        };
                                        p.animated.insert(uri.clone());
                                        egui::Image::from_uri(uri).corner_radius(4).show_loading_spinner(false).paint_at(ui, rect);
                                        if r.on_hover_text(&g.title).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                            send = Some(g.url.clone());
                                        }
                                    }
                                });
                            }
                        });
                        if let Some(url) = send {
                            actions.push(Action::SendGif { channel: p.channel, url });
                        }
                    }
                }
            });
        });
    });
    // Clicking outside closes it (but not the click that opened it).
    if p.opened.elapsed() > Duration::from_millis(200) && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() {
        close = true;
    }
    if close {
        actions.push(Action::CloseGifPicker);
    }
}

/// Whether a URL is a still image we can show as is.
fn is_still(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or("").to_ascii_lowercase();
    [".png", ".jpg", ".jpeg"].iter().any(|e| path.ends_with(e))
}
