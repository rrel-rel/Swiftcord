//! The main Discord gateway connection: zlib-stream transport compression, heartbeats with zombie
//! detection, and automatic resume or re-identify after disconnects.

use std::time::Duration;

use flate2::{Decompress, FlushDecompress, Status};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use tokio::sync::mpsc;
use tokio::time::{sleep, sleep_until, Instant};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::events::Event;
use crate::models::PresenceStatus;
use crate::Snowflake;

pub mod op {
    pub const DISPATCH: u8 = 0;
    pub const HEARTBEAT: u8 = 1;
    pub const IDENTIFY: u8 = 2;
    pub const PRESENCE_UPDATE: u8 = 3;
    pub const VOICE_STATE_UPDATE: u8 = 4;
    pub const RESUME: u8 = 6;
    pub const RECONNECT: u8 = 7;
    pub const INVALID_SESSION: u8 = 9;
    pub const HELLO: u8 = 10;
    pub const HEARTBEAT_ACK: u8 = 11;
    /// User accounts: subscribe to typing, activities, threads and member-list ranges for a guild.
    pub const GUILD_SUBSCRIPTIONS: u8 = 14;
    pub const STREAM_CREATE: u8 = 18;
    pub const STREAM_DELETE: u8 = 19;
    pub const STREAM_WATCH: u8 = 20;
}

const DEFAULT_URL: &str = "wss://gateway.discord.gg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// Waiting before a reconnect attempt.
    Reconnecting,
    Disconnected,
}

#[derive(Debug)]
pub enum GatewayOutput {
    Event(Event),
    State(ConnectionState),
    /// The gateway closed with a code that prevents reconnecting, e.g. 4004 for an invalid token.
    Fatal { code: u16, reason: String },
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub token: String,
    pub locale: String,
    pub url: String,
}

impl GatewayConfig {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into(), locale: "en-US".into(), url: DEFAULT_URL.into() }
    }
}

enum Command {
    Send(String),
    Stop,
}

/// Cheap to clone; dropping every handle doesn't stop the connection, call [`Gateway::stop`].
#[derive(Clone)]
pub struct Gateway {
    tx: mpsc::UnboundedSender<Command>,
}

impl Gateway {
    /// Starts the connection on the current Tokio runtime. `sink` is called on a runtime thread for every
    /// event and state change.
    pub fn spawn(config: GatewayConfig, sink: impl Fn(GatewayOutput) + Send + Sync + 'static) -> Gateway {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(config, rx, sink));
        Gateway { tx }
    }

    pub fn send(&self, op: u8, d: serde_json::Value) {
        let _ = self.tx.send(Command::Send(json!({ "op": op, "d": d }).to_string()));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }

    /// Joins, moves between or (with `channel_id: None`) leaves voice channels.
    pub fn update_voice_state(&self, guild_id: Option<Snowflake>, channel_id: Option<Snowflake>, mute: bool, deaf: bool) {
        self.send(
            op::VOICE_STATE_UPDATE,
            json!({ "guild_id": guild_id, "channel_id": channel_id, "self_mute": mute, "self_deaf": deaf, "self_video": false }),
        );
    }

    /// Discord only delivers typing and some member updates for larger guilds after this.
    pub fn subscribe_guild(&self, guild_id: Snowflake) {
        self.send(
            op::GUILD_SUBSCRIPTIONS,
            json!({ "guild_id": guild_id, "typing": true, "threads": true, "activities": true }),
        );
    }

    pub fn update_presence(&self, status: PresenceStatus) {
        self.send(op::PRESENCE_UPDATE, json!({ "status": status.as_str(), "since": 0, "activities": [], "afk": false }));
    }
}

#[derive(Default)]
struct Session {
    id: Option<String>,
    resume_url: Option<String>,
    seq: Option<u64>,
}

enum Outcome {
    Stopped,
    Fatal(u16, String),
    Reconnect { reidentify: bool, delay: Option<Duration>, was_ready: bool },
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    #[serde(default)]
    d: Option<Box<RawValue>>,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

async fn run(config: GatewayConfig, mut rx: mpsc::UnboundedReceiver<Command>, sink: impl Fn(GatewayOutput)) {
    let mut session = Session::default();
    let mut attempt = 0u32;
    loop {
        sink(GatewayOutput::State(ConnectionState::Connecting));
        let outcome = match run_session(&config, &mut session, &mut rx, &sink).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("gateway connection failed: {e}");
                Outcome::Reconnect { reidentify: false, delay: None, was_ready: false }
            }
        };
        match outcome {
            Outcome::Stopped => break,
            Outcome::Fatal(code, reason) => {
                tracing::error!("gateway closed with fatal code {code}: {reason}");
                sink(GatewayOutput::Fatal { code, reason });
                break;
            }
            Outcome::Reconnect { reidentify, delay, was_ready } => {
                if reidentify {
                    session = Session::default();
                }
                if was_ready {
                    attempt = 0;
                }
                let delay = delay.unwrap_or_else(|| backoff(attempt));
                attempt += 1;
                tracing::info!("gateway disconnected; reconnecting in {delay:?}");
                sink(GatewayOutput::State(ConnectionState::Reconnecting));
                tokio::select! {
                    _ = sleep(delay) => {}
                    cmd = rx.recv() => if matches!(cmd, None | Some(Command::Stop)) { break },
                }
            }
        }
    }
    sink(GatewayOutput::State(ConnectionState::Disconnected));
}

/// Exponential backoff with jitter: ~1s, 2s, 4s … capped at 60s.
fn backoff(attempt: u32) -> Duration {
    let secs = 2f64.powi(attempt.min(6) as i32).min(60.0);
    Duration::from_secs_f64(secs * rand::thread_rng().gen_range(0.75..1.25))
}

fn is_fatal(code: u16) -> bool {
    matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
}

async fn run_session(
    config: &GatewayConfig,
    session: &mut Session,
    rx: &mut mpsc::UnboundedReceiver<Command>,
    sink: &impl Fn(GatewayOutput),
) -> anyhow_lite::Result<Outcome> {
    let resuming = session.id.is_some();
    let base = session.resume_url.as_deref().filter(|_| resuming).unwrap_or(&config.url);
    let url = format!("{}/?v=10&encoding=json&compress=zlib-stream", base.trim_end_matches('/'));
    let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await?;

    let mut inflate = Inflater::new();
    let mut interval: Option<Duration> = None;
    let mut next_heartbeat = Instant::now() + Duration::from_secs(3600);
    let mut acked = true;
    let mut was_ready = false;

    loop {
        tokio::select! {
            frame = ws.next() => {
                let text = match frame {
                    None => return Ok(Outcome::Reconnect { reidentify: false, delay: None, was_ready }),
                    Some(Err(e)) => { tracing::warn!("gateway read error: {e}"); return Ok(Outcome::Reconnect { reidentify: false, delay: None, was_ready }); }
                    Some(Ok(WsMessage::Binary(b))) => match inflate.push(&b)? { Some(t) => t, None => continue },
                    Some(Ok(WsMessage::Text(t))) => t.to_string(),
                    Some(Ok(WsMessage::Close(frame))) => {
                        let (code, reason) = frame.map(|f| (u16::from(f.code), f.reason.to_string())).unwrap_or((1006, String::new()));
                        if is_fatal(code) {
                            return Ok(Outcome::Fatal(code, reason));
                        }
                        // Invalid sequence / session timed out: the session can't be resumed.
                        let reidentify = matches!(code, 4007 | 4009);
                        return Ok(Outcome::Reconnect { reidentify, delay: None, was_ready });
                    }
                    Some(Ok(_)) => continue,
                };

                let payload: Payload = match serde_json::from_str(&text) {
                    Ok(p) => p,
                    Err(e) => { tracing::warn!("unreadable gateway payload: {e}"); continue; }
                };
                match payload.op {
                    op::HELLO => {
                        #[derive(Deserialize)]
                        struct Hello { heartbeat_interval: u64 }
                        let hello: Hello = serde_json::from_str(payload.d.as_deref().map_or("{}", RawValue::get))?;
                        let iv = Duration::from_millis(hello.heartbeat_interval);
                        interval = Some(iv);
                        // The first heartbeat is jittered so reconnecting clients don't stampede.
                        next_heartbeat = Instant::now() + iv.mul_f64(rand::thread_rng().r#gen::<f64>());
                        let msg = if resuming { resume_payload(config, session) } else { identify_payload(config) };
                        ws.send(WsMessage::text(msg)).await?;
                    }
                    op::HEARTBEAT_ACK => acked = true,
                    op::HEARTBEAT => {
                        ws.send(WsMessage::text(json!({ "op": op::HEARTBEAT, "d": session.seq }).to_string())).await?;
                    }
                    op::RECONNECT => {
                        tracing::info!("gateway requested reconnect");
                        close(&mut ws, 4000).await;
                        return Ok(Outcome::Reconnect { reidentify: false, delay: Some(Duration::ZERO), was_ready });
                    }
                    op::INVALID_SESSION => {
                        let resumable = payload.d.as_deref().is_some_and(|d| d.get() == "true");
                        tracing::info!("invalid session (resumable: {resumable})");
                        close(&mut ws, 4000).await;
                        // Discord asks clients to wait 1–5 seconds before identifying again.
                        let delay = Duration::from_millis(rand::thread_rng().gen_range(1000..5000));
                        return Ok(Outcome::Reconnect { reidentify: !resumable, delay: Some(delay), was_ready });
                    }
                    op::DISPATCH => {
                        if let Some(s) = payload.s { session.seq = Some(s); }
                        let (Some(name), Some(d)) = (payload.t, payload.d) else { continue };
                        if name == "READY" {
                            #[derive(Deserialize)]
                            struct R { session_id: String, resume_gateway_url: Option<String> }
                            if let Ok(r) = serde_json::from_str::<R>(d.get()) {
                                session.id = Some(r.session_id);
                                session.resume_url = r.resume_gateway_url;
                            }
                        }
                        if name == "READY" || name == "RESUMED" {
                            was_ready = true;
                            sink(GatewayOutput::State(ConnectionState::Connected));
                        }
                        match Event::parse(&name, d) {
                            Ok(event) => sink(GatewayOutput::Event(event)),
                            Err(e) => tracing::warn!("couldn't read {name}: {e}"),
                        }
                    }
                    _ => {}
                }
            }
            _ = sleep_until(next_heartbeat), if interval.is_some() => {
                if !acked {
                    tracing::warn!("heartbeat not acknowledged; reconnecting");
                    close(&mut ws, 4000).await;
                    return Ok(Outcome::Reconnect { reidentify: false, delay: None, was_ready });
                }
                acked = false;
                ws.send(WsMessage::text(json!({ "op": op::HEARTBEAT, "d": session.seq }).to_string())).await?;
                next_heartbeat = Instant::now() + interval.unwrap();
            }
            cmd = rx.recv() => match cmd {
                Some(Command::Send(text)) => ws.send(WsMessage::text(text)).await?,
                None | Some(Command::Stop) => {
                    // Closing with 1000 ends the session rather than keeping it resumable.
                    close(&mut ws, 1000).await;
                    return Ok(Outcome::Stopped);
                }
            },
        }
    }
}

async fn close<S>(ws: &mut S, code: u16)
where
    S: SinkExt<WsMessage> + Unpin,
{
    let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
    let _ = ws.send(WsMessage::Close(Some(frame))).await;
}

fn identify_payload(config: &GatewayConfig) -> String {
    json!({
        "op": op::IDENTIFY,
        "d": {
            "token": config.token,
            "capabilities": 0,
            "compress": false,
            "properties": {
                "os": "Windows",
                "browser": "Swiftcord",
                "device": "Swiftcord",
                "system_locale": config.locale,
            },
            "presence": { "status": "online", "since": 0, "activities": [], "afk": false },
            "client_state": { "guild_versions": {} },
        }
    })
    .to_string()
}

fn resume_payload(config: &GatewayConfig, session: &Session) -> String {
    json!({
        "op": op::RESUME,
        "d": { "token": config.token, "session_id": session.id, "seq": session.seq }
    })
    .to_string()
}

/// Discord's zlib-stream: one zlib context spans the connection and each complete message ends with a
/// Z_SYNC_FLUSH marker (00 00 FF FF).
struct Inflater {
    z: Decompress,
    pending: Vec<u8>,
}

impl Inflater {
    fn new() -> Self {
        Self { z: Decompress::new(true), pending: Vec::new() }
    }

    fn push(&mut self, chunk: &[u8]) -> anyhow_lite::Result<Option<String>> {
        self.pending.extend_from_slice(chunk);
        if !self.pending.ends_with(&[0, 0, 0xff, 0xff]) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(self.pending.len() * 4);
        let mut input = &self.pending[..];
        loop {
            if out.capacity() - out.len() < 4096 {
                out.reserve(out.capacity().max(16 * 1024));
            }
            let before_in = self.z.total_in();
            let status = self.z.decompress_vec(input, &mut out, FlushDecompress::Sync)?;
            input = &input[(self.z.total_in() - before_in) as usize..];
            if input.is_empty() && out.len() < out.capacity() {
                break;
            }
            if matches!(status, Status::StreamEnd) {
                break;
            }
        }
        self.pending.clear();
        Ok(Some(String::from_utf8(out)?))
    }
}

/// A tiny boxed error so the gateway doesn't need anyhow.
mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    #[test]
    fn inflates_across_messages_with_one_context() {
        let mut z = Compress::new(Compression::default(), true);
        let mut frames = Vec::new();
        for text in [r#"{"op":10}"#, r#"{"op":11,"d":null}"#] {
            let mut out = Vec::with_capacity(256);
            z.compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync).unwrap();
            frames.push(out);
        }
        let mut inf = Inflater::new();
        // Split the first frame to check partial chunks are buffered.
        let (a, b) = frames[0].split_at(3);
        assert_eq!(inf.push(a).unwrap(), None);
        assert_eq!(inf.push(b).unwrap().as_deref(), Some(r#"{"op":10}"#));
        assert_eq!(inf.push(&frames[1]).unwrap().as_deref(), Some(r#"{"op":11,"d":null}"#));
    }
}
