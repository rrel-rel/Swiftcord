//! Videos in chat (attachments and embedded video files), played in place with sound, seeking and a larger
//! view.
//!
//! Starting a video gives it a decoder thread that streams the file from its URL through Media Foundation's
//! source reader and keeps a little queued ahead: frames, scaled to the size they're shown at, and 48 kHz
//! stereo sound. The UI shows whichever frame is due by the playback clock, and the sound device plays the
//! queued audio in step with that clock. A video that scrolls out of view pauses, and is closed after a while.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, ColorImage, Rect, Response, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use parking_lot::Mutex;

use crate::theme::{self, icon};

/// Media Foundation's time unit is 100 ns.
const SECOND: i64 = 10_000_000;
const RATE: usize = 48_000;
/// Frames and sound queued ahead of playback.
const AHEAD_FRAMES: usize = 10;
const AHEAD_SOUND: i64 = SECOND / 2;
/// A video pauses once off screen for this long, and is closed after `FORGET`.
const OFF_SCREEN: Duration = Duration::from_millis(500);
const FORGET: Duration = Duration::from_secs(60);
const BAR: f32 = 34.0;

#[derive(Default)]
struct Queues {
    frames: VecDeque<(i64, ColorImage)>,
    /// Interleaved stereo chunks with their start times; `audio_pos` samples of the first have been played.
    audio: VecDeque<(i64, Vec<f32>)>,
    audio_pos: usize,
    duration: Option<i64>,
    opened: bool,
    error: Option<String>,
    /// The decoder has reached the end.
    ended: bool,
    /// A seek the decoder hasn't done yet.
    seek: Option<i64>,
}

impl Queues {
    fn clear(&mut self) {
        self.frames.clear();
        self.audio.clear();
        self.audio_pos = 0;
        self.ended = false;
    }

    fn sound_ahead(&self) -> i64 {
        let samples = self.audio.iter().map(|(_, c)| c.len()).sum::<usize>() - self.audio_pos;
        (samples / 2) as i64 * SECOND / RATE as i64
    }
}

/// The playback position: `base`, plus the time since `since` while running.
struct Clock {
    base: i64,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> i64 {
        self.base + self.since.map_or(0, |s| s.elapsed().as_nanos() as i64 / 100)
    }

    fn stop(&mut self) {
        self.base = self.now();
        self.since = None;
    }
}

struct Shared {
    q: Mutex<Queues>,
    clock: Mutex<Clock>,
    stop: AtomicBool,
    max_width: AtomicU32,
    /// Volume as `f32` bits (0 when muted).
    gain: AtomicU32,
}

struct Player {
    shared: Arc<Shared>,
    texture: Option<TextureHandle>,
    /// Paused by the user, by finishing, or by scrolling away.
    paused: bool,
    /// Waiting for frames: starting, seeking or a slow connection.
    buffering: bool,
    finished: bool,
    /// Show the next frame even while paused (after seeking).
    want_frame: bool,
    position: i64,
    duration: Option<i64>,
    error: Option<String>,
    /// Where the seek bar is being dragged to.
    dragging: Option<f32>,
    last_seen: Instant,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Player {
    fn start(ctx: &egui::Context, url: &str, output: Option<String>, max_width: u32) -> Self {
        let shared = Arc::new(Shared {
            q: Mutex::new(Queues::default()),
            clock: Mutex::new(Clock { base: 0, since: None }),
            stop: AtomicBool::new(false),
            max_width: AtomicU32::new(max_width),
            gain: AtomicU32::new(1f32.to_bits()),
        });
        let (sh, ctx2, url) = (shared.clone(), ctx.clone(), url.to_owned());
        std::thread::Builder::new()
            .name("swiftcord-video".into())
            .spawn(move || {
                if let Err(e) = decode::run(&url, output, &sh, &ctx2) {
                    tracing::info!("video {url} can't play: {e}");
                    sh.q.lock().error = Some(e);
                    ctx2.request_repaint();
                }
            })
            .ok();
        Player {
            shared,
            texture: None,
            paused: false,
            buffering: true,
            finished: false,
            want_frame: false,
            position: 0,
            duration: None,
            error: None,
            dragging: None,
            last_seen: Instant::now(),
        }
    }

    /// Advances the clock and picks up the frame that's due.
    fn update(&mut self, ctx: &egui::Context, url: &str) {
        let sh = self.shared.clone();
        let mut clock = sh.clock.lock();
        let mut q = sh.q.lock();
        if !self.paused {
            if self.buffering {
                if q.opened && q.seek.is_none() && (q.frames.len() >= 3 || q.ended) {
                    clock.since = Some(Instant::now());
                    self.buffering = false;
                }
            } else if q.ended && q.frames.is_empty() && (q.audio.is_empty() || q.duration.is_some_and(|d| clock.now() >= d)) {
                clock.stop();
                self.paused = true;
                self.finished = true;
            } else if q.frames.is_empty() && !q.ended {
                clock.stop();
                self.buffering = true;
            }
        }
        let pos = clock.now();
        let mut due = None;
        if self.want_frame && !q.frames.is_empty() {
            due = q.frames.pop_front();
            self.want_frame = false;
        }
        while q.frames.front().is_some_and(|(pts, _)| *pts <= pos) {
            due = q.frames.pop_front();
        }
        if let Some((_, img)) = due {
            match &mut self.texture {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture(format!("video:{url}"), img, TextureOptions::LINEAR)),
            }
        }
        if clock.since.is_some() {
            let next = q.frames.front().map_or(SECOND / 60, |(pts, _)| (pts - pos).clamp(0, SECOND / 4));
            ctx.request_repaint_after(Duration::from_nanos(next as u64 * 100));
        } else if self.buffering || (self.want_frame && q.error.is_none()) {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.position = pos;
        self.duration = q.duration;
        self.error.clone_from(&q.error);
    }

    fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if !paused && self.finished {
            self.finished = false;
            self.seek(0);
            return;
        }
        if paused {
            self.shared.clock.lock().stop();
            self.buffering = false;
        } else {
            // Starts the clock once frames are queued.
            self.buffering = true;
        }
    }

    fn seek(&mut self, to: i64) {
        let mut clock = self.shared.clock.lock();
        let mut q = self.shared.q.lock();
        q.clear();
        q.seek = Some(to);
        clock.base = to;
        clock.since = None;
        self.position = to;
        self.buffering = !self.paused;
        self.want_frame = true;
        self.finished = false;
    }
}

/// A video to show, with what to fall back on.
pub struct Video<'a> {
    /// The file to play.
    pub url: &'a str,
    /// What to open in the browser instead (the post the video came from, for embeds).
    pub link: &'a str,
    /// A still to show until it plays.
    pub poster: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

struct Viewer {
    url: String,
    link: String,
    poster: Option<String>,
    size: Vec2,
}

#[derive(Default)]
pub struct Videos {
    players: HashMap<String, Player>,
    pub volume: Option<f32>,
    pub muted: bool,
    /// The output device for sound.
    pub output: Option<String>,
    viewer: Option<Viewer>,
}

impl Videos {
    fn gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume.unwrap_or(1.0) }
    }

    /// A video in chat at `size`: its poster with a play button until clicked, then the player.
    pub fn show(&mut self, ui: &mut Ui, v: &Video, size: Vec2) -> Response {
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        if ui.is_rect_visible(rect) {
            self.show_at(ui, rect, v);
        }
        response
    }

    /// A video placed in `rect` by the caller.
    pub fn show_at(&mut self, ui: &mut Ui, rect: Rect, v: &Video) {
        if self.viewer.as_ref().is_some_and(|w| w.url == v.url) {
            poster(ui, rect, v.poster.as_deref());
            ui.painter().rect_filled(rect, 6, Color32::from_black_alpha(170));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Playing in the viewer", theme::font_semibold(13.0), Color32::WHITE);
            return;
        }
        if !self.players.contains_key(v.url) {
            poster(ui, rect, v.poster.as_deref());
            let r = ui.interact(rect, egui::Id::unique(("video-poster", v.url)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let hot = r.hovered();
            ui.painter().circle_filled(rect.center(), 26.0, Color32::from_black_alpha(if hot { 210 } else { 160 }));
            ui.painter().text(rect.center() + Vec2::new(2.0, 0.0), egui::Align2::CENTER_CENTER, icon::PLAY, theme::icon_font(22.0), Color32::WHITE);
            if r.clicked() {
                self.play(ui.ctx(), v.url, rect.width());
            }
            return;
        }
        self.player(ui, rect, v.url, v.link, v.poster.as_deref(), Some((v.width, v.height)));
    }

    pub fn is_open(&self, url: &str) -> bool {
        self.players.contains_key(url)
    }

    /// Starts (or resumes) a video, pausing any other.
    pub fn play(&mut self, ctx: &egui::Context, url: &str, width: f32) {
        for (u, p) in &mut self.players {
            if u != url {
                p.set_paused(true);
            }
        }
        let max_width = (width * ctx.pixels_per_point()).ceil() as u32;
        let output = self.output.clone();
        self.players.entry(url.to_owned()).or_insert_with(|| Player::start(ctx, url, output, max_width)).set_paused(false);
    }

    /// The playing video and its controls. `expand` offers the viewer, with the video's size.
    fn player(&mut self, ui: &mut Ui, rect: Rect, url: &str, link: &str, poster_url: Option<&str>, expand: Option<(Option<u32>, Option<u32>)>) {
        // In chat or in the viewer: both can be drawn in the frame that opens the viewer, so their widgets differ.
        let inline = expand.is_some();
        let gain = self.gain();
        let ppp = ui.ctx().pixels_per_point();
        let Some(p) = self.players.get_mut(url) else { return };
        p.last_seen = Instant::now();
        p.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
        p.shared.max_width.store((rect.width() * ppp).ceil() as u32, Ordering::Relaxed);
        p.update(ui.ctx(), url);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6, Color32::BLACK);
        match &p.texture {
            Some(t) => {
                let s = t.size_vec2();
                let fitted = s * (rect.width() / s.x).min(rect.height() / s.y);
                egui::Image::new(t).corner_radius(6).paint_at(ui, Rect::from_center_size(rect.center(), fitted));
            }
            None => poster(ui, rect, poster_url),
        }
        let id = egui::Id::unique(("video", url, inline));
        let surface = ui.interact(rect, id, Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        if let Some(e) = &p.error {
            painter.rect_filled(rect, 6, Color32::from_black_alpha(190));
            painter.text(rect.center() - Vec2::new(0.0, 12.0), egui::Align2::CENTER_CENTER, e, theme::font_semibold(13.0), Color32::WHITE);
            let b = Rect::from_center_size(rect.center() + Vec2::new(0.0, 16.0), Vec2::new(150.0, 26.0));
            let r = ui.put(b, egui::Button::new("Open in browser"));
            if r.clicked() {
                crate::platform::open_url(link);
            }
            return;
        }
        if p.buffering && !p.paused {
            let spinner = Rect::from_center_size(rect.center(), Vec2::splat(32.0));
            ui.put(spinner, egui::Spinner::new().size(32.0).color(Color32::WHITE));
        } else if p.paused {
            let glyph = if p.finished { icon::REPLAY } else { icon::PLAY };
            painter.circle_filled(rect.center(), 26.0, Color32::from_black_alpha(160));
            painter.text(rect.center() + Vec2::new(if p.finished { 0.0 } else { 2.0 }, 0.0), egui::Align2::CENTER_CENTER, glyph, theme::icon_font(22.0), Color32::WHITE);
        }
        let bar = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - BAR), rect.right_bottom());
        let show_bar = hovered || p.paused || p.dragging.is_some();
        let over_bar = show_bar && ui.rect_contains_pointer(bar);
        if surface.clicked() && !over_bar {
            let pause = !p.paused;
            if pause {
                p.set_paused(true);
            } else {
                self.play(ui.ctx(), url, rect.width());
            }
            return;
        }
        if !show_bar {
            return;
        }
        let p = self.players.get_mut(url).expect("player");
        painter.rect_filled(bar, egui::CornerRadius { nw: 0, ne: 0, sw: 6, se: 6 }, Color32::from_black_alpha(170));
        let mut x = bar.left() + 6.0;
        let cy = bar.center().y;
        let button = |ui: &mut Ui, x: &mut f32, glyph: &str, key: &str, tip: &str| -> bool {
            let r = Rect::from_center_size(egui::pos2(*x + 13.0, cy), Vec2::splat(26.0));
            *x += 28.0;
            let resp = ui.interact(r, egui::Id::unique(("video-btn", url, inline, key)), Sense::click()).on_hover_text(tip);
            let color = if resp.hovered() { Color32::WHITE } else { Color32::from_gray(210) };
            painter.text(r.center(), egui::Align2::CENTER_CENTER, glyph, theme::icon_font(15.0), color);
            resp.clicked()
        };
        let (glyph, tip) = match (p.paused, p.finished) {
            (_, true) => (icon::REPLAY, "Replay"),
            (true, _) => (icon::PLAY, "Play"),
            _ => (icon::PAUSE, "Pause"),
        };
        let toggle = button(ui, &mut x, glyph, "play", tip);
        // Right side, from the edge inwards.
        let mut rx = bar.right() - 6.0 - 28.0;
        let mut open_viewer = false;
        let mut close_viewer = false;
        {
            let mut b = |ui: &mut Ui, glyph: &str, key: &str, tip: &str| {
                let mut at = rx;
                rx -= 28.0;
                button(ui, &mut at, glyph, key, tip)
            };
            if b(ui, icon::OPEN_EXTERNAL, "open", "Open in browser") {
                crate::platform::open_url(link);
            }
            match expand {
                Some(_) => open_viewer = b(ui, icon::FULL_SCREEN, "expand", "Expand"),
                None => close_viewer = b(ui, icon::CLOSE, "close", "Close"),
            }
        }
        // Volume: a mute button, and a slider when there's room.
        let wide = rect.width() >= 360.0;
        let slider_w = if wide { 64.0 } else { 0.0 };
        let mute_glyph = if self.muted || self.volume == Some(0.0) { icon::MUTED } else { icon::VOLUME };
        let mut mute_at = rx - slider_w;
        let mute_tip = if self.muted { "Unmute" } else { "Mute" };
        let toggle_mute = button(ui, &mut mute_at, mute_glyph, "mute", mute_tip);
        if wide {
            let track = Rect::from_min_size(egui::pos2(rx - slider_w + 30.0, cy - 2.0), Vec2::new(slider_w - 6.0, 4.0));
            let hit = ui.interact(track.expand2(Vec2::new(4.0, 8.0)), egui::Id::unique(("video-vol", url, inline)), Sense::click_and_drag());
            if let Some(pt) = hit.interact_pointer_pos().filter(|_| hit.dragged() || hit.clicked()) {
                self.volume = Some(((pt.x - track.left()) / track.width()).clamp(0.0, 1.0));
                self.muted = false;
            }
            let v = if self.muted { 0.0 } else { self.volume.unwrap_or(1.0) };
            painter.rect_filled(track, 2, Color32::from_white_alpha(60));
            painter.rect_filled(Rect::from_min_size(track.min, Vec2::new(track.width() * v, 4.0)), 2, Color32::WHITE);
            painter.circle_filled(egui::pos2(track.left() + track.width() * v, track.center().y), 5.0, Color32::WHITE);
        }
        if toggle_mute {
            self.muted = !self.muted;
            if !self.muted && self.volume == Some(0.0) {
                self.volume = Some(0.5);
            }
        }
        let right_edge = rx - slider_w - 6.0;
        let p = self.players.get_mut(url).expect("player");
        // Time, then the seek bar filling the middle.
        let shown = p.dragging.zip(p.duration).map_or(p.position, |(f, d)| (f as f64 * d as f64) as i64);
        let time = match p.duration {
            Some(d) => format!("{} / {}", clock_label(shown), clock_label(d)),
            None => clock_label(shown),
        };
        let galley = painter.layout_no_wrap(time, egui::FontId::proportional(12.0), Color32::WHITE);
        let tw = galley.size().x;
        painter.galley(egui::pos2(x + 4.0, cy - galley.size().y / 2.0), galley, Color32::WHITE);
        x += tw + 14.0;
        if let Some(d) = p.duration.filter(|d| *d > 0 && right_edge - x > 30.0) {
            let track = Rect::from_min_max(egui::pos2(x, cy - 2.0), egui::pos2(right_edge, cy + 2.0));
            let hit = ui.interact(track.expand2(Vec2::new(4.0, 9.0)), egui::Id::unique(("video-seek", url, inline)), Sense::click_and_drag());
            let at = |pt: egui::Pos2| ((pt.x - track.left()) / track.width()).clamp(0.0, 1.0);
            if hit.dragged() {
                p.dragging = hit.interact_pointer_pos().map(at);
            }
            if hit.drag_stopped() || hit.clicked() {
                if let Some(f) = hit.interact_pointer_pos().map(at).or(p.dragging) {
                    p.seek((f as f64 * d as f64) as i64);
                }
                p.dragging = None;
            }
            let frac = p.dragging.unwrap_or((p.position as f64 / d as f64).clamp(0.0, 1.0) as f32);
            painter.rect_filled(track, 2, Color32::from_white_alpha(60));
            painter.rect_filled(Rect::from_min_size(track.min, Vec2::new(track.width() * frac, 4.0)), 2, theme::BLURPLE());
            if hit.hovered() || p.dragging.is_some() {
                painter.circle_filled(egui::pos2(track.left() + track.width() * frac, cy), 6.0, Color32::WHITE);
            }
        }
        if p.dragging.is_some() || hovered {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if toggle {
            if p.paused {
                self.play(ui.ctx(), url, rect.width());
            } else {
                p.set_paused(true);
            }
        }
        if open_viewer {
            let (w, h) = expand.unwrap_or_default();
            let size = Vec2::new(w.unwrap_or(rect.width() as u32) as f32, h.unwrap_or(rect.height() as u32) as f32);
            self.viewer = Some(Viewer { url: url.to_owned(), link: link.to_owned(), poster: poster_url.map(str::to_owned), size });
        }
        if close_viewer {
            self.viewer = None;
        }
    }

    /// The larger view, over everything else, when a video is expanded.
    pub fn viewer(&mut self, ctx: &egui::Context) {
        let Some(v) = &self.viewer else { return };
        let (url, link, poster_url, size) = (v.url.clone(), v.link.clone(), v.poster.clone(), v.size);
        if !self.players.contains_key(&url) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.viewer = None;
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("video-viewer")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let backdrop = ui.interact(screen, egui::Id::unique("video-viewer-backdrop"), Sense::click());
            ui.painter().rect_filled(screen, 0, Color32::from_black_alpha(220));
            let max = screen.size() - Vec2::new(80.0, 80.0);
            let scale = (max.x / size.x.max(1.0)).min(max.y / size.y.max(1.0));
            let rect = Rect::from_center_size(screen.center(), size * scale);
            if ui.input(|i| i.key_pressed(egui::Key::Space)) {
                if let Some(p) = self.players.get_mut(&url) {
                    let pause = !p.paused;
                    if pause {
                        p.set_paused(true);
                    } else {
                        self.play(ui.ctx(), &url, rect.width());
                    }
                }
            }
            self.player(ui, rect, &url, &link, poster_url.as_deref(), None);
            if backdrop.clicked() && !ui.rect_contains_pointer(rect) {
                self.viewer = None;
            }
        });
        if self.viewer.is_none() {
            // Back in chat, where it was playing.
            if let Some(p) = self.players.get_mut(&url) {
                p.last_seen = Instant::now();
            }
        }
    }

    /// Pauses videos scrolled out of view and closes long-unseen ones.
    pub fn tick(&mut self) {
        let viewing = self.viewer.as_ref().map(|v| v.url.clone());
        self.players.retain(|u, p| viewing.as_ref() == Some(u) || p.last_seen.elapsed() < FORGET);
        for (u, p) in &mut self.players {
            if p.last_seen.elapsed() > OFF_SCREEN && !p.paused && viewing.as_ref() != Some(u) {
                p.set_paused(true);
            }
        }
    }
}

fn poster(ui: &Ui, rect: Rect, url: Option<&str>) {
    ui.painter().rect_filled(rect, 6, Color32::BLACK);
    if let Some(u) = url.filter(|u| !u.is_empty()) {
        egui::Image::from_uri(u.to_owned()).corner_radius(6).show_loading_spinner(false).paint_at(ui, rect);
    }
}

/// `m:ss`, or `h:mm:ss` for long videos.
fn clock_label(t: i64) -> String {
    let s = (t.max(0) / SECOND) as u64;
    match s / 3600 {
        0 => format!("{}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// Feeds the sound device from the queue, in step with the clock: sound that's fallen behind is skipped, and
/// silence is played until sound that's ahead is due.
fn sound_feed(sh: Arc<Shared>) -> impl FnMut(&mut [f32]) + Send + 'static {
    let per_frame = SECOND as f64 / RATE as f64;
    move |out: &mut [f32]| {
        out.fill(0.0);
        let pos = {
            let c = sh.clock.lock();
            if c.since.is_none() {
                return;
            }
            c.now()
        };
        let gain = f32::from_bits(sh.gain.load(Ordering::Relaxed));
        let mut q = sh.q.lock();
        let Queues { audio, audio_pos, .. } = &mut *q;
        let late = pos - SECOND / 12;
        let mut i = 0;
        while let Some((pts, chunk)) = audio.front() {
            let head = pts + ((*audio_pos / 2) as f64 * per_frame) as i64;
            let end = pts + ((chunk.len() / 2) as f64 * per_frame) as i64;
            if end <= late {
                audio.pop_front();
                *audio_pos = 0;
                continue;
            }
            if head < late {
                *audio_pos = ((((pos - pts) as f64 / per_frame) as usize) * 2).min(chunk.len());
            } else if head > pos + SECOND / 25 {
                i = ((((head - pos) as f64 / per_frame) as usize) * 2).min(out.len());
            }
            break;
        }
        while i < out.len() {
            let Some((_, chunk)) = audio.front() else { break };
            let n = (out.len() - i).min(chunk.len() - *audio_pos);
            for (o, v) in out[i..i + n].iter_mut().zip(&chunk[*audio_pos..*audio_pos + n]) {
                *o = v * gain;
            }
            i += n;
            *audio_pos += n;
            if *audio_pos >= chunk.len() {
                audio.pop_front();
                *audio_pos = 0;
            }
        }
    }
}

/// Converts decoded sound to 48 kHz stereo: extra channels dropped, mono doubled, and linear resampling.
pub(crate) struct Converter {
    channels: usize,
    step: f64,
    pos: f64,
    prev: [f32; 2],
}

impl Converter {
    pub(crate) fn new(rate: u32, channels: usize) -> Self {
        Self { channels: channels.max(1), step: rate as f64 / RATE as f64, pos: 0.0, prev: [0.0; 2] }
    }

    pub(crate) fn convert(&mut self, input: &[f32]) -> Vec<f32> {
        let frames = input.len() / self.channels;
        let stereo = |i: usize| -> [f32; 2] {
            let f = &input[i * self.channels..];
            if self.channels == 1 { [f[0], f[0]] } else { [f[0], f[1]] }
        };
        let mut out = Vec::with_capacity((frames as f64 / self.step) as usize * 2 + 4);
        if (self.step - 1.0).abs() < f64::EPSILON {
            for i in 0..frames {
                out.extend(stereo(i));
            }
            return out;
        }
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize - 1;
            let t = self.pos.fract() as f32;
            let a = if i < 0 { self.prev } else { stereo(i as usize) };
            let b = stereo((i + 1) as usize);
            out.extend([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.prev = stereo(frames - 1);
        }
        out
    }

    pub(crate) fn reset(&mut self) {
        self.pos = 0.0;
        self.prev = [0.0; 2];
    }
}

#[cfg(windows)]
pub(crate) mod decode {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use eframe::egui;
    use windows::core::HSTRING;
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
    use windows::Win32::System::Variant::VT_I8;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::{sound_feed, Converter, Shared, AHEAD_FRAMES, AHEAD_SOUND, RATE, SECOND};
    use crate::clip::decode::{frame_image, select_video};

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

    /// Selects the first audio stream decoded to float, returning its rate and channel count.
    pub(crate) fn select_audio(reader: &IMFSourceReader) -> windows::core::Result<(u32, usize)> {
        unsafe {
            reader.SetStreamSelection(AUDIO, true)?;
            let want = MFCreateMediaType()?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            if let Err(e) = reader.SetCurrentMediaType(AUDIO, None, &want) {
                let _ = reader.SetStreamSelection(AUDIO, false);
                return Err(e);
            }
            let got = reader.GetCurrentMediaType(AUDIO)?;
            let rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
            let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? as usize;
            Ok((rate, channels))
        }
    }

    pub(crate) fn pcm(sample: &IMFSample) -> windows::core::Result<Vec<f32>> {
        unsafe {
            let buf = sample.ConvertToContiguousBuffer()?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut ptr, None, Some(&mut len))?;
            let floats = std::slice::from_raw_parts(ptr as *const f32, len as usize / 4).to_vec();
            buf.Unlock()?;
            Ok(floats)
        }
    }

    pub(super) fn run(url: &str, output: Option<String>, sh: &Arc<Shared>, ctx: &egui::Context) -> Result<(), String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| e.to_string())?;
        }
        let reader = unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
            MFCreateSourceReaderFromURL(&HSTRING::from(url), &attrs).map_err(|e| {
                tracing::info!("can't open {url}: {e}");
                "Couldn't load this video.".to_owned()
            })?
        };
        unsafe {
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        }
        let mut video = select_video(reader.clone()).map_err(|_| "This video's format isn't supported here.".to_owned())?;
        let format = select_audio(&reader).ok();
        let duration = unsafe { reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) }
            .ok()
            .map(|v| unsafe { v.Anonymous.Anonymous.Anonymous.uhVal } as i64)
            .filter(|d| *d > 0);
        let sound = format.and_then(|_| swiftcord_voice::play(output, sound_feed(sh.clone())).map_err(|e| tracing::warn!("video sound unavailable: {e}")).ok());
        let mut conv = format.filter(|_| sound.is_some()).map(|(rate, ch)| Converter::new(rate, ch));
        {
            let mut q = sh.q.lock();
            q.opened = true;
            q.duration = duration;
        }
        ctx.request_repaint();
        let (mut video_end, mut audio_end) = (false, conv.is_none());
        let (mut last_v, mut last_a) = (i64::MIN, i64::MIN);
        // After a seek, what's decoded before the target (from the keyframe before it) is skipped.
        let mut skip_before = 0;
        while !sh.stop.load(Ordering::Relaxed) {
            let (seek, frames, sound_ahead) = {
                let q = sh.q.lock();
                (q.seek, q.frames.len(), q.sound_ahead())
            };
            if let Some(t) = seek {
                let at = PROPVARIANT {
                    Anonymous: PROPVARIANT_0 {
                        Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 { vt: VT_I8, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: PROPVARIANT_0_0_0 { hVal: t } }),
                    },
                };
                unsafe { reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &at) }.map_err(|e| e.to_string())?;
                let mut q = sh.q.lock();
                if q.seek == Some(t) {
                    q.seek = None;
                }
                q.clear();
                (video_end, audio_end) = (false, conv.is_none());
                (last_v, last_a) = (i64::MIN, i64::MIN);
                skip_before = t;
                if let Some(c) = &mut conv {
                    c.reset();
                }
                continue;
            }
            if video_end && audio_end {
                sh.q.lock().ended = true;
                ctx.request_repaint();
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let video_full = video_end || frames >= AHEAD_FRAMES;
            let sound_full = audio_end || sound_ahead >= AHEAD_SOUND;
            if video_full && sound_full {
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            // Read whichever stream is behind, so neither piles up inside the reader.
            let read_audio = !audio_end && (video_full || (!sound_full && last_a <= last_v));
            let stream = if read_audio { AUDIO } else { VIDEO };
            let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
            unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) }.map_err(|e| e.to_string())?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err("The video stopped loading.".into());
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if read_audio {
                    audio_end = true;
                } else {
                    video_end = true;
                }
            }
            if !read_audio && flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                video.refresh().map_err(|e| e.to_string())?;
            }
            let Some(sample) = sample else { continue };
            if read_audio {
                last_a = ts;
                let Some(c) = &mut conv else { continue };
                let mut chunk = c.convert(&pcm(&sample).map_err(|e| e.to_string())?);
                let mut start = ts;
                if start < skip_before {
                    let cut = (((skip_before - start) * RATE as i64 / SECOND) as usize * 2).min(chunk.len());
                    chunk.drain(..cut);
                    start = skip_before;
                }
                let mut q = sh.q.lock();
                if !chunk.is_empty() && q.seek.is_none() {
                    q.audio.push_back((start, chunk));
                }
            } else {
                last_v = ts;
                // Frames shortly before the target are kept, so a seek always lands on a picture.
                if ts < skip_before - SECOND / 20 {
                    continue;
                }
                let max_width = sh.max_width.load(Ordering::Relaxed);
                if let Some(img) = unsafe { frame_image(&sample, &video, max_width) }.map_err(|e| e.to_string())? {
                    let mut q = sh.q.lock();
                    if q.seek.is_none() {
                        q.frames.push_back((ts.max(skip_before), img));
                    }
                    drop(q);
                    ctx.request_repaint();
                }
            }
        }
        drop(sound);
        Ok(())
    }
}

#[cfg(not(windows))]
mod decode {
    pub fn run(_: &str, _: Option<String>, _: &std::sync::Arc<super::Shared>, _: &eframe::egui::Context) -> Result<(), String> {
        Err("Videos need Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_times() {
        assert_eq!(clock_label(0), "0:00");
        assert_eq!(clock_label(65 * SECOND), "1:05");
        assert_eq!(clock_label(3725 * SECOND), "1:02:05");
    }

    #[test]
    fn converts_to_48k_stereo() {
        let mut c = Converter::new(44_100, 1);
        let out = c.convert(&vec![0.5; 44_100]);
        // One second in, one second (of stereo) out, give or take a sample.
        assert!((out.len() as i64 - 96_000).abs() <= 4, "{}", out.len());
        assert!(out.iter().skip(4).all(|s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn sound_follows_the_clock() {
        let sh = Arc::new(Shared {
            q: Mutex::new(Queues::default()),
            clock: Mutex::new(Clock { base: SECOND, since: None }),
            stop: AtomicBool::new(false),
            max_width: AtomicU32::new(0),
            gain: AtomicU32::new(1f32.to_bits()),
        });
        // Half a second behind (skipped) and the moment now.
        sh.q.lock().audio.push_back((SECOND / 2, vec![0.25; RATE]));
        sh.q.lock().audio.push_back((SECOND, vec![0.75; RATE * 2]));
        let mut feed = sound_feed(sh.clone());
        let mut out = vec![1.0; 960];
        feed(&mut out);
        assert!(out.iter().all(|s| *s == 0.0), "silent while paused");
        sh.clock.lock().since = Some(Instant::now());
        feed(&mut out);
        assert!(out.iter().all(|s| *s == 0.75), "plays what's due now");
        assert_eq!(sh.q.lock().audio.len(), 1);
    }
}

#[cfg(all(test, windows))]
mod timing {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;

    /// Times decoding `SWIFTCORD_TEST_VIDEO` with the reader converting to RGB32 and with the decoder's own output.
    #[test]
    fn decode_speed() {
        let Ok(path) = std::env::var("SWIFTCORD_TEST_VIDEO") else { return };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE).unwrap() };
        for (name, sub, processing) in [("rgb32", MFVideoFormat_RGB32, 1u32), ("nv12", MFVideoFormat_NV12, 0)] {
            let t = std::time::Instant::now();
            let mut n = 0;
            unsafe {
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 1).unwrap();
                let attrs = attrs.unwrap();
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, processing).unwrap();
                let r = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_str()), &attrs).unwrap();
                let v = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
                let want = MFCreateMediaType().unwrap();
                want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
                want.SetGUID(&MF_MT_SUBTYPE, &sub).unwrap();
                r.SetCurrentMediaType(v, None, &want).unwrap();
                loop {
                    let (mut flags, mut s) = (0u32, None);
                    r.ReadSample(v, 0, None, Some(&mut flags), None, Some(&mut s)).unwrap();
                    if s.is_some() {
                        n += 1;
                    }
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                }
            }
            println!("{name}: {n} frames in {:?}", t.elapsed());
        }
    }
}
