//! Egui's texture cache, plus forgetting images that haven't been shown for a while. Otherwise each image's
//! download, decoded pixels and texture stay in memory for good, and a long session holds everything it
//! ever scrolled past.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::load::{DefaultTextureLoader, SizeHint, TextureLoadResult, TextureLoader};
use egui::mutex::Mutex;
use egui::{self, TextureOptions};

/// Images not shown for this long are forgotten (they load again, from the disk cache where there is one,
/// if they come back into view).
const KEEP_UNSHOWN: Duration = Duration::from_secs(120);
/// How often to look for them.
const CHECK_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct ImageCache {
    inner: DefaultTextureLoader,
    /// When each image was last drawn.
    shown: Mutex<HashMap<String, Instant>>,
}

impl TextureLoader for ImageCache {
    fn id(&self) -> &str {
        egui::generate_loader_id!(ImageCache)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, options: TextureOptions, size_hint: SizeHint) -> TextureLoadResult {
        let now = Instant::now();
        {
            let mut shown = self.shown.lock();
            match shown.get_mut(uri) {
                Some(at) => *at = now,
                None => {
                    shown.insert(uri.to_owned(), now);
                }
            }
        }
        self.inner.load(ctx, uri, options, size_hint)
    }

    fn forget(&self, uri: &str) {
        self.inner.forget(uri);
        self.shown.lock().remove(uri);
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
    let stale: Vec<String> = cache.shown.lock().iter().filter(|(_, at)| at.elapsed() > KEEP_UNSHOWN).map(|(uri, _)| uri.clone()).collect();
    if !stale.is_empty() {
        tracing::debug!("forgetting {} images not shown lately", stale.len());
    }
    for uri in stale {
        ctx.forget_image(&uri);
    }
}
