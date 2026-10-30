//! Unicode emoji drawn from one Twemoji sprite sheet (emoji-datasource's), instead of an image per emoji: the
//! sheet is downloaded once, kept on disk, and becomes a single texture, so the picker, autocomplete,
//! reactions and messages draw every emoji at once without further requests. Emoji shown bigger than the
//! sheet's cells (jumbo ones), or missing from it, still use Twemoji's individual images.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{self, Color32, ColorImage, Rect, TextureHandle, TextureOptions, Vec2};
use parking_lot::Mutex;
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    sheet: SheetInfo,
}

#[derive(Deserialize)]
struct SheetInfo {
    version: String,
    cells: u32,
    positions: HashMap<String, (u16, u16)>,
}

enum State {
    Idle,
    Loading,
    /// The texture and its cell size in pixels.
    Ready(TextureHandle, u32),
    Failed,
}

static INFO: OnceLock<Option<SheetInfo>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::Idle);

fn info() -> Option<&'static SheetInfo> {
    INFO.get_or_init(|| serde_json::from_str::<File>(crate::ui::emoji::EMOJI_JSON).ok().map(|f| f.sheet)).as_ref()
}

/// Sheet lookups ignore presentation selectors, which messages may or may not include.
fn key(emoji: &str) -> String {
    emoji.replace('\u{FE0F}', "")
}

/// An emoji as an image `size` big: from the sheet when it can be, else Twemoji's own image.
pub fn image(ctx: &egui::Context, emoji: &str, size: Vec2) -> egui::Image<'static> {
    let url = || egui::Image::from_uri(swiftcord_core::cdn::twemoji(emoji)).fit_to_exact_size(size).show_loading_spinner(false);
    let Some(info) = info() else { return url() };
    let Some(&(x, y)) = info.positions.get(&key(emoji)) else { return url() };
    let ppp = ctx.pixels_per_point();
    let start = {
        let mut state = STATE.lock();
        let idle = matches!(*state, State::Idle);
        if idle {
            *state = State::Loading;
        }
        idle
    };
    if start {
        // Sharp at this display's scale: 64 px cells above 115%, else 32 px (a quarter of the memory).
        let cell = if ppp > 1.15 && ctx.input(|i| i.max_texture_side) >= (info.cells * 66) as usize { 64 } else { 32 };
        load(ctx, &info.version, cell);
    }
    let state = STATE.lock();
    match &*state {
        State::Ready(texture, cell) if size.x * ppp <= *cell as f32 * 1.15 => {
            let side = texture.size_vec2();
            let pitch = (*cell + 2) as f32;
            let min = egui::pos2((x as f32 * pitch + 1.0) / side.x, (y as f32 * pitch + 1.0) / side.y);
            let uv = Rect::from_min_size(min, Vec2::new(*cell as f32 / side.x, *cell as f32 / side.y));
            egui::Image::new(egui::load::SizedTexture::new(texture.id(), size)).uv(uv).fit_to_exact_size(size)
        }
        // Nothing yet while the sheet loads (it's quick, and avoids fetching each emoji meanwhile).
        State::Loading => egui::Image::new(egui::load::SizedTexture::new(egui::TextureId::default(), size)).tint(Color32::TRANSPARENT),
        _ => url(),
    }
}

/// Any emoji (a server's own, or Unicode) as an image `size` big.
pub fn any(ctx: &egui::Context, e: &swiftcord_core::models::Emoji, size: Vec2) -> egui::Image<'static> {
    match (e.id, e.name.as_deref()) {
        (Some(id), _) => egui::Image::from_uri(swiftcord_core::cdn::emoji(id, e.animated, 48)).fit_to_exact_size(size).show_loading_spinner(false),
        (None, Some(n)) => image(ctx, n, size),
        _ => egui::Image::new(egui::load::SizedTexture::new(egui::TextureId::default(), size)).tint(Color32::TRANSPARENT),
    }
}

fn sheet_url(version: &str, cell: u32) -> String {
    format!("https://cdn.jsdelivr.net/npm/emoji-datasource-twitter@{version}/img/twitter/sheets/{cell}.png")
}

/// Fetches the sheet (or reads it from disk), decodes it and makes it a texture, in the background.
fn load(ctx: &egui::Context, version: &str, cell: u32) {
    let path = crate::platform::data_dir().join("cache").join(format!("emoji-sheet-{version}-{cell}.png"));
    let url = sheet_url(version, cell);
    let ctx = ctx.clone();
    let Some(rt) = crate::clip::RUNTIME.get() else {
        *STATE.lock() = State::Failed;
        return;
    };
    rt.spawn(async move {
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => Some(b),
            Err(_) => {
                let b = crate::clip::download(&url).await;
                if let Some(b) = &b {
                    let _ = tokio::fs::create_dir_all(path.parent().expect("cache dir")).await;
                    let _ = tokio::fs::write(&path, b).await;
                }
                b
            }
        };
        let image = tokio::task::spawn_blocking(move || {
            let rgba = image::load_from_memory(&bytes?).ok()?.to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
        })
        .await
        .ok()
        .flatten();
        *STATE.lock() = match image {
            Some(img) => State::Ready(ctx.load_texture("emoji-sheet", img, TextureOptions::LINEAR), cell),
            None => {
                tracing::warn!("couldn't load the emoji sheet; using individual images");
                let _ = std::fs::remove_file(&path);
                State::Failed
            }
        };
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_where_emoji_are() {
        let info = info().expect("sheet info");
        assert_eq!(info.positions.get(&key("👍")), Some(&(12, 61)));
        // With or without the selector, and skin tones too.
        assert_eq!(info.positions.get(&key("❤️")), info.positions.get(&key("❤")));
        assert!(info.positions.contains_key(&key("👍🏽")));
        assert!(info.positions.len() > 3500);
    }
}
