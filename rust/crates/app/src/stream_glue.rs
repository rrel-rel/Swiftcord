//! Watching Go Live streams.
//!
//! Watching needs a connection to the stream's voice channel. We send STREAM_WATCH; Discord answers with
//! STREAM_CREATE (which RTC server carries the stream) and STREAM_SERVER_UPDATE (its endpoint and token), and a
//! second, receive-only voice connection to that server brings the video and the stream's audio.

use eframe::egui::{self, ColorImage, TextureOptions};
use serde::Deserialize;
use serde_json::value::RawValue;
use swiftcord_core::Snowflake;
use swiftcord_voice::{ConnectInfo, Event, Voice};

use crate::app::{App, AppEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStatus {
    /// Joining the voice channel or waiting for Discord to assign a stream server.
    Waiting,
    Connecting,
    Live,
}

pub struct Watch {
    key: String,
    owner: Snowflake,
    channel: Snowflake,
    requested: bool,
    rtc_server_id: Option<u64>,
    server: Option<(String, String)>,
    voice: Option<Voice>,
    generation: u64,
}

/// What the UI shows for the stream being watched.
pub struct StreamView {
    pub owner: Snowflake,
    pub guild: Option<Snowflake>,
    pub status: WatchStatus,
    pub texture: Option<egui::TextureHandle>,
    /// Shown in the main area (otherwise the chat is, with a bar to come back).
    pub visible: bool,
    pub fullscreen: bool,
}

pub fn stream_key(guild: Option<Snowflake>, channel: Snowflake, user: Snowflake) -> String {
    match guild {
        Some(g) => format!("guild:{g}:{channel}:{user}"),
        None => format!("call:{channel}:{user}"),
    }
}

pub fn watch(app: &mut App, guild: Option<Snowflake>, channel: Snowflake, user: Snowflake) {
    let key = stream_key(guild, channel, user);
    if app.watch.as_ref().is_some_and(|w| w.key == key) {
        if let Some(v) = &mut app.ui.stream {
            v.visible = true;
        }
        return;
    }
    stop(app);
    // Streams can only be watched from inside their voice channel.
    if app.voice.as_ref().is_none_or(|c| c.channel != channel) {
        crate::voice_glue::join(app, guild, channel);
        if app.voice.as_ref().is_none_or(|c| c.channel != channel) {
            return; // Couldn't join (no permission); join() said why.
        }
    }
    app.watch = Some(Watch { key, owner: user, channel, requested: false, rtc_server_id: None, server: None, voice: None, generation: 0 });
    app.ui.stream = Some(StreamView { owner: user, guild, status: WatchStatus::Waiting, texture: None, visible: true, fullscreen: false });
    request_if_ready(app);
}

/// Sends STREAM_WATCH once our own voice connection is up.
pub fn request_if_ready(app: &mut App) {
    let connected = app.voice.as_ref().is_some_and(|c| matches!(c.status, crate::voice_glue::Status::Connected { .. }));
    let (Some(w), Some(s)) = (&mut app.watch, &app.session) else { return };
    if connected && !w.requested && app.voice.as_ref().is_some_and(|c| c.channel == w.channel) {
        w.requested = true;
        tracing::info!("watching stream {}", w.key);
        s.gateway.watch_stream(&w.key);
    }
}

pub fn stop(app: &mut App) {
    if let Some(w) = app.watch.take() {
        if w.requested {
            if let Some(s) = &app.session {
                s.gateway.stop_stream(&w.key);
            }
        }
        drop(w.voice);
    }
    if app.ui.stream.as_ref().is_some_and(|v| v.fullscreen) {
        app.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
    app.ui.stream = None;
}

#[derive(Deserialize)]
pub struct StreamEvent {
    pub stream_key: String,
    #[serde(default)]
    pub rtc_server_id: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Handles STREAM_* gateway events.
pub fn on_gateway(app: &mut App, name: &str, data: &RawValue) {
    if !name.starts_with("STREAM_") {
        return;
    }
    let Ok(e) = serde_json::from_str::<StreamEvent>(data.get()) else { return };
    tracing::info!("{name} for {} (server {:?}, endpoint {:?})", e.stream_key, e.rtc_server_id, e.endpoint);
    if crate::share_glue::on_stream_event(app, name, &e) {
        return;
    }
    let Some(w) = &mut app.watch else { return };
    if e.stream_key != w.key {
        return;
    }
    match name {
        "STREAM_CREATE" => {
            w.rtc_server_id = e.rtc_server_id.and_then(|s| s.parse().ok());
            connect(app);
        }
        "STREAM_SERVER_UPDATE" => {
            if let (Some(endpoint), Some(token)) = (e.endpoint, e.token) {
                w.server = Some((endpoint, token));
                drop(w.voice.take());
                connect(app);
            }
        }
        "STREAM_DELETE" => {
            tracing::info!("stream ended: {:?}", e.reason);
            // The server already ended it; don't send our own delete.
            w.requested = false;
            stop(app);
            let msg = match e.reason.as_deref() {
                Some("unauthorized") => "You aren't allowed to watch that stream.",
                Some("stream_full") => "That stream is full.",
                _ => "The stream has ended.",
            };
            app.banner(msg);
        }
        _ => {}
    }
}

fn connect(app: &mut App) {
    let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|u| u.id) else { return };
    let Some(session_id) = app.voice.as_ref().and_then(|c| c.session_id().map(str::to_owned)) else { return };
    let settings = swiftcord_voice::Settings {
        devices: swiftcord_voice::Devices { input: None, output: app.settings.output_device.clone() },
        vad_threshold_db: app.settings.vad_threshold_db,
        muted: true,
        deafened: app.ui.self_deaf && !app.settings.stream_audio_when_deafened,
        // Never sends audio.
        noise_suppression: false,
    };
    let sink = app.event_sink();
    let Some(w) = &mut app.watch else { return };
    let (Some(server_id), Some((endpoint, token))) = (w.rtc_server_id, &w.server) else { return };
    if w.voice.is_some() {
        return;
    }
    let info = ConnectInfo {
        user_id: me.0,
        server_id,
        channel_id: w.channel.0,
        session_id,
        token: token.clone(),
        endpoint: endpoint.clone(),
        stream_owner: Some(w.owner.0),
        share: None,
    };
    w.generation += 1;
    let generation = w.generation;
    w.voice = Some(Voice::connect(&app.rt, info, settings, move |e| sink(AppEvent::Stream { generation, event: e })));
    update_self_state(app);
    if let Some(v) = &mut app.ui.stream {
        v.status = WatchStatus::Connecting;
    }
}

pub fn on_event(app: &mut App, generation: u64, event: Event) {
    let Some(w) = &app.watch else { return };
    if w.generation != generation {
        return;
    }
    match event {
        Event::VideoFrame => {
            let Some(p) = w.voice.as_ref().and_then(|v| v.take_picture()) else { return };
            let image = ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
            let ctx = app.ctx().clone();
            let Some(view) = &mut app.ui.stream else { return };
            view.status = WatchStatus::Live;
            match &mut view.texture {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => view.texture = Some(ctx.load_texture("go-live", image, TextureOptions::LINEAR)),
            }
        }
        // Stream connections have no camera.
        Event::Connected { .. } | Event::Connecting | Event::CameraStopped => {}
        Event::Error(e) => app.banner(e),
        Event::Disconnected(reason) => {
            tracing::info!("stream connection closed: {reason}");
            stop(app);
            app.banner(format!("Stopped watching: {reason}"));
        }
    }
}

/// Applies the stream's sound settings: its volume, and whether deafening silences it too (it does unless
/// "keep stream sound when deafened" is on).
pub fn update_self_state(app: &App) {
    let Some(w) = &app.watch else { return };
    let Some(v) = &w.voice else { return };
    v.set_deafened(app.ui.self_deaf && !app.settings.stream_audio_when_deafened);
    v.set_user_volume(w.owner.0, app.settings.stream_volume.clamp(0.0, 2.0));
}
