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
        if ui.is_rect_visible(rect) {
            self.paint(ui, rect, url, still);
        }
        response
    }

    /// Paints a looping clip into `rect` (for layouts that place it themselves).
    pub fn paint(&mut self, ui: &Ui, rect: egui::Rect, url: &str, still: Option<&str>) {
        if !self.players.contains_key(url) && self.players.len() < MAX_PLAYERS {
            self.players.insert(url.to_owned(), self.start(ui.ctx(), url, rect.size()));
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
                    ui.painter().rect_filled(rect, 6, crate::theme::RAIL());
                }
            }
        }
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

pub(crate) async fn download(url: &str) -> Option<Vec<u8>> {
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

/// The first frame of a video clip, for still previews (e.g. favourite GIFs, which are mostly MP4s).
pub fn first_frame(bytes: &[u8], max_width: u32) -> Option<ColorImage> {
    #[cfg(windows)]
    return decode::first_frame(bytes, max_width).ok();
    #[cfg(not(windows))]
    return None;
}

#[cfg(windows)]
pub(crate) mod decode {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use eframe::egui::{self, ColorImage};
    use parking_lot::Mutex;
    use windows::core::Interface;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    pub(crate) struct Reader {
        pub reader: IMFSourceReader,
        /// The decoded frame, which may have padding below and to the right of the picture.
        width: usize,
        height: usize,
        /// The picture within the frame.
        x0: usize,
        y0: usize,
        shown_w: usize,
        shown_h: usize,
        /// Negative for bottom-up rows.
        stride: i32,
        /// NV12 straight from the decoder (converted here); otherwise RGB32 from the reader's converter.
        nv12: bool,
        bt709: bool,
    }

    fn open(bytes: &[u8]) -> windows::core::Result<Reader> {
        unsafe {
            let stream = SHCreateMemStream(Some(bytes)).ok_or_else(windows::core::Error::from_thread)?;
            let byte_stream = MFCreateMFByteStreamOnStream(&stream)?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(windows::core::Error::from_thread)?;
            // Lets the reader convert to RGB32 for codecs that don't output NV12.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let reader = MFCreateSourceReaderFromByteStream(&byte_stream, &attrs)?;
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            select_video(reader)
        }
    }

    /// Selects the first video stream. The decoder's own NV12 is preferred: converting a whole frame to RGB32
    /// costs more than decoding it, and only the pixels shown need converting.
    pub(crate) fn select_video(reader: IMFSourceReader) -> windows::core::Result<Reader> {
        unsafe {
            reader.SetStreamSelection(VIDEO, true)?;
            let mut last = Ok(());
            for sub in [MFVideoFormat_NV12, MFVideoFormat_RGB32] {
                let want = MFCreateMediaType()?;
                want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                want.SetGUID(&MF_MT_SUBTYPE, &sub)?;
                last = reader.SetCurrentMediaType(VIDEO, None, &want);
                if last.is_ok() {
                    break;
                }
            }
            last?;
            let mut r = Reader { reader, width: 0, height: 0, x0: 0, y0: 0, shown_w: 0, shown_h: 0, stride: 0, nv12: false, bt709: false };
            r.refresh()?;
            Ok(r)
        }
    }

    impl Reader {
        /// Reads the frame format, again after the decoder changes it mid-stream.
        pub fn refresh(&mut self) -> windows::core::Result<()> {
            unsafe {
                let got = self.reader.GetCurrentMediaType(VIDEO)?;
                let size = got.GetUINT64(&MF_MT_FRAME_SIZE)?;
                (self.width, self.height) = (((size >> 32) as usize).max(1), ((size & 0xffff_ffff) as usize).max(1));
                self.nv12 = got.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12;
                let bytes = if self.nv12 { 1 } else { 4 };
                self.stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((self.width * bytes) as i32);
                let mut area = MFVideoArea::default();
                let blob = std::slice::from_raw_parts_mut(&mut area as *mut MFVideoArea as *mut u8, std::mem::size_of::<MFVideoArea>());
                (self.x0, self.y0, self.shown_w, self.shown_h) = match got.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None) {
                    Ok(()) if area.Area.cx > 0 && area.Area.cy > 0 => {
                        let (x0, y0) = (area.OffsetX.value.max(0) as usize, area.OffsetY.value.max(0) as usize);
                        let (x0, y0) = (x0.min(self.width - 1), y0.min(self.height - 1));
                        (x0, y0, (area.Area.cx as usize).min(self.width - x0), (area.Area.cy as usize).min(self.height - y0))
                    }
                    _ => (0, 0, self.width, self.height),
                };
                self.bt709 = got.GetUINT32(&MF_MT_YUV_MATRIX).map_or(self.height >= 720, |m| m == MFVideoTransferMatrix_BT709.0 as u32);
            }
            Ok(())
        }

        /// The output size for `max_width`, the source pixels per output pixel, and whether to average 2×2
        /// source pixels (when shrinking a lot, so detail doesn't shimmer).
        fn scaling(&self, max_width: u32) -> (usize, usize, f32, usize) {
            let scale = (self.shown_w as f32 / max_width.max(1) as f32).max(1.0);
            let (w, h) = (((self.shown_w as f32 / scale) as usize).max(1), ((self.shown_h as f32 / scale) as usize).max(1));
            (w, h, scale, usize::from(scale >= 1.5 && self.shown_w > 1 && self.shown_h > 1))
        }
    }

    /// Converts a BGRX frame to RGBA at most `max_width` wide. `row0` is the top row and `pitch` the signed
    /// distance between rows, as the buffer reports them.
    unsafe fn rgb_image(row0: *const u8, pitch: isize, r: &Reader, max_width: u32) -> ColorImage {
        let (w, h, scale, step) = r.scaling(max_width);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let sy = r.y0 + ((y as f32 * scale) as usize).min(r.shown_h - 1 - step);
            let line = unsafe { std::slice::from_raw_parts(row0.offset(sy as isize * pitch), r.width * 4) };
            let next = unsafe { std::slice::from_raw_parts(row0.offset((sy + step) as isize * pitch), r.width * 4) };
            for x in 0..w {
                let sx = (r.x0 + ((x as f32 * scale) as usize).min(r.shown_w - 1 - step)) * 4;
                let sx2 = sx + step * 4;
                let avg = |c: usize| ((line[sx + c] as u16 + line[sx2 + c] as u16 + next[sx + c] as u16 + next[sx2 + c] as u16) / 4) as u8;
                px.push(egui::Color32::from_rgb(avg(2), avg(1), avg(0)));
            }
        }
        ColorImage::new([w, h], px)
    }

    /// Converts an NV12 frame (`rows` rows of luma, then half as many of interleaved U and V, `pitch` bytes
    /// apart) to RGBA at most `max_width` wide. The conversion lives in the voice crate, which debug builds
    /// optimise.
    unsafe fn nv12_image(base: *const u8, pitch: usize, rows: usize, r: &Reader, max_width: u32) -> ColorImage {
        let luma = unsafe { std::slice::from_raw_parts(base, pitch * rows) };
        let chroma = unsafe { std::slice::from_raw_parts(base.add(pitch * rows), pitch * rows / 2) };
        let crop = swiftcord_voice::video::Crop { x: r.x0, y: r.y0, width: r.shown_w, height: r.shown_h };
        let (w, h, rgba) = swiftcord_voice::video::nv12_to_rgba_scaled(luma, chroma, pitch, crop, max_width as usize, r.bt709);
        ColorImage::from_rgba_unmultiplied([w, h], &rgba)
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

    /// The row pitch and luma rows of an NV12 buffer of `len` bytes (a luma plane and a half-height chroma
    /// plane), allowing for padded rows and columns.
    pub(super) fn nv12_layout(len: usize, width: usize, height: usize) -> Option<(usize, usize)> {
        (height..=height + 64).find_map(|rows| {
            let pitch = len * 2 / (rows * 3);
            (pitch * rows * 3 == len * 2 && pitch >= width && pitch <= width + 512).then_some((pitch, rows))
        })
    }

    /// Reads a decoded sample. Video buffers are usually 2D, which report the real row pitch (rows are often
    /// padded beyond the width); otherwise the layout is inferred from the length.
    pub(crate) unsafe fn frame_image(sample: &IMFSample, r: &Reader, max_width: u32) -> windows::core::Result<Option<ColorImage>> {
        unsafe {
            let buf = sample.GetBufferByIndex(0)?;
            if r.nv12 {
                if let Ok(b2) = buf.cast::<IMF2DBuffer2>() {
                    let (mut row0, mut pitch, mut start, mut len) = (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
                    b2.Lock2DSize(MF2DBuffer_LockFlags_Read, &mut row0, &mut pitch, &mut start, &mut len)?;
                    let p = pitch.max(1) as usize;
                    // The luma plane is two thirds of the rows.
                    let rows = (len as usize / p) * 2 / 3;
                    let ok = pitch > 0 && p >= r.width && rows >= r.height;
                    let img = ok.then(|| nv12_image(row0, p, rows, r, max_width));
                    b2.Unlock2D()?;
                    return Ok(img);
                }
                let buf = sample.ConvertToContiguousBuffer()?;
                let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
                buf.Lock(&mut ptr, None, Some(&mut len))?;
                let img = nv12_layout(len as usize, r.width, r.height).map(|(pitch, rows)| nv12_image(ptr, pitch, rows, r, max_width));
                buf.Unlock()?;
                return Ok(img);
            }
            if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                let (mut row0, mut pitch) = (std::ptr::null_mut(), 0i32);
                b2.Lock2D(&mut row0, &mut pitch)?;
                let ok = pitch.unsigned_abs() as usize >= r.width * 4;
                let img = ok.then(|| rgb_image(row0, pitch as isize, r, max_width));
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
                    rgb_image(ptr.add(pitch * (r.height - 1)), -(pitch as isize), r, max_width)
                } else {
                    rgb_image(ptr, pitch as isize, r, max_width)
                }
            });
            buf.Unlock()?;
            Ok(img)
        }
    }

    /// The first frame, for still previews.
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

    #[test]
    fn infers_nv12_layout() {
        // 1920×1080 decoded as 1920×1088.
        assert_eq!(nv12_layout(1920 * 1088 * 3 / 2, 1920, 1088), Some((1920, 1088)));
        // 220×164 with rows padded to 256 bytes and 176 rows.
        assert_eq!(nv12_layout(256 * 176 * 3 / 2, 220, 164), Some((256, 176)));
        assert_eq!(nv12_layout(12345, 220, 164), None);
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
