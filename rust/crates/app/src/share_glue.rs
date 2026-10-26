//! Going live: sharing a screen or window into the current call.
//!
//! STREAM_CREATE (gateway op 18) asks for a stream in our voice channel; as when watching, Discord answers with
//! STREAM_CREATE (which RTC server carries it) and STREAM_SERVER_UPDATE (endpoint and token) for our own stream
//! key, and a second voice connection to that server carries the video. Stopping sends STREAM_DELETE.

use eframe::egui::{self, ColorImage, TextureOptions};
use swiftcord_core::Snowflake;
use swiftcord_voice::capture::{self, SourceInfo};
use swiftcord_voice::{ConnectInfo, Event, Share, ShareStats, Voice};

use crate::app::{App, AppEvent};
use crate::stream_glue::{stream_key, StreamEvent};

pub struct Sharing {
    key: String,
    channel: Snowflake,
    share: Share,
    rtc_server_id: Option<u64>,
    server: Option<(String, String)>,
    voice: Option<Voice>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareStatus {
    /// Waiting for Discord to set up the stream, or connecting to its server.
    Starting,
    Live,
}

/// What the UI shows about our own stream.
pub struct ShareView {
    pub status: ShareStatus,
    pub label: String,
    pub preview: Option<egui::TextureHandle>,
    pub stats: Option<ShareStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Output height (16:9 box); 0 keeps the source size.
    pub height: u32,
    pub fps: u32,
}

impl Default for Quality {
    fn default() -> Self {
        Quality { height: 720, fps: 30 }
    }
}

impl Quality {
    pub fn max_size(self) -> (u32, u32) {
        match self.height {
            0 => (3840, 2160),
            h => (h * 16 / 9, h),
        }
    }

    /// Enough bits for screen content (text, sharp edges) at this size and rate.
    pub fn bitrate(self, width: u32, height: u32) -> u32 {
        let bits = width as f64 * height as f64 * self.fps as f64 * 0.09;
        bits.clamp(600_000.0, 8_000_000.0) as u32
    }
}

/// The open "share your screen" window: sources enumerated once when it opened.
pub struct Picker {
    pub sources: Vec<SourceInfo>,
    pub selected: Option<usize>,
    pub quality: Quality,
}

pub fn open_picker(app: &mut App) {
    if !in_call(app) {
        app.banner("Join a voice channel or call to go live.");
        return;
    }
    let sources = capture::sources();
    let selected = (!sources.is_empty()).then_some(0);
    app.ui.share_picker = Some(Picker { sources, selected, quality: app.ui.share_quality });
}

fn in_call(app: &App) -> bool {
    app.voice.as_ref().is_some_and(|c| matches!(c.status, crate::voice_glue::Status::Connected { .. }))
}

pub fn start(app: &mut App) {
    let Some(picker) = app.ui.share_picker.take() else { return };
    let Some(source) = picker.selected.and_then(|i| picker.sources.get(i)).cloned() else { return };
    app.ui.share_quality = picker.quality;
    stop(app);
    let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|u| u.id) else { return };
    let Some(call) = app.voice.as_ref().filter(|_| in_call(app)) else {
        app.banner("Join a voice channel or call to go live.");
        return;
    };
    let (guild, channel) = (call.guild, call.channel);
    let key = stream_key(guild, channel, me);
    let (max_width, max_height) = picker.quality.max_size();
    let (w, h) = capture::output_size(&source.source, max_width, max_height).unwrap_or((max_width, max_height));
    let share = Share { source: source.source, max_width, max_height, fps: picker.quality.fps, bitrate: picker.quality.bitrate(w, h) };
    tracing::info!("going live as {key}: {w}x{h} at {} fps, {} kbps", share.fps, share.bitrate / 1000);
    if let Some(s) = &app.session {
        s.gateway.create_stream(guild, channel);
        s.gateway.set_stream_paused(&key, false);
    }
    app.sharing = Some(Sharing { key, channel, share, rtc_server_id: None, server: None, voice: None, generation: 0 });
    app.ui.share = Some(ShareView { status: ShareStatus::Starting, label: source.label, preview: None, stats: None });
}

pub fn stop(app: &mut App) {
    if let Some(sh) = app.sharing.take() {
        if let Some(s) = &app.session {
            s.gateway.stop_stream(&sh.key);
        }
        drop(sh.voice);
    }
    app.ui.share = None;
}

/// Handles STREAM_* events about our own stream; false if the event is about someone else's.
pub fn on_stream_event(app: &mut App, name: &str, e: &StreamEvent) -> bool {
    let Some(sh) = &mut app.sharing else { return false };
    if e.stream_key != sh.key {
        return false;
    }
    match name {
        "STREAM_CREATE" => {
            sh.rtc_server_id = e.rtc_server_id.as_deref().and_then(|s| s.parse().ok());
            connect(app);
        }
        "STREAM_SERVER_UPDATE" => {
            if let (Some(endpoint), Some(token)) = (e.endpoint.clone(), e.token.clone()) {
                sh.server = Some((endpoint, token));
                drop(sh.voice.take());
                connect(app);
            }
        }
        "STREAM_DELETE" => {
            tracing::info!("our stream was ended: {:?}", e.reason);
            if let Some(sh) = app.sharing.take() {
                drop(sh.voice);
            }
            app.ui.share = None;
            if e.reason.as_deref() != Some("user_requested") {
                app.banner("Your stream has ended.");
            }
        }
        _ => {}
    }
    true
}

fn connect(app: &mut App) {
    let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|u| u.id) else { return };
    let Some(session_id) = app.voice.as_ref().and_then(|c| c.session_id().map(str::to_owned)) else { return };
    let sink = app.event_sink();
    let Some(sh) = &mut app.sharing else { return };
    let (Some(server_id), Some((endpoint, token))) = (sh.rtc_server_id, &sh.server) else { return };
    if sh.voice.is_some() {
        return;
    }
    let info = ConnectInfo {
        user_id: me.0,
        server_id,
        channel_id: sh.channel.0,
        session_id,
        token: token.clone(),
        endpoint: endpoint.clone(),
        stream_owner: None,
        share: Some(sh.share.clone()),
    };
    let settings = swiftcord_voice::Settings { muted: true, ..Default::default() };
    sh.generation += 1;
    let generation = sh.generation;
    sh.voice = Some(Voice::connect(&app.rt, info, settings, move |e| sink(AppEvent::Share { generation, event: e })));
}

pub fn on_event(app: &mut App, generation: u64, event: Event) {
    let Some(sh) = &app.sharing else { return };
    if sh.generation != generation {
        return;
    }
    match event {
        Event::VideoFrame => {
            let Some(p) = sh.voice.as_ref().and_then(|v| v.take_picture()) else { return };
            let image = ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
            let ctx = app.ctx().clone();
            let Some(view) = &mut app.ui.share else { return };
            match &mut view.preview {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => view.preview = Some(ctx.load_texture("my-stream", image, TextureOptions::LINEAR)),
            }
        }
        Event::Connected { .. } => {
            if let Some(view) = &mut app.ui.share {
                view.status = ShareStatus::Live;
            }
        }
        Event::Connecting => {}
        Event::Error(e) => app.banner(e),
        Event::Disconnected(reason) => {
            tracing::info!("stream connection closed: {reason}");
            stop(app);
            app.banner(format!("Stopped streaming: {reason}"));
        }
    }
}

pub fn tick(app: &mut App) {
    let stats = app.sharing.as_ref().and_then(|s| s.voice.as_ref()).and_then(|v| v.share_stats());
    if let Some(view) = &mut app.ui.share {
        view.stats = stats;
    }
}
