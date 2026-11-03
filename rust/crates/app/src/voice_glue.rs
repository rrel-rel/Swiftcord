//! Connects the voice engine to the session.
//!
//! Joining asks the main gateway for a voice state; Discord answers with our VOICE_STATE_UPDATE (the
//! session ID) and a VOICE_SERVER_UPDATE (endpoint and token), in either order. Once both are in, the voice
//! connection starts. A later VOICE_SERVER_UPDATE means the call moved servers, so we reconnect.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{self, Align, Color32, Layout, RichText, Ui};
use swiftcord_core::events::VoiceServerUpdate;
use swiftcord_core::state::Notice;
use swiftcord_core::Snowflake;
use swiftcord_voice::{ConnectInfo, Event, Voice};

use crate::app::{Action, App, AppEvent, Session, UiState};
use crate::theme::{self, icon};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waiting for Discord to assign a voice server.
    Joining,
    Connecting,
    Connected { e2ee: bool },
}

pub struct Call {
    pub guild: Option<Snowflake>,
    pub channel: Snowflake,
    pub status: Status,
    session_id: Option<String>,
    server: Option<VoiceServerUpdate>,
    voice: Option<Voice>,
    /// Bumped per connection, so events from a replaced connection are ignored.
    generation: u64,
    /// Who else was in the channel at the last tick (once connected), for join and leave sounds.
    others: Option<HashSet<Snowflake>>,
    /// When others joined during this call, to tell a join sound from someone using the soundboard.
    joined_at: HashMap<Snowflake, Instant>,
}

/// How soon after someone joins a sound of theirs counts as their join sound. Discord's event doesn't say which
/// kind a sound is.
const JOIN_SOUND_WINDOW: Duration = Duration::from_secs(6);

/// What the UI shows about the current call, refreshed every tick.
#[derive(Debug, Clone)]
pub struct VoiceView {
    pub guild: Option<Snowflake>,
    pub channel: Snowflake,
    pub status: Status,
    pub ping_ms: u32,
    pub privacy_code: Option<String>,
}

impl Call {
    pub fn voice(&self) -> Option<&Voice> {
        self.voice.as_ref()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

pub fn join(app: &mut App, guild: Option<Snowflake>, channel: Snowflake) {
    if app.voice.as_ref().is_some_and(|c| c.channel == channel) {
        return;
    }
    // The demo's call goes nowhere: it only shows the call's controls.
    if crate::demo::enabled() && channel == crate::demo::CHANNEL {
        app.voice = Some(Call { guild, channel, status: Status::Connected { e2ee: true }, session_id: None, server: None, voice: None, generation: 0, others: None, joined_at: HashMap::new() });
        return;
    }
    let Some(s) = &app.session else { return };
    if s.state.permissions(channel) & swiftcord_core::permissions::perm::CONNECT == 0 {
        app.banner("You don't have permission to join that voice channel.");
        return;
    }
    // Switching channels: the old connection goes, the gateway moves us. A new call starts with the camera off.
    if let Some(old) = app.voice.take() {
        drop(old.voice);
    }
    app.ui.self_video = false;
    app.ui.cameras.clear();
    s.gateway.update_voice_state(guild, Some(channel), app.ui.self_mute, app.ui.self_deaf, false);
    if guild.is_none() && s.state.call(channel).is_none() {
        // Starting a DM call (not answering one): ring the other people, as the official client does.
        let rest = s.rest.clone();
        app.rt.spawn(async move {
            let _ = rest.ring(channel).await;
        });
    }
    app.voice = Some(Call { guild, channel, status: Status::Joining, session_id: None, server: None, voice: None, generation: 0, others: None, joined_at: HashMap::new() });
}

/// A call sound, unless they're turned off or we're deafened.
fn call_sound(app: &mut App, sound: crate::sounds::Sound) {
    if app.settings.call_sounds && !app.ui.self_deaf {
        app.sounds.play(sound, &app.settings.output_device);
    }
}

pub fn leave(app: &mut App) {
    crate::stream_glue::stop(app);
    crate::share_glue::stop(app);
    let Some(call) = app.voice.take() else { return };
    if matches!(call.status, Status::Connected { .. }) {
        call_sound(app, crate::sounds::Sound::Leave);
    }
    drop(call.voice);
    if let Some(s) = &app.session {
        s.gateway.update_voice_state(call.guild, None, false, false, false);
    }
    app.ui.speaking.clear();
    app.ui.voice = None;
    app.ui.self_video = false;
    app.ui.cameras.clear();
}

pub fn update_self_state(app: &mut App) {
    let (mute, deaf, video) = (app.ui.self_mute, app.ui.self_deaf, app.ui.self_video);
    let Some(call) = &app.voice else { return };
    if let Some(v) = &call.voice {
        v.set_muted(mute);
        v.set_deafened(deaf);
    }
    if let Some(s) = &app.session {
        s.gateway.update_voice_state(call.guild, Some(call.channel), mute, deaf, video);
    }
    crate::stream_glue::update_self_state(app);
}

/// Turns our camera on or off in the current call.
pub fn toggle_camera(app: &mut App) {
    let connected = app.voice.as_ref().is_some_and(|c| matches!(c.status, Status::Connected { .. }) && c.voice.is_some());
    if !connected {
        app.banner("Join a voice channel or call to turn on your camera.");
        return;
    }
    app.ui.self_video = !app.ui.self_video;
    let device = app.settings.camera_device.clone();
    if let Some(v) = app.voice.as_ref().and_then(|c| c.voice.as_ref()) {
        v.set_camera(app.ui.self_video.then_some(device));
    }
    if !app.ui.self_video {
        if let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()) {
            app.ui.cameras.remove(&me.id);
        }
    }
    update_self_state(app);
}

/// Uploads new camera pictures, and forgets the cameras of people who turned theirs off or left.
fn update_cameras(app: &mut App) {
    let Some(s) = &app.session else { return };
    let me = s.state.me.as_ref().map(|m| m.id);
    let Some(call) = &app.voice else { return };
    let ctx = app.ctx().clone();
    if let Some(v) = &call.voice {
        for (user, p) in v.take_cameras() {
            let user = Snowflake(user);
            if Some(user) == me && !app.ui.self_video {
                continue;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
            match app.ui.cameras.get_mut(&user) {
                Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                None => {
                    let t = ctx.load_texture(format!("camera-{user}"), image, egui::TextureOptions::LINEAR);
                    app.ui.cameras.insert(user, t);
                }
            }
        }
    }
    let on: HashSet<Snowflake> = s
        .state
        .channel(call.channel)
        .map(|c| s.state.voice_members(c).iter().filter(|v| v.self_video).map(|v| v.user_id).collect())
        .unwrap_or_default();
    app.ui.cameras.retain(|u, _| on.contains(u) || (Some(*u) == me && app.ui.self_video));
}

pub fn on_notice(app: &mut App, notice: Notice) {
    match notice {
        Notice::OwnVoiceState => {
            let Some(s) = &app.session else { return };
            let mine = s.state.my_voice_state().cloned();
            let Some(call) = &mut app.voice else { return };
            match mine {
                Some(vs) => {
                    // We may have been moved by a moderator.
                    if let Some(c) = vs.channel_id.filter(|&c| c != call.channel) {
                        call.channel = c;
                        call.others = None;
                    }
                    call.guild = vs.guild_id;
                    if call.session_id.as_deref() != Some(&vs.session_id) {
                        call.session_id = Some(vs.session_id);
                        connect(app);
                    }
                }
                None if call.status != Status::Joining => {
                    // Disconnected from elsewhere (kicked, or joined on another device).
                    crate::stream_glue::stop(app);
                    crate::share_glue::stop(app);
                    drop(app.voice.take());
                    app.ui.speaking.clear();
                    app.ui.voice = None;
                }
                None => {}
            }
        }
        Notice::VoiceServer(server) => {
            let Some(call) = &mut app.voice else { return };
            let ours = match server.guild_id {
                Some(g) => call.guild == Some(g),
                None => server.channel_id.is_none_or(|c| c == call.channel),
            };
            if !ours {
                return;
            }
            // No endpoint means the server went away; another update follows.
            if server.endpoint.is_none() {
                call.status = Status::Connecting;
                drop(call.voice.take());
                return;
            }
            call.server = Some(server);
            drop(call.voice.take());
            connect(app);
        }
        // Only sent to people who manage the server.
        Notice::Other(name, _) if name == "AUTO_MODERATION_MENTION_RAID_DETECTION" => {
            app.banner("AutoMod detected a mention raid in one of your servers.");
        }
        Notice::Other(name, data) if name == "VOICE_CHANNEL_EFFECT_SEND" => on_channel_effect(app, data.get()),
        Notice::Other(name, data) => {
            if !crate::interactions_glue::on_gateway(app, &name, &data) {
                crate::stream_glue::on_gateway(app, &name, &data);
            }
        }
        Notice::MessageCreated { .. } | Notice::Ring(_) | Notice::RingStopped(_) | Notice::FrecencyChanged | Notice::OwnStatusChanged => {}
    }
}

/// Whether a sound from `user` in this call is their join sound: they joined moments ago, or are joining now
/// (the event can arrive before the voice state saying so).
fn is_joining(others: Option<&HashSet<Snowflake>>, joined_at: &HashMap<Snowflake, Instant>, user: Snowflake) -> bool {
    let Some(others) = others else { return false };
    !others.contains(&user) || joined_at.get(&user).is_some_and(|t| t.elapsed() < JOIN_SOUND_WINDOW)
}

/// Someone sent a reaction or sound to the voice channel. Join sounds are played; the rest of the soundboard isn't.
fn on_channel_effect(app: &mut App, data: &str) {
    #[derive(serde::Deserialize)]
    struct Effect {
        channel_id: Snowflake,
        user_id: Snowflake,
        sound_id: Option<serde_json::Value>,
        sound_volume: Option<f32>,
    }
    let Ok(e) = serde_json::from_str::<Effect>(data) else { return };
    let me = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|m| m.id);
    let Some(call) = &app.voice else { return };
    // Its ID can come as a string or (in the default set) a number.
    let id = match e.sound_id {
        Some(serde_json::Value::String(s)) => s,
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return,
    };
    if call.channel != e.channel_id || Some(e.user_id) == me || !matches!(call.status, Status::Connected { .. }) {
        return;
    }
    if !is_joining(call.others.as_ref(), &call.joined_at, e.user_id) {
        tracing::debug!("ignoring soundboard sound {id}: not a join sound");
        return;
    }
    if !app.settings.join_sounds || app.ui.self_deaf || app.settings.muted_users.contains(&e.user_id.0) {
        return;
    }
    crate::join_sounds::play(app, id, e.sound_volume.unwrap_or(1.0).clamp(0.0, 1.0));
}

fn connect(app: &mut App) {
    let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|u| u.id) else { return };
    let settings = swiftcord_voice::Settings {
        devices: swiftcord_voice::Devices { input: app.settings.input_device.clone(), output: app.settings.output_device.clone() },
        vad_threshold_db: app.settings.vad_threshold_db,
        muted: app.ui.self_mute,
        deafened: app.ui.self_deaf,
        noise_suppression: app.settings.noise_suppression,
    };
    let sink = app.event_sink();
    let Some(call) = &mut app.voice else { return };
    let (Some(session_id), Some(server)) = (&call.session_id, &call.server) else { return };
    if call.voice.is_some() {
        return;
    }
    let info = ConnectInfo {
        user_id: me.0,
        server_id: server.guild_id.unwrap_or(call.channel).0,
        channel_id: call.channel.0,
        session_id: session_id.clone(),
        token: server.token.clone(),
        endpoint: server.endpoint.clone().unwrap_or_default(),
        stream_owner: None,
        share: None,
    };
    call.status = Status::Connecting;
    call.generation += 1;
    let generation = call.generation;
    let voice = Voice::connect(&app.rt, info, settings, move |e| sink(AppEvent::Voice { generation, event: e }));
    // People's volumes and mutes carry over between calls.
    for &user in app.settings.user_volumes.keys().chain(&app.settings.muted_users) {
        voice.set_user_volume(user, app.settings.user_gain(user));
    }
    call.voice = Some(voice);
}

pub fn on_event(app: &mut App, generation: u64, event: Event) {
    let Some(call) = &mut app.voice else { return };
    if call.generation != generation {
        return;
    }
    match event {
        Event::Connecting => call.status = Status::Connecting,
        Event::Connected { e2ee } => {
            let first = !matches!(call.status, Status::Connected { .. });
            call.status = Status::Connected { e2ee };
            if first {
                call_sound(app, crate::sounds::Sound::Join);
            }
            crate::stream_glue::request_if_ready(app);
        }
        Event::Error(e) => app.banner(e),
        Event::VideoFrame | Event::Speaking => {}
        Event::CameraStopped => {
            app.ui.self_video = false;
            update_self_state(app);
        }
        Event::Disconnected(reason) => {
            tracing::info!("voice disconnected: {reason}");
            app.banner(reason);
            leave(app);
        }
    }
}

/// Plays a sound when someone joins or leaves the channel we're connected to.
fn join_leave_sounds(app: &mut App) {
    let Some(s) = &app.session else { return };
    let me = s.state.me.as_ref().map(|m| m.id);
    let Some(call) = &mut app.voice else { return };
    if !matches!(call.status, Status::Connected { .. }) {
        return;
    }
    let Some(channel) = s.state.channel(call.channel) else { return };
    let now: HashSet<Snowflake> = s.state.voice_members(channel).iter().map(|v| v.user_id).filter(|u| Some(*u) != me).collect();
    if let Some(before) = &call.others {
        for u in now.difference(before) {
            call.joined_at.insert(*u, Instant::now());
        }
    }
    call.joined_at.retain(|_, t| t.elapsed() < JOIN_SOUND_WINDOW);
    let sound = match call.others.replace(now.clone()) {
        Some(before) if now.difference(&before).next().is_some() => Some(crate::sounds::Sound::Join),
        Some(before) if before.difference(&now).next().is_some() => Some(crate::sounds::Sound::Leave),
        _ => None,
    };
    if let Some(sound) = sound {
        call_sound(app, sound);
    }
}

pub fn tick(app: &mut App) {
    join_leave_sounds(app);
    update_cameras(app);
    let Some(call) = &app.voice else { return };
    let mut speaking: HashSet<Snowflake> = HashSet::new();
    if let Some(v) = &call.voice {
        speaking.extend(v.speaking_users().into_iter().map(Snowflake));
        if v.self_speaking() {
            if let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()) {
                speaking.insert(me.id);
            }
        }
    }
    app.ui.speaking = speaking;
    app.ui.voice = Some(VoiceView {
        guild: call.guild,
        channel: call.channel,
        status: call.status,
        ping_ms: call.voice.as_ref().map_or(0, |v| v.ping_ms()),
        privacy_code: call.voice.as_ref().and_then(|v| v.privacy_code()),
    });
    // The voice engine says when someone starts speaking; while anyone is, look again a few times a second
    // to notice them stop. (The ping is shown too, and is fine updated with whatever redraws next.)
    if !app.ui.speaking.is_empty() {
        app.ctx().request_repaint_after(Duration::from_millis(150));
    }
}

pub fn status_panel(ui: &mut Ui, s: &Session, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(call) = &uis.voice else { return };
    let (label, color) = match call.status {
        Status::Connected { .. } => ("Voice Connected", theme::GREEN()),
        Status::Connecting => ("Connecting…", theme::YELLOW()),
        Status::Joining => ("Joining…", theme::YELLOW()),
    };
    let where_ = {
        let ch = s.state.channel(call.channel).map(|c| c.label()).unwrap_or_default();
        match call.guild.and_then(|g| s.state.guild(g)) {
            Some(g) => format!("{ch} / {}", g.name),
            None => ch,
        }
    };
    let connected = matches!(call.status, Status::Connected { .. });
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        // The status and where, each on a line of its own, then the controls below them.
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::SIGNAL).font(theme::icon_font(13.0)).color(color));
            ui.label(RichText::new(label).font(theme::font_semibold(13.5)).color(color));
            if let Status::Connected { e2ee: true } = call.status {
                let code = call.privacy_code.clone().unwrap_or_default();
                ui.label(RichText::new("🔒").size(12.0)).on_hover_text(format!("End-to-end encrypted\nPrivacy code: {code}"));
            }
            if call.ping_ms > 0 {
                ui.label(RichText::new(format!("{} ms", call.ping_ms)).size(11.0).color(theme::FAINT()));
            }
        });
        let r = ui.add(egui::Label::new(RichText::new(where_).size(12.5).color(theme::MUTED())).truncate().sense(egui::Sense::click()));
        if r.clicked() && call.guild.is_some() {
            actions.push(Action::SelectChannel(call.channel));
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if connected {
                let tip = if uis.self_video { "Turn Off Camera" } else { "Turn On Camera" };
                if crate::ui::widgets::toggle_icon_colored(ui, icon::CAMERA, uis.self_video, theme::GREEN(), tip).clicked() {
                    actions.push(Action::ToggleCamera);
                }
                if uis.share.is_none() && crate::ui::widgets::icon_button(ui, icon::SCREEN, "Share Your Screen").clicked() {
                    actions.push(Action::OpenSharePicker);
                }
                if crate::ui::widgets::toggle_icon_colored(ui, icon::SIGNAL, uis.stats_open, theme::GREEN(), "Connection Stats").clicked() {
                    actions.push(Action::ToggleStats);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let hang = ui.add(egui::Button::new(RichText::new(icon::HANG_UP).font(theme::icon_font(16.0)).color(Color32::WHITE)).fill(theme::RED()).min_size(egui::vec2(32.0, 32.0)));
                if hang.on_hover_text("Disconnect").clicked() {
                    actions.push(Action::LeaveVoice);
                }
            });
        });
    });
    crate::ui::share::live_panel(ui, uis, actions);
    ui.separator();
}

pub fn is_speaking(uis: &UiState, user: Snowflake) -> bool {
    uis.speaking.contains(&user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_someone_joining_plays_a_join_sound() {
        let (old, new, arriving) = (Snowflake(1), Snowflake(2), Snowflake(3));
        let others: HashSet<Snowflake> = [old, new].into();
        let joined_at: HashMap<Snowflake, Instant> = [(new, Instant::now())].into();
        assert!(!is_joining(Some(&others), &joined_at, old), "already here: their soundboard");
        assert!(is_joining(Some(&others), &joined_at, new), "just joined");
        assert!(is_joining(Some(&others), &joined_at, arriving), "the event beat their voice state");
        assert!(!is_joining(None, &joined_at, new), "we haven't seen who's here yet");
    }
}
