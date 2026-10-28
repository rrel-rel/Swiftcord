//! Microphone capture and speaker playback through WASAPI (cpal), converted to and from 48 kHz.
//!
//! cpal streams aren't `Send` on every platform, so both live on one dedicated thread that owns them until
//! the call ends. Capture hands mono 48 kHz samples to the sender over a channel; playback pulls from the
//! [`Mixer`] that the receiver fills.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use parking_lot::Mutex;

use crate::codec::{Decoder, FRAME, SAMPLE_RATE, SILENCE};

/// Linear resampling: plenty for voice, and nearly free.
struct Resampler {
    step: f64,
    pos: f64,
    prev: Vec<f32>,
    channels: usize,
}

impl Resampler {
    fn new(from: u32, to: u32, channels: usize) -> Self {
        Self { step: from as f64 / to as f64, pos: 0.0, prev: vec![0.0; channels], channels }
    }

    fn is_identity(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    /// Resamples interleaved frames from `input`, appending to `out`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        let frames = input.len() / ch;
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        let at = |i: isize, c: usize, prev: &[f32]| if i < 0 { prev[c] } else { input[i as usize * ch + c] };
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize - 1;
            let t = self.pos.fract() as f32;
            for c in 0..ch {
                let a = at(i, c, &self.prev);
                let b = at(i + 1, c, &self.prev);
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.prev.copy_from_slice(&input[(frames - 1) * ch..frames * ch]);
        }
    }
}

/// One remote speaker: a jitter buffer of Opus packets, decoded as they're played.
///
/// Packets wait until `target` frames are buffered, then play in sequence order. A packet missing while later
/// ones have arrived is recovered from the next packet's error-correction copy when it has one, and otherwise
/// covered by repeating the last frame more and more quietly (Opus's own concealment can burst to several
/// times the real level, which pops). The seams this leaves are smoothed. When the speaker stops (silence
/// frames, or nothing more arrives), the last few milliseconds of real sound fade out, and the next talk
/// spurt fades in. The decoder is never started afresh mid-stream: Opus codes each frame relative to the
/// one before, so silence frames are still decoded (just not played). If the buffer runs dry while the speaker is still sending, `target`
/// grows so the next run has more cushion, and shrinks again after a stretch without trouble. If it keeps
/// filling up (the sender's clock runs fast), two frames are now and then blended into one to catch up.
struct Stream {
    decoder: Decoder,
    packets: BTreeMap<u64, Vec<u8>>,
    /// Highest extended sequence number seen, for unwrapping 16-bit sequence numbers.
    highest: Option<u64>,
    next: u64,
    pcm: VecDeque<f32>,
    playing: bool,
    /// Between talk spurts: what was playing has been faded out, so the next real packet starts a fresh
    /// decoder and fades in.
    quiet: bool,
    target: usize,
    concealed: u32,
    last_arrival: Instant,
    last_underrun: Instant,
    /// The last stereo sample queued, to smooth seams against.
    tail: [f32; 2],
    /// The next frame doesn't continue the last one (after a covered loss): smooth the join.
    seam: bool,
    /// The last real decoded frame, repeated (quieter) to cover a lost packet.
    last_frame: Vec<f32>,
    /// Frames played since two were last blended into one.
    since_merge: u32,
}

/// How long a seam is smoothed over (2.5 ms).
const DECLICK: usize = SAMPLE_RATE as usize / 400;
/// Catch up (blend two frames into one) when this many frames beyond the target are waiting…
const CATCH_UP_AT: usize = 2;
/// …at most once in this many frames played (twice a second).
const MERGE_EVERY: u32 = 25;

// Internet jitter of a few tens of ms is normal, so keep at least 60 ms of cushion.
const MIN_TARGET: usize = 3;
const MAX_TARGET: usize = 12;
/// Decoded sound kept in hand (5 ms, in stereo samples) so a stop can be faded.
const HOLD: usize = SAMPLE_RATE as usize / 200 * 2;
/// More than this many frames beyond the target are skipped to catch up (we've fallen behind).
const MAX_BACKLOG: usize = 25;

impl Stream {
    fn new() -> Option<Self> {
        Some(Self {
            decoder: Decoder::new()?,
            packets: BTreeMap::new(),
            highest: None,
            next: 0,
            pcm: VecDeque::new(),
            playing: false,
            quiet: true,
            target: 4,
            concealed: 0,
            last_arrival: Instant::now(),
            last_underrun: Instant::now(),
            tail: [0.0; 2],
            seam: false,
            last_frame: Vec::new(),
            since_merge: 0,
        })
    }

    /// Queues a decoded frame. After a covered loss the join is smoothed: the jump between the last sample
    /// played and this frame's first is eased out over a few milliseconds instead of clicking.
    fn emit(&mut self, frame: &mut [f32]) {
        if frame.len() < 2 {
            return;
        }
        if std::mem::take(&mut self.seam) {
            let d = [self.tail[0] - frame[0], self.tail[1] - frame[1]];
            let n = (frame.len() / 2).min(DECLICK);
            for i in 0..n {
                let k = 1.0 - i as f32 / n as f32;
                frame[i * 2] += d[0] * k;
                frame[i * 2 + 1] += d[1] * k;
            }
        }
        self.tail = [frame[frame.len() - 2], frame[frame.len() - 1]];
        self.since_merge = self.since_merge.saturating_add(1);
        self.pcm.extend(frame.iter());
    }

    fn unwrap_seq(&self, seq: u16) -> u64 {
        let Some(h) = self.highest else { return (1 << 16) + seq as u64 };
        // Pick the candidate nearest the highest seen.
        let base = h & !0xffff;
        [base.wrapping_sub(1 << 16), base, base + (1 << 16)]
            .into_iter()
            .map(|b| b + seq as u64)
            .min_by_key(|&c| c.abs_diff(h))
            .unwrap()
    }

    fn push(&mut self, seq: u16, opus: Vec<u8>) {
        let ext = self.unwrap_seq(seq);
        self.last_arrival = Instant::now();
        if ext < self.next {
            return; // Too late: already concealed or faded past.
        }
        self.highest = Some(self.highest.map_or(ext, |h| h.max(ext)));
        self.packets.insert(ext, opus);
        if self.packets.len() > self.target + MAX_BACKLOG {
            // Far behind (e.g. after a long stall): fade out what's playing, drop down to the target and carry
            // on from there (the next packet starts afresh and fades in).
            while self.packets.len() > self.target {
                self.packets.pop_first();
            }
            self.next = *self.packets.keys().next().unwrap();
            if !self.quiet {
                self.fade_out();
            }
        }
    }

    /// Decodes the next frame into `pcm`; false when there's nothing to play.
    fn decode_next(&mut self, scratch: &mut [f32]) -> bool {
        if !self.playing {
            if self.packets.len() < self.target {
                return false;
            }
            self.playing = true;
            self.next = *self.packets.keys().next().unwrap();
            self.concealed = 0;
        }
        if self.catch_up(scratch) {
            return true;
        }
        let n = match self.packets.remove(&self.next) {
            // The silence frames sent as someone stops talking: end with our own fade rather than decoding
            // them, which makes the decoder switch modes abruptly (a click).
            Some(p) if p == SILENCE => {
                if !self.quiet {
                    self.fade_out();
                }
                // Still decoded (and not played), so the decoder keeps step with the sender's encoder: Opus codes
                // each frame relative to the one before, and a decoder that missed frames can get the next
                // one's loudness badly wrong.
                self.decoder.decode(Some(&p), scratch);
                self.pcm.extend(std::iter::repeat_n(0.0, FRAME * 2));
                self.next += 1;
                return true;
            }
            Some(p) => {
                self.concealed = 0;
                // A new talk spurt: ramp up from silence over 5 ms, so it doesn't start with a jump. (The decoder
                // carries on rather than starting afresh, for the reason above.)
                let fade_in = std::mem::take(&mut self.quiet);
                if fade_in {
                    self.seam = false;
                }
                let n = self.decoder.decode(Some(&p), scratch);
                if fade_in {
                    let ramp = (SAMPLE_RATE as usize / 200 * 2).min(n);
                    for (i, s) in scratch[..ramp].iter_mut().enumerate() {
                        *s *= (i / 2) as f32 / (ramp / 2) as f32;
                    }
                }
                self.last_frame.clear();
                self.last_frame.extend_from_slice(&scratch[..n]);
                n
            }
            None if !self.packets.is_empty() => self.conceal(scratch),
            None => {
                // Ran dry: they've stopped (or the network stalled). If packets were still arriving, the buffer
                // was too shallow: deepen it. Either way, fade out now instead of having the decoder invent
                // more speech and then stop, which pops.
                self.playing = false;
                if self.last_arrival.elapsed() < Duration::from_millis(60) {
                    self.target = (self.target + 1).min(MAX_TARGET);
                    self.last_underrun = Instant::now();
                }
                if self.quiet {
                    return false;
                }
                self.fade_out();
                return true;
            }
        };
        if n == usize::MAX {
            return true;
        }
        self.next += 1;
        if self.last_underrun.elapsed() > Duration::from_secs(15) && self.target > MIN_TARGET {
            self.target -= 1;
            self.last_underrun = Instant::now();
        }
        let mut frame = scratch[..n].to_vec();
        self.emit(&mut frame);
        true
    }

    /// When packets keep piling up beyond the target (the sender's clock runs a little fast, so we'd drift
    /// further and further behind), plays two frames as one, blending the first into the second, at most
    /// twice a second. That keeps the delay steady without a jump. True when it did.
    fn catch_up(&mut self, scratch: &mut [f32]) -> bool {
        if self.quiet || self.packets.len() <= self.target + CATCH_UP_AT || self.since_merge < MERGE_EVERY {
            return false;
        }
        let (Some(a), Some(b)) = (self.packets.get(&self.next), self.packets.get(&(self.next + 1))) else { return false };
        if *a == SILENCE || *b == SILENCE {
            return false;
        }
        let (a, b) = (self.packets.remove(&self.next).unwrap(), self.packets.remove(&(self.next + 1)).unwrap());
        let (first, second) = scratch.split_at_mut(scratch.len() / 2);
        let na = self.decoder.decode(Some(&a), first);
        let nb = self.decoder.decode(Some(&b), second);
        let n = na.min(nb);
        // The first frame's start continues what's playing and the second's end leads into the next frame,
        // so blending across the frame keeps both joins seamless.
        let frames = (n / 2).max(1) as f32;
        let mut blended: Vec<f32> = (0..n).map(|i| {
            let w = (i / 2) as f32 / frames;
            first[i] * (1.0 - w) + second[i] * w
        })
        .collect();
        self.next += 2;
        self.concealed = 0;
        self.since_merge = 0;
        self.last_frame.clear();
        self.last_frame.extend_from_slice(&second[..nb]);
        self.emit(&mut blended);
        true
    }

    /// Fades out the real sound still waiting to play (the mixer keeps [`HOLD`] of it in hand), then marks
    /// the stream quiet. Concealment isn't used for this: on a sudden stop it tends to burst, which pops.
    fn fade_out(&mut self) {
        let from = self.pcm.len().saturating_sub(HOLD);
        let frames = ((self.pcm.len() - from) / 2).max(1) as f32;
        for (i, s) in self.pcm.iter_mut().skip(from).enumerate() {
            *s *= 1.0 - ((i / 2) as f32 + 1.0) / frames;
        }
        self.quiet = true;
        self.tail = [0.0; 2];
        self.seam = false;
    }

    /// A packet is missing but later ones have arrived. If the next one carries an error-correction copy of
    /// it, that's decoded; otherwise the last real frame is repeated, quieter each time (Opus's own
    /// concealment can burst to several times the real level, which pops). After three in a row the sound
    /// has faded away and playback skips ahead to what's there. Returns the samples produced, or
    /// `usize::MAX` after skipping.
    fn conceal(&mut self, scratch: &mut [f32]) -> usize {
        if self.quiet {
            // Nothing playing to continue: just move on.
            self.next = *self.packets.keys().next().unwrap();
            return usize::MAX;
        }
        // Recovered from the next packet's copy: real audio, decoded in order. Voice packets don't always
        // carry one, and then libopus quietly conceals instead, which can burst; a result much louder than
        // the frame before it is that, and isn't used.
        if let Some(next) = self.packets.get(&(self.next + 1)).filter(|p| crate::codec::carries_fec(p)) {
            let before = self.decoder.snapshot();
            let n = self.decoder.decode_fec(next, &mut scratch[..FRAME * 2]);
            let peak = |s: &[f32]| s.iter().fold(0f32, |m, v| m.max(v.abs()));
            if n > 0 && peak(&scratch[..n]) <= (peak(&self.last_frame) * 1.3).max(0.05) {
                self.concealed = 0;
                self.last_frame.clear();
                self.last_frame.extend_from_slice(&scratch[..n]);
                return n;
            }
            // The burst also lives on in the decoder's state, where it would spill into the next frame: go back
            // to before the attempt.
            self.decoder.restore(&before);
        }
        self.concealed += 1;
        if self.concealed > 3 || self.last_frame.is_empty() {
            // Faded away: skip to what's there, which starts afresh and fades in.
            self.fade_out();
            self.next = *self.packets.keys().next().unwrap();
            self.concealed = 0;
            return usize::MAX;
        }
        // The last real frame again, ramping down a third of the way to silence each time.
        let n = self.last_frame.len();
        let frames = (n / 2) as f32;
        let (from, to) = (1.0 - (self.concealed - 1) as f32 / 3.0, 1.0 - self.concealed as f32 / 3.0);
        for (i, (o, s)) in scratch[..n].iter_mut().zip(&self.last_frame).enumerate() {
            let t = (i / 2) as f32 / frames;
            *o = s * (from + (to - from) * t);
        }
        // Where the repeat begins doesn't follow where the frame ended, and the next real frame won't follow
        // the repeat either: both joins get smoothed.
        self.seam = true;
        let mut frame = scratch[..n].to_vec();
        self.emit(&mut frame);
        self.seam = true;
        if self.concealed == 3 {
            // Down to silence: the next real packet starts afresh and fades in.
            self.quiet = true;
            self.tail = [0.0; 2];
            self.seam = false;
        }
        self.next += 1;
        usize::MAX
    }
}

/// Mixes every remote speaker into the output.
#[derive(Default)]
pub struct Mixer {
    streams: Mutex<HashMap<u32, Stream>>,
    /// Per-SSRC gain, set from per-user volume.
    gains: Mutex<HashMap<u32, f32>>,
}

impl Mixer {
    /// Queues a received Opus packet.
    pub fn push(&self, ssrc: u32, seq: u16, opus: Vec<u8>) {
        let mut streams = self.streams.lock();
        if !streams.contains_key(&ssrc) {
            let Some(s) = Stream::new() else { return };
            streams.insert(ssrc, s);
        }
        streams.get_mut(&ssrc).unwrap().push(seq, opus);
    }

    pub fn set_gain(&self, ssrc: u32, gain: f32) {
        self.gains.lock().insert(ssrc, gain);
    }

    pub fn remove(&self, ssrc: u32) {
        self.streams.lock().remove(&ssrc);
    }

    pub fn clear(&self) {
        self.streams.lock().clear();
    }

    /// Fills `out` (interleaved stereo at 48 kHz) with the mix.
    fn mix(&self, out: &mut [f32]) {
        out.fill(0.0);
        let gains = self.gains.lock();
        let mut streams = self.streams.lock();
        streams.retain(|_, s| s.last_arrival.elapsed() < Duration::from_secs(30));
        let mut scratch = [0f32; FRAME * 2 * 6];
        for (ssrc, s) in streams.iter_mut() {
            let g = gains.get(ssrc).copied().unwrap_or(1.0);
            // Decode ahead so HOLD of real sound is always left over: if the speaker stops, that's what fades.
            while s.pcm.len() < out.len() + HOLD && s.decode_next(&mut scratch) {}
            let n = out.len().min(s.pcm.len());
            for (o, v) in out[..n].iter_mut().zip(s.pcm.drain(..n)) {
                *o += v * g;
            }
        }
        for o in out.iter_mut() {
            *o = soft_clip(*o);
        }
    }
}

/// Leaves normal levels alone and bends loud peaks (several people talking at once, or volume above 100%)
/// smoothly towards ±1, instead of squaring them off into crackle.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

/// Reports device errors to the user, except buffer under/overruns: those mean the system was briefly too
/// busy to service the device (common on real hardware too), cost at most a tiny glitch and recover on their
/// own, so they're only logged.
fn device_errors(what: &'static str, errors: SyncSender<String>) -> impl FnMut(cpal::Error) + Send + 'static {
    let mut xruns = 0u64;
    move |e: cpal::Error| {
        if e.kind() == cpal::ErrorKind::Xrun {
            xruns += 1;
            // In the log (1st, 2nd, 4th, 8th… time), so a tester's log shows whether glitches they heard were the
            // system falling behind rather than the network.
            if xruns.is_power_of_two() {
                tracing::info!("{what}: buffer under/overrun ({xruns} so far)");
            }
            return;
        }
        tracing::warn!("{what} error: {e}");
        let _ = errors.try_send(format!("{what} error: {e}"));
    }
}

pub struct AudioIo {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Devices {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// Input and output device names, for settings.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let name = |d: cpal::Device| d.description().ok().map(|n| n.name().to_owned());
    let inputs = host.input_devices().map(|i| i.filter_map(name).collect()).unwrap_or_default();
    let outputs = host.output_devices().map(|i| i.filter_map(name).collect()).unwrap_or_default();
    (inputs, outputs)
}

fn pick(devices: impl Iterator<Item = cpal::Device>, name: Option<&str>, default: Option<cpal::Device>) -> Option<cpal::Device> {
    if let Some(n) = name {
        for d in devices {
            if d.description().is_ok_and(|desc| desc.name() == n) {
                return Some(d);
            }
        }
    }
    default
}

/// Starts capture (sending mono 48 kHz chunks to `mic`, when given) and playback (pulling from `mixer`).
/// Either side may be unavailable; that's reported and the other keeps working.
pub fn start(devices: Devices, mic: Option<SyncSender<Vec<f32>>>, mixer: Arc<Mixer>, deafened: Arc<AtomicBool>) -> (AudioIo, Receiver<String>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (err_tx, err_rx) = std::sync::mpsc::sync_channel(8);
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("swiftcord-audio".into())
        .spawn(move || {
            let host = cpal::default_host();
            let input = mic.and_then(|mic| host
                .input_devices()
                .ok()
                .and_then(|d| pick(d, devices.input.as_deref(), host.default_input_device()))
                .and_then(|d| build_input(&d, mic, err_tx.clone()).map_err(|e| { let _ = err_tx.try_send(format!("Microphone unavailable: {e}")); }).ok()));
            let output = host
                .output_devices()
                .ok()
                .and_then(|d| pick(d, devices.output.as_deref(), host.default_output_device()))
                .and_then(|d| build_output(&d, mix_fill(mixer, deafened), err_tx.clone()).map_err(|e| { let _ = err_tx.try_send(format!("Speakers unavailable: {e}")); }).ok());
            for st in input.iter().chain(output.iter()) {
                let _ = st.play();
            }
            while !s.load(Ordering::Relaxed) {
                std::thread::park_timeout(Duration::from_millis(100));
            }
            drop(input);
            drop(output);
        })
        .expect("audio thread");
    (AudioIo { stop, thread: Some(thread) }, err_rx)
}

fn build_input(device: &cpal::Device, mic: SyncSender<Vec<f32>>, errors: SyncSender<String>) -> Result<cpal::Stream, String> {
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    tracing::info!("microphone: {} ch @ {rate} Hz ({format:?})", channels);
    let mut resampler = Resampler::new(rate, SAMPLE_RATE, 1);
    let mut mono = Vec::new();
    let mut dropped = 0u64;
    let mut process = move |samples: &mut dyn Iterator<Item = f32>| {
        mono.clear();
        let mut acc = 0.0;
        for (i, s) in samples.enumerate() {
            acc += s;
            if i % channels == channels - 1 {
                mono.push(acc / channels as f32);
                acc = 0.0;
            }
        }
        let mut out = Vec::with_capacity(mono.len() * 2);
        resampler.process(&mono, &mut out);
        // If the sender falls behind, drop audio rather than block the device thread.
        if mic.try_send(out).is_err() {
            dropped += 1;
            if dropped % 50 == 1 {
                tracing::warn!("microphone audio dropped ({dropped} chunks): the sender is falling behind");
            }
        }
    };
    let err = device_errors("Microphone", errors);
    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(config, move |d: &[f32], _: &_| process(&mut d.iter().copied()), err, None),
        SampleFormat::I16 => device.build_input_stream(config, move |d: &[i16], _: &_| process(&mut d.iter().map(|&s| s as f32 / 32768.0)), err, None),
        SampleFormat::I32 => device.build_input_stream(config, move |d: &[i32], _: &_| process(&mut d.iter().map(|&s| s as f32 / 2147483648.0)), err, None),
        other => return Err(format!("unsupported sample format {other:?}")),
    };
    stream.map_err(|e| e.to_string())
}

fn mix_fill(mixer: Arc<Mixer>, deafened: Arc<AtomicBool>) -> impl FnMut(&mut [f32]) + Send + 'static {
    move |out| {
        if deafened.load(Ordering::Relaxed) {
            out.fill(0.0);
            mixer.clear();
        } else {
            mixer.mix(out);
        }
    }
}

/// Plays sound from somewhere other than a call (a video's soundtrack) on the chosen output device until
/// dropped. `fill` is asked for interleaved 48 kHz stereo, from the device's own thread.
pub fn play(output: Option<String>, fill: impl FnMut(&mut [f32]) + Send + 'static) -> Result<AudioIo, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("swiftcord-playback".into())
        .spawn(move || {
            let host = cpal::default_host();
            let (err_tx, _err_rx) = std::sync::mpsc::sync_channel(8);
            let stream = host
                .output_devices()
                .ok()
                .and_then(|d| pick(d, output.as_deref(), host.default_output_device()))
                .ok_or_else(|| "no output device".to_owned())
                .and_then(|d| build_output(&d, fill, err_tx));
            let stream = match stream.and_then(|st| st.play().map(|()| st).map_err(|e| e.to_string())) {
                Ok(st) => {
                    let _ = ready_tx.send(Ok(()));
                    st
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            while !s.load(Ordering::Relaxed) {
                std::thread::park_timeout(Duration::from_millis(100));
            }
            drop(stream);
        })
        .map_err(|e| e.to_string())?;
    let io = AudioIo { stop, thread: Some(thread) };
    ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(io)
}

/// Turns `fill` (48 kHz stereo) into a function giving `frames` device frames as interleaved stereo at `rate`.
///
/// Sound is pulled in small fixed chunks and whatever the device doesn't take yet is kept for the next
/// callback, so no sample is ever dropped or repeated (that used to click every callback).
fn renderer(rate: u32, mut fill: impl FnMut(&mut [f32]) + Send + 'static) -> impl FnMut(usize) -> Vec<f32> + Send + 'static {
    const CHUNK: usize = SAMPLE_RATE as usize / 200;
    let mut resampler = Resampler::new(SAMPLE_RATE, rate, 2);
    let mut mixed = vec![0f32; CHUNK * 2];
    let mut pending: Vec<f32> = Vec::new();
    move |frames| {
        while pending.len() < frames * 2 {
            fill(&mut mixed);
            resampler.process(&mixed, &mut pending);
        }
        let rest = pending.split_off(frames * 2);
        std::mem::replace(&mut pending, rest)
    }
}

fn build_output(device: &cpal::Device, fill: impl FnMut(&mut [f32]) + Send + 'static, errors: SyncSender<String>) -> Result<cpal::Stream, String> {
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    tracing::info!("speakers: {} ch @ {rate} Hz ({format:?})", channels);
    let mut render = renderer(rate, fill);
    let write = move |out: &mut dyn FnMut(usize, f32), frames: usize, render: &mut dyn FnMut(usize) -> Vec<f32>| {
        let st = render(frames);
        for f in 0..frames {
            let (l, r) = (st[f * 2], st[f * 2 + 1]);
            match channels {
                1 => out(f, (l + r) / 2.0),
                _ => {
                    out(f * channels, l);
                    out(f * channels + 1, r);
                    for c in 2..channels {
                        out(f * channels + c, 0.0);
                    }
                }
            }
        }
    };
    let err = device_errors("Speaker", errors);
    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |d: &mut [f32], _: &_| {
                let frames = d.len() / channels;
                write(&mut |i, v| d[i] = v, frames, &mut render);
            },
            err,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |d: &mut [i16], _: &_| {
                let frames = d.len() / channels;
                write(&mut |i, v| d[i] = (v * 32767.0) as i16, frames, &mut render);
            },
            err,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    };
    stream.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_preserves_duration() {
        let mut r = Resampler::new(44_100, 48_000, 1);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.process(&vec![0.5; 441], &mut out);
        }
        // One second in, one second (±1 sample) out.
        assert!((out.len() as i64 - 48_000).abs() <= 1, "{}", out.len());
        // Interpolation runs one input sample behind (from the previous block's last sample), so only the first
        // couple of outputs ramp up from silence.
        assert!(out.iter().skip(2).all(|&s| (s - 0.5).abs() < 1e-6));
    }

    fn packets(n: usize) -> Vec<Vec<u8>> {
        let mut enc = crate::codec::Encoder::new(64_000).unwrap();
        let mut buf = [0u8; 1400];
        (0..n)
            .map(|f| {
                let pcm: Vec<f32> = (0..FRAME).map(|i| ((f * FRAME + i) as f32 * 0.06).sin() * 0.5).collect();
                let k = enc.encode(&pcm, &mut buf).unwrap();
                buf[..k].to_vec()
            })
            .collect()
    }

    fn energy(out: &[f32]) -> f32 {
        out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32
    }

    #[test]
    fn mixer_buffers_then_plays_and_conceals_loss() {
        let m = Mixer::default();
        let p = packets(8);
        let mut out = vec![0.0; FRAME * 2];
        m.push(1, 10, p[0].clone());
        m.push(1, 11, p[1].clone());
        m.push(1, 12, p[2].clone());
        m.mix(&mut out);
        assert_eq!(energy(&out), 0.0, "waits for the jitter buffer to fill");
        m.push(1, 13, p[3].clone());
        m.mix(&mut out);
        m.mix(&mut out);
        assert!(energy(&out) > 0.01, "plays once buffered");
        // Packet 15 is lost; 14 and 16 arrive. The gap is concealed rather than silent.
        m.push(1, 14, p[4].clone());
        m.push(1, 16, p[6].clone());
        m.push(1, 17, p[7].clone());
        for _ in 0..3 {
            m.mix(&mut out);
            assert!(energy(&out) > 0.001);
        }
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut s = Stream::new().unwrap();
        s.push(65534, vec![1]);
        s.push(65535, vec![2]);
        s.push(0, vec![3]);
        s.push(1, vec![4]);
        let keys: Vec<u64> = s.packets.keys().copied().collect();
        assert!(keys.windows(2).all(|w| w[1] == w[0] + 1), "{keys:?}");
    }

    #[test]
    fn underrun_while_talking_deepens_the_buffer() {
        let m = Mixer::default();
        let p = packets(5);
        let mut out = vec![0.0; FRAME * 2];
        for (i, pk) in p.iter().take(4).enumerate() {
            m.push(1, i as u16, pk.clone());
        }
        for _ in 0..8 {
            m.mix(&mut out);
        }
        // Still "talking" (a packet just arrived) but nothing to play.
        m.push(1, 100, p[4].clone());
        let target = m.streams.lock()[&1].target;
        assert!(target > 4, "target {target}");
    }

    #[test]
    fn running_dry_fades_out_instead_of_cutting_off() {
        let m = Mixer::default();
        for (i, pk) in packets(4).into_iter().enumerate() {
            m.push(1, i as u16, pk);
        }
        // They've stopped sending (rather than a packet being merely late).
        std::thread::sleep(Duration::from_millis(100));
        // Four frames of audio whose last 5 ms fade out, then silence.
        let mut out = vec![0.0; FRAME * 2 * 5];
        m.mix(&mut out);
        let last = &out[FRAME * 2 * 3..FRAME * 2 * 4];
        assert!(energy(&last[..FRAME]) > 0.01, "plays to the end");
        assert!(last[last.len() - 2..].iter().all(|s| s.abs() < 0.01), "ends near silence");
        assert_eq!(energy(&out[FRAME * 2 * 4..]), 0.0, "then stops");
    }

    #[test]
    fn a_new_talk_spurt_ramps_up_from_silence() {
        let m = Mixer::default();
        let p = packets(12);
        for (i, pk) in p.iter().take(4).enumerate() {
            m.push(1, i as u16, pk.clone());
        }
        std::thread::sleep(Duration::from_millis(100));
        let mut out = vec![0.0; FRAME * 2 * 6];
        m.mix(&mut out);
        // They start talking again mid-waveform (the test tone doesn't start at zero).
        for (i, pk) in p.iter().enumerate().skip(6) {
            m.push(1, i as u16, pk.clone());
        }
        let mut out = vec![0.0; FRAME * 2];
        m.mix(&mut out);
        assert!(out[..8].iter().all(|s| s.abs() < 0.02), "starts near silence: {:?}", &out[..8]);
        assert!(energy(&out[FRAME..]) > 0.01, "then plays at full level");
    }

    /// The largest jump between neighbouring samples (a click shows up as a big one).
    fn max_step(out: &[f32]) -> f32 {
        out.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn silence_frames_end_a_spurt_with_a_fade_and_the_next_one_ramps_up() {
        let m = Mixer::default();
        let p = packets(12);
        for i in 0..4 {
            m.push(1, i, p[i as usize].clone());
        }
        for i in 4..9 {
            m.push(1, i, SILENCE.to_vec());
        }
        // Speech again straight after the silence frames (no gap in sequence numbers).
        for i in 9..13 {
            m.push(1, i, p[i as usize - 1].clone());
        }
        let mut out = vec![0.0; FRAME * 2 * 13];
        m.mix(&mut out);
        let frame = |k: usize| &out[FRAME * 2 * k..FRAME * 2 * (k + 1)];
        // Four frames of speech fading out at the end, then the silence frames as silence.
        assert!(energy(frame(3)) > 0.01);
        assert!(frame(3)[FRAME * 2 - 2..].iter().all(|s| s.abs() < 0.01), "fades to silence");
        assert!((4..9).all(|k| energy(frame(k)) == 0.0), "silence frames play as silence");
        // The next spurt starts from silence rather than jumping in.
        assert!(frame(9)[..8].iter().all(|s| s.abs() < 0.02), "{:?}", &frame(9)[..8]);
        assert!(energy(frame(10)) > 0.01);
        // And nowhere does the sound jump: the test tone moves at most ~0.03 per sample.
        assert!(max_step(&out) < 0.1, "largest step {}", max_step(&out));
    }

    #[test]
    fn running_dry_mid_word_fades_instead_of_inventing_speech() {
        let m = Mixer::default();
        let p = packets(4);
        for (i, pk) in p.iter().enumerate() {
            m.push(1, i as u16, pk.clone());
        }
        // Packets stop arriving (but only just: this used to be concealed for up to 60 ms first).
        let mut out = vec![0.0; FRAME * 2 * 7];
        m.mix(&mut out);
        let tail = &out[FRAME * 2 * 4..];
        assert_eq!(energy(tail), 0.0, "silent once the real sound has faded");
        assert!(max_step(&out) < 0.1, "largest step {}", max_step(&out));
    }

    /// A continuous 220 Hz tone, 0.3 loud, as Opus packets (its natural step is under 0.01 per sample).
    fn steady(n: usize) -> Vec<Vec<u8>> {
        let mut enc = crate::codec::Encoder::new(64_000).unwrap();
        let mut buf = [0u8; 1400];
        (0..n)
            .map(|f| {
                let pcm: Vec<f32> = (0..FRAME).map(|i| ((f * FRAME + i) as f32 * 220.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3).collect();
                let k = enc.encode(&pcm, &mut buf).unwrap();
                buf[..k].to_vec()
            })
            .collect()
    }

    /// Plays `schedule` (per 20 ms tick: which packets arrive) through a mixer and returns the left channel.
    fn play(schedule: &[Vec<(u16, Vec<u8>)>], ticks: usize, per_tick: usize) -> Vec<f32> {
        let m = Mixer::default();
        let mut left = Vec::new();
        let mut out = vec![0.0; per_tick * 2];
        for t in 0..ticks {
            for (seq, p) in schedule.get(t).into_iter().flatten() {
                m.push(1, *seq, p.clone());
            }
            m.mix(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
        }
        left
    }

    fn worst_step(left: &[f32]) -> (f32, usize) {
        left.windows(2).enumerate().map(|(i, w)| ((w[1] - w[0]).abs(), i)).fold((0.0, 0), |a, b| if b.0 > a.0 { b } else { a })
    }

    /// Voice-like audio: a 150 Hz voice with harmonics, a little vibrato, and syllable-like swells.
    fn voicey(n: usize, bitrate: i32) -> Vec<Vec<u8>> {
        let mut enc = crate::codec::Encoder::new(bitrate).unwrap();
        let mut buf = [0u8; 1400];
        let tau = std::f32::consts::TAU;
        (0..n)
            .map(|f| {
                let pcm: Vec<f32> = (0..FRAME)
                    .map(|i| {
                        let t = (f * FRAME + i) as f32 / 48_000.0;
                        let pitch = 150.0 + 8.0 * (tau * 5.0 * t).sin();
                        let swell = 0.55 + 0.45 * (tau * 3.0 * t).sin();
                        let voice: f32 = (1..=6).map(|h| (tau * pitch * h as f32 * t).sin() / h as f32).sum();
                        voice * swell * 0.2
                    })
                    .collect();
                let k = enc.encode(&pcm, &mut buf).unwrap();
                buf[..k].to_vec()
            })
            .collect()
    }

    /// Plays `packets` with the ones `lost` (by index) missing, and returns the worst step relative to the
    /// same audio with nothing lost, and how loud the lost stretches are relative to normal.
    fn loss_effect(packets: &[Vec<u8>], lost: impl Fn(usize) -> bool) -> (f32, f32) {
        let n = packets.len();
        let clean: Vec<_> = (0..n).map(|i| vec![(i as u16, packets[i].clone())]).collect();
        let lossy: Vec<_> = (0..n).map(|i| if lost(i) { vec![] } else { vec![(i as u16, packets[i].clone())] }).collect();
        let (a, b) = (play(&clean, n, FRAME), play(&lossy, n, FRAME));
        let skip = FRAME * 10;
        let base = worst_step(&a[skip..]).0;
        let worst = worst_step(&b[skip..]).0;
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
        (worst / base, rms(&b[skip..]) / rms(&a[skip..]))
    }

    #[test]
    fn lost_packets_dont_click() {
        // A tone: Opus's own concealment burst to three times its level here (a loud click).
        let (step, level) = loss_effect(&steady(400), |i| i % 50 == 25);
        assert!(step < 1.5, "worst step {step:.2}x normal");
        assert!(level > 0.9, "level {level:.2}x");
        // Voice-like audio in voice mode, which carries copies of lost frames, and in music mode.
        for bitrate in [24_000, 64_000] {
            let (step, level) = loss_effect(&voicey(400, bitrate), |i| i % 10 == 5);
            assert!(step < 1.5, "{bitrate} bps: worst step {step:.2}x normal");
            assert!(level > 0.85, "{bitrate} bps: level {level:.2}x");
            let (step, _) = loss_effect(&voicey(400, bitrate), |i| i % 60 >= 30 && i % 60 < 33);
            assert!(step < 1.5, "{bitrate} bps, bursts: worst step {step:.2}x normal");
        }
    }

    #[test]
    fn talk_spurts_from_one_encoder_start_and_stop_cleanly() {
        // As official clients send: one encoder running throughout; each spurt ends with five silence frames,
        // then nothing for a while (sequence numbers carry on), then speech again.
        let p = steady(400);
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = Vec::new();
        let (mut seq, mut frame) = (0u16, 0usize);
        for _ in 0..4 {
            for _ in 0..40 {
                sched.push(vec![(seq, p[frame].clone())]);
                seq = seq.wrapping_add(1);
                frame += 1;
            }
            for _ in 0..5 {
                sched.push(vec![(seq, SILENCE.to_vec())]);
                seq = seq.wrapping_add(1);
            }
            // A pause: the encoder keeps running on silence, nothing is sent.
            frame += 25;
            sched.extend(std::iter::repeat_n(Vec::new(), 25));
        }
        let left = play(&sched, sched.len(), FRAME);
        let (step, at) = worst_step(&left);
        assert!(step < 0.015, "worst step {step} at {:.2} s", at as f32 / 48_000.0);
    }

    #[test]
    fn a_fast_sender_is_caught_up_without_a_jump() {
        // Their clock 2% fast for 30 s: packets pile up, which used to end in an abrupt cut (a pop) once
        // half a second had built up.
        let p = steady(1600);
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = vec![Vec::new(); 1500];
        let mut seq = 0usize;
        for (t, slot) in sched.iter_mut().enumerate() {
            slot.push((seq as u16, p[seq].clone()));
            seq += 1;
            if t % 50 == 49 {
                slot.push((seq as u16, p[seq].clone()));
                seq += 1;
            }
        }
        let m = Mixer::default();
        let mut left = Vec::new();
        let mut out = vec![0.0; FRAME * 2];
        let mut most = 0;
        for slot in &sched {
            for (s, pk) in slot {
                m.push(1, *s, pk.clone());
            }
            m.mix(&mut out);
            left.extend(out.chunks(2).map(|f| f[0]));
            most = most.max(m.streams.lock()[&1].packets.len());
        }
        let (step, _) = worst_step(&left[FRAME * 10..]);
        assert!(step < 0.015, "worst step {step}");
        assert!(most <= 4 + CATCH_UP_AT + 2, "fell {most} packets behind");
    }

    #[test]
    #[ignore = "diagnostic: prints how losses sound with voice-like audio"]
    fn voice_loss_scenarios() {
        for bitrate in [24_000, 64_000] {
            let p = voicey(600, bitrate);
            let mode = if crate::codec::carries_fec(&p[300]) { "voice mode" } else { "music mode" };
            for (name, lost) in [
                ("2% single losses", Box::new(|i: usize| i % 50 == 25) as Box<dyn Fn(usize) -> bool>),
                ("10% single losses", Box::new(|i: usize| i % 10 == 5)),
                ("bursts of 3 lost", Box::new(|i: usize| i % 60 >= 30 && i % 60 < 33)),
            ] {
                let (step, level) = loss_effect(&p, lost);
                println!("{bitrate} bps ({mode}), {name:<20} worst step {step:.2}x normal, level {level:.2}x");
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: prints the worst pop in each network scenario"]
    fn pop_scenarios() {
        let p = steady(3100);
        let report = |name: &str, left: &[f32]| {
            let (step, at) = worst_step(&left[FRAME * 10..]);
            println!("{name:<40} worst step {step:.4} at {:.2} s", (at + FRAME * 10) as f32 / 48_000.0);
        };
        // Steady, every packet on time.
        let sched: Vec<_> = (0..600).map(|i| vec![(i as u16, p[i].clone())]).collect();
        report("steady", &play(&sched, 600, FRAME));
        // One packet in 50 lost.
        let sched: Vec<_> = (0..600).map(|i| if i % 50 == 25 { vec![] } else { vec![(i as u16, p[i].clone())] }).collect();
        let left = play(&sched, 600, FRAME);
        report("2% loss", &left);
        let (_, at) = worst_step(&left[FRAME * 10..]);
        let at = at + FRAME * 10;
        println!("  at frame {} sample {}; around: {:?}", at / FRAME, at % FRAME, &left[at - 3..at + 4].iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>());
        // The sender's clock 1% fast: an extra packet every 100.
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = vec![Vec::new(); 3000];
        let mut seq = 0usize;
        for (t, slot) in sched.iter_mut().enumerate() {
            slot.push((seq as u16, p[seq].clone()));
            seq += 1;
            if t % 100 == 99 {
                slot.push((seq as u16, p[seq].clone()));
                seq += 1;
            }
        }
        report("sender 1% fast (60 s)", &play(&sched[..2900], 2900, FRAME));
        // The sender's clock 1% slow: a packet missing every 100 ticks (but nothing lost).
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = vec![Vec::new(); 3000];
        let mut seq = 0usize;
        for (t, slot) in sched.iter_mut().enumerate() {
            if t % 100 != 99 {
                slot.push((seq as u16, p[seq].clone()));
                seq += 1;
            }
        }
        report("sender 1% slow (60 s)", &play(&sched, 3000, FRAME));
        // A 600 ms stall, then everything at once.
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = (0..600).map(|i| vec![(i as u16, p[i].clone())]).collect();
        let held: Vec<_> = (100..130).flat_map(|t| std::mem::take(&mut sched[t])).collect();
        sched[130].extend(held);
        report("600 ms stall then burst", &play(&sched, 600, FRAME));
        // Neighbouring packets swapped now and then.
        let mut sched: Vec<Vec<(u16, Vec<u8>)>> = (0..600).map(|i| vec![(i as u16, p[i].clone())]).collect();
        for t in (20..590).step_by(37) {
            sched.swap(t, t + 1);
        }
        report("reordering", &play(&sched, 600, FRAME));
        // A device that asks for 10 ms at a time.
        let sched: Vec<_> = (0..1200).map(|i| if i % 2 == 0 { vec![((i / 2) as u16, p[i / 2].clone())] } else { vec![] }).collect();
        report("10 ms callbacks", &play(&sched, 1200, FRAME / 2));
    }

    #[test]
    fn output_keeps_every_sample_whatever_the_callback_size() {
        for rate in [48_000, 44_100] {
            let mut n = 0u32;
            // A ramp, so a dropped or repeated sample shows up as a step that isn't 1.
            let mut render = renderer(rate, move |out: &mut [f32]| {
                for f in out.chunks_mut(2) {
                    n += 1;
                    f.fill(n as f32);
                }
            });
            let mut all = Vec::new();
            for frames in [441, 480, 7, 1024, 333, 480, 480] {
                let st = render(frames);
                assert_eq!(st.len(), frames * 2);
                all.extend(st.chunks(2).map(|f| f[0]));
            }
            let step = SAMPLE_RATE as f32 / rate as f32;
            assert!(all.windows(2).skip(2).all(|w| (w[1] - w[0] - step).abs() < 1e-3), "{rate} Hz: {:?}", &all[..8]);
        }
    }

    #[test]
    fn soft_clip_is_transparent_then_bounded() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.8), -0.8);
        assert!(soft_clip(3.0) <= 1.0 && soft_clip(3.0) > 0.95);
        assert!(soft_clip(-3.0) >= -1.0);
        assert!(soft_clip(0.9) > soft_clip(0.85));
    }
}
