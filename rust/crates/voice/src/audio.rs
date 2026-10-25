//! Microphone capture and speaker playback through WASAPI (cpal), converted to and from 48 kHz.
//!
//! cpal streams aren't `Send` on every platform, so both live on one dedicated thread that owns them until
//! the call ends. Capture hands mono 48 kHz samples to the sender over a channel; playback pulls from the
//! [`Mixer`] that the receiver fills.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use parking_lot::Mutex;

use crate::codec::SAMPLE_RATE;

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

/// One remote speaker's decoded audio (48 kHz interleaved stereo) waiting to be played.
struct Source {
    buf: VecDeque<f32>,
    /// Playback starts after a little audio has buffered, to absorb network jitter.
    primed: bool,
    last_push: Instant,
}

/// Mixes every remote speaker into the output.
#[derive(Default)]
pub struct Mixer {
    sources: Mutex<HashMap<u32, Source>>,
    /// Per-SSRC gain, set from per-user volume.
    gains: Mutex<HashMap<u32, f32>>,
}

/// 60 ms of stereo audio before starting a source; at most 300 ms queued (older audio is dropped).
const PRIME: usize = 960 * 2 * 3;
const MAX_QUEUE: usize = 960 * 2 * 15;

impl Mixer {
    pub fn push(&self, ssrc: u32, stereo: &[f32]) {
        let mut sources = self.sources.lock();
        let s = sources.entry(ssrc).or_insert_with(|| Source { buf: VecDeque::new(), primed: false, last_push: Instant::now() });
        s.buf.extend(stereo);
        s.last_push = Instant::now();
        let excess = s.buf.len().saturating_sub(MAX_QUEUE);
        s.buf.drain(..excess);
    }

    pub fn set_gain(&self, ssrc: u32, gain: f32) {
        self.gains.lock().insert(ssrc, gain);
    }

    pub fn remove(&self, ssrc: u32) {
        self.sources.lock().remove(&ssrc);
    }

    pub fn clear(&self) {
        self.sources.lock().clear();
    }

    /// Fills `out` (interleaved stereo at 48 kHz) with the mix.
    fn mix(&self, out: &mut [f32]) {
        out.fill(0.0);
        let gains = self.gains.lock().clone();
        let mut sources = self.sources.lock();
        sources.retain(|_, s| s.last_push.elapsed() < Duration::from_secs(5) || !s.buf.is_empty());
        for (ssrc, s) in sources.iter_mut() {
            if !s.primed {
                if s.buf.len() < PRIME {
                    continue;
                }
                s.primed = true;
            }
            let g = gains.get(ssrc).copied().unwrap_or(1.0);
            let n = out.len().min(s.buf.len());
            for (o, v) in out.iter_mut().zip(s.buf.drain(..n)) {
                *o += v * g;
            }
            if s.buf.is_empty() {
                // Underrun: wait for a fresh cushion rather than stuttering sample by sample.
                s.primed = false;
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

/// Starts capture (sending mono 48 kHz chunks to `mic`) and playback (pulling from `mixer`).
/// Either side may be unavailable; that's reported and the other keeps working.
pub fn start(devices: Devices, mic: SyncSender<Vec<f32>>, mixer: Arc<Mixer>, deafened: Arc<AtomicBool>) -> (AudioIo, Receiver<String>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (err_tx, err_rx) = std::sync::mpsc::sync_channel(8);
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("swiftcord-audio".into())
        .spawn(move || {
            let host = cpal::default_host();
            let input = host
                .input_devices()
                .ok()
                .and_then(|d| pick(d, devices.input.as_deref(), host.default_input_device()))
                .and_then(|d| build_input(&d, mic, err_tx.clone()).map_err(|e| { let _ = err_tx.try_send(format!("Microphone unavailable: {e}")); }).ok());
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
        let _ = mic.try_send(out);
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

    #[test]
    fn mixer_primes_then_mixes() {
        let m = Mixer::default();
        let mut out = vec![0.0; 960 * 2];
        m.push(1, &vec![0.25; 960 * 2]);
        m.mix(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "not primed yet");
        m.push(1, &vec![0.25; PRIME]);
        m.push(2, &vec![0.25; PRIME + 960 * 2]);
        m.mix(&mut out);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }
}
