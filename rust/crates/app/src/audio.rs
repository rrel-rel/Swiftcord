//! Sound attachments played in the chat, one at a time: voice messages (a waveform player, like Discord's)
//! and audio files (their file card gains a play button and seek bar, keeping the download button).
//!
//! Voice messages and other Ogg Opus files are small, so they're downloaded and decoded whole with the
//! call's Opus decoder. Other formats (MP3, AAC, WAV, FLAC…) are streamed through Media Foundation a couple
//! of seconds ahead, so a long track never sits decoded in memory.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use parking_lot::Mutex;
use swiftcord_core::models::Attachment;

use crate::theme::{self, icon};
use crate::ui::widgets;

/// Decoded clips kept (about 1 MB a minute each).
const MAX_CACHED: usize = 16;
const RATE: usize = 48_000;
/// How far ahead a streamed file is decoded, in stereo samples.
const AHEAD: usize = RATE * 2 * 2;

enum Clip {
    Loading,
    Ready(Arc<Vec<f32>>),
    Failed(String),
}

/// What plays: a decoded clip, or a file being streamed.
enum Source {
    Decoded(Arc<Vec<f32>>),
    Streamed(Arc<Stream>),
}

/// A file being decoded on its own thread, ahead of playback.
#[derive(Default)]
struct Stream {
    buf: Mutex<VecDeque<f32>>,
    /// A seek asked for (stereo sample index), or -1.
    seek: AtomicI64,
    ended: AtomicBool,
    stop: AtomicBool,
    error: Mutex<Option<String>>,
}

struct Playing {
    url: String,
    /// Position in stereo samples.
    pos: Arc<AtomicUsize>,
    /// Length in stereo samples (0 until a streamed file's is known).
    len: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    stream: Option<Arc<Stream>>,
    _io: swiftcord_voice::AudioIo,
}

impl Drop for Playing {
    fn drop(&mut self) {
        if let Some(s) = &self.stream {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

impl Playing {
    fn progress(&self) -> f32 {
        let len = self.len.load(Ordering::Relaxed);
        if len == 0 { 0.0 } else { (self.pos.load(Ordering::Relaxed) as f32 / len as f32).min(1.0) }
    }

    fn seek(&self, fraction: f32) {
        let to = ((fraction * self.len.load(Ordering::Relaxed) as f32) as usize) & !1;
        self.pos.store(to, Ordering::Relaxed);
        if let Some(s) = &self.stream {
            s.seek.store(to as i64, Ordering::Relaxed);
        }
        self.done.store(false, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct AudioClips {
    clips: HashMap<String, Clip>,
    playing: Option<Playing>,
    /// Where to start a clip that's still loading, as a fraction (a click on its waveform).
    start_at: HashMap<String, f32>,
    /// Why a streamed file couldn't play.
    errors: HashMap<String, String>,
}

fn time(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Discord's waveform: up to 256 base64 bytes, each a loudness from 0 to 255.
fn waveform(a: &Attachment) -> Vec<u8> {
    use base64::Engine;
    a.waveform.as_deref().and_then(|w| base64::engine::general_purpose::STANDARD.decode(w).ok()).unwrap_or_default()
}

/// Ogg Opus files are decoded whole; everything else is streamed.
fn is_ogg_opus(a: &Attachment) -> bool {
    let name = a.filename.to_ascii_lowercase();
    a.is_voice_message() || a.content_type.as_deref().is_some_and(|t| t.starts_with("audio/ogg") || t.starts_with("audio/opus")) || name.ends_with(".ogg") || name.ends_with(".opus")
}

/// A round play / pause button (a spinner while loading); true when clicked.
fn play_button(ui: &Ui, rect: Rect, id: egui::Id, paused: bool, loading: bool) -> egui::Response {
    let b = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if b.hovered() { theme::BLURPLE.gamma_multiply(0.85) } else { theme::BLURPLE };
    let c = rect.center();
    ui.painter().circle_filled(c, rect.width() / 2.0, fill);
    if loading {
        egui::Spinner::new().size(16.0).color(Color32::WHITE).paint_at(ui, Rect::from_center_size(c, Vec2::splat(16.0)));
    } else if paused {
        let pts = vec![c + Vec2::new(-4.0, -7.0), c + Vec2::new(7.0, 0.0), c + Vec2::new(-4.0, 7.0)];
        ui.painter().add(egui::Shape::convex_polygon(pts, Color32::WHITE, egui::Stroke::NONE));
    } else {
        for dx in [-4.0, 4.0] {
            ui.painter().rect_filled(Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(4.0, 13.0)), 1, Color32::WHITE);
        }
    }
    b
}

impl AudioClips {
    /// What's known about `url` for drawing: playing (and paused?), loading, the error, progress.
    fn status(&self, url: &str) -> (Option<&Playing>, bool, bool, Option<String>, f32) {
        let playing = self.playing.as_ref().filter(|p| p.url == url);
        let paused = playing.is_none_or(|p| p.paused.load(Ordering::Relaxed));
        let loading = matches!(self.clips.get(url), Some(Clip::Loading));
        let failed = match self.clips.get(url) {
            Some(Clip::Failed(e)) => Some(e.clone()),
            _ => self.errors.get(url).cloned(),
        };
        let progress = playing.map_or(0.0, Playing::progress);
        (playing, paused, loading, failed, progress)
    }

    /// A voice message: a pill with a play button, the waveform (click to seek) and the time.
    pub fn voice_message(&mut self, ui: &mut Ui, a: &Attachment, output: Option<String>, volume: f32) {
        let width = 320.0_f32.min(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        ui.painter().rect_filled(rect, 26, theme::EMBED_BG);
        let (playing, paused, loading, failed, progress) = self.status(&a.url);
        let duration = a.duration_secs.or_else(|| playing.map(|p| p.len.load(Ordering::Relaxed) as f64 / 2.0 / RATE as f64)).unwrap_or(0.0);
        let started = playing.is_some();

        let button = Rect::from_center_size(egui::pos2(rect.left() + 26.0, rect.center().y), Vec2::splat(34.0));
        let b = play_button(ui, button, ui.scope_id().with(("vm-play", &a.url)), paused, loading);

        // Waveform, coloured up to the playback position.
        let label = match &failed {
            Some(_) => "Can't play".to_owned(),
            None if started => time(progress as f64 * duration),
            None => time(duration),
        };
        let galley = ui.painter().layout_no_wrap(label, theme::font(12.5), theme::MUTED);
        let wave = Rect::from_min_max(egui::pos2(button.right() + 12.0, rect.top() + 12.0), egui::pos2(rect.right() - galley.size().x - 22.0, rect.bottom() - 12.0));
        let w = ui.interact(wave, ui.scope_id().with(("vm-wave", &a.url)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let samples = waveform(a);
        let bars = ((wave.width() / 4.0) as usize).max(1);
        for i in 0..bars {
            let level = if samples.is_empty() {
                0.3
            } else {
                let from = i * samples.len() / bars;
                let to = ((i + 1) * samples.len() / bars).max(from + 1).min(samples.len());
                samples[from..to].iter().copied().max().unwrap_or(0) as f32 / 255.0
            };
            let h = 3.0 + level * (wave.height() - 3.0);
            let x = wave.left() + i as f32 * 4.0 + 1.5;
            let color = if (i as f32 + 0.5) / bars as f32 <= progress { theme::HEADER } else { theme::FAINT };
            ui.painter().rect_filled(Rect::from_center_size(egui::pos2(x, wave.center().y), Vec2::new(2.5, h)), 1, color);
        }
        ui.painter().galley(egui::pos2(rect.right() - galley.size().x - 16.0, rect.center().y - galley.size().y / 2.0), galley, theme::MUTED);
        let b = match &failed {
            Some(e) => b.on_hover_text(e),
            None => b,
        };

        if !paused {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        let seek = w.clicked().then(|| w.interact_pointer_pos()).flatten().map(|p| ((p.x - wave.left()) / wave.width()).clamp(0.0, 1.0));
        if b.clicked() || seek.is_some() {
            self.toggle(ui.ctx(), a, seek, output, volume);
        }
    }

    /// An audio file: its file card (name, size, download) with a play button, seek bar and time.
    pub fn file(&mut self, ui: &mut Ui, a: &Attachment, output: Option<String>, volume: f32) {
        let (playing, paused, loading, failed, progress) = self.status(&a.url);
        let len = playing.map_or(0, |p| p.len.load(Ordering::Relaxed));
        let started = playing.is_some();
        let mut toggle = false;
        let mut seek = None;
        egui::Frame::new().fill(theme::EMBED_BG).corner_radius(6).inner_margin(10).stroke(egui::Stroke::new(1.0, theme::RAIL)).show(ui, |ui| {
            ui.set_width(380.0_f32.min(ui.available_width()));
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::FILE).font(theme::icon_font(28.0)).color(theme::BLURPLE));
                ui.vertical(|ui| {
                    if ui.link(&a.filename).clicked() {
                        crate::platform::open_url(&a.url);
                    }
                    ui.label(RichText::new(widgets::human_size(a.size)).size(12.0).color(theme::MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, icon::DOWNLOAD, "Download").clicked() {
                        crate::platform::open_url(&a.url);
                    }
                });
            });
            ui.add_space(6.0);
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
            let button = Rect::from_center_size(egui::pos2(row.left() + 16.0, row.center().y), Vec2::splat(30.0));
            let b = play_button(ui, button, ui.scope_id().with(("af-play", &a.url)), paused, loading || (started && len == 0 && failed.is_none()));
            let label = match &failed {
                Some(_) => "Can't play".to_owned(),
                None if len > 0 => {
                    let total = len as f64 / 2.0 / RATE as f64;
                    format!("{} / {}", time(progress as f64 * total), time(total))
                }
                None => String::new(),
            };
            let galley = ui.painter().layout_no_wrap(label, theme::font(12.0), theme::MUTED);
            let bar = Rect::from_min_max(egui::pos2(button.right() + 12.0, row.center().y - 3.0), egui::pos2(row.right() - galley.size().x - 12.0, row.center().y + 3.0));
            let hit = ui.interact(bar.expand2(Vec2::new(0.0, 8.0)), ui.scope_id().with(("af-bar", &a.url)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.painter().rect_filled(bar, 3, theme::RAIL);
            let filled = Rect::from_min_max(bar.min, egui::pos2(bar.left() + bar.width() * progress, bar.bottom()));
            ui.painter().rect_filled(filled, 3, theme::BLURPLE);
            if hit.hovered() || started {
                ui.painter().circle_filled(egui::pos2(filled.right(), bar.center().y), 6.0, Color32::WHITE);
            }
            ui.painter().galley(egui::pos2(row.right() - galley.size().x, row.center().y - galley.size().y / 2.0), galley, theme::MUTED);
            let b = match &failed {
                Some(e) => b.on_hover_text(e),
                None => b,
            };
            toggle = b.clicked();
            seek = hit.clicked().then(|| hit.interact_pointer_pos()).flatten().map(|p| ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0));
        });
        if !paused {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if toggle || seek.is_some() {
            self.toggle(ui.ctx(), a, seek, output, volume);
        }
    }

    /// Plays, pauses or seeks (`seek` as a fraction of the clip).
    fn toggle(&mut self, ctx: &egui::Context, a: &Attachment, seek: Option<f32>, output: Option<String>, volume: f32) {
        if let Some(p) = self.playing.as_ref().filter(|p| p.url == a.url) {
            match seek {
                Some(f) => p.seek(f),
                None => {
                    p.paused.fetch_xor(true, Ordering::Relaxed);
                }
            }
            return;
        }
        if !is_ogg_opus(a) {
            self.errors.remove(&a.url);
            let stream = Arc::new(Stream { seek: AtomicI64::new(-1), ..Default::default() });
            self.start(&a.url, Source::Streamed(stream.clone()), 0.0, output, volume);
            // A click on the seek bar of a file not yet playing just starts it: its length isn't known yet.
            if let Some(p) = &self.playing {
                spawn_stream(&a.url, stream, p.len.clone(), p.pos.clone(), ctx.clone());
            }
            return;
        }
        match self.clips.get(&a.url) {
            Some(Clip::Ready(pcm)) => {
                let pcm = pcm.clone();
                self.start(&a.url, Source::Decoded(pcm), seek.unwrap_or(0.0), output, volume);
            }
            Some(Clip::Loading) => {}
            _ => {
                if self.clips.len() >= MAX_CACHED {
                    let playing = self.playing.as_ref().map(|p| p.url.clone());
                    self.clips.retain(|u, c| Some(u) == playing.as_ref() || matches!(c, Clip::Loading));
                }
                self.clips.insert(a.url.clone(), Clip::Loading);
                self.start_at.insert(a.url.clone(), seek.unwrap_or(0.0));
                let (url, ctx) = (a.url.clone(), ctx.clone());
                let Some(rt) = crate::clip::RUNTIME.get() else { return };
                let done = LOADED.get_or_init(Default::default).clone();
                rt.spawn(async move {
                    let result = match crate::clip::download(&url).await {
                        Some(bytes) => tokio::task::spawn_blocking(move || swiftcord_voice::ogg::decode_ogg_opus(&bytes)).await.map_err(|e| e.to_string()).and_then(|r| r),
                        None => Err("couldn't download it".into()),
                    };
                    if let Err(e) = &result {
                        tracing::warn!("audio clip {url}: {e}");
                    }
                    done.lock().push((url, result));
                    ctx.request_repaint();
                });
            }
        }
    }

    fn start(&mut self, url: &str, source: Source, at: f32, output: Option<String>, volume: f32) {
        self.playing = None;
        let len = Arc::new(AtomicUsize::new(match &source {
            Source::Decoded(pcm) => pcm.len(),
            Source::Streamed(_) => 0,
        }));
        let pos = Arc::new(AtomicUsize::new(((at * len.load(Ordering::Relaxed) as f32) as usize) & !1));
        let paused = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let stream = match &source {
            Source::Streamed(s) => Some(s.clone()),
            Source::Decoded(_) => None,
        };
        let (p, z, d) = (pos.clone(), paused.clone(), done.clone());
        let fill = move |out: &mut [f32]| {
            out.fill(0.0);
            if z.load(Ordering::Relaxed) {
                return;
            }
            let from = p.load(Ordering::Relaxed);
            let n = match &source {
                Source::Decoded(pcm) => {
                    let n = out.len().min(pcm.len().saturating_sub(from));
                    for (o, s) in out[..n].iter_mut().zip(&pcm[from..from + n]) {
                        *o = s * volume;
                    }
                    if from + n >= pcm.len() {
                        d.store(true, Ordering::Relaxed);
                    }
                    n
                }
                Source::Streamed(s) => {
                    let mut buf = s.buf.lock();
                    let n = out.len().min(buf.len());
                    for (o, v) in out[..n].iter_mut().zip(buf.drain(..n)) {
                        *o = v * volume;
                    }
                    if n == 0 && s.ended.load(Ordering::Relaxed) && s.seek.load(Ordering::Relaxed) < 0 {
                        d.store(true, Ordering::Relaxed);
                    }
                    n
                }
            };
            p.fetch_add(n, Ordering::Relaxed);
        };
        match swiftcord_voice::play(output, fill) {
            Ok(io) => self.playing = Some(Playing { url: url.to_owned(), pos, len, paused, done, stream, _io: io }),
            Err(e) => {
                tracing::warn!("audio playback: {e}");
                self.errors.insert(url.to_owned(), format!("No speakers: {e}"));
            }
        }
    }

    /// Picks up clips that finished loading (starting the one just asked for), and stops whatever finished
    /// or failed, giving the audio device back.
    pub fn tick(&mut self, output: Option<String>, volume: f32) {
        let loaded: Vec<_> = LOADED.get().map(|l| std::mem::take(&mut *l.lock())).unwrap_or_default();
        for (url, result) in loaded {
            match result {
                Ok(pcm) => {
                    let pcm = Arc::new(pcm);
                    self.clips.insert(url.clone(), Clip::Ready(pcm.clone()));
                    if let Some(at) = self.start_at.remove(&url) {
                        self.start(&url, Source::Decoded(pcm), at, output.clone(), volume);
                    }
                }
                Err(e) => {
                    self.start_at.remove(&url);
                    self.clips.insert(url, Clip::Failed(e));
                }
            }
        }
        let error = self.playing.as_ref().and_then(|p| Some((p.url.clone(), p.stream.as_ref()?.error.lock().clone()?)));
        if let Some((url, e)) = error {
            self.errors.insert(url, e);
            self.playing = None;
        }
        if self.playing.as_ref().is_some_and(|p| p.done.load(Ordering::Relaxed)) {
            self.playing = None;
        }
    }
}

/// Clips finished loading on the runtime, picked up by [`AudioClips::tick`].
type Loaded = Arc<Mutex<Vec<(String, Result<Vec<f32>, String>)>>>;
static LOADED: std::sync::OnceLock<Loaded> = std::sync::OnceLock::new();

/// Decodes a file on its own thread, keeping [`AHEAD`] of playback buffered, until stopped.
fn spawn_stream(url: &str, stream: Arc<Stream>, len: Arc<AtomicUsize>, pos: Arc<AtomicUsize>, ctx: egui::Context) {
    let url = url.to_owned();
    let s = stream.clone();
    let spawned = std::thread::Builder::new().name("swiftcord-audio-file".into()).spawn(move || {
        if let Err(e) = decode::run(&url, &s, &len, &pos, &ctx) {
            tracing::info!("audio file {url}: {e}");
            *s.error.lock() = Some(e);
        }
        s.ended.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    });
    if spawned.is_err() {
        *stream.error.lock() = Some("couldn't start".into());
    }
}

#[cfg(windows)]
mod decode {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use eframe::egui;
    use windows::core::{GUID, HSTRING};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Variant::VT_I8;

    use super::{Stream, AHEAD, RATE};
    use crate::player::decode::{pcm, select_audio};
    use crate::player::Converter;

    const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    /// Media Foundation time units per second.
    const SECOND: i64 = 10_000_000;

    pub fn run(url: &str, s: &Stream, len: &AtomicUsize, pos: &AtomicUsize, ctx: &egui::Context) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| e.to_string())?;
        }
        let reader = unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(url), None) }.map_err(|e| {
            tracing::info!("can't open {url}: {e}");
            "Couldn't load this file.".to_owned()
        })?;
        unsafe {
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        }
        let (rate, channels) = select_audio(&reader).map_err(|_| "This audio format isn't supported here.".to_owned())?;
        let mut conv = Converter::new(rate, channels);
        let duration = unsafe { reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) }
            .ok()
            .map(|v| unsafe { v.Anonymous.Anonymous.Anonymous.uhVal } as i64)
            .unwrap_or(0);
        len.store(((duration as f64 / SECOND as f64 * RATE as f64) as usize) * 2, Ordering::Relaxed);
        ctx.request_repaint();
        let mut ended = false;
        while !s.stop.load(Ordering::Relaxed) {
            let seek = s.seek.swap(-1, Ordering::Relaxed);
            if seek >= 0 {
                let t = seek / 2 * SECOND / RATE as i64;
                let at = PROPVARIANT {
                    Anonymous: PROPVARIANT_0 {
                        Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 { vt: VT_I8, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: PROPVARIANT_0_0_0 { hVal: t } }),
                    },
                };
                unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &at) }.map_err(|e| e.to_string())?;
                s.buf.lock().clear();
                conv.reset();
                ended = false;
                s.ended.store(false, Ordering::Relaxed);
                pos.store(seek as usize, Ordering::Relaxed);
            }
            if ended || s.buf.lock().len() >= AHEAD {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let (mut flags, mut sample) = (0u32, None);
            unsafe { reader.ReadSample(AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample)) }.map_err(|e| e.to_string())?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err("The file stopped loading.".into());
            }
            if let Some(sample) = sample {
                let out = conv.convert(&pcm(&sample).map_err(|e| e.to_string())?);
                s.buf.lock().extend(out);
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                ended = true;
                s.ended.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod decode {
    pub fn run(_: &str, _: &super::Stream, _: &std::sync::atomic::AtomicUsize, _: &std::sync::atomic::AtomicUsize, _: &eframe::egui::Context) -> Result<(), String> {
        Err("Playing audio files needs Windows.".into())
    }
}
