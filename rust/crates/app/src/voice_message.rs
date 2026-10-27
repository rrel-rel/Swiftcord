//! Voice messages: Ogg Opus attachments played in the chat with a waveform, like Discord's. A clip is
//! downloaded and decoded (with the call's Opus decoder) when first played, and one plays at a time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, Rect, Sense, Ui, Vec2};
use swiftcord_core::models::Attachment;

use crate::theme;

/// Decoded clips kept (about 1 MB a minute each).
const MAX_CACHED: usize = 16;
const RATE: usize = 48_000;

enum Clip {
    Loading,
    Ready(Arc<Vec<f32>>),
    Failed(String),
}

struct Playing {
    url: String,
    /// Position in stereo samples.
    pos: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    len: usize,
    _io: swiftcord_voice::AudioIo,
}

#[derive(Default)]
pub struct VoiceMessages {
    clips: HashMap<String, Clip>,
    playing: Option<Playing>,
    /// Where to start a clip that's still loading, as a fraction (a click on its waveform).
    start_at: HashMap<String, f32>,
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

impl VoiceMessages {
    pub fn show(&mut self, ui: &mut Ui, a: &Attachment, output: Option<String>, volume: f32) {
        let width = 320.0_f32.min(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter();
        painter.rect_filled(rect, 26, theme::EMBED_BG);

        let playing = self.playing.as_ref().filter(|p| p.url == a.url);
        let paused = playing.is_none_or(|p| p.paused.load(Ordering::Relaxed));
        let loading = matches!(self.clips.get(&a.url), Some(Clip::Loading));
        let failed = match self.clips.get(&a.url) {
            Some(Clip::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        let duration = a.duration_secs.or_else(|| playing.map(|p| p.len as f64 / 2.0 / RATE as f64)).unwrap_or(0.0);
        let progress = playing.map_or(0.0, |p| p.pos.load(Ordering::Relaxed) as f32 / p.len.max(1) as f32).min(1.0);

        // Play / pause.
        let button = Rect::from_center_size(egui::pos2(rect.left() + 26.0, rect.center().y), Vec2::splat(34.0));
        let b = ui.interact(button, ui.scope_id().with(("vm-play", &a.url)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let fill = if b.hovered() { theme::BLURPLE.gamma_multiply(0.85) } else { theme::BLURPLE };
        ui.painter().circle_filled(button.center(), 17.0, fill);
        let c = button.center();
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

        // Waveform, coloured up to the playback position; click to seek.
        let label = match &failed {
            Some(_) => "Can't play".to_owned(),
            None if playing.is_some() => time(progress as f64 * duration),
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
                let (from, to) = (i * samples.len() / bars, ((i + 1) * samples.len() / bars).max(i * samples.len() / bars + 1));
                samples[from..to.min(samples.len())].iter().copied().max().unwrap_or(0) as f32 / 255.0
            };
            let h = 3.0 + level * (wave.height() - 3.0);
            let x = wave.left() + i as f32 * 4.0 + 1.5;
            let played = (i as f32 + 0.5) / bars as f32 <= progress;
            let color = if played { theme::HEADER } else { theme::FAINT };
            ui.painter().rect_filled(Rect::from_center_size(egui::pos2(x, wave.center().y), Vec2::new(2.5, h)), 1, color);
        }
        ui.painter().galley(egui::pos2(rect.right() - galley.size().x - 16.0, rect.center().y - galley.size().y / 2.0), galley, theme::MUTED);
        if let Some(e) = &failed {
            b.clone().on_hover_text(e);
        }

        if !paused {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        let seek = w.clicked().then(|| w.interact_pointer_pos()).flatten().map(|p| ((p.x - wave.left()) / wave.width()).clamp(0.0, 1.0));
        if b.clicked() || seek.is_some() {
            self.toggle(ui.ctx(), a, seek, output, volume);
        }
    }

    /// Plays, pauses or seeks (`seek` as a fraction of the clip).
    fn toggle(&mut self, ctx: &egui::Context, a: &Attachment, seek: Option<f32>, output: Option<String>, volume: f32) {
        if let Some(p) = self.playing.as_ref().filter(|p| p.url == a.url) {
            match seek {
                Some(f) => {
                    p.pos.store(((f * p.len as f32) as usize) & !1, Ordering::Relaxed);
                    p.paused.store(false, Ordering::Relaxed);
                }
                None => {
                    // At the end, play again from the start.
                    if p.pos.load(Ordering::Relaxed) >= p.len {
                        p.pos.store(0, Ordering::Relaxed);
                    }
                    p.paused.fetch_xor(true, Ordering::Relaxed);
                }
            }
            return;
        }
        match self.clips.get(&a.url) {
            Some(Clip::Ready(pcm)) => {
                let pcm = pcm.clone();
                self.start(&a.url, pcm, seek.unwrap_or(0.0), output, volume);
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
                        tracing::warn!("voice message {url}: {e}");
                    }
                    done.lock().push((url, result));
                    ctx.request_repaint();
                });
            }
        }
    }

    fn start(&mut self, url: &str, pcm: Arc<Vec<f32>>, at: f32, output: Option<String>, volume: f32) {
        self.playing = None;
        let len = pcm.len();
        let pos = Arc::new(AtomicUsize::new(((at * len as f32) as usize) & !1));
        let paused = Arc::new(AtomicBool::new(false));
        let (p, z) = (pos.clone(), paused.clone());
        let fill = move |out: &mut [f32]| {
            out.fill(0.0);
            if z.load(Ordering::Relaxed) {
                return;
            }
            let from = p.load(Ordering::Relaxed).min(len);
            let n = out.len().min(len - from);
            for (o, s) in out[..n].iter_mut().zip(&pcm[from..from + n]) {
                *o = s * volume;
            }
            p.store(from + n, Ordering::Relaxed);
            if from + n >= len {
                z.store(true, Ordering::Relaxed);
            }
        };
        match swiftcord_voice::play(output, fill) {
            Ok(io) => self.playing = Some(Playing { url: url.to_owned(), pos, paused, len, _io: io }),
            Err(e) => {
                tracing::warn!("voice message playback: {e}");
                self.clips.insert(url.to_owned(), Clip::Failed(format!("No speakers: {e}")));
            }
        }
    }

    /// Picks up clips that finished loading (starting the one just asked for) and closes the audio device
    /// when nothing is playing.
    pub fn tick(&mut self, output: Option<String>, volume: f32) {
        let loaded: Vec<_> = LOADED.get().map(|l| std::mem::take(&mut *l.lock())).unwrap_or_default();
        for (url, result) in loaded {
            match result {
                Ok(pcm) => {
                    let pcm = Arc::new(pcm);
                    self.clips.insert(url.clone(), Clip::Ready(pcm.clone()));
                    if let Some(at) = self.start_at.remove(&url) {
                        self.start(&url, pcm, at, output.clone(), volume);
                    }
                }
                Err(e) => {
                    self.start_at.remove(&url);
                    self.clips.insert(url, Clip::Failed(e));
                }
            }
        }
        // A finished clip goes back to the start and gives the audio device back.
        if self.playing.as_ref().is_some_and(|p| p.pos.load(Ordering::Relaxed) >= p.len) {
            self.playing = None;
        }
    }
}

/// Clips finished loading on the runtime, picked up by [`VoiceMessages::tick`].
type Loaded = Arc<parking_lot::Mutex<Vec<(String, Result<Vec<f32>, String>)>>>;
static LOADED: std::sync::OnceLock<Loaded> = std::sync::OnceLock::new();
