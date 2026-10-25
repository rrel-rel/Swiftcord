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

use crate::codec::{Decoder, FRAME, SAMPLE_RATE};

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
/// Packets wait until `target` frames are buffered, then play in sequence order. A missing packet is
/// concealed by the decoder (packet-loss concealment) instead of leaving a gap; if the buffer runs dry while
/// the speaker is still sending, `target` grows so the next run has more cushion, and shrinks again after a
/// stretch without trouble.
struct Stream {
    decoder: Decoder,
    packets: BTreeMap<u64, Vec<u8>>,
    /// Highest extended sequence number seen, for unwrapping 16-bit sequence numbers.
    highest: Option<u64>,
    next: u64,
    pcm: VecDeque<f32>,
    playing: bool,
    target: usize,
    concealed: u32,
    last_arrival: Instant,
    last_underrun: Instant,
}

const MIN_TARGET: usize = 2;
const MAX_TARGET: usize = 12;
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
            target: 3,
            concealed: 0,
            last_arrival: Instant::now(),
            last_underrun: Instant::now(),
        })
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
        if self.playing && ext < self.next {
            return; // Too late; already concealed.
        }
        self.highest = Some(self.highest.map_or(ext, |h| h.max(ext)));
        self.packets.insert(ext, opus);
        if self.packets.len() > self.target + MAX_BACKLOG {
            // Far behind (e.g. after a stall): drop down to the target and carry on from there.
            while self.packets.len() > self.target {
                self.packets.pop_first();
            }
            self.next = *self.packets.keys().next().unwrap();
            self.pcm.clear();
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
        let n = if let Some(p) = self.packets.remove(&self.next) {
            self.concealed = 0;
            self.decoder.decode(Some(&p), scratch)
        } else if !self.packets.is_empty() || (self.concealed < 3 && self.last_arrival.elapsed() < Duration::from_millis(80)) {
            // Lost or late: conceal. After several concealed frames with later packets waiting, skip ahead.
            self.concealed += 1;
            if self.concealed > 5 {
                if let Some(&first) = self.packets.keys().next() {
                    self.next = first;
                    self.concealed = 0;
                    return true;
                }
            }
            // Conceal exactly one frame (the decoder would otherwise fill the whole buffer).
            self.decoder.decode(None, &mut scratch[..FRAME * 2])
        } else {
            // Ran dry. If they're still talking, the buffer was too shallow: deepen it.
            self.playing = false;
            if self.last_arrival.elapsed() < Duration::from_millis(60) {
                self.target = (self.target + 1).min(MAX_TARGET);
                self.last_underrun = Instant::now();
            }
            return false;
        };
        self.next += 1;
        if self.last_underrun.elapsed() > Duration::from_secs(15) && self.target > MIN_TARGET {
            self.target -= 1;
            self.last_underrun = Instant::now();
        }
        self.pcm.extend(&scratch[..n]);
        true
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
        let gains = self.gains.lock().clone();
        let mut streams = self.streams.lock();
        streams.retain(|_, s| s.last_arrival.elapsed() < Duration::from_secs(30));
        let mut scratch = [0f32; FRAME * 2 * 6];
        for (ssrc, s) in streams.iter_mut() {
            let g = gains.get(ssrc).copied().unwrap_or(1.0);
            let mut i = 0;
            while i < out.len() {
                if s.pcm.is_empty() && !s.decode_next(&mut scratch) {
                    break;
                }
                let n = (out.len() - i).min(s.pcm.len());
                for (o, v) in out[i..i + n].iter_mut().zip(s.pcm.drain(..n)) {
                    *o += v * g;
                }
                i += n;
            }
        }
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
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
                .and_then(|d| build_output(&d, mixer, deafened, err_tx.clone()).map_err(|e| { let _ = err_tx.try_send(format!("Speakers unavailable: {e}")); }).ok());
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
    let err = move |e: cpal::Error| {
        let _ = errors.try_send(format!("Microphone error: {e}"));
    };
    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(config, move |d: &[f32], _: &_| process(&mut d.iter().copied()), err, None),
        SampleFormat::I16 => device.build_input_stream(config, move |d: &[i16], _: &_| process(&mut d.iter().map(|&s| s as f32 / 32768.0)), err, None),
        SampleFormat::I32 => device.build_input_stream(config, move |d: &[i32], _: &_| process(&mut d.iter().map(|&s| s as f32 / 2147483648.0)), err, None),
        other => return Err(format!("unsupported sample format {other:?}")),
    };
    stream.map_err(|e| e.to_string())
}

fn build_output(device: &cpal::Device, mixer: Arc<Mixer>, deafened: Arc<AtomicBool>, errors: SyncSender<String>) -> Result<cpal::Stream, String> {
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    tracing::info!("speakers: {} ch @ {rate} Hz ({format:?})", channels);
    let mut resampler = Resampler::new(SAMPLE_RATE, rate, 2);
    let mut mixed: Vec<f32> = Vec::new();
    let mut stereo: Vec<f32> = Vec::new();
    // Fills `frames` device frames as interleaved stereo at the device rate.
    let mut render = move |frames: usize| -> Vec<f32> {
        let need = ((frames as f64) * SAMPLE_RATE as f64 / rate as f64).ceil() as usize + 1;
        mixed.resize(need * 2, 0.0);
        if deafened.load(Ordering::Relaxed) {
            mixed.fill(0.0);
            mixer.clear();
        } else {
            mixer.mix(&mut mixed);
        }
        stereo.clear();
        resampler.process(&mixed, &mut stereo);
        stereo.resize(frames * 2, 0.0);
        stereo.clone()
    };
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
    let err = move |e: cpal::Error| {
        let _ = errors.try_send(format!("Speaker error: {e}"));
    };
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
        m.mix(&mut out);
        assert_eq!(energy(&out), 0.0, "waits for the jitter buffer to fill");
        m.push(1, 11, p[1].clone());
        m.push(1, 12, p[2].clone());
        m.mix(&mut out);
        m.mix(&mut out);
        assert!(energy(&out) > 0.01, "plays once buffered");
        // Packet 14 is lost; 13 and 15 arrive. The gap is concealed rather than silent.
        m.push(1, 13, p[3].clone());
        m.push(1, 15, p[5].clone());
        m.push(1, 16, p[6].clone());
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
        let p = packets(4);
        let mut out = vec![0.0; FRAME * 2];
        for (i, pk) in p.iter().take(3).enumerate() {
            m.push(1, i as u16, pk.clone());
        }
        for _ in 0..8 {
            m.mix(&mut out);
        }
        // Still "talking" (a packet just arrived) but nothing to play.
        m.push(1, 100, p[3].clone());
        let target = m.streams.lock()[&1].target;
        assert!(target > 3, "target {target}");
    }
}
