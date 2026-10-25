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
    pub dave: Mutex<Dave>,
    pub ssrc_users: Mutex<HashMap<u32, u64>>,
    pub last_heard: Mutex<HashMap<u64, Instant>>,
    pub user_volume: Mutex<HashMap<u64, f32>>,
    /// The latest decoded video picture (stream connections).
    pub video: Mutex<Option<crate::video::Picture>>,
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
            dave: Mutex::new(Dave::default()),
            ssrc_users: Mutex::new(HashMap::new()),
            last_heard: Mutex::new(HashMap::new()),
            user_volume: Mutex::new(HashMap::new()),
            video: Mutex::new(None),
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
        let stream = info.stream_owner.is_some();
        let (mic_tx, mic_rx) = std::sync::mpsc::sync_channel(256);
        // Watching a stream plays its audio but never opens the microphone.
        let (io, errors) = audio::start(settings.devices.clone(), (!stream).then_some(mic_tx), shared.mixer.clone(), shared.deafened.clone());
        self.audio = Some(io);
        if !stream {
            self.threads.push(spawn_sender(&media.socket, cipher.clone(), media.ssrc, mic_rx, shared.clone(), cmd.clone()));
        }
        let video = stream.then(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
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
}

pub(crate) enum Cmd {
    Speaking(bool),
    Disconnect,
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
                "video": info.stream_owner.is_some(),
                "streams": if info.stream_owner.is_some() { json!([{ "type": "screen", "rid": "100", "quality": 100 }]) } else { json!([]) },
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
                                *media = Some(Media { socket: Arc::new(socket), ssrc, mode });
                                send!(json_msg(op::SELECT_PROTOCOL, json!({
                                    "protocol": "udp",
                                    "data": { "address": my_ip, "port": my_port, "mode": mode.name() },
                                    "codecs": codecs(info.stream_owner.is_some()),
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

/// Binds a UDP socket to the voice server and asks it for our public address.
fn discover(ip: &str, port: u16, ssrc: u32) -> Option<(UdpSocket, String, u16)> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
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

/// The codecs we offer: Opus always, and H.264 (decode only) when watching a stream, which makes the
/// streamer's client encode in a format Windows can decode natively.
fn codecs(video: bool) -> Value {
    let mut list = vec![json!({ "name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS_PAYLOAD_TYPE })];
    if video {
        list.push(json!({
            "name": "H264", "type": "video", "priority": 1000,
            "payload_type": video::H264_PAYLOAD_TYPE, "rtx_payload_type": video::H264_RTX_PAYLOAD_TYPE,
            "encode": false, "decode": true,
        }));
    }
    Value::Array(list)
}

/// Decodes stream video on its own thread (Media Foundation wants a COM thread of its own) and publishes the
/// latest picture.
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
                    events(Event::Error(format!("Can't decode video: {e}. Windows' H.264 decoder is missing (N editions need the Media Feature Pack).")));
                    return;
                }
            };
            for frame in frames.iter() {
                match decoder.decode(&frame.data) {
                    Ok(pictures) => {
                        if let Some(p) = pictures.into_iter().last() {
                            *shared.video.lock() = Some(p);
                            events(Event::VideoFrame);
                        }
                    }
                    Err(e) => tracing::debug!("video decode error: {e}"),
                }
            }
        })
        .expect("video thread")
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
            let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
            let mut buf = [0u8; 4096];
            let mut depacketizers: HashMap<u32, video::H264Depacketizer> = HashMap::new();
            let mut seen_types = HashSet::new();
            let mut last_pli = Instant::now() - Duration::from_secs(10);
            // RTCP shares the key; start its nonce counter far from the (unused) RTP one.
            let mut rtcp_counter = 0x8000_0000u32 | rand::random::<u32>() >> 1;
            while !shared.stop.load(Ordering::Relaxed) {
                let n = match socket.recv(&mut buf) {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                let Some(p) = rtp::open(&cipher, &buf[..n]) else { continue };
                // Everything in a stream connection comes from the streamer.
                let user = stream_owner.or_else(|| shared.ssrc_users.lock().get(&p.ssrc).copied());
                match p.payload_type {
                    rtp::OPUS_PAYLOAD_TYPE => {
                        if shared.deafened.load(Ordering::Relaxed) {
                            continue;
                        }
                        let Some(opus) = e2ee_decrypt(&shared, user, MediaType::AUDIO, p.payload) else { continue };
                        if opus == codec::SILENCE {
                            continue;
                        }
                        if let Some(u) = user {
                            shared.last_heard.lock().insert(u, Instant::now());
                        }
                        // Decoding happens at playback, in the jitter buffer.
                        shared.mixer.push(p.ssrc, p.seq, opus);
                    }
                    video::H264_PAYLOAD_TYPE => {
                        let Some(out) = &video_out else { continue };
                        let d = depacketizers.entry(p.ssrc).or_insert_with(video::H264Depacketizer::awaiting_keyframe);
                        if let Some(mut frame) = d.push(p.seq, p.timestamp, p.marker, p.payload) {
                            // DAVE encrypts whole frames, so decryption happens after reassembly.
                            match e2ee_decrypt(&shared, user, MediaType::VIDEO, std::mem::take(&mut frame.data)) {
                                Some(data) => {
                                    frame.data = data;
                                    let _ = out.try_send(frame);
                                }
                                None => tracing::debug!("couldn't decrypt a video frame"),
                            }
                        }
                        // Ask for a keyframe when starting or after loss, at most once a second.
                        if d.needs_keyframe && last_pli.elapsed() > Duration::from_secs(1) {
                            last_pli = Instant::now();
                            if let Some(pli) = rtp::seal_pli(&cipher, our_ssrc, p.ssrc, rtcp_counter) {
                                let _ = socket.send(&pli);
                            }
                            rtcp_counter = rtcp_counter.wrapping_add(1);
                        }
                    }
                    other => {
                        if video_out.is_some() && other != video::H264_RTX_PAYLOAD_TYPE && seen_types.insert(other) {
                            tracing::warn!("ignoring RTP payload type {other} on the stream connection");
                        }
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
