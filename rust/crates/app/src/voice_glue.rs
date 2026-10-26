//! Connects the voice engine to the session.
//!
//! Joining asks the main gateway for a voice state; Discord answers with our VOICE_STATE_UPDATE (the
//! session ID) and a VOICE_SERVER_UPDATE (endpoint and token), in either order. Once both are in, the voice
//! connection starts. A later VOICE_SERVER_UPDATE means the call moved servers, so we reconnect.

use std::collections::HashSet;
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};
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
}

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
    let Some(s) = &app.session else { return };
    if s.state.permissions(channel) & swiftcord_core::permissions::perm::CONNECT == 0 {
        app.banner("You don't have permission to join that voice channel.");
        return;
    }
    // Switching channels: the old connection goes, the gateway moves us.
    if let Some(old) = app.voice.take() {
        drop(old.voice);
    }
    s.gateway.update_voice_state(guild, Some(channel), app.ui.self_mute, app.ui.self_deaf);
    if guild.is_none() && s.state.call(channel).is_none() {
        // Starting a DM call (not answering one): ring the other people, as the official client does.
        let rest = s.rest.clone();
        app.rt.spawn(async move {
            let _ = rest.ring(channel).await;
        });
    }
    app.voice = Some(Call { guild, channel, status: Status::Joining, session_id: None, server: None, voice: None, generation: 0 });
}

pub fn leave(app: &mut App) {
    crate::stream_glue::stop(app);
    let Some(call) = app.voice.take() else { return };
    drop(call.voice);
    if let Some(s) = &app.session {
        s.gateway.update_voice_state(call.guild, None, false, false);
    }
    app.ui.speaking.clear();
    app.ui.voice = None;
}

pub fn update_self_state(app: &mut App) {
    let (mute, deaf) = (app.ui.self_mute, app.ui.self_deaf);
    let Some(call) = &app.voice else { return };
    if let Some(v) = &call.voice {
        v.set_muted(mute);
        v.set_deafened(deaf);
    }
    if let Some(s) = &app.session {
        s.gateway.update_voice_state(call.guild, Some(call.channel), mute, deaf);
    }
    crate::stream_glue::update_self_state(app);
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
                    if let Some(c) = vs.channel_id {
                        call.channel = c;
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
        Notice::Other(name, data) => crate::stream_glue::on_gateway(app, &name, &data),
        Notice::MessageCreated { .. } | Notice::Ring(_) | Notice::RingStopped(_) | Notice::FrecencyChanged => {}
    }
}

fn connect(app: &mut App) {
    let Some(me) = app.session.as_ref().and_then(|s| s.state.me.as_ref()).map(|u| u.id) else { return };
    let settings = swiftcord_voice::Settings {
        devices: swiftcord_voice::Devices { input: app.settings.input_device.clone(), output: app.settings.output_device.clone() },
        vad_threshold_db: app.settings.vad_threshold_db,
        muted: app.ui.self_mute,
        deafened: app.ui.self_deaf,
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
    };
    call.status = Status::Connecting;
    call.generation += 1;
    let generation = call.generation;
    call.voice = Some(Voice::connect(&app.rt, info, settings, move |e| sink(AppEvent::Voice { generation, event: e })));
}

pub fn on_event(app: &mut App, generation: u64, event: Event) {
    let Some(call) = &mut app.voice else { return };
    if call.generation != generation {
        return;
    }
    match event {
        Event::Connecting => call.status = Status::Connecting,
        Event::Connected { e2ee } => {
            call.status = Status::Connected { e2ee };
            crate::stream_glue::request_if_ready(app);
        }
        Event::Error(e) => app.banner(e),
        Event::VideoFrame => {}
        Event::Disconnected(reason) => {
            tracing::info!("voice disconnected: {reason}");
            app.banner(reason);
            leave(app);
        }
    }
}

pub fn tick(app: &mut App) {
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
    // Poll speaking state a few times a second while in a call.
    app.ctx().request_repaint_after(Duration::from_millis(150));
}

pub fn status_panel(ui: &mut Ui, s: &Session, uis: &UiState, actions: &mut Vec<Action>) {
    let Some(call) = &uis.voice else { return };
    let (label, color) = match call.status {
        Status::Connected { .. } => ("Voice Connected", theme::GREEN),
        Status::Connecting => ("Connecting…", theme::YELLOW),
        Status::Joining => ("Joining…", theme::YELLOW),
    };
    let where_ = {
        let ch = s.state.channel(call.channel).map(|c| c.label()).unwrap_or_default();
        match call.guild.and_then(|g| s.state.guild(g)) {
            Some(g) => format!("{ch} / {}", g.name),
            None => ch,
        }
    };
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width() - 40.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SIGNAL).font(theme::icon_font(13.0)).color(color));
                    ui.label(RichText::new(label).font(theme::font_semibold(13.5)).color(color));
                    if let Status::Connected { e2ee: true } = call.status {
                        let code = call.privacy_code.clone().unwrap_or_default();
                        ui.label(RichText::new("🔒").size(12.0)).on_hover_text(format!("End-to-end encrypted\nPrivacy code: {code}"));
                    }
                    if call.ping_ms > 0 {
                        ui.label(RichText::new(format!("{} ms", call.ping_ms)).size(11.0).color(theme::FAINT));
                    }
                });
                let r = ui.add(egui::Label::new(RichText::new(where_).size(12.5).color(theme::MUTED)).truncate().sense(egui::Sense::click()));
                if r.clicked() && call.guild.is_some() {
                    actions.push(Action::SelectChannel(call.channel));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let hang = ui.add(egui::Button::new(RichText::new(icon::HANG_UP).font(theme::icon_font(16.0)).color(Color32::WHITE)).fill(theme::RED).min_size(egui::vec2(32.0, 32.0)));
                if hang.on_hover_text("Disconnect").clicked() {
                    actions.push(Action::LeaveVoice);
                }
            });
        });
    });
    ui.separator();
}

pub fn is_speaking(uis: &UiState, user: Snowflake) -> bool {
    uis.speaking.contains(&user)
}
