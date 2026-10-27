//! Drawing stickers. PNG and GIF ones are plain images. APNG and Lottie ones (Lottie being Discord's own
//! packs, drawn by [`crate::lottie`]) show still and animate while the pointer is over them, so a channel
//! full of stickers costs nothing until one is looked at; their files are fetched when first needed and
//! dropped a minute after they were last on screen.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Rect, Response, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use parking_lot::Mutex;
use swiftcord_core::models::StickerItem;
use swiftcord_core::Snowflake;

use crate::lottie::Animation;
use crate::theme;

const FORGET: Duration = Duration::from_secs(60);

enum Loaded {
    Lottie(Box<Animation>),
    /// APNG frames and how long each shows.
    Frames(Vec<(ColorImage, Duration)>),
}

struct Entry {
    /// Filled in by the loader: the animation, or `None` if it couldn't be loaded.
    loaded: Arc<Mutex<Option<Option<Loaded>>>>,
    texture: Option<TextureHandle>,
    /// What the texture shows: (frame, size in pixels).
    shown: Option<(f64, u16)>,
    hovered_since: Option<Instant>,
    last_seen: Instant,
}

#[derive(Default)]
pub struct Stickers {
    entries: HashMap<Snowflake, Entry>,
}

impl Stickers {
    /// A sticker at `size`, its name on hover.
    pub fn show(&mut self, ui: &mut Ui, s: &StickerItem, size: f32) -> Response {
        let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        if ui.is_rect_visible(rect) {
            self.paint(ui, rect, s, r.hovered());
        }
        r.on_hover_text(&s.name)
    }

    /// Paints a sticker into `rect`, animating it if `animate`.
    pub fn paint(&mut self, ui: &Ui, rect: Rect, s: &StickerItem, animate: bool) {
        let still = swiftcord_core::cdn::sticker(s.id, s.format_type);
        let Some(file) = swiftcord_core::cdn::sticker_file(s.id, s.format_type) else {
            match still {
                Some(url) => image(ui, rect, url),
                None => placeholder(ui, rect, &s.name),
            }
            return;
        };
        // An APNG's still is enough until it's hovered; Lottie ones have no still, so load them right away.
        if still.is_some() && !animate && !self.entries.contains_key(&s.id) {
            image(ui, rect, still.unwrap());
            return;
        }
        let e = self.entries.entry(s.id).or_insert_with(|| Entry {
            loaded: start(ui.ctx(), file),
            texture: None,
            shown: None,
            hovered_since: None,
            last_seen: Instant::now(),
        });
        e.last_seen = Instant::now();
        let elapsed = if animate { e.hovered_since.get_or_insert_with(Instant::now).elapsed() } else {
            e.hovered_since = None;
            Duration::ZERO
        };
        let px = (rect.width() * ui.ctx().pixels_per_point()).round().clamp(16.0, 512.0) as u16;
        let mut guard = e.loaded.lock();
        let next = match guard.as_mut() {
            Some(Some(Loaded::Lottie(a))) => {
                let frame = a.frame_at(elapsed);
                if e.shown != Some((frame, px)) {
                    let img = a.render(frame, px);
                    set(ui.ctx(), &mut e.texture, s.id, img);
                    e.shown = Some((frame, px));
                }
                animate.then(|| a.frame_time())
            }
            Some(Some(Loaded::Frames(frames))) if !frames.is_empty() => {
                let total: Duration = frames.iter().map(|f| f.1).sum();
                let mut t = Duration::from_secs_f64(elapsed.as_secs_f64() % total.as_secs_f64().max(0.001));
                let mut i = 0;
                while i + 1 < frames.len() && t >= frames[i].1 {
                    t -= frames[i].1;
                    i += 1;
                }
                if e.shown.map(|(f, _)| f) != Some(i as f64) {
                    set(ui.ctx(), &mut e.texture, s.id, frames[i].0.clone());
                    e.shown = Some((i as f64, px));
                }
                animate.then(|| frames[i].1.saturating_sub(t))
            }
            Some(_) => {
                drop(guard);
                match still {
                    Some(url) => image(ui, rect, url),
                    None => placeholder(ui, rect, &s.name),
                }
                return;
            }
            None => {
                drop(guard);
                match still {
                    Some(url) => image(ui, rect, url),
                    None => {
                        // Still loading: a faint tile.
                        ui.painter().rect_filled(rect.shrink(rect.width() * 0.15), 12, theme::EMBED_BG());
                    }
                }
                return;
            }
        };
        drop(guard);
        if let Some(t) = &e.texture {
            egui::Image::new(t).paint_at(ui, rect);
        }
        if let Some(wait) = next {
            ui.ctx().request_repaint_after(wait.max(Duration::from_millis(16)));
        }
    }

    /// Drops stickers that haven't been on screen for a while.
    pub fn tick(&mut self) {
        self.entries.retain(|_, e| e.last_seen.elapsed() < FORGET);
    }
}

fn image(ui: &Ui, rect: Rect, url: String) {
    egui::Image::from_uri(url).show_loading_spinner(false).paint_at(ui, rect);
}

fn placeholder(ui: &Ui, rect: Rect, name: &str) {
    ui.painter().rect_filled(rect, 8, theme::EMBED_BG());
    let galley = ui.painter().layout(name.to_owned(), theme::font(13.0), theme::MUTED(), rect.width() - 16.0);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, theme::MUTED());
}

fn set(ctx: &egui::Context, texture: &mut Option<TextureHandle>, id: Snowflake, img: ColorImage) {
    match texture {
        Some(t) => t.set(img, TextureOptions::LINEAR),
        None => *texture = Some(ctx.load_texture(format!("sticker:{id}"), img, TextureOptions::LINEAR)),
    }
}

/// Fetches and decodes a sticker's animation in the background.
fn start(ctx: &egui::Context, url: String) -> Arc<Mutex<Option<Option<Loaded>>>> {
    let slot = Arc::new(Mutex::new(None));
    let (out, ctx) = (slot.clone(), ctx.clone());
    match crate::clip::RUNTIME.get() {
        Some(rt) => {
            rt.spawn(async move {
                let bytes = crate::clip::download(&url).await;
                let loaded = tokio::task::spawn_blocking(move || bytes.and_then(|b| decode(&url, &b))).await.ok().flatten();
                *out.lock() = Some(loaded);
                ctx.request_repaint();
            });
        }
        None => *slot.lock() = Some(None),
    }
    slot
}

fn decode(url: &str, bytes: &[u8]) -> Option<Loaded> {
    if url.ends_with(".json") {
        return Animation::parse(bytes).map(|a| Loaded::Lottie(Box::new(a)));
    }
    use image::AnimationDecoder;
    let decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let frames = decoder.apng().ok()?.into_frames().collect_frames().ok()?;
    let frames = frames
        .into_iter()
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let delay = Duration::from_micros((n as u64 * 1000) / d.max(1) as u64).max(Duration::from_millis(16));
            let buf = f.into_buffer();
            (ColorImage::from_rgba_unmultiplied([buf.width() as usize, buf.height() as usize], buf.as_raw()), delay)
        })
        .collect();
    Some(Loaded::Frames(frames))
}
