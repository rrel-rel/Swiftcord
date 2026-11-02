//! Egui's texture cache, plus playing animations and forgetting images that haven't been shown for a while.
//! Otherwise each image's download, decoded pixels and texture stay in memory for good, and a long session
//! holds everything it ever scrolled past.
//!
//! Egui only plays an image whose URI ends in `.gif` or `.webp`, and Discord's nearly all end in a query
//! (`.gif?size=48`). So animations are played here, for any image found to have frames when it was decoded.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::load::{DefaultTextureLoader, SizeHint, TextureLoadResult, TextureLoader};
use egui::mutex::Mutex;
use egui::{self, FrameDurations, Id, TextureOptions};

/// Images not shown for this long are forgotten (they load again, from the disk cache where there is one,
/// if they come back into view).
const KEEP_UNSHOWN: Duration = Duration::from_secs(120);
/// How often to look for them.
const CHECK_EVERY: Duration = Duration::from_secs(30);

struct Shown {
    at: Instant,
    /// How many of its frames may have a texture.
    frames: usize,
}

#[derive(Default)]
pub struct ImageCache {
    inner: DefaultTextureLoader,
    /// When each image was last drawn.
    shown: Mutex<HashMap<String, Shown>>,
}

/// The frame showing at `now` of an animation that loops, and how long until the next.
fn frame_at(durations: &FrameDurations, now: Duration) -> (usize, Duration) {
    let total: Duration = durations.all().sum();
    let position = now.as_millis() % total.as_millis().max(1);
    let mut end = 0;
    for (index, duration) in durations.all().enumerate() {
        end += duration.as_millis();
        if position < end {
            return (index, Duration::from_millis((end - position) as u64));
        }
    }
    (0, Duration::ZERO)
}

/// The image's own URI, without the frame index egui appends for those it plays itself.
fn image_of(uri: &str) -> &str {
    egui::decode_animated_image_uri(uri).map_or(uri, |(image, _)| image)
}

impl ImageCache {
    fn note_shown(&self, image: &str, frame: Option<usize>) {
        let frames = frame.map_or(0, |f| f + 1);
        let mut shown = self.shown.lock();
        match shown.get_mut(image) {
            Some(s) => {
                s.at = Instant::now();
                s.frames = s.frames.max(frames);
            }
            None => {
                shown.insert(image.to_owned(), Shown { at: Instant::now(), frames });
            }
        }
    }
}

impl TextureLoader for ImageCache {
    fn id(&self) -> &str {
        egui::generate_loader_id!(ImageCache)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, options: TextureOptions, size_hint: SizeHint) -> TextureLoadResult {
        if let Ok((image, frame)) = egui::decode_animated_image_uri(uri) {
            self.note_shown(image, Some(frame));
            return self.inner.load(ctx, uri, options, size_hint);
        }
        let durations: Option<FrameDurations> = ctx.data(|d| d.get_temp(Id::unique(uri)));
        let Some(durations) = durations else {
            self.note_shown(uri, None);
            return self.inner.load(ctx, uri, options, size_hint);
        };
        let (frame, left) = frame_at(&durations, Duration::from_secs_f64(ctx.input(|i| i.time)));
        ctx.request_repaint_after(left);
        self.note_shown(uri, Some(frame));
        self.inner.load(ctx, &format!("{uri}#{frame}"), options, size_hint)
    }

    fn forget(&self, uri: &str) {
        let image = image_of(uri);
        let frames = self.shown.lock().remove(image).map_or(0, |s| s.frames);
        self.inner.forget(image);
        for frame in 0..frames {
            self.inner.forget(&format!("{image}#{frame}"));
        }
    }

    fn forget_all(&self) {
        self.inner.forget_all();
        self.shown.lock().clear();
    }

    fn end_pass(&self, pass_index: u64) {
        self.inner.end_pass(pass_index);
    }

    fn byte_size(&self) -> usize {
        self.inner.byte_size()
    }
}

/// Keeps track of images shown, in front of egui's own texture cache.
pub fn install(ctx: &egui::Context) -> Arc<ImageCache> {
    let cache = Arc::new(ImageCache::default());
    // Loaders added later are asked first.
    ctx.add_texture_loader(cache.clone());
    cache
}

/// Now and then, forgets images (in every loader: download, pixels and texture) not shown for a while.
pub fn tick(ctx: &egui::Context, cache: &ImageCache, last_check: &mut Instant) {
    if last_check.elapsed() < CHECK_EVERY {
        return;
    }
    *last_check = Instant::now();
    let stale: Vec<String> = cache.shown.lock().iter().filter(|(_, s)| s.at.elapsed() > KEEP_UNSHOWN).map(|(uri, _)| uri.clone()).collect();
    if !stale.is_empty() {
        tracing::debug!("forgetting {} images not shown lately", stale.len());
    }
    for uri in stale {
        ctx.forget_image(&uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::load::TexturePoll;

    #[test]
    fn frames_follow_their_durations_and_loop() {
        let durations = FrameDurations::new(vec![Duration::from_millis(100), Duration::from_millis(200)]);
        let at = |ms| frame_at(&durations, Duration::from_millis(ms));
        assert_eq!(at(0), (0, Duration::from_millis(100)));
        assert_eq!(at(150), (1, Duration::from_millis(150)));
        assert_eq!(at(300), (0, Duration::from_millis(100)));
    }

    /// Through the real loaders: a GIF whose URI ends in a query, as Discord's do, shows both its frames.
    #[test]
    fn a_gif_with_a_query_animates() {
        const URI: &str = "bytes://cat.gif?size=48";
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ctx = egui::Context::default();
        ctx.add_image_loader(Arc::new(crate::image_decode::WicLoader::new(rt.handle().clone())));
        let cache = install(&ctx);
        ctx.include_bytes(URI, crate::platform::wic::tests::two_frame_gif());

        let texture_at = |seconds: f64| {
            let mut texture = None;
            let raw = egui::RawInput { time: Some(seconds), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                if let Ok(TexturePoll::Ready { texture: t }) = ui.ctx().try_load_texture(URI, TextureOptions::default(), SizeHint::default()) {
                    texture = Some(t.id);
                }
            });
            out.textures_delta.clear();
            texture
        };
        // The frames last 0.1 s and 0.2 s. Decoding takes a moment.
        let started = Instant::now();
        let (first, second) = loop {
            let textures = (texture_at(0.05), texture_at(0.15));
            if let (Some(a), Some(b)) = textures {
                if a != b {
                    break (a, b);
                }
            }
            assert!(started.elapsed() < Duration::from_secs(10), "never moved to the second frame: {textures:?}");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(texture_at(0.35), Some(first), "loops back to the first frame");
        assert_eq!(texture_at(0.45), Some(second));

        assert!(cache.shown.lock().get(URI).is_some_and(|s| s.frames == 2));
        ctx.forget_image(URI);
        assert!(cache.shown.lock().is_empty());
    }
}
