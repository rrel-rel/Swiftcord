//! One voice connection: the voice gateway (v8, with DAVE's MLS opcodes), UDP setup, and the media threads.

use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use davey::{DaveSession, MediaType, ProposalsOperationType};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;

use crate::audio::{self, Mixer};
use crate::codec::{self, FRAME};
use crate::transmit::{Output, Transmitter};
use crate::rtp::{self, Cipher, Mode};
use crate::video;
use crate::{ConnectInfo, Event, Settings};

mod op {
    pub const IDENTIFY: u8 = 0;
    pub const SELECT_PROTOCOL: u8 = 1;
    pub const READY: u8 = 2;
    pub const HEARTBEAT: u8 = 3;
    pub const SESSION_DESCRIPTION: u8 = 4;
    pub const SPEAKING: u8 = 5;
    pub const HEARTBEAT_ACK: u8 = 6;
    pub const RESUME: u8 = 7;
    pub const HELLO: u8 = 8;
    pub const RESUMED: u8 = 9;
    pub const CLIENTS_CONNECT: u8 = 11;
    pub const VIDEO: u8 = 12;
    pub const CLIENT_DISCONNECT: u8 = 13;
    pub const SESSION_UPDATE: u8 = 14;
    pub const MEDIA_SINK_WANTS: u8 = 15;
    pub const DAVE_PREPARE_TRANSITION: u8 = 21;
    pub const DAVE_EXECUTE_TRANSITION: u8 = 22;
    pub const DAVE_TRANSITION_READY: u8 = 23;
    pub const DAVE_PREPARE_EPOCH: u8 = 24;
    pub const DAVE_EXTERNAL_SENDER: u8 = 25;
    pub const DAVE_KEY_PACKAGE: u8 = 26;
    pub const DAVE_PROPOSALS: u8 = 27;
    pub const DAVE_COMMIT_WELCOME: u8 = 28;
    pub const DAVE_ANNOUNCE_COMMIT: u8 = 29;
    pub const DAVE_WELCOME: u8 = 30;
    pub const DAVE_INVALID_COMMIT_WELCOME: u8 = 31;
}

/// State shared between the gateway task, the media threads and the handle.
pub(crate) struct Shared {
    pub muted: AtomicBool,
    pub deafened: Arc<AtomicBool>,
    pub stop: AtomicBool,
    pub mixer: Arc<Mixer>,
    pub ping_ms: AtomicU32,
    pub self_speaking: AtomicBool,
    /// The microphone level in dBFS (f32 bits), for meters.
    pub input_db: AtomicU32,
    /// Voice-activity threshold in dBFS × -1 (so it fits an atomic), e.g. 50 for -50 dBFS.
    pub threshold: AtomicU16,
    pub noise_suppression: AtomicBool,
    pub dave: Mutex<Dave>,
    pub ssrc_users: Mutex<HashMap<u32, u64>>,
    pub last_heard: Mutex<HashMap<u64, Instant>>,
    pub user_volume: Mutex<HashMap<u64, f32>>,
    /// The latest decoded video picture (stream connections).
    pub video: Mutex<Option<crate::video::Picture>>,
    /// Video SSRCs announced by the server, and their retransmission (RTX) SSRCs.
    pub video_ssrcs: Mutex<VideoSsrcs>,
    /// Set when the decoder dropped or failed a frame and needs a keyframe.
    pub video_resync: AtomicBool,
    /// While sharing: a viewer asked for a keyframe.
    pub keyframe: AtomicBool,
    pub share_stats: Mutex<Option<crate::ShareStats>>,
}

#[derive(Default)]
pub(crate) struct VideoSsrcs {
    pub media: HashSet<u32>,
    pub rtx: HashMap<u32, u32>,
}

impl Shared {
    pub fn new(settings: &Settings) -> Self {
        Self {
            muted: AtomicBool::new(settings.muted),
            deafened: Arc::new(AtomicBool::new(settings.deafened)),
            stop: AtomicBool::new(false),
            mixer: Arc::new(Mixer::default()),
            ping_ms: AtomicU32::new(0),
            self_speaking: AtomicBool::new(false),
            input_db: AtomicU32::new((-100f32).to_bits()),
            threshold: AtomicU16::new((-settings.vad_threshold_db).clamp(0.0, 100.0) as u16),
            noise_suppression: AtomicBool::new(settings.noise_suppression),
            dave: Mutex::new(Dave::default()),
            ssrc_users: Mutex::new(HashMap::new()),
            last_heard: Mutex::new(HashMap::new()),
            user_volume: Mutex::new(HashMap::new()),
            video: Mutex::new(None),
            video_ssrcs: Mutex::new(VideoSsrcs::default()),
            video_resync: AtomicBool::new(false),
            keyframe: AtomicBool::new(false),
            share_stats: Mutex::new(None),
        }
    }
}

/// End-to-end encryption state. `protocol` 0 means the call isn't end-to-end encrypted.
#[derive(Default)]
pub(crate) struct Dave {
    pub protocol: u16,
    pub session: Option<DaveSession>,
    pending: HashMap<u16, u16>,
}

impl Dave {
    fn active(&self) -> bool {
        self.protocol > 0
    }
}

/// The UDP side once IP discovery is done; the cipher arrives later with the session description.
struct Media {
    socket: Arc<UdpSocket>,
    ssrc: u32,
    mode: Mode,
    /// Where our own video goes (stream connections we share on).
    video_ssrc: u32,
    rtx_ssrc: u32,
}

/// Audio devices and the media threads, restarted whenever a new key arrives.
#[derive(Default)]
struct Running {
    audio: Option<audio::AudioIo>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Running {
    fn stop(&mut self, shared: &Shared) {
        shared.stop.store(true, Ordering::Relaxed);
        self.audio = None;
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        shared.stop.store(false, Ordering::Relaxed);
    }

    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, media: &Media, cipher: Cipher, info: &ConnectInfo, settings: &Settings, shared: &Arc<Shared>, cmd: &mpsc::UnboundedSender<Cmd>, events: &Arc<dyn Fn(Event) + Send + Sync>) {
        self.stop(shared);
        let cipher = Arc::new(cipher);
        if let Some(share) = &info.share {
            // Sharing sends video and nothing else: no microphone, no playback.
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            self.threads.push(spawn_capture(share.clone(), tx, shared.clone(), cmd.clone(), events.clone()));
            self.threads.push(spawn_video_sender(media, cipher, rx, shared.clone()));
            return;
        }
        let stream = info.stream_owner.is_some();
        let (mic_tx, mic_rx) = std::sync::mpsc::sync_channel(256);
        // Watching a stream plays its audio but never opens the microphone.
        let (io, errors) = audio::start(settings.devices.clone(), (!stream).then_some(mic_tx), shared.mixer.clone(), shared.deafened.clone());
        self.audio = Some(io);
        if !stream {
            self.threads.push(spawn_sender(&media.socket, cipher.clone(), media.ssrc, mic_rx, shared.clone(), cmd.clone()));
        }
        let video = stream.then(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel(64);
            self.threads.push(spawn_video_decoder(rx, shared.clone(), events.clone()));
            tx
        });
        self.threads.push(spawn_receiver(&media.socket, cipher, media.ssrc, info.stream_owner, video, shared.clone()));
        // Device errors are reported from their own thread; it ends when the audio stops.
        let ev = events.clone();
        std::thread::spawn(move || {
            for e in errors.iter() {
                ev(Event::Error(e));
            }
        });
    }
}

enum Outcome {
    /// Ended for good (left, kicked, or the call ended).
    Closed(String),
    /// Lost; try resuming.
    Lost(String),
}

#[derive(Deserialize)]
struct Frame {
    op: u8,
    #[serde(default)]
    d: Value,
    #[serde(default)]
    seq: Option<i64>,
}

#[derive(Deserialize)]
struct ReadyData {
    ssrc: u32,
    ip: String,
    port: u16,
    modes: Vec<String>,
    #[serde(default)]
    streams: Vec<Value>,
}

pub(crate) enum Cmd {
    Speaking(bool),
    Disconnect,
    /// Ended from our side for a reason (a shared window closed).
    End(String),
}

pub(crate) async fn run(
    info: ConnectInfo,
    settings: Settings,
    shared: Arc<Shared>,
    mut cmd: mpsc::UnboundedReceiver<Cmd>,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    events: Arc<dyn Fn(Event) + Send + Sync>,
) {
    events(Event::Connecting);
    let mut media: Option<Media> = None;
    let mut running = Running::default();
    let mut seq_ack: i64 = -1;
    let mut resume = false;
    let mut attempts = 0;

    let reason = loop {
        let result = session(&info, &settings, &shared, &mut cmd, &cmd_tx, &events, &mut media, &mut running, &mut seq_ack, resume).await;
        match result {
            Outcome::Closed(r) => break r,
            Outcome::Lost(r) if attempts < 5 && !shared.stop.load(Ordering::Relaxed) => {
                attempts += 1;
                tracing::warn!("voice connection lost ({r}); resuming (attempt {attempts})");
                events(Event::Connecting);
                tokio::time::sleep(Duration::from_millis(500 * attempts)).await;
                resume = media.is_some();
            }
            Outcome::Lost(r) => break r,
        }
    };
    running.stop(&shared);
    shared.stop.store(true, Ordering::Relaxed);
    events(Event::Disconnected(reason));
}

fn json_msg(op: u8, d: Value) -> Ws {
    Ws::text(json!({ "op": op, "d": d }).to_string())
}

fn binary_msg(op: u8, payload: &[u8]) -> Ws {
    let mut v = Vec::with_capacity(payload.len() + 1);
    v.push(op);
    v.extend_from_slice(payload);
    Ws::binary(v)
}

#[allow(clippy::too_many_arguments)]
async fn session(
    info: &ConnectInfo,
    settings: &Settings,
    shared: &Arc<Shared>,
    cmd: &mut mpsc::UnboundedReceiver<Cmd>,
    cmd_tx: &mpsc::UnboundedSender<Cmd>,
    events: &Arc<dyn Fn(Event) + Send + Sync>,
    media: &mut Option<Media>,
    running: &mut Running,
    seq_ack: &mut i64,
    resume: bool,
) -> Outcome {
    let host = info.endpoint.trim_start_matches("wss://").trim_end_matches('/');
    let url = format!("wss://{host}/?v=8");
    let mut ws = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((ws, _)) => ws,
        Err(e) => return Outcome::Lost(format!("couldn't reach the voice server: {e}")),
    };

    let hello = if resume {
        json_msg(op::RESUME, json!({ "server_id": info.server_id.to_string(), "session_id": info.session_id, "token": info.token, "seq_ack": seq_ack }))
    } else {
        json_msg(
            op::IDENTIFY,
            json!({
                "server_id": info.server_id.to_string(),
                "user_id": info.user_id.to_string(),
                "session_id": info.session_id,
                "token": info.token,
                "video": is_stream(info),
                "streams": if is_stream(info) { json!([{ "type": "screen", "rid": "100", "quality": 100 }]) } else { json!([]) },
                "max_dave_protocol_version": davey::DAVE_PROTOCOL_VERSION,
            }),
        )
    };
    if ws.send(hello).await.is_err() {
        return Outcome::Lost("voice server closed the connection".into());
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(3600));
    heartbeat.reset();
    let mut ssrc = media.as_ref().map_or(0, |m| m.ssrc);
    let mut connected: HashSet<u64> = HashSet::from([info.user_id]);

    macro_rules! send {
        ($m:expr) => {
            if ws.send($m).await.is_err() {
                return Outcome::Lost("voice server connection dropped".into());
            }
        };
    }

    loop {
        tokio::select! {
            frame = ws.next() => {
                let frame = match frame {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => return Outcome::Lost(e.to_string()),
                    None => return Outcome::Lost("voice server closed the connection".into()),
                };
                match frame {
                    Ws::Text(text) => {
                        let Ok(f) = serde_json::from_str::<Frame>(&text) else { continue };
                        if let Some(s) = f.seq {
                            *seq_ack = s;
                        }
                        match f.op {
                            op::HELLO => {
                                let iv = f.d["heartbeat_interval"].as_f64().unwrap_or(13_750.0);
                                heartbeat = tokio::time::interval(Duration::from_millis(iv as u64));
                            }
                            op::READY => {
                                let Ok(ready) = serde_json::from_value::<ReadyData>(f.d) else {
                                    return Outcome::Closed("the voice server sent an unreadable response".into());
                                };
                                ssrc = ready.ssrc;
                                let Some(mode) = Mode::choose(&ready.modes) else {
                                    return Outcome::Closed("no supported encryption mode".into());
                                };
                                let (ip, port) = (ready.ip.clone(), ready.port);
                                let found = tokio::task::spawn_blocking(move || discover(&ip, port, ready.ssrc)).await.ok().flatten();
                                let Some((socket, my_ip, my_port)) = found else {
                                    return Outcome::Lost("UDP connection to the voice server failed".into());
                                };
                                // The server assigns our video SSRCs; older servers leave them to follow the audio one.
                                let first = ready.streams.first();
                                let ssrc_of = |k: &str, fallback: u32| first.and_then(|s| s[k].as_u64()).filter(|&v| v != 0).map_or(fallback, |v| v as u32);
                                let (video_ssrc, rtx_ssrc) = (ssrc_of("ssrc", ssrc.wrapping_add(1)), ssrc_of("rtx_ssrc", ssrc.wrapping_add(2)));
                                *media = Some(Media { socket: Arc::new(socket), ssrc, mode, video_ssrc, rtx_ssrc });
                                send!(json_msg(op::SELECT_PROTOCOL, json!({
                                    "protocol": "udp",
                                    "data": { "address": my_ip, "port": my_port, "mode": mode.name() },
                                    "codecs": codecs(is_stream(info), info.share.is_some()),
                                })));
                            }
                            op::SESSION_DESCRIPTION => {
                                let key: Vec<u8> = f.d["secret_key"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u8).collect()).unwrap_or_default();
                                let Some(m) = media.as_ref() else { continue };
                                let mode = f.d["mode"].as_str().and_then(|n| Mode::choose(&[n.to_owned()])).unwrap_or(m.mode);
                                let Some(cipher) = Cipher::new(mode, &key) else {
                                    return Outcome::Closed("the voice server sent an invalid key".into());
                                };
                                let dave_version = f.d["dave_protocol_version"].as_u64().unwrap_or(0) as u16;
                                {
                                    let mut dave = shared.dave.lock();
                                    dave.protocol = dave_version;
                                    if let Some(v) = NonZeroU16::new(dave_version) {
                                        if dave.session.is_none() {
                                            dave.session = DaveSession::new(v, info.user_id, info.channel_id, None).ok();
                                        }
                                    }
                                }
                                running.start(m, cipher, info, settings, shared, cmd_tx, events);
                                if let Some(share) = &info.share {
                                    // Announce our video so the server forwards it to viewers.
                                    let (w, h) = crate::capture::output_size(&share.source, share.max_width, share.max_height).unwrap_or((share.max_width, share.max_height));
                                    send!(json_msg(op::VIDEO, json!({
                                        "audio_ssrc": m.ssrc,
                                        "video_ssrc": m.video_ssrc,
                                        "rtx_ssrc": m.rtx_ssrc,
                                        "streams": [{
                                            "type": "video", "rid": "100", "ssrc": m.video_ssrc, "rtx_ssrc": m.rtx_ssrc,
                                            "active": true, "quality": 100,
                                            "max_bitrate": share.bitrate, "max_framerate": share.fps,
                                            "max_resolution": { "type": "fixed", "width": w, "height": h },
                                        }],
                                    })));
                                    send!(json_msg(op::SPEAKING, json!({ "speaking": 1, "delay": 0, "ssrc": m.ssrc })));
                                }
                                if info.stream_owner.is_some() {
                                    check_video_codec(&f.d, events);
                                    // Ask for the stream's video at full quality.
                                    send!(json_msg(op::MEDIA_SINK_WANTS, json!({ "any": 100 })));
                                }
                                events(Event::Connected { e2ee: dave_version > 0 });
                            }
                            op::SESSION_UPDATE => {
                                if info.stream_owner.is_some() {
                                    check_video_codec(&f.d, events);
                                }
                            }
                            op::RESUMED => events(Event::Connected { e2ee: shared.dave.lock().active() }),
                            op::HEARTBEAT_ACK => {
                                if let Some(t) = f.d["t"].as_u64() {
                                    let now = now_ms();
                                    shared.ping_ms.store(now.saturating_sub(t) as u32, Ordering::Relaxed);
                                }
                            }
                            op::SPEAKING => {
                                let (Some(user), Some(s)) = (f.d["user_id"].as_str().and_then(|u| u.parse().ok()), f.d["ssrc"].as_u64()) else { continue };
                                shared.ssrc_users.lock().insert(s as u32, user);
                                let vol = shared.user_volume.lock().get(&user).copied().unwrap_or(1.0);
                                shared.mixer.set_gain(s as u32, vol);
                            }
                            op::VIDEO => {
                                // Which SSRCs carry the video and its retransmissions.
                                tracing::info!("video sources: {}", f.d);
                                let mut map = shared.video_ssrcs.lock();
                                let streams = f.d["streams"].as_array().cloned().unwrap_or_default();
                                let top = [json!({ "ssrc": f.d["video_ssrc"], "rtx_ssrc": f.d["rtx_ssrc"] })];
                                for s in streams.iter().chain(top.iter()) {
                                    let media = s["ssrc"].as_u64().filter(|&v| v != 0);
                                    if let Some(m) = media {
                                        map.media.insert(m as u32);
                                        if let Some(r) = s["rtx_ssrc"].as_u64().filter(|&v| v != 0) {
                                            map.rtx.insert(r as u32, m as u32);
                                        }
                                    }
                                }
                            }
                            op::CLIENTS_CONNECT => {
                                for id in f.d["user_ids"].as_array().into_iter().flatten().filter_map(|v| v.as_str()?.parse().ok()) {
                                    connected.insert(id);
                                }
                            }
                            op::CLIENT_DISCONNECT => {
                                if let Some(id) = f.d["user_id"].as_str().and_then(|u| u.parse::<u64>().ok()) {
                                    connected.remove(&id);
                                    let mut map = shared.ssrc_users.lock();
                                    let gone: Vec<u32> = map.iter().filter(|(_, u)| **u == id).map(|(s, _)| *s).collect();
                                    for s in gone {
                                        map.remove(&s);
                                        shared.mixer.remove(s);
                                    }
                                }
                            }
                            op::DAVE_PREPARE_TRANSITION => {
                                let tid = f.d["transition_id"].as_u64().unwrap_or(0) as u16;
                                let version = f.d["protocol_version"].as_u64().unwrap_or(0) as u16;
                                {
                                    let mut dave = shared.dave.lock();
                                    dave.pending.insert(tid, version);
                                    if version == 0 {
                                        // Downgrading: accept unencrypted media from others during the transition.
                                        if let Some(s) = &mut dave.session {
                                            s.set_passthrough_mode(true, Some(10));
                                        }
                                    }
                                    if tid == 0 {
                                        execute_transition(&mut dave, 0);
                                    }
                                }
                                if tid != 0 {
                                    send!(json_msg(op::DAVE_TRANSITION_READY, json!({ "transition_id": tid })));
                                }
                            }
                            op::DAVE_EXECUTE_TRANSITION => {
                                let tid = f.d["transition_id"].as_u64().unwrap_or(0) as u16;
                                execute_transition(&mut shared.dave.lock(), tid);
                                events(Event::Connected { e2ee: shared.dave.lock().active() });
                            }
                            op::DAVE_PREPARE_EPOCH => {
                                let epoch = f.d["epoch"].as_u64().unwrap_or(0);
                                let version = f.d["protocol_version"].as_u64().unwrap_or(0) as u16;
                                if epoch == 1 {
                                    // A new MLS group: start over with a fresh key package.
                                    let kp = {
                                        let mut dave = shared.dave.lock();
                                        dave.protocol = version;
                                        match NonZeroU16::new(version) {
                                            Some(v) => {
                                                match &mut dave.session {
                                                    Some(s) => { let _ = s.reinit(v, info.user_id, info.channel_id, None); }
                                                    None => dave.session = DaveSession::new(v, info.user_id, info.channel_id, None).ok(),
                                                }
                                                dave.session.as_mut().and_then(|s| s.create_key_package().ok())
                                            }
                                            None => None,
                                        }
                                    };
                                    if let Some(kp) = kp {
                                        send!(binary_msg(op::DAVE_KEY_PACKAGE, &kp));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Ws::Binary(data) => {
                        // Server binary frames: sequence (u16) ‖ opcode (u8) ‖ payload.
                        if data.len() < 3 {
                            continue;
                        }
                        *seq_ack = u16::from_be_bytes([data[0], data[1]]) as i64;
                        let (opcode, payload) = (data[2], &data[3..]);
                        let reply = handle_dave_binary(shared, info, opcode, payload, &connected);
                        for m in reply {
                            send!(m);
                        }
                    }
                    Ws::Close(frame) => {
                        let code = frame.as_ref().map_or(1006, |f| u16::from(f.code));
                        let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                        return match code {
                            4014 => Outcome::Closed("Disconnected from voice.".into()),
                            4022 => Outcome::Closed("The call ended.".into()),
                            4004 | 4006 | 4011 | 4016 | 4021 => Outcome::Closed(format!("The voice server ended the session ({code} {reason}).")),
                            _ => Outcome::Lost(format!("{code} {reason}")),
                        };
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                send!(json_msg(op::HEARTBEAT, json!({ "t": now_ms(), "seq_ack": seq_ack })));
            }
            c = cmd.recv() => match c {
                Some(Cmd::Speaking(on)) => {
                    send!(json_msg(op::SPEAKING, json!({ "speaking": if on { 1 } else { 0 }, "delay": 0, "ssrc": ssrc })));
                }
                Some(Cmd::Disconnect) | None => {
                    let _ = ws.close(None).await;
                    return Outcome::Closed("Left the call.".into());
                }
                Some(Cmd::End(reason)) => {
                    let _ = ws.close(None).await;
                    return Outcome::Closed(reason);
                }
            }
        }
    }
}

fn execute_transition(dave: &mut Dave, tid: u16) {
    let Some(version) = dave.pending.remove(&tid) else { return };
    dave.protocol = version;
    if version > 0 {
        if let Some(s) = &mut dave.session {
            s.set_passthrough_mode(false, Some(10));
        }
    }
    tracing::info!("DAVE transition {tid} executed; protocol {version}");
}

/// Handles DAVE's binary MLS opcodes, returning messages to send back.
fn handle_dave_binary(shared: &Shared, info: &ConnectInfo, opcode: u8, payload: &[u8], connected: &HashSet<u64>) -> Vec<Ws> {
    let mut dave = shared.dave.lock();
    let protocol = dave.protocol;
    let Some(session) = dave.session.as_mut() else { return Vec::new() };
    let mut out = Vec::new();
    match opcode {
        op::DAVE_EXTERNAL_SENDER => {
            if let Err(e) = session.set_external_sender(payload) {
                tracing::warn!("DAVE external sender: {e:?}");
            }
            match session.create_key_package() {
                Ok(kp) => out.push(binary_msg(op::DAVE_KEY_PACKAGE, &kp)),
                Err(e) => tracing::warn!("DAVE key package: {e:?}"),
            }
        }
        op::DAVE_PROPOSALS => {
            let Some((&kind, proposals)) = payload.split_first() else { return out };
            let kind = if kind == 0 { ProposalsOperationType::APPEND } else { ProposalsOperationType::REVOKE };
            let expected: Vec<u64> = connected.iter().copied().collect();
            match session.process_proposals(kind, proposals, Some(&expected)) {
                Ok(Some(cw)) => {
                    let mut msg = cw.commit;
                    if let Some(w) = cw.welcome {
                        msg.extend_from_slice(&w);
                    }
                    out.push(binary_msg(op::DAVE_COMMIT_WELCOME, &msg));
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("DAVE proposals: {e:?}"),
            }
        }
        op::DAVE_ANNOUNCE_COMMIT | op::DAVE_WELCOME => {
            if payload.len() < 2 {
                return out;
            }
            let tid = u16::from_be_bytes([payload[0], payload[1]]);
            let body = &payload[2..];
            let result = if opcode == op::DAVE_ANNOUNCE_COMMIT {
                session.process_commit(body).map_err(|e| format!("{e:?}"))
            } else {
                session.process_welcome(body).map_err(|e| format!("{e:?}"))
            };
            match result {
                Ok(()) => {
                    if let Some(code) = session.voice_privacy_code() {
                        tracing::info!("DAVE privacy code {code}");
                    }
                    if tid != 0 {
                        dave.pending.insert(tid, protocol);
                        out.push(json_msg(op::DAVE_TRANSITION_READY, json!({ "transition_id": tid })));
                    }
                }
                Err(e) => {
                    tracing::warn!("DAVE commit/welcome failed: {e}; requesting a fresh welcome");
                    out.push(json_msg(op::DAVE_INVALID_COMMIT_WELCOME, json!({ "transition_id": tid })));
                    if let Some(v) = NonZeroU16::new(protocol) {
                        let _ = session.reinit(v, info.user_id, info.channel_id, None);
                        if let Ok(kp) = session.create_key_package() {
                            out.push(binary_msg(op::DAVE_KEY_PACKAGE, &kp));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Video arrives in bursts (a keyframe can be hundreds of packets at once) that overflow Windows' small default
/// socket buffer, so give it room.
fn enlarge_receive_buffer(socket: &UdpSocket) {
    #[cfg(windows)]
    unsafe {
        use std::os::windows::io::AsRawSocket;
        use windows::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_RCVBUF};
        let size: i32 = 4 * 1024 * 1024;
        setsockopt(SOCKET(socket.as_raw_socket() as usize), SOL_SOCKET, SO_RCVBUF, Some(&size.to_ne_bytes()));
    }
    #[cfg(not(windows))]
    let _ = socket;
}

/// Binds a UDP socket to the voice server and asks it for our public address.
fn discover(ip: &str, port: u16, ssrc: u32) -> Option<(UdpSocket, String, u16)> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    enlarge_receive_buffer(&socket);
    socket.connect((ip, port)).ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    let mut buf = [0u8; 128];
    for _ in 0..4 {
        socket.send(&rtp::discovery_request(ssrc)).ok()?;
        if let Ok(n) = socket.recv(&mut buf) {
            if let Some((my_ip, my_port)) = rtp::parse_discovery(&buf[..n]) {
                return Some((socket, my_ip, my_port));
            }
        }
    }
    None
}

/// Encodes and sends microphone audio, gated by voice activity.
fn spawn_sender(
    socket: &Arc<UdpSocket>,
    cipher: Arc<Cipher>,
    ssrc: u32,
    mic: std::sync::mpsc::Receiver<Vec<f32>>,
    shared: Arc<Shared>,
    cmd: mpsc::UnboundedSender<Cmd>,
) -> std::thread::JoinHandle<()> {
    let socket = socket.clone();
    std::thread::Builder::new()
        .name("swiftcord-voice-send".into())
        .spawn(move || {
            let Some(mut tx) = Transmitter::new(64_000) else { return };
            let mut pending: Vec<f32> = Vec::with_capacity(FRAME * 4);
            // The RTP timestamp advances with every captured frame, sent or not, so receivers see real time.
            let (mut seq, mut ts, mut counter) = (rand::random::<u16>(), rand::random::<u32>(), 0u32);
            let mut last_keepalive = Instant::now();
            let mut send = |payload: &[u8], ts: u32| {
                if let Some(p) = rtp::seal(&cipher, rtp::OPUS_PAYLOAD_TYPE, seq, ts, ssrc, counter, payload) {
                    let _ = socket.send(&p);
                }
                seq = seq.wrapping_add(1);
                counter = counter.wrapping_add(1);
            };
            while !shared.stop.load(Ordering::Relaxed) {
                let chunk = match mic.recv_timeout(Duration::from_millis(100)) {
                    Ok(c) => c,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Keep the NAT mapping alive while nothing is being sent.
                        if last_keepalive.elapsed() > Duration::from_secs(5) {
                            let _ = socket.send(&ssrc.to_be_bytes());
                            last_keepalive = Instant::now();
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                pending.extend_from_slice(&chunk);
                while pending.len() >= FRAME {
                    let frame: Vec<f32> = pending.drain(..FRAME).collect();
                    ts = ts.wrapping_add(FRAME as u32);
                    let muted = shared.muted.load(Ordering::Relaxed) || shared.deafened.load(Ordering::Relaxed);
                    let threshold = -(shared.threshold.load(Ordering::Relaxed) as f32);
                    tx.set_noise_suppression(shared.noise_suppression.load(Ordering::Relaxed));
                    let (out, change) = tx.process(frame, muted, threshold);
                    shared.input_db.store(tx.level_db.to_bits(), Ordering::Relaxed);
                    match change {
                        Some(true) => {
                            shared.self_speaking.store(true, Ordering::Relaxed);
                            let _ = cmd.send(Cmd::Speaking(true));
                        }
                        Some(false) => {
                            shared.self_speaking.store(false, Ordering::Relaxed);
                            let _ = cmd.send(Cmd::Speaking(false));
                        }
                        None => {}
                    }
                    match out {
                        Output::Audio(packets) => {
                            let first_ts = ts.wrapping_sub(FRAME as u32 * (packets.len() as u32 - 1));
                            for (i, opus) in packets.iter().enumerate() {
                                let mut dave = shared.dave.lock();
                                let protected = if dave.active() {
                                    match dave.session.as_mut().filter(|s| s.is_ready()).map(|s| s.encrypt_opus(opus)) {
                                        Some(Ok(p)) => Some(p.into_owned()),
                                        // Never fall back to sending unencrypted audio into an E2EE call.
                                        _ => None,
                                    }
                                } else {
                                    Some(opus.clone())
                                };
                                drop(dave);
                                if let Some(p) = protected {
                                    send(&p, first_ts.wrapping_add(FRAME as u32 * i as u32));
                                    last_keepalive = Instant::now();
                                }
                            }
                        }
                        Output::Silence => send(&codec::SILENCE, ts),
                        Output::Nothing => {}
                    }
                }
            }
        })
        .expect("voice send thread")
}

fn is_stream(info: &ConnectInfo) -> bool {
    info.stream_owner.is_some() || info.share.is_some()
}

/// The codecs we offer: Opus always, and H.264 on stream connections. Watching, it makes the streamer's
/// client encode in a format Windows can decode natively; sharing, it's what we send.
fn codecs(video: bool, encode: bool) -> Value {
    let mut list = vec![json!({ "name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS_PAYLOAD_TYPE })];
    if video {
        list.push(json!({
            "name": "H264", "type": "video", "priority": 1000,
            "payload_type": video::H264_PAYLOAD_TYPE, "rtx_payload_type": video::H264_RTX_PAYLOAD_TYPE,
            "encode": encode, "decode": true,
        }));
    }
    Value::Array(list)
}

/// Decodes stream video on its own thread (Media Foundation wants a COM thread of its own) and publishes the
/// latest picture. Every frame is decoded so later frames keep their references; when frames queue up (a slow
/// machine), only the newest is converted for display.
fn spawn_video_decoder(
    frames: std::sync::mpsc::Receiver<video::EncodedFrame>,
    shared: Arc<Shared>,
    events: Arc<dyn Fn(Event) + Send + Sync>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("swiftcord-video".into())
        .spawn(move || {
            let mut decoder = match video::Decoder::new() {
                Ok(d) => d,
                Err(e) => {
                    events(Event::Error(format!(
                        "Can't decode video: {e}. Windows' H.264 decoder is missing (N editions need the Media Feature Pack)."
                    )));
                    return;
                }
            };
            let mut size = (0, 0);
            while let Ok(first) = frames.recv() {
                let mut batch = vec![first];
                batch.extend(frames.try_iter());
                let last = batch.len() - 1;
                for (i, frame) in batch.iter().enumerate() {
                    match decoder.decode(&frame.data, i == last) {
                        Ok(pictures) => {
                            if let Some(p) = pictures.into_iter().last() {
                                if (p.width, p.height) != size {
                                    size = (p.width, p.height);
                                    tracing::info!("stream video is {}x{}", p.width, p.height);
                                }
                                *shared.video.lock() = Some(p);
                                events(Event::VideoFrame);
                            }
                        }
                        Err(e) => {
                            tracing::debug!("video decode error: {e}");
                            shared.video_resync.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .expect("video thread")
}

/// Captures and encodes our screen or window at the stream's frame rate. When the machine can't keep up,
/// frames are skipped rather than queued, so the stream stays live.
fn spawn_capture(
    share: crate::Share,
    out: std::sync::mpsc::SyncSender<video::EncodedFrame>,
    shared: Arc<Shared>,
    cmd: mpsc::UnboundedSender<Cmd>,
    events: Arc<dyn Fn(Event) + Send + Sync>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("swiftcord-capture".into())
        .spawn(move || {
            let Some(mut capturer) = crate::capture::Capturer::new(share.source.clone(), share.max_width, share.max_height) else {
                let _ = cmd.send(Cmd::End("Couldn't capture that screen or window.".into()));
                return;
            };
            let (w, h) = capturer.size();
            let mut encoder = match video::Encoder::new(w as u32, h as u32, share.fps, share.bitrate) {
                Ok(e) => e,
                Err(e) => {
                    let _ = cmd.send(Cmd::End(format!("Can't encode video: {e}. Windows' H.264 encoder is missing (N editions need the Media Feature Pack).")));
                    return;
                }
            };
            tracing::info!("sharing {:?} at {w}x{h}, {} fps, {} kbps", share.source, share.fps, share.bitrate / 1000);
            let interval = Duration::from_secs_f64(1.0 / share.fps.max(1) as f64);
            let start = Instant::now();
            let ts_base = rand::random::<u32>();
            let mut next = start;
            let (mut frames, mut window_start, mut last_preview) = (0u32, Instant::now(), Instant::now() - Duration::from_secs(1));
            while !shared.stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now < next {
                    std::thread::sleep(next - now);
                }
                next += interval;
                if next < Instant::now() {
                    next = Instant::now();
                }
                let Some(frame) = capturer.grab() else {
                    let _ = cmd.send(Cmd::End("The shared window was closed.".into()));
                    return;
                };
                if last_preview.elapsed() > Duration::from_millis(500) {
                    last_preview = Instant::now();
                    *shared.video.lock() = Some(preview(&frame));
                    events(Event::VideoFrame);
                }
                let nv12 = video::bgra_to_nv12(frame.bgra, w, h);
                if shared.keyframe.swap(false, Ordering::Relaxed) {
                    encoder.force_keyframe();
                }
                // RTP video runs on a 90 kHz clock.
                let timestamp = ts_base.wrapping_add((start.elapsed().as_secs_f64() * 90_000.0) as u32);
                match encoder.encode(&nv12) {
                    Ok(units) => {
                        for data in units {
                            let keyframe = video::nal_units(&data).iter().any(|n| n[0] & 0x1f == 5);
                            if out.try_send(video::EncodedFrame { data, timestamp, keyframe }).is_err() {
                                // The sender is behind; the next frames would reference this one.
                                shared.keyframe.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => tracing::warn!("video encode failed: {e}"),
                }
                frames += 1;
                if window_start.elapsed() >= Duration::from_secs(2) {
                    let fps = frames as f32 / window_start.elapsed().as_secs_f32();
                    *shared.share_stats.lock() = Some(crate::ShareStats { width: w as u32, height: h as u32, fps });
                    (frames, window_start) = (0, Instant::now());
                }
            }
        })
        .expect("capture thread")
}

/// A small RGBA copy of a captured frame for the "you're live" preview.
fn preview(frame: &crate::capture::Frame) -> video::Picture {
    let step = (frame.width / 480).max(1);
    let (w, h) = (frame.width / step, frame.height / step);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = (y * step * frame.width + x * step) * 4;
            rgba.extend_from_slice(&[frame.bgra[i + 2], frame.bgra[i + 1], frame.bgra[i], 255]);
        }
    }
    video::Picture { width: w, height: h, rgba }
}

/// Packets kept for retransmission: a few seconds of video.
const RTX_HISTORY: usize = 4096;
/// RTP payload budget per packet, leaving room for headers, the AEAD tag and the nonce within a safe MTU.
const MAX_PAYLOAD: usize = 1100;

/// End-to-end encrypts, packetizes and sends our video, and answers viewers' feedback: keyframe requests go
/// to the encoder and NACKed packets are resent over RTX.
fn spawn_video_sender(media: &Media, cipher: Arc<Cipher>, frames: std::sync::mpsc::Receiver<video::EncodedFrame>, shared: Arc<Shared>) -> std::thread::JoinHandle<()> {
    let socket = media.socket.clone();
    let (ssrc, rtx_ssrc) = (media.video_ssrc, media.rtx_ssrc);
    std::thread::Builder::new()
        .name("swiftcord-video-send".into())
        .spawn(move || {
            let _ = socket.set_nonblocking(true);
            let (mut seq, mut rtx_seq, mut counter) = (rand::random::<u16>(), rand::random::<u16>(), 0u32);
            let mut history: std::collections::VecDeque<(u16, u32, bool, Vec<u8>)> = std::collections::VecDeque::with_capacity(RTX_HISTORY);
            let mut buf = [0u8; 2048];
            let mut last_keyframe_request = Instant::now() - Duration::from_secs(10);
            let (mut sent_frames, mut resent, mut held_back) = (0u64, 0u64, 0u64);
            let mut last_stats = Instant::now();
            while !shared.stop.load(Ordering::Relaxed) {
                match frames.recv_timeout(Duration::from_millis(5)) {
                    Ok(frame) => {
                        let protected = {
                            let mut dave = shared.dave.lock();
                            if dave.active() {
                                match dave.session.as_mut().filter(|s| s.is_ready()).map(|s| s.encrypt(MediaType::VIDEO, davey::Codec::H264, &frame.data)) {
                                    Some(Ok(p)) => Some(p.into_owned()),
                                    // Never send unencrypted video into an end-to-end encrypted call.
                                    _ => None,
                                }
                            } else {
                                Some(frame.data)
                            }
                        };
                        let Some(protected) = protected else {
                            held_back += 1;
                            shared.keyframe.store(true, Ordering::Relaxed);
                            continue;
                        };
                        let payloads = video::packetize(&protected, MAX_PAYLOAD);
                        let last = payloads.len().saturating_sub(1);
                        for (i, payload) in payloads.into_iter().enumerate() {
                            if let Some(p) = rtp::seal_marked(&cipher, video::H264_PAYLOAD_TYPE, i == last, seq, frame.timestamp, ssrc, counter, &payload) {
                                send_all(&socket, &p);
                            }
                            counter = counter.wrapping_add(1);
                            if history.len() == RTX_HISTORY {
                                history.pop_front();
                            }
                            history.push_back((seq, frame.timestamp, i == last, payload));
                            seq = seq.wrapping_add(1);
                        }
                        sent_frames += 1;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                // Feedback from the server about what viewers lost.
                while let Ok(n) = socket.recv(&mut buf) {
                    for f in rtp::open_feedback(&cipher, &buf[..n]) {
                        match f {
                            rtp::Feedback::Keyframe => {
                                if last_keyframe_request.elapsed() > Duration::from_millis(500) {
                                    last_keyframe_request = Instant::now();
                                    shared.keyframe.store(true, Ordering::Relaxed);
                                }
                            }
                            rtp::Feedback::Nack(lost) => {
                                for s in lost {
                                    let Some((_, ts, marker, payload)) = history.iter().find(|h| h.0 == s) else { continue };
                                    // RTX: the original sequence number, then the original payload.
                                    let mut rtx = s.to_be_bytes().to_vec();
                                    rtx.extend_from_slice(payload);
                                    if let Some(p) = rtp::seal_marked(&cipher, video::H264_RTX_PAYLOAD_TYPE, *marker, rtx_seq, *ts, rtx_ssrc, counter, &rtx) {
                                        send_all(&socket, &p);
                                    }
                                    counter = counter.wrapping_add(1);
                                    rtx_seq = rtx_seq.wrapping_add(1);
                                    resent += 1;
                                }
                            }
                        }
                    }
                }
                if last_stats.elapsed() > Duration::from_secs(10) {
                    last_stats = Instant::now();
                    tracing::info!("sent {sent_frames} video frames; {resent} packets retransmitted; {held_back} held back waiting for encryption");
                }
            }
        })
        .expect("video send thread")
}

/// Sends on a non-blocking socket, waiting briefly if its buffer is full (a keyframe is a big burst).
fn send_all(socket: &UdpSocket, packet: &[u8]) {
    for _ in 0..50 {
        match socket.send(packet) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
            _ => return,
        }
    }
}

/// What a received packet is, by SSRC (as announced in op 12) and then payload type.
enum Kind {
    Audio,
    Video,
    /// A retransmission of a packet from this video SSRC.
    Rtx(u32),
    Unknown,
}

/// Receives and decrypts everyone else's audio into the mixer, and (for streams) video into the decoder.
fn spawn_receiver(
    socket: &Arc<UdpSocket>,
    cipher: Arc<Cipher>,
    our_ssrc: u32,
    stream_owner: Option<u64>,
    video_out: Option<std::sync::mpsc::SyncSender<video::EncodedFrame>>,
    shared: Arc<Shared>,
) -> std::thread::JoinHandle<()> {
    let socket = socket.clone();
    std::thread::Builder::new()
        .name("swiftcord-voice-recv".into())
        .spawn(move || {
            let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
            let mut buf = [0u8; 4096];
            let mut receivers: HashMap<u32, video::VideoReceiver> = HashMap::new();
            let mut last_video_ssrc = 0u32;
            let mut last_pli = Instant::now() - Duration::from_secs(10);
            let mut last_stats = Instant::now();
            let mut decrypt_failures = 0u64;
            let mut dropped_frames = 0u64;
            let mut unknown: HashMap<u8, u64> = HashMap::new();
            // RTCP shares the key; start its nonce counter far from the (unused) RTP one.
            let mut rtcp_counter = 0x8000_0000u32 | (rand::random::<u32>() >> 1);
            while !shared.stop.load(Ordering::Relaxed) {
                let received = socket.recv(&mut buf).ok().and_then(|n| rtp::open(&cipher, &buf[..n]));
                let mut touched = None;
                if let Some(p) = received {
                    // Everything in a stream connection comes from the streamer.
                    let user = stream_owner.or_else(|| shared.ssrc_users.lock().get(&p.ssrc).copied());
                    let kind = if p.payload_type == rtp::OPUS_PAYLOAD_TYPE {
                        Kind::Audio
                    } else if video_out.is_none() {
                        Kind::Unknown
                    } else {
                        let map = shared.video_ssrcs.lock();
                        if let Some(&media) = map.rtx.get(&p.ssrc) {
                            Kind::Rtx(media)
                        } else if map.media.contains(&p.ssrc) || p.payload_type == video::H264_PAYLOAD_TYPE {
                            Kind::Video
                        } else if p.payload_type == video::H264_RTX_PAYLOAD_TYPE && last_video_ssrc != 0 {
                            Kind::Rtx(last_video_ssrc)
                        } else {
                            Kind::Unknown
                        }
                    };
                    match kind {
                        Kind::Audio => {
                            if shared.deafened.load(Ordering::Relaxed) || p.payload.is_empty() {
                                continue;
                            }
                            let Some(opus) = e2ee_decrypt(&shared, user, MediaType::AUDIO, p.payload) else { continue };
                            // Silence frames (sent as someone stops talking) still go to the jitter buffer, so
                            // their decoder settles to quiet instead of guessing at more speech, but they don't
                            // count as talking.
                            if let (Some(u), false) = (user, opus == codec::SILENCE) {
                                shared.last_heard.lock().insert(u, Instant::now());
                            }
                            // Decoding happens at playback, in the jitter buffer.
                            shared.mixer.push(p.ssrc, p.seq, opus);
                        }
                        Kind::Video => {
                            last_video_ssrc = p.ssrc;
                            receivers.entry(p.ssrc).or_default().push(p.seq, p.timestamp, p.marker, p.payload, false);
                            touched = Some(p.ssrc);
                        }
                        Kind::Rtx(media) => {
                            // RTX payload: the original sequence number, then the original payload.
                            if p.payload.len() >= 2 {
                                let osn = u16::from_be_bytes([p.payload[0], p.payload[1]]);
                                receivers.entry(media).or_default().push(osn, p.timestamp, p.marker, p.payload[2..].to_vec(), true);
                                touched = Some(media);
                            }
                        }
                        Kind::Unknown => {
                            let n = unknown.entry(p.payload_type).or_default();
                            if *n == 0 {
                                tracing::warn!("ignoring RTP payload type {} from SSRC {}", p.payload_type, p.ssrc);
                            }
                            *n += 1;
                        }
                    }
                }

                // Emit finished frames, ask for missing packets, and recover from losses.
                let resync = shared.video_resync.swap(false, Ordering::Relaxed);
                for (&ssrc, rx) in receivers.iter_mut() {
                    if touched.is_some_and(|t| t != ssrc) && !resync {
                        continue;
                    }
                    if resync {
                        rx.resync();
                    }
                    for mut frame in rx.poll() {
                        // DAVE encrypts whole frames, so decryption happens after reassembly.
                        match e2ee_decrypt(&shared, stream_owner, MediaType::VIDEO, std::mem::take(&mut frame.data)) {
                            Some(data) => {
                                frame.data = data;
                                if let Some(out) = &video_out {
                                    if out.try_send(frame).is_err() {
                                        dropped_frames += 1;
                                        rx.resync();
                                    }
                                }
                            }
                            None => {
                                decrypt_failures += 1;
                                rx.resync();
                            }
                        }
                    }
                    let lost = rx.nacks();
                    if !lost.is_empty() {
                        if let Some(p) = rtp::seal_nack(&cipher, our_ssrc, ssrc, &lost, rtcp_counter) {
                            let _ = socket.send(&p);
                        }
                        rtcp_counter = rtcp_counter.wrapping_add(1);
                    }
                    // Ask for a keyframe when starting or after an unrecoverable loss, at most once a second.
                    if rx.needs_keyframe && last_pli.elapsed() > Duration::from_secs(1) {
                        last_pli = Instant::now();
                        if let Some(p) = rtp::seal_pli(&cipher, our_ssrc, ssrc, rtcp_counter) {
                            let _ = socket.send(&p);
                        }
                        rtcp_counter = rtcp_counter.wrapping_add(1);
                    }
                }

                if !receivers.is_empty() && last_stats.elapsed() > Duration::from_secs(10) {
                    last_stats = Instant::now();
                    for (ssrc, rx) in &receivers {
                        let s = rx.stats;
                        tracing::info!(
                            "video {ssrc}: {} packets ({} retransmitted, {} NACKed), {} frames, {} lost; {dropped_frames} dropped by a busy decoder, {decrypt_failures} undecryptable; other payload types {unknown:?}",
                            s.packets,
                            s.retransmitted,
                            s.nacked,
                            s.frames,
                            s.lost_frames
                        );
                    }
                }
            }
        })
        .expect("voice receive thread")
}

/// Warns when a stream uses a video codec other than H.264, which is all we can decode.
fn check_video_codec(d: &Value, events: &Arc<dyn Fn(Event) + Send + Sync>) {
    if let Some(codec) = d["video_codec"].as_str() {
        tracing::info!("stream video codec: {codec}");
        if !codec.eq_ignore_ascii_case("H264") {
            events(Event::Error(format!("This stream uses {codec} video, which Swiftcord can't play yet (only H.264).")));
        }
    }
}

/// Removes DAVE end-to-end encryption when the call uses it.
fn e2ee_decrypt(shared: &Shared, user: Option<u64>, kind: MediaType, payload: Vec<u8>) -> Option<Vec<u8>> {
    let mut dave = shared.dave.lock();
    if !dave.active() {
        return Some(payload);
    }
    let (user, session) = (user?, dave.session.as_mut()?);
    match session.decrypt(user, kind, &payload) {
        Ok(f) => Some(f),
        Err(_) if session.can_passthrough(user) => Some(payload),
        Err(_) => None,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Our video sender against a local socket standing in for the voice server: frames arrive, reassemble
    /// and decode, and a NACK is answered over RTX.
    #[test]
    fn sent_video_reassembles_and_retransmits() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(server.local_addr().unwrap()).unwrap();
        server.connect(client.local_addr().unwrap()).unwrap();
        server.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let key = [9u8; 32];
        let media = Media { socket: Arc::new(client), ssrc: 1, mode: Mode::Aes256Gcm, video_ssrc: 2, rtx_ssrc: 3 };
        let shared = Arc::new(Shared::new(&Settings::default()));
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let sender = spawn_video_sender(&media, Arc::new(Cipher::new(Mode::Aes256Gcm, &key).unwrap()), rx, shared.clone());

        let (w, h) = (640usize, 360usize);
        let mut enc = video::Encoder::new(w as u32, h as u32, 30, 2_000_000).unwrap();
        let mut bgra = vec![0u8; w * h * 4];
        for (i, px) in bgra.chunks_mut(4).enumerate() {
            // Noise-like content so the keyframe spans many packets.
            let v = ((i * 2654435761) >> 13) as u8;
            px.copy_from_slice(&[v, v.wrapping_mul(3), v.wrapping_add(90), 255]);
        }
        for (n, data) in enc.encode(&video::bgra_to_nv12(&bgra, w, h)).unwrap().into_iter().enumerate() {
            tx.send(video::EncodedFrame { data, timestamp: 1000 + n as u32, keyframe: true }).unwrap();
        }

        let cipher = Cipher::new(Mode::Aes256Gcm, &key).unwrap();
        let mut rx_video = video::VideoReceiver::default();
        let mut buf = [0u8; 2048];
        let mut packets = Vec::new();
        let mut frames = Vec::new();
        while frames.is_empty() {
            let n = server.recv(&mut buf).expect("video packets");
            let p = rtp::open(&cipher, &buf[..n]).expect("decrypts");
            assert_eq!((p.ssrc, p.payload_type), (2, video::H264_PAYLOAD_TYPE));
            packets.push(p.seq);
            rx_video.push(p.seq, p.timestamp, p.marker, p.payload, false);
            frames.extend(rx_video.poll());
        }
        assert!(packets.len() > 5, "a keyframe this size spans packets: {}", packets.len());
        let mut dec = video::Decoder::new().unwrap();
        let mut pics = dec.decode(&frames[0].data, true).unwrap();
        if pics.is_empty() {
            pics = dec.decode(&frames[0].data, true).unwrap();
        }
        assert_eq!((pics[0].width, pics[0].height), (w, h));

        // Ask for the third packet again: it comes back on the RTX SSRC carrying its original sequence number.
        let nack = rtp::seal_nack(&cipher, 7, 2, &[packets[2]], 0x8000_0000).unwrap();
        server.send(&nack).unwrap();
        let n = server.recv(&mut buf).expect("retransmission");
        let p = rtp::open(&cipher, &buf[..n]).expect("decrypts");
        assert_eq!((p.ssrc, p.payload_type), (3, video::H264_RTX_PAYLOAD_TYPE));
        assert_eq!(u16::from_be_bytes([p.payload[0], p.payload[1]]), packets[2]);

        // A keyframe request reaches the encoder's flag.
        server.send(&rtp::seal_pli(&cipher, 7, 2, 0x8000_0001).unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !shared.keyframe.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.keyframe.load(Ordering::Relaxed));

        shared.stop.store(true, Ordering::Relaxed);
        drop(tx);
        sender.join().unwrap();
    }
}
