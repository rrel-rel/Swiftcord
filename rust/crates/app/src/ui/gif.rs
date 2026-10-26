//! The GIF picker: Favourites and trending categories, then search results in two columns. Tiles show still
//! frames and only play under the pointer, and everything the picker loaded is freed when it closes.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::proto::FavoriteGif;
use swiftcord_core::rest::Gif;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::clip::Clips;
use crate::media::STILL;
use crate::theme;
use crate::theme::icon::{STAR, STAR_FILLED};

const SIZE: Vec2 = Vec2::new(440.0, 480.0);
const DEBOUNCE: Duration = Duration::from_millis(350);
const FAVORITES: &str = "Favourites";
const TRENDING: &str = "Trending GIFs";

#[derive(Debug, Clone, PartialEq, Eq)]
enum View {
    Categories,
    Favorites,
    /// Search results ("" is trending).
    Results(String),
}

pub struct GifPicker {
    pub channel: Snowflake,
    at: Pos2,
    query: String,
    edited: Instant,
    view: View,
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
            view: View::Categories,
            results: None,
            categories: None,
            categories_requested: false,
            animated: HashSet::new(),
            opened: Instant::now(),
        }
    }

    /// The query whose results are showing or loading, for matching responses.
    pub fn requested(&self) -> Option<&str> {
        match &self.view {
            View::Results(q) => Some(q),
            _ => None,
        }
    }

    fn open_results(&mut self, query: String, actions: &mut Vec<Action>) {
        self.results = None;
        self.view = View::Results(query.clone());
        actions.push(Action::GifSearch(query));
    }
}

/// One GIF as the grid shows it.
struct Tile {
    url: String,
    /// A still image (or `still+` URI) for when it isn't hovered.
    still: String,
    /// What plays on hover: an MP4 clip or an animated image.
    play: Play,
    width: u32,
    height: u32,
    title: String,
    favorite: FavoriteGif,
}

enum Play {
    Clip(String),
    Image(String),
}

fn tile_from_gif(g: &Gif) -> Tile {
    let (play, favorite) = if !g.src.is_empty() {
        (Play::Clip(g.src.clone()), FavoriteGif { url: g.url.clone(), src: g.src.clone(), format: 2, width: g.width, height: g.height, order: 0 })
    } else {
        (Play::Image(g.gif_src.clone()), FavoriteGif { url: g.url.clone(), src: g.gif_src.clone(), format: 1, width: g.width, height: g.height, order: 0 })
    };
    let still = if is_still(&g.preview) { g.preview.clone() } else { format!("{STILL}{}", g.gif_src) };
    Tile { url: g.url.clone(), still, play, width: g.width, height: g.height, title: g.title.clone(), favorite }
}

fn tile_from_favorite(f: &FavoriteGif) -> Tile {
    let play = if f.format == 2 { Play::Clip(f.src.clone()) } else { Play::Image(f.src.clone()) };
    Tile { url: f.url.clone(), still: format!("{STILL}{}", f.src), play, width: f.width, height: f.height, title: String::new(), favorite: f.clone() }
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

/// What happened in the grid.
enum Picked {
    Send(String),
    ToggleFavorite(FavoriteGif),
}

/// Two-column masonry of GIF tiles; each goes into the shorter column.
fn grid(ui: &mut Ui, tiles: &[Tile], favorites: &HashSet<&str>, clips: &mut Clips, loaded: &mut HashSet<String>) -> Option<Picked> {
    let col_w = (ui.available_width() - 6.0) / 2.0;
    let mut heights = [0.0f32; 2];
    let mut cols: [Vec<&Tile>; 2] = [Vec::new(), Vec::new()];
    for t in tiles {
        let h = col_w * t.height.max(1) as f32 / t.width.max(1) as f32;
        let c = if heights[0] <= heights[1] { 0 } else { 1 };
        heights[c] += h + 6.0;
        cols[c].push(t);
    }
    let mut picked = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for col in &cols {
            ui.vertical(|ui| {
                ui.set_width(col_w);
                for t in col {
                    let h = (col_w * t.height.max(1) as f32 / t.width.max(1) as f32).clamp(40.0, 400.0);
                    let (rect, r) = ui.allocate_exact_size(Vec2::new(col_w, h), Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    ui.painter().rect_filled(rect, 4, theme::RAIL);
                    let hovered = r.hovered();
                    if hovered {
                        match &t.play {
                            Play::Clip(url) => clips.paint(ui, rect, url, Some(&t.still)),
                            Play::Image(url) => {
                                loaded.insert(url.clone());
                                egui::Image::from_uri(url.clone()).corner_radius(4).show_loading_spinner(false).paint_at(ui, rect);
                            }
                        }
                    } else {
                        loaded.insert(t.still.clone());
                        egui::Image::from_uri(t.still.clone()).corner_radius(4).show_loading_spinner(false).paint_at(ui, rect);
                    }
                    // A star in the corner adds or removes the favourite.
                    let starred = favorites.contains(t.url.as_str());
                    let star = Rect::from_min_size(rect.right_top() + Vec2::new(-30.0, 4.0), Vec2::splat(26.0));
                    if hovered || starred {
                        let over_star = ui.rect_contains_pointer(star);
                        ui.painter().circle_filled(star.center(), 13.0, Color32::from_black_alpha(if over_star { 200 } else { 140 }));
                        let (glyph, color) = if starred { (STAR_FILLED, theme::YELLOW) } else { (STAR, Color32::WHITE) };
                        ui.painter().text(star.center(), egui::Align2::CENTER_CENTER, glyph, theme::icon_font(14.0), color);
                    }
                    let r = if t.title.is_empty() { r } else { r.on_hover_text(&t.title) };
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        picked = Some(if ui.rect_contains_pointer(star) {
                            Picked::ToggleFavorite(t.favorite.clone())
                        } else {
                            Picked::Send(t.url.clone())
                        });
                    }
                }
            });
        }
    });
    picked
}

pub fn show(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let UiState { gif_picker, favorite_gifs, clips, .. } = uis;
    let Some(p) = gif_picker else { return };
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    let mut pos = p.at - Vec2::new(SIZE.x, SIZE.y + 8.0);
    pos.x = pos.x.clamp(screen.left() + 8.0, (screen.right() - SIZE.x - 8.0).max(screen.left() + 8.0));
    pos.y = pos.y.clamp(screen.top() + 8.0, (screen.bottom() - SIZE.y - 8.0).max(screen.top() + 8.0));

    // Search as the user pauses typing.
    let typed = p.query.trim().to_owned();
    if !typed.is_empty() && p.requested() != Some(typed.as_str()) {
        if p.edited.elapsed() >= DEBOUNCE {
            p.open_results(typed, actions);
        } else {
            ctx.request_repaint_after(DEBOUNCE);
        }
    }
    if p.view == View::Categories && !p.categories_requested {
        p.categories_requested = true;
        actions.push(Action::GifCategories);
    }
    let favorite_urls: HashSet<&str> = favorite_gifs.iter().map(|f| f.url.as_str()).collect();

    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut picked = None;
    let area = egui::Area::new(egui::Id::unique("gif-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR).inner_margin(10).show(ui, |ui| {
            ui.set_width(SIZE.x - 20.0);
            ui.set_height(SIZE.y - 20.0);
            ui.horizontal(|ui| {
                if p.view != View::Categories && ui.button("←").on_hover_text("Back").clicked() {
                    p.view = View::Categories;
                    p.results = None;
                    p.query.clear();
                }
                let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text("Search for GIFs").desired_width(f32::INFINITY));
                if r.changed() {
                    p.edited = Instant::now();
                    if p.query.trim().is_empty() {
                        p.view = View::Categories;
                        p.results = None;
                    }
                }
                if p.opened.elapsed() < Duration::from_millis(300) {
                    r.request_focus();
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let col_w = (ui.available_width() - 6.0) / 2.0;
                match &p.view {
                    View::Categories => {
                        let mut names = vec![FAVORITES.to_owned(), TRENDING.to_owned()];
                        names.extend(p.categories.clone().unwrap_or_default());
                        let mut chosen = None;
                        for row in names.chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                for name in row {
                                    let (rect, r) = ui.allocate_exact_size(Vec2::new(col_w, 64.0), Sense::click());
                                    let base = if name == FAVORITES { theme::BLURPLE } else { tile_color(name) };
                                    ui.painter().rect_filled(rect, 6, if r.hovered() { base.gamma_multiply(1.25) } else { base });
                                    if name == FAVORITES {
                                        ui.painter().text(rect.center() - Vec2::new(46.0, 0.0), egui::Align2::CENTER_CENTER, STAR_FILLED, theme::icon_font(15.0), Color32::WHITE);
                                        ui.painter().text(rect.center() + Vec2::new(10.0, 0.0), egui::Align2::CENTER_CENTER, name, theme::font_bold(15.0), Color32::WHITE);
                                    } else {
                                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, name, theme::font_bold(15.0), Color32::WHITE);
                                    }
                                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        chosen = Some(name.clone());
                                    }
                                }
                            });
                        }
                        if p.categories.is_none() {
                            ui.add_space(8.0);
                            ui.vertical_centered(|ui| ui.spinner());
                        }
                        match chosen.as_deref() {
                            Some(FAVORITES) => p.view = View::Favorites,
                            Some(TRENDING) => p.open_results(String::new(), actions),
                            Some(name) => {
                                p.query = name.to_owned();
                                p.open_results(name.to_owned(), actions);
                            }
                            None => {}
                        }
                    }
                    View::Favorites => {
                        if favorite_gifs.is_empty() {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("No favourites yet.").color(theme::MUTED));
                                ui.label(RichText::new("Hover over a GIF and click the star to add one.").small().color(theme::FAINT));
                            });
                        } else {
                            let tiles: Vec<Tile> = favorite_gifs.iter().map(tile_from_favorite).collect();
                            picked = grid(ui, &tiles, &favorite_urls, clips, &mut p.animated);
                        }
                    }
                    View::Results(_) => match &p.results {
                        None => {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| ui.spinner());
                        }
                        Some(gifs) if gifs.is_empty() => {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| ui.label(RichText::new("No GIFs found.").color(theme::MUTED)));
                        }
                        Some(gifs) => {
                            let tiles: Vec<Tile> = gifs.iter().map(tile_from_gif).collect();
                            picked = grid(ui, &tiles, &favorite_urls, clips, &mut p.animated);
                        }
                    },
                }
            });
        });
    });
    match picked {
        Some(Picked::Send(url)) => actions.push(Action::SendGif { channel: p.channel, url }),
        Some(Picked::ToggleFavorite(f)) => actions.push(Action::ToggleFavoriteGif(f)),
        None => {}
    }
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
