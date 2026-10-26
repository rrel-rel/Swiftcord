//! Short looping videos in chat, which is what most "GIFs" are now (Tenor/Klipy send MP4s).
//!
//! Each visible clip gets a player thread that reads the MP4 with Media Foundation's source reader (which
//! demuxes, decodes and converts to RGB), paces frames by their timestamps, and hands the latest one to a single
//! texture. Clips that scroll out of view are stopped and their textures dropped, so memory stays flat no
//! matter how many GIFs a channel has.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Response, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use parking_lot::Mutex;

/// The app's Tokio runtime, for downloads.
pub static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Players are stopped once their clip hasn't been drawn for this long.
const UNSEEN: Duration = Duration::from_secs(2);
const MAX_PLAYERS: usize = 8;
/// Downloaded clips kept for when they scroll back into view.
const MAX_CACHED: usize = 32;

struct Player {
    frame: Arc<Mutex<Option<ColorImage>>>,
    stop: Arc<AtomicBool>,
    texture: Option<TextureHandle>,
    last_seen: Instant,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Clips {
    players: HashMap<String, Player>,
    cache: Arc<Mutex<HashMap<String, Arc<Vec<u8>>>>>,
}

impl Clips {
    /// Draws a looping clip at `size`, showing `still` (an image URL) until the first frame is ready or if the
    /// clip can't play.
    pub fn show(&mut self, ui: &mut Ui, url: &str, still: Option<&str>, size: Vec2) -> Response {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        if !self.players.contains_key(url) && self.players.len() < MAX_PLAYERS {
            self.players.insert(url.to_owned(), self.start(ui.ctx(), url, size));
        }
        let mut drawn = false;
        if let Some(p) = self.players.get_mut(url) {
            p.last_seen = Instant::now();
            if let Some(img) = p.frame.lock().take() {
                match &mut p.texture {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => p.texture = Some(ui.ctx().load_texture(format!("clip:{url}"), img, TextureOptions::LINEAR)),
                }
            }
            if let Some(t) = &p.texture {
                egui::Image::new(t).corner_radius(6).paint_at(ui, rect);
                drawn = true;
            }
        }
        if !drawn {
            match still {
                Some(s) => {
                    egui::Image::from_uri(s.to_owned()).corner_radius(6).show_loading_spinner(false).paint_at(ui, rect);
                }
                None => {
                    ui.painter().rect_filled(rect, 6, crate::theme::RAIL);
                }
            }
        }
        response
    }

    /// Stops players for clips no longer on screen.
    pub fn tick(&mut self) {
        // Failed players stay while visible too, showing the still, so they aren't retried every frame.
        self.players.retain(|_, p| p.last_seen.elapsed() < UNSEEN);
    }

    fn start(&self, ctx: &egui::Context, url: &str, size: Vec2) -> Player {
        let frame = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let max_width = (size.x * ctx.pixels_per_point()).ceil() as u32;
        let (f, s, ctx, url, cache) = (frame.clone(), stop.clone(), ctx.clone(), url.to_owned(), self.cache.clone());
        if let Some(rt) = RUNTIME.get() {
            rt.spawn(async move {
                let cached = cache.lock().get(&url).cloned();
                let bytes = match cached {
                    Some(b) => b,
                    None => match download(&url).await {
                        Some(b) => {
                            let b = Arc::new(b);
                            let mut c = cache.lock();
                            if c.len() >= MAX_CACHED {
                                if let Some(k) = c.keys().next().cloned() {
                                    c.remove(&k);
                                }
                            }
                            c.insert(url.clone(), b.clone());
                            b
                        }
                        None => return,
                    },
                };
                std::thread::Builder::new()
                    .name("swiftcord-clip".into())
                    .spawn(move || {
                        if let Err(e) = decode::play(&bytes, max_width, &f, &s, &ctx) {
                            tracing::debug!("clip {url} can't play: {e}");
                        }
                    })
                    .ok();
            });
        }
        Player { frame, stop, texture: None, last_seen: Instant::now() }
    }
}

async fn download(url: &str) -> Option<Vec<u8>> {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let http = HTTP.get_or_init(|| reqwest::Client::builder().user_agent(swiftcord_core::rest::USER_AGENT).build().expect("HTTP client"));
    let resp = http.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // GIF-style clips are small; refuse anything that isn't.
    if resp.content_length().is_some_and(|l| l > 20 * 1024 * 1024) {
        return None;
    }
    resp.bytes().await.ok().map(|b| b.to_vec())
}

#[cfg(windows)]
mod decode {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use eframe::egui::{self, ColorImage};
    use parking_lot::Mutex;
    use windows::core::Interface;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    struct Reader {
        reader: IMFSourceReader,
        width: usize,
        height: usize,
        /// Negative for bottom-up rows.
        stride: i32,
    }

    fn open(bytes: &[u8]) -> windows::core::Result<Reader> {
        unsafe {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(windows::core::Error::from_thread)?;
            let byte_stream = MFCreateMFByteStreamOnStream(&stream)?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(windows::core::Error::from_thread)?;
            // Let the reader convert whatever the codec outputs into RGB32 for us.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromByteStream(&byte_stream, &attrs)?;
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            reader.SetStreamSelection(VIDEO, true)?;
            let want = MFCreateMediaType()?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(VIDEO, None, &want)?;
            let got = reader.GetCurrentMediaType(VIDEO)?;
            let size = got.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (width, height) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
            let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(width as i32 * 4);
            Ok(Reader { reader, width, height, stride })
        }
    }

    /// Converts a BGRX frame to RGBA, scaling down (nearest neighbour) to at most `max_width` wide. `row0` is the
    /// top row and `pitch` the signed distance between rows, as the buffer reports them.
    unsafe fn to_image(row0: *const u8, pitch: isize, r: &Reader, max_width: u32) -> ColorImage {
        let scale = (r.width as f32 / max_width.max(1) as f32).max(1.0);
        let (w, h) = (((r.width as f32 / scale) as usize).max(1), ((r.height as f32 / scale) as usize).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let sy = ((y as f32 * scale) as usize).min(r.height - 1);
            let line = unsafe { std::slice::from_raw_parts(row0.offset(sy as isize * pitch), r.width * 4) };
            for x in 0..w {
                let sx = ((x as f32 * scale) as usize).min(r.width - 1) * 4;
                px.push(egui::Color32::from_rgb(line[sx + 2], line[sx + 1], line[sx]));
            }
        }
        ColorImage::new([w, h], px)
    }

    /// The row pitch of a padded RGB32 buffer. Decoders pad frames to their block size (e.g. 220×164 becomes
    /// 224×176) without saying so, so find the smallest padded height that splits the buffer into whole,
    /// 4-byte-aligned rows at least as wide as the picture.
    pub(super) fn infer_pitch(len: usize, width: usize, height: usize) -> usize {
        (height..=height + 64)
            .find_map(|h| {
                let pitch = len / h;
                (len % h == 0 && pitch % 4 == 0 && pitch >= width * 4 && pitch <= width * 4 + 512).then_some(pitch)
            })
            .unwrap_or(width * 4)
    }

    /// Reads a decoded sample. Video buffers are usually 2D, which report the real row pitch (rows are often
    /// padded beyond the width); otherwise the pitch is inferred from the length.
    unsafe fn frame_image(sample: &IMFSample, r: &Reader, max_width: u32) -> windows::core::Result<Option<ColorImage>> {
        unsafe {
            let buf = sample.GetBufferByIndex(0)?;
            if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                let (mut row0, mut pitch) = (std::ptr::null_mut(), 0i32);
                b2.Lock2D(&mut row0, &mut pitch)?;
                let ok = pitch.unsigned_abs() as usize >= r.width * 4;
                let img = ok.then(|| to_image(row0, pitch as isize, r, max_width));
                b2.Unlock2D()?;
                return Ok(img);
            }
            let buf = sample.ConvertToContiguousBuffer()?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len))?;
            let pitch = infer_pitch(len as usize, r.width, r.height);
            let img = (pitch >= r.width * 4).then(|| {
                if r.stride < 0 {
                    // Bottom-up: the top row is last.
                    to_image(ptr.add(pitch * (r.height - 1)), -(pitch as isize), r, max_width)
                } else {
                    to_image(ptr, pitch as isize, r, max_width)
                }
            });
            buf.Unlock()?;
            Ok(img)
        }
    }

    /// The first frame, for tests.
    #[cfg(test)]
    pub fn first_frame(bytes: &[u8], max_width: u32) -> windows::core::Result<ColorImage> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
        }
        let r = open(bytes)?;
        loop {
            let (mut flags, mut sample) = (0u32, None);
            unsafe { r.reader.ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))? };
            if let Some(s) = sample {
                return unsafe { frame_image(&s, &r, max_width) }?.ok_or_else(windows::core::Error::from_thread);
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Err(windows::core::Error::from_thread());
            }
        }
    }

    /// Plays the clip in a loop until `stop`, publishing frames at their presentation times.
    pub fn play(bytes: &[u8], max_width: u32, frame: &Mutex<Option<ColorImage>>, stop: &AtomicBool, ctx: &egui::Context) -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
        }
        while !stop.load(Ordering::Relaxed) {
            // Looping by reopening: simpler than seeking, and the bytes are already in memory.
            let r = open(bytes)?;
            let start = Instant::now();
            let mut frames = 0;
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
                unsafe { r.reader.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))? };
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };
                // Timestamps are in 100 ns units.
                let due = start + Duration::from_nanos(ts.max(0) as u64 * 100);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait.min(Duration::from_millis(500)));
                }
                let image = unsafe { frame_image(&sample, &r, max_width)? };
                if let Some(img) = image {
                    *frame.lock() = Some(img);
                    ctx.request_repaint();
                    frames += 1;
                }
            }
            if frames == 0 {
                return Err(windows::core::Error::from_thread());
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod decode {
    pub fn play(_: &[u8], _: u32, _: &parking_lot::Mutex<Option<eframe::egui::ColorImage>>, _: &std::sync::atomic::AtomicBool, _: &eframe::egui::Context) -> Result<(), String> {
        Err("video clips need Windows".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::decode::*;

    #[test]
    fn infers_padded_pitch() {
        // 220×164 padded to 224×176.
        assert_eq!(infer_pitch(157_696, 220, 164), 896);
        // Unpadded.
        assert_eq!(infer_pitch(880 * 164, 220, 164), 880);
    }

    /// Decodes a local clip (`SWIFTCORD_TEST_CLIP`) and saves its first frame next to it as a PNG.
    #[test]
    fn decodes_a_local_clip() {
        let Ok(path) = std::env::var("SWIFTCORD_TEST_CLIP") else { return };
        let bytes = std::fs::read(&path).unwrap();
        let frame = parking_lot::Mutex::new(None::<eframe::egui::ColorImage>);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let img = first_frame(&bytes, 400).expect("decode");
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        image::save_buffer(format!("{path}.png"), &rgba, img.size[0] as u32, img.size[1] as u32, image::ExtendedColorType::Rgba8).unwrap();
        let _ = (frame, stop);
    }
}
