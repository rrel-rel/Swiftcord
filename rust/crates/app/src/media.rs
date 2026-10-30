//! An egui bytes loader for http(s) images on the shared reqwest client. Immutable CDN assets (avatars,
//! icons, emoji, stickers) are also cached on disk so they don't hit the network on every launch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::Poll;

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use egui::{self, mutex::Mutex};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct File {
    bytes: Arc<[u8]>,
    mime: Option<String>,
}

type Entry = Poll<Result<File, String>>;

pub struct MediaLoader {
    cache: Arc<Mutex<HashMap<String, Entry>>>,
    http: reqwest::Client,
    rt: tokio::runtime::Handle,
    limit: Arc<Semaphore>,
    disk: PathBuf,
}

impl MediaLoader {
    pub const ID: &'static str = egui::generate_loader_id!(MediaLoader);

    pub fn new(rt: tokio::runtime::Handle, disk: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(swiftcord_core::rest::USER_AGENT)
            .build()
            .expect("HTTP client");
        let _ = std::fs::create_dir_all(&disk);
        Self { cache: Arc::default(), http, rt, limit: Arc::new(Semaphore::new(8)), disk }
    }
}

/// Content-addressed assets whose URL changes whenever the content does.
fn is_immutable(uri: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "https://cdn.discordapp.com/avatars/",
        "https://cdn.discordapp.com/icons/",
        "https://cdn.discordapp.com/emojis/",
        "https://cdn.discordapp.com/embed/avatars/",
        "https://cdn.discordapp.com/channel-icons/",
        "https://cdn.discordapp.com/guilds/",
        "https://media.discordapp.net/stickers/",
        "https://cdn.jsdelivr.net/gh/jdecked/twemoji",
    ];
    PREFIXES.iter().any(|p| uri.starts_with(p))
}

/// FNV-1a, stable across Rust versions (unlike `DefaultHasher`), for cache file names.
fn cache_name(uri: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in uri.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn mime_from_bytes(b: &[u8]) -> Option<String> {
    let m = if b.starts_with(b"\x89PNG") {
        "image/png"
    } else if b.starts_with(b"GIF8") {
        "image/gif"
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if b.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else {
        return None;
    };
    Some(m.to_owned())
}

/// URIs starting with this load only the first frame of an animated image (as a PNG), for still previews
/// that don't hold every frame in memory. The still is kept on disk, so it's only made once.
pub const STILL: &str = "still+";
/// URIs starting with this are kept on disk too (small images that are shown again and again, like the
/// stills of favourite GIFs).
pub const KEEP: &str = "keep+";

fn first_frame_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    // MP4 clips (an "ftyp" box first) go through the video decoder; everything else through the image crate.
    let img = if bytes.get(4..8) == Some(b"ftyp") {
        let frame = crate::clip::first_frame(bytes, 320).ok_or("undecodable video")?;
        let rgba: Vec<u8> = frame.pixels.iter().flat_map(|c| c.to_array()).collect();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(frame.size[0] as u32, frame.size[1] as u32, rgba).ok_or("bad frame")?)
    } else {
        image::load_from_memory(bytes).map_err(|e| e.to_string())?
    };
    // Previews are small on screen; cap their size too.
    let img = if img.width() > 320 { img.thumbnail(320, 320) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

async fn fetch(http: &reqwest::Client, uri: &str, disk: Option<PathBuf>) -> Result<File, String> {
    if let Some(path) = &disk {
        if let Ok(bytes) = tokio::fs::read(path).await {
            let mime = mime_from_bytes(&bytes);
            return Ok(File { bytes: bytes.into(), mime });
        }
    }
    let derived = if let Some(real) = uri.strip_prefix(KEEP) {
        Some(Box::pin(fetch(http, real, None)).await?)
    } else if let Some(real) = uri.strip_prefix(STILL) {
        // The whole animation is fetched to make the still, but isn't kept.
        let full = Box::pin(fetch(http, real, None)).await?;
        let png = tokio::task::spawn_blocking(move || first_frame_png(&full.bytes)).await.map_err(|e| e.to_string())??;
        Some(File { bytes: png.into(), mime: Some("image/png".into()) })
    } else {
        None
    };
    if let Some(file) = derived {
        if let Some(path) = disk {
            let b = file.bytes.clone();
            tokio::spawn(async move {
                let _ = tokio::fs::write(path, b).await;
            });
        }
        return Ok(file);
    }
    let resp = http.get(uri).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("{} for {uri}", resp.status()));
    }
    let header_mime = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if let Some(path) = disk {
        let b = bytes.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::write(path, b).await;
        });
    }
    // Trust the bytes over the header: the CDN sometimes labels WebP as something else.
    let mime = mime_from_bytes(&bytes).or(header_mime);
    Ok(File { bytes: bytes.to_vec().into(), mime })
}

impl BytesLoader for MediaLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        let real = uri.strip_prefix(KEEP).or_else(|| uri.strip_prefix(STILL)).unwrap_or(uri);
        if !(real.starts_with("https://") || real.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(uri) {
            return match entry {
                Poll::Ready(Ok(f)) => Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(f.bytes.clone()), mime: f.mime.clone() }),
                Poll::Ready(Err(e)) => Err(LoadError::Loading(e.clone())),
                Poll::Pending => Ok(BytesPoll::Pending { size: None }),
            };
        }
        cache.insert(uri.to_owned(), Poll::Pending);
        drop(cache);

        let uri = uri.to_owned();
        let disk = (is_immutable(&uri) || uri.starts_with(KEEP) || uri.starts_with(STILL)).then(|| self.disk.join(cache_name(&uri)));
        let (http, cache, limit, ctx) = (self.http.clone(), self.cache.clone(), self.limit.clone(), ctx.clone());
        self.rt.spawn(async move {
            let _permit = limit.acquire().await;
            let result = fetch(&http, &uri, disk).await;
            if let Err(e) = &result {
                tracing::debug!("image load failed: {e}");
            }
            let repaint = {
                let mut cache = cache.lock();
                // Absent if the image was forgotten while loading.
                cache.get_mut(&uri).map(|e| *e = Poll::Ready(result)).is_some()
            };
            if repaint {
                ctx.request_repaint();
            }
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache.lock().values().map(|e| if let Poll::Ready(Ok(f)) = e { f.bytes.len() } else { 0 }).sum()
    }

    fn has_pending(&self) -> bool {
        self.cache.lock().values().any(Poll::is_pending)
    }
}
