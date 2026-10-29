//! Discord voice for Swiftcord: the voice gateway (v8) with DAVE end-to-end encryption, encrypted RTP over
//! UDP, Opus, and WASAPI audio through cpal.

pub mod audio;
pub mod camera;
pub mod capture;
pub mod codec;
mod connection;
pub mod ogg;
pub mod rtp;
pub mod transmit;
pub mod video;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

pub use audio::{list_devices, play, AudioIo, Devices};

/// Everything needed to connect, from the main gateway's VOICE_STATE_UPDATE and VOICE_SERVER_UPDATE.
#[derive(Debug, Clone)]
pub struct ConnectInfo {
    pub user_id: u64,
    /// The guild ID, or the channel ID for DM calls.
    pub server_id: u64,
    pub channel_id: u64,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
    /// For a Go Live stream connection: the streamer. Stream connections receive video and the stream's
    /// audio and never send.
    pub stream_owner: Option<u64>,
    /// For our own Go Live stream connection: what to send. It sends video only and plays nothing.
    pub share: Option<Share>,
}

/// Going live: what to capture and at what quality.
#[derive(Debug, Clone)]
pub struct Share {
    pub source: capture::Source,
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
    /// Video bitrate in bits per second.
    pub bitrate: u32,
    /// For a whole screen: try Desktop Duplication, which can see fullscreen-exclusive games (experimental).
    pub fullscreen_games: bool,
}

/// How a stream we're sending is going.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShareStats {
    pub width: u32,
    pub height: u32,
    /// Frames captured and encoded per second (lower than asked when the machine can't keep up).
    pub fps: f32,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub devices: Devices,
    /// Voice activity threshold in dBFS; quieter input isn't transmitted.
    pub vad_threshold_db: f32,
    pub muted: bool,
    pub deafened: bool,
    /// Remove background noise from the microphone (RNNoise).
    pub noise_suppression: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { devices: Devices::default(), vad_threshold_db: -50.0, muted: false, deafened: false, noise_suppression: true }
    }
}

/// How long after someone's last sound they still count as speaking.
pub const SPEAKING_HOLD: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub enum Event {
    Connecting,
    /// Audio is flowing. `e2ee` says whether the call is end-to-end encrypted (DAVE).
    Connected { e2ee: bool },
    /// A non-fatal problem, e.g. a missing microphone.
    Error(String),
    /// A new video picture is ready (see [`Voice::take_picture`]).
    VideoFrame,
    /// Someone (or we) started or stopped speaking: speaking indicators need redrawing.
    Speaking,
    /// Our camera stopped by itself (it failed or was unplugged); the reason has been sent as an `Error`.
    CameraStopped,
    Disconnected(String),
}

/// A live voice connection. Dropping it disconnects.
pub struct Voice {
    shared: Arc<connection::Shared>,
    cmd: mpsc::UnboundedSender<connection::Cmd>,
}

impl Voice {
    pub fn connect(rt: &tokio::runtime::Handle, info: ConnectInfo, settings: Settings, events: impl Fn(Event) + Send + Sync + 'static) -> Voice {
        let shared = Arc::new(connection::Shared::new(&settings));
        let (tx, rx) = mpsc::unbounded_channel();
        rt.spawn(connection::run(info, settings, shared.clone(), rx, tx.clone(), Arc::new(events)));
        Voice { shared, cmd: tx }
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.shared.deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn set_vad_threshold(&self, db: f32) {
        self.shared.threshold.store((-db).clamp(0.0, 100.0) as u16, Ordering::Relaxed);
    }

    pub fn set_noise_suppression(&self, on: bool) {
        self.shared.noise_suppression.store(on, Ordering::Relaxed);
    }

    /// Turns our camera on (the named one, or the first) or off.
    pub fn set_camera(&self, camera: Option<Option<String>>) {
        let _ = self.cmd.send(connection::Cmd::Camera(camera));
    }

    /// Camera pictures that arrived since the last call, by user (our own preview included).
    pub fn take_cameras(&self) -> HashMap<u64, video::Picture> {
        std::mem::take(&mut *self.shared.cameras.lock())
    }

    /// 0.0–2.0 for one person's volume.
    pub fn set_user_volume(&self, user: u64, volume: f32) {
        self.shared.user_volume.lock().insert(user, volume);
        for (&ssrc, &u) in self.shared.ssrc_users.lock().iter() {
            if u == user {
                self.shared.mixer.set_gain(ssrc, volume);
            }
        }
    }

    /// Whether we heard this user in the last quarter second.
    pub fn is_speaking(&self, user: u64) -> bool {
        self.shared.last_heard.lock().get(&user).is_some_and(|t| t.elapsed() < SPEAKING_HOLD)
    }

    pub fn speaking_users(&self) -> Vec<u64> {
        let cutoff = Instant::now() - SPEAKING_HOLD;
        self.shared.last_heard.lock().iter().filter(|(_, t)| **t > cutoff).map(|(u, _)| *u).collect()
    }

    pub fn self_speaking(&self) -> bool {
        self.shared.self_speaking.load(Ordering::Relaxed)
    }

    pub fn ping_ms(&self) -> u32 {
        self.shared.ping_ms.load(Ordering::Relaxed)
    }

    /// The DAVE voice privacy code, when the call is end-to-end encrypted.
    pub fn privacy_code(&self) -> Option<String> {
        let dave = self.shared.dave.lock();
        dave.session.as_ref().filter(|_| dave.protocol > 0).and_then(|s| s.voice_privacy_code().map(str::to_owned))
    }

    pub fn disconnect(&self) {
        let _ = self.cmd.send(connection::Cmd::Disconnect);
    }
}

/// Plays your microphone back through the same gate, Opus encoder, jitter buffer and decoder a call uses,
/// without connecting anywhere, so you can hear what others would and set the sensitivity.
pub struct MicTest {
    stop: Arc<std::sync::atomic::AtomicBool>,
    level: Arc<std::sync::atomic::AtomicU32>,
    open: Arc<std::sync::atomic::AtomicBool>,
    threshold: Arc<std::sync::atomic::AtomicU32>,
    noise_suppression: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    errors: std::sync::mpsc::Receiver<String>,
    _io: audio::AudioIo,
}

impl MicTest {
    pub fn start(devices: Devices, threshold_db: f32, noise_suppression: bool) -> MicTest {
        use std::sync::atomic::{AtomicBool, AtomicU32};
        let mixer = Arc::new(audio::Mixer::default());
        let (mic_tx, mic_rx) = std::sync::mpsc::sync_channel(256);
        let (io, errors) = audio::start(devices, Some(mic_tx), mixer.clone(), Arc::new(AtomicBool::new(false)));
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new((-100f32).to_bits()));
        let open = Arc::new(AtomicBool::new(false));
        let threshold = Arc::new(AtomicU32::new(threshold_db.to_bits()));
        let noise_suppression = Arc::new(AtomicBool::new(noise_suppression));
        let (s, l, o, t, ns) = (stop.clone(), level.clone(), open.clone(), threshold.clone(), noise_suppression.clone());
        let thread = std::thread::spawn(move || {
            let Some(mut tx) = transmit::Transmitter::new(64_000) else { return };
            let mut pending = Vec::new();
            let mut seq = 0u16;
            while !s.load(Ordering::Relaxed) {
                let Ok(chunk) = mic_rx.recv_timeout(Duration::from_millis(100)) else { continue };
                pending.extend_from_slice(&chunk);
                while pending.len() >= codec::FRAME {
                    let frame: Vec<f32> = pending.drain(..codec::FRAME).collect();
                    tx.set_noise_suppression(ns.load(Ordering::Relaxed));
                    let (out, _) = tx.process(frame, false, f32::from_bits(t.load(Ordering::Relaxed)));
                    l.store(tx.level_db.to_bits(), Ordering::Relaxed);
                    o.store(tx.is_open(), Ordering::Relaxed);
                    if let transmit::Output::Audio(packets) = out {
                        for p in packets {
                            mixer.push(0, seq, p);
                            seq = seq.wrapping_add(1);
                        }
                    }
                }
            }
        });
        MicTest { stop, level, open, threshold, noise_suppression, thread: Some(thread), errors, _io: io }
    }

    pub fn set_noise_suppression(&self, on: bool) {
        self.noise_suppression.store(on, Ordering::Relaxed);
    }

    /// The microphone's current level in dBFS.
    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Whether the voice-activity gate is open (you'd be transmitting).
    pub fn transmitting(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    pub fn set_threshold(&self, db: f32) {
        self.threshold.store(db.to_bits(), Ordering::Relaxed);
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors.try_iter().collect()
    }
}

impl Drop for MicTest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Voice {
    /// The latest decoded video picture, if a new one arrived since the last call. While sharing, a small
    /// preview of what's being sent.
    pub fn take_picture(&self) -> Option<video::Picture> {
        self.shared.video.lock().take()
    }

    pub fn share_stats(&self) -> Option<ShareStats> {
        *self.shared.share_stats.lock()
    }

    /// The microphone's current level in dBFS.
    pub fn input_level_db(&self) -> f32 {
        f32::from_bits(self.shared.input_db.load(Ordering::Relaxed))
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        self.disconnect();
    }
}
