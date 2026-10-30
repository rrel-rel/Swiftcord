//! Egui's image decoding, through Windows Imaging Component (see `platform::wic`). Animated GIFs and APNGs
//! keep all their frames; egui asks for one with a `#index` suffix and reads the timing from
//! `FrameDurations`, as with egui_extras' loaders.

use std::collections::HashMap;
use std::sync::Arc;
use std::task::Poll;

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::mutex::Mutex;
use egui::{ColorImage, FrameDurations, Id};

type Frames = Arc<Vec<Arc<ColorImage>>>;

pub struct WicLoader {
    cache: Arc<Mutex<HashMap<String, Poll<Result<Frames, String>>>>>,
    rt: tokio::runtime::Handle,
    /// Decodes at once: a screenful of images would otherwise start a thread (and a WIC instance) each.
    limit: Arc<tokio::sync::Semaphore>,
}

impl WicLoader {
    pub const ID: &'static str = egui::generate_loader_id!(WicLoader);

    pub fn new(rt: tokio::runtime::Handle) -> Self {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4));
        Self { cache: Arc::default(), rt, limit: Arc::new(tokio::sync::Semaphore::new(threads)) }
    }
}

/// The image's own URI, without the frame index egui appends for animations.
fn split_frame(uri: &str) -> (&str, usize) {
    egui::decode_animated_image_uri(uri).unwrap_or((uri, 0))
}

fn is_image_mime(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
    // SVG would need a renderer; nothing here shows it.
    (mime.starts_with("image/") && !mime.starts_with("image/svg")) || mime.contains("octet-stream") || mime.contains("download")
}

impl ImageLoader for WicLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        let (uri, frame) = split_frame(uri);
        let entry = self.cache.lock().get(uri).cloned();
        let entry = match entry {
            Some(entry) => entry,
            None => match ctx.try_load_bytes(uri) {
                Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                    if let Some(mime) = mime.filter(|m| !is_image_mime(m)) {
                        return Err(LoadError::FormatNotSupported { detected_format: Some(mime) });
                    }
                    self.cache.lock().insert(uri.to_owned(), Poll::Pending);
                    let (cache, ctx, uri, limit) = (self.cache.clone(), ctx.clone(), uri.to_owned(), self.limit.clone());
                    let decode = move || {
                        let result = crate::platform::wic::decode_frames(&bytes).map(|frames| {
                            if frames.len() > 1 {
                                let durations = FrameDurations::new(frames.iter().map(|(_, d)| *d).collect());
                                ctx.data_mut(|d| d.insert_temp(Id::unique(&uri), durations));
                            }
                            Arc::new(frames.into_iter().map(|(f, _)| Arc::new(f.to_color_image())).collect::<Vec<_>>())
                        });
                        // Unless it was forgotten meanwhile.
                        if let Some(entry) = cache.lock().get_mut(&uri) {
                            *entry = Poll::Ready(result);
                            ctx.request_repaint();
                        }
                    };
                    self.rt.spawn(async move {
                        let _permit = limit.acquire_owned().await;
                        let _ = tokio::task::spawn_blocking(decode).await;
                    });
                    Poll::Pending
                }
                Ok(BytesPoll::Pending { size }) => return Ok(ImagePoll::Pending { size }),
                Err(e) => return Err(e),
            },
        };
        match entry {
            Poll::Ready(Ok(frames)) => Ok(ImagePoll::Ready { image: frames[frame % frames.len()].clone() }),
            Poll::Ready(Err(e)) => Err(LoadError::Loading(e)),
            Poll::Pending => Ok(ImagePoll::Pending { size: None }),
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().remove(split_frame(uri).0);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|e| match e {
                Poll::Ready(Ok(frames)) => frames.iter().map(|f| f.pixels.len() * 4).sum(),
                Poll::Ready(Err(e)) => e.len(),
                Poll::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.cache.lock().values().any(|e| e.is_pending())
    }
}
