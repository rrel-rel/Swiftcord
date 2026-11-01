//! Microphone frames → Opus packets, gated by voice activity. Shared by calls and the mic test.

use crate::codec::{Encoder, FRAME};

/// Consecutive loud frames needed to open the gate, so brief noise doesn't.
const ATTACK: u32 = 2;
/// Frames the gate stays open after the last loud one (400 ms), so speech isn't chopped between words.
const HANGOVER: u32 = 20;
/// Opus silence frames sent after a talk spurt so receivers don't interpolate into the gap.
const TRAILING_SILENCE: u32 = 5;

pub enum Output {
    /// Packets to send (two when the gate opens: the frame before it opened, and this one).
    Audio(Vec<Vec<u8>>),
    Silence,
    Nothing,
}

pub struct Transmitter {
    encoder: Encoder,
    open: bool,
    loud_run: u32,
    hangover: u32,
    silence_left: u32,
    /// The previous frame, sent as pre-roll when the gate opens so the first syllable isn't clipped.
    previous: Vec<f32>,
    /// The latest frame's level, for meters.
    pub level_db: f32,
    /// RNNoise, when noise suppression is on.
    denoiser: Option<Box<nnnoiseless::DenoiseState<'static>>>,
    denoise_buf: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
}

/// The level of a frame in dBFS.
pub fn level_db(frame: &[f32]) -> f32 {
    let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-9).log10()
}

fn fade(frame: &mut [f32], fade_in: bool) {
    let n = frame.len() as f32;
    for (i, s) in frame.iter_mut().enumerate() {
        let t = i as f32 / n;
        *s *= if fade_in { t } else { 1.0 - t };
    }
}

impl Transmitter {
    pub fn new(bitrate: i32) -> Option<Self> {
        Some(Self {
            encoder: Encoder::new(bitrate)?,
            open: false,
            loud_run: 0,
            hangover: 0,
            silence_left: 0,
            previous: vec![0.0; FRAME],
            level_db: -100.0,
            denoiser: None,
            denoise_buf: [0.0; nnnoiseless::DenoiseState::FRAME_SIZE],
        })
    }

    /// Turns noise suppression on or off (cheap to call every frame).
    pub fn set_noise_suppression(&mut self, on: bool) {
        match (on, self.denoiser.is_some()) {
            (true, false) => self.denoiser = Some(nnnoiseless::DenoiseState::new()),
            (false, true) => self.denoiser = None,
            _ => {}
        }
    }

    /// Removes steady background noise (fans, hum, keyboards…) in place, with RNNoise.
    fn denoise(&mut self, frame: &mut [f32]) {
        let Some(d) = self.denoiser.as_mut() else { return };
        // RNNoise works on 10 ms at 48 kHz, at 16-bit sample scale.
        for chunk in frame.chunks_mut(nnnoiseless::DenoiseState::FRAME_SIZE) {
            let input: Vec<f32> = chunk.iter().map(|s| s * 32768.0).collect();
            let n = input.len();
            if n < nnnoiseless::DenoiseState::FRAME_SIZE {
                return;
            }
            d.process_frame(&mut self.denoise_buf, &input);
            for (s, o) in chunk.iter_mut().zip(&self.denoise_buf[..n]) {
                *s = o / 32768.0;
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Processes one mono 20 ms frame. The second value reports the gate opening (`Some(true)`) or the
    /// talk spurt fully ending (`Some(false)`, after the trailing silence).
    pub fn process(&mut self, mut frame: Vec<f32>, muted: bool, threshold_db: f32) -> (Output, Option<bool>) {
        debug_assert_eq!(frame.len(), FRAME);
        // Before the gate, so background noise alone doesn't open it.
        self.denoise(&mut frame);
        self.level_db = level_db(&frame);
        let loud = !muted && self.level_db > threshold_db;
        self.loud_run = if loud { self.loud_run + 1 } else { 0 };

        let mut out = Output::Nothing;
        let mut change = None;
        if !self.open && self.loud_run >= ATTACK {
            self.open = true;
            self.hangover = HANGOVER;
            self.silence_left = 0;
            self.encoder.reset();
            change = Some(true);
            let mut pre = std::mem::replace(&mut self.previous, frame.clone());
            fade(&mut pre, true);
            let packets = [pre, frame].iter().filter_map(|f| self.encode(f)).collect();
            return (Output::Audio(packets), change);
        }
        if self.open {
            if loud {
                self.hangover = HANGOVER;
            } else {
                self.hangover = self.hangover.saturating_sub(1);
            }
            let closing = muted || self.hangover == 0;
            if closing {
                fade(&mut frame, false);
                self.open = false;
                self.silence_left = TRAILING_SILENCE;
            }
            if let Some(p) = self.encode(&frame) {
                out = Output::Audio(vec![p]);
            }
        } else if self.silence_left > 0 {
            self.silence_left -= 1;
            out = Output::Silence;
            if self.silence_left == 0 {
                change = Some(false);
            }
        }
        self.previous = frame;
        (out, change)
    }

    fn encode(&mut self, frame: &[f32]) -> Option<Vec<u8>> {
        let mut buf = [0u8; 1400];
        self.encoder.encode(frame, &mut buf).map(|n| buf[..n].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded Mic Test (diag's mic and played WAVs, in SWIFTCORD_MIC_WAV and SWIFTCORD_PLAYED_WAV), taken
    /// apart: the microphone replayed through each stage alone (noise suppression, then Opus) to see which
    /// changes it, and the playback against the microphone (diagnostic; prints).
    #[test]
    #[ignore]
    fn diag_recorded_mic_test() {
        fn read(var: &str) -> (Vec<f32>, usize) {
            let b = std::fs::read(std::env::var(var).expect(var)).unwrap();
            let channels = u16::from_le_bytes([b[22], b[23]]) as usize;
            let data = &b[44..];
            (data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(), channels)
        }
        let (mic, _) = read("SWIFTCORD_MIC_WAV");
        let (played, ch) = read("SWIFTCORD_PLAYED_WAV");
        let played: Vec<f32> = played.chunks_exact(ch).map(|f| f[0]).collect();
        let db = |v: f32| 20.0 * v.max(1e-9).log10();
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
        let peak = |s: &[f32]| s.iter().fold(0f32, |m, v| m.max(v.abs()));
        println!("mic:    {:.2} s, peak {:6.1} dBFS, rms {:6.1} dBFS", mic.len() as f32 / 48e3, db(peak(&mic)), db(rms(&mic)));
        // DC offset and how much of the mic's power is below 80 Hz (a one-pole low-pass at 80 Hz).
        let mean = mic.iter().sum::<f32>() / mic.len() as f32;
        let a = (-std::f32::consts::TAU * 80.0 / 48_000.0).exp();
        let mut lp = 0f32;
        let low: f32 = mic.iter().map(|v| { lp = a * lp + (1.0 - a) * (v - mean); lp * lp }).sum();
        let all: f32 = mic.iter().map(|v| (v - mean).powi(2)).sum();
        println!("mic DC offset {mean:.5} ({:.1} dBFS), power below ~80 Hz: {:.0}% of the total", db(mean.abs()), 100.0 * low / all);
        println!("played: {:.2} s, peak {:6.1} dBFS, rms {:6.1} dBFS", played.len() as f32 / 48e3, db(peak(&played)), db(rms(&played)));

        // Each stage alone, on the recorded voice.
        let mut denoiser = nnnoiseless::DenoiseState::new();
        let mut buf = [0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let mut denoised = Vec::new();
        for chunk in mic.chunks_exact(nnnoiseless::DenoiseState::FRAME_SIZE) {
            let input: Vec<f32> = chunk.iter().map(|s| s * 32768.0).collect();
            denoiser.process_frame(&mut buf, &input);
            denoised.extend(buf.iter().map(|s| s / 32768.0));
        }
        let opus = |input: &[f32]| {
            let mut enc = crate::codec::Encoder::new(64_000).unwrap();
            let mut dec = crate::codec::Decoder::new().unwrap();
            let (mut packet, mut out, mut decoded) = ([0u8; 1500], vec![0f32; FRAME * 2], Vec::new());
            for f in input.chunks_exact(FRAME) {
                let n = enc.encode(f, &mut packet).unwrap();
                let k = dec.decode(Some(&packet[..n]), &mut out);
                decoded.extend(out[..k].chunks_exact(2).map(|c| c[0]));
            }
            decoded
        };
        // How `y` differs from `x` once lined up (best delay up to 50 ms) and level-matched: what's left, in dB
        // against `y`, and the level change.
        let compare = |x: &[f32], y: &[f32]| {
            let n = x.len().min(y.len()).saturating_sub(14_400);
            let mut best = (f32::MAX, 0usize, 1f32);
            for d in (0..14_400).step_by(if y.len() == played.len() { 4 } else { 1 }) {
                let (a, b) = (&x[..n], &y[d..d + n]);
                let g = a.iter().zip(b).map(|(p, q)| p * q).sum::<f32>() / a.iter().map(|p| p * p).sum::<f32>().max(1e-12);
                let e: f32 = a.iter().zip(b).map(|(p, q)| (q - g * p).powi(2)).sum();
                if e < best.0 {
                    best = (e, d, g);
                }
            }
            let power: f32 = y[best.1..best.1 + n].iter().map(|v| v * v).sum();
            (10.0 * (best.0 / power.max(1e-12)).log10(), best.1, db(best.2.abs()))
        };
        let coded = opus(&mic);
        let both = opus(&denoised);
        for (name, x, y) in [
            ("noise suppression alone", &mic, &denoised),
            ("Opus alone", &mic, &coded),
            ("noise suppression + Opus", &mic, &both),
            ("what was played", &mic, &played),
        ] {
            let (residual, delay, gain) = compare(x, y);
            println!("{name:<26} difference {residual:6.1} dB, delay {:5.1} ms, level {gain:+5.1} dB, peak {:6.1} dBFS", delay as f32 / 48.0, db(peak(y)));
        }
        // Spectrum (tone colour) change, phase aside: the log-magnitude spectra of 20 ms windows of each, lined
        // up, compared band by band where the mic has sound. Reported overall and per band.
        let spectra = |x: &[f32], at: usize| -> Vec<f32> {
            const N: usize = 960;
            let w: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
            // 48 bins of 250 Hz up to 12 kHz (a coarse DFT, enough to see a band boosted or lost).
            (1..=48)
                .map(|k| {
                    let f = k as f32 * 250.0;
                    let (mut re, mut im) = (0f32, 0f32);
                    for i in 0..N {
                        let v = x.get(at + i).copied().unwrap_or(0.0) * w[i];
                        let ph = std::f32::consts::TAU * f * i as f32 / 48_000.0;
                        re += v * ph.cos();
                        im -= v * ph.sin();
                    }
                    (re * re + im * im).sqrt()
                })
                .collect()
        };
        let bands = [(250.0, 1000.0), (1000.0, 3000.0), (3000.0, 6000.0), (6000.0, 12000.0)];
        for (name, y) in [("noise suppression alone", &denoised), ("Opus alone", &coded), ("noise suppression + Opus", &both), ("what was played", &played)] {
            if std::env::var("DIAG_SPECTRA").is_err() {
                break;
            }
            let (_, delay, gain_db) = compare(&mic, y);
            let g = 10f32.powf(gain_db / 20.0);
            let (mut total, mut count) = (0f32, 0);
            let mut per_band = [0f32; 4];
            let mut per_band_n = [0usize; 4];
            let mut at = 0;
            while at + delay + 960 < y.len() && at + 960 < mic.len() {
                if rms(&mic[at..at + 960]) > 0.005 {
                    let (a, b) = (spectra(&mic, at), spectra(y, at + delay));
                    for (k, (p, q)) in a.iter().zip(&b).enumerate() {
                        let f = (k + 1) as f32 * 250.0;
                        let d = (db(q / g) - db(*p)).abs();
                        total += d;
                        count += 1;
                        if let Some(i) = bands.iter().position(|(lo, hi)| f >= *lo && f < *hi) {
                            per_band[i] += db(q / g) - db(*p);
                            per_band_n[i] += 1;
                        }
                    }
                }
                at += 960;
            }
            let avg: Vec<String> = per_band.iter().zip(per_band_n).zip(bands).map(|((s, n), (lo, hi))| format!("{:.0}-{:.0} Hz {:+.1}", lo, hi, s / n.max(1) as f32)).collect();
            println!("{name:<26} spectrum off by {:4.1} dB on average; band level change (dB): {}", total / count.max(1) as f32, avg.join(", "));
        }
        // The same voice through the encoder set up other ways, and which mode it picked (by the packets' first
        // byte: configurations 0-11 SILK, 12-15 hybrid, 16-31 CELT).
        for (name, app, bitrate, fec) in [
            ("VOIP 64k, FEC (as sent)", unsafe_libopus::OPUS_APPLICATION_VOIP, 64_000, true),
            ("VOIP 64k, no FEC", unsafe_libopus::OPUS_APPLICATION_VOIP, 64_000, false),
            ("AUDIO 64k, no FEC", unsafe_libopus::OPUS_APPLICATION_AUDIO, 64_000, false),
            ("AUDIO 64k, FEC", unsafe_libopus::OPUS_APPLICATION_AUDIO, 64_000, true),
            ("VOIP 96k, FEC", unsafe_libopus::OPUS_APPLICATION_VOIP, 96_000, true),
        ] {
            let mut err = 0;
            let enc = unsafe { unsafe_libopus::opus_encoder_create(48_000, 1, app, &mut err) };
            unsafe {
                unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_BITRATE_REQUEST, bitrate);
                if fec {
                    unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_INBAND_FEC_REQUEST, 1);
                    unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10);
                }
            }
            let mut dec = crate::codec::Decoder::new().unwrap();
            let (mut packet, mut out, mut decoded) = ([0u8; 1500], vec![0f32; FRAME * 2], Vec::new());
            let mut modes = [0usize; 3];
            for f in mic.chunks_exact(FRAME) {
                let n = unsafe { unsafe_libopus::opus_encode_float(enc, f.as_ptr(), FRAME as i32, packet.as_mut_ptr(), 1500) } as usize;
                modes[match packet[0] >> 3 { 0..=11 => 0, 12..=15 => 1, _ => 2 }] += 1;
                let k = dec.decode(Some(&packet[..n]), &mut out);
                decoded.extend(out[..k].chunks_exact(2).map(|c| c[0]));
            }
            unsafe { unsafe_libopus::opus_encoder_destroy(enc) };
            let (residual, _, gain) = compare(&mic, &decoded);
            println!("{name:<24} difference {residual:6.1} dB, level {gain:+5.1} dB, peak {:6.1} dBFS; frames SILK {} hybrid {} CELT {}", db(peak(&decoded)), modes[0], modes[1], modes[2]);
        }
        // Each version as a WAV to listen to side by side (in SWIFTCORD_AB_DIR, when set).
        if let Ok(dir) = std::env::var("SWIFTCORD_AB_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let encode_with = |input: &[f32], app: i32, fec: bool| {
                let mut err = 0;
                let enc = unsafe { unsafe_libopus::opus_encoder_create(48_000, 1, app, &mut err) };
                unsafe {
                    unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_BITRATE_REQUEST, 64_000);
                    if fec {
                        unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_INBAND_FEC_REQUEST, 1);
                        unsafe_libopus::opus_encoder_ctl!(enc, unsafe_libopus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10);
                    }
                }
                let mut dec = crate::codec::Decoder::new().unwrap();
                let (mut packet, mut out, mut decoded) = ([0u8; 1500], vec![0f32; FRAME * 2], Vec::new());
                for f in input.chunks_exact(FRAME) {
                    let n = unsafe { unsafe_libopus::opus_encode_float(enc, f.as_ptr(), FRAME as i32, packet.as_mut_ptr(), 1500) } as usize;
                    let k = dec.decode(Some(&packet[..n]), &mut out);
                    decoded.extend(out[..k].chunks_exact(2).map(|c| c[0]));
                }
                unsafe { unsafe_libopus::opus_encoder_destroy(enc) };
                decoded
            };
            let wav = |name: &str, samples: &[f32]| {
                let data: Vec<u8> = samples.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
                let mut w = b"RIFF".to_vec();
                w.extend((36 + data.len() as u32).to_le_bytes());
                w.extend(b"WAVEfmt ");
                w.extend(16u32.to_le_bytes());
                w.extend(1u16.to_le_bytes());
                w.extend(1u16.to_le_bytes());
                w.extend(48_000u32.to_le_bytes());
                w.extend(96_000u32.to_le_bytes());
                w.extend(2u16.to_le_bytes());
                w.extend(16u16.to_le_bytes());
                w.extend(b"data");
                w.extend((data.len() as u32).to_le_bytes());
                w.extend(data);
                std::fs::write(dir.join(name), w).unwrap();
            };
            wav("1 - microphone (raw).wav", &mic);
            wav("2 - noise suppression only.wav", &denoised);
            wav("3 - Opus only (VOIP, as sent).wav", &encode_with(&mic, unsafe_libopus::OPUS_APPLICATION_VOIP, true));
            wav("4 - noise suppression + Opus (what others hear).wav", &encode_with(&denoised, unsafe_libopus::OPUS_APPLICATION_VOIP, true));
            wav("5 - Opus only (AUDIO mode).wav", &encode_with(&mic, unsafe_libopus::OPUS_APPLICATION_AUDIO, true));
            wav("6 - noise suppression + Opus (AUDIO mode).wav", &encode_with(&denoised, unsafe_libopus::OPUS_APPLICATION_AUDIO, true));
            wav("7 - what the mic test played.wav", &played);
            println!("wrote the A/B files to {}", dir.display());
        }
        // Loudness over time, 100 ms at a time, for pumping or bursts: the played level against the mic's.
        let (_, delay, _) = compare(&mic, &played);
        let mut worst = Vec::new();
        for (i, w) in mic.chunks_exact(4800).enumerate() {
            let at = i * 4800 + delay;
            if at + 4800 > played.len() || rms(w) < 0.003 {
                continue;
            }
            worst.push((db(rms(&played[at..at + 4800])) - db(rms(w)), i));
        }
        worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("loudest 100 ms stretches, played vs mic: {:?}", worst.iter().take(6).map(|(d, i)| format!("{:.1}s {d:+.1} dB", *i as f32 / 10.0)).collect::<Vec<_>>());
        println!("quietest: {:?}", worst.iter().rev().take(4).map(|(d, i)| format!("{:.1}s {d:+.1} dB", *i as f32 / 10.0)).collect::<Vec<_>>());
    }

    fn tone(amp: f32) -> Vec<f32> {
        (0..FRAME).map(|i| (i as f32 * 0.05).sin() * amp).collect()
    }

    #[test]
    fn gate_needs_two_loud_frames_and_sends_preroll() {
        let mut t = Transmitter::new(64_000).unwrap();
        // A single noise pop doesn't open the gate.
        assert!(matches!(t.process(tone(0.5), false, -40.0), (Output::Nothing, None)));
        assert!(matches!(t.process(tone(0.0), false, -40.0), (Output::Nothing, None)));
        // Two loud frames do, and the earlier one goes out as pre-roll.
        t.process(tone(0.5), false, -40.0);
        let (out, change) = t.process(tone(0.5), false, -40.0);
        assert_eq!(change, Some(true));
        assert!(matches!(out, Output::Audio(p) if p.len() == 2));
    }

    #[test]
    fn closes_after_hangover_then_sends_silence() {
        let mut t = Transmitter::new(64_000).unwrap();
        t.process(tone(0.5), false, -40.0);
        t.process(tone(0.5), false, -40.0);
        let mut sent = 0;
        for _ in 0..HANGOVER {
            if matches!(t.process(tone(0.0), false, -40.0).0, Output::Audio(_)) {
                sent += 1;
            }
        }
        assert_eq!(sent, HANGOVER);
        assert!(!t.is_open());
        let mut ended = false;
        for _ in 0..TRAILING_SILENCE {
            let (out, change) = t.process(tone(0.0), false, -40.0);
            assert!(matches!(out, Output::Silence));
            ended |= change == Some(false);
        }
        assert!(ended);
    }

    #[test]
    fn noise_suppression_quiets_steady_noise_and_keeps_the_gate_shut() {
        // Low-pass filtered noise, like a fan or air conditioning, at about -37 dBFS.
        let (mut state, mut y) = (12345u32, 0.0f32);
        let mut noise = || {
            (0..FRAME)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    let x = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                    y = 0.97 * y + 0.012 * x;
                    y
                })
                .collect::<Vec<f32>>()
        };
        let (mut plain, mut quiet) = (Transmitter::new(64_000).unwrap(), Transmitter::new(64_000).unwrap());
        quiet.set_noise_suppression(true);
        let (mut plain_db, mut quiet_db, mut opened) = (0.0, 0.0, false);
        // A second of fan-like noise: RNNoise adapts within the first few frames.
        for i in 0..50 {
            let n = noise();
            plain.process(n.clone(), false, -40.0);
            let (_, change) = quiet.process(n, false, -40.0);
            if i >= 10 {
                plain_db += plain.level_db / 40.0;
                quiet_db += quiet.level_db / 40.0;
                opened |= change == Some(true);
            }
        }
        assert!(plain_db > -40.0, "the noise alone is above the threshold: {plain_db}");
        assert!(quiet_db < plain_db - 30.0, "suppressed {plain_db:.1} dB → {quiet_db:.1} dB");
        assert!(!opened, "noise alone doesn't open the gate");
    }

    /// Two talk spurts through the transmitter, decoded as listeners do (one decoder throughout, the silence
    /// frames included): the level of each 20 ms of the second spurt's start against what went in.
    fn second_spurt_levels(t: &mut Transmitter) -> Vec<f32> {
        let tau = std::f32::consts::TAU;
        let voice = |f: usize| -> Vec<f32> {
            (0..FRAME)
                .map(|i| {
                    let t = (f * FRAME + i) as f32 / 48_000.0;
                    (1..=6).map(|h| (tau * 150.0 * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.2
                })
                .collect()
        };
        let mut dec = crate::codec::Decoder::new().unwrap();
        let mut out = vec![0f32; FRAME * 2];
        let mut levels = Vec::new();
        let mut frame = 0;
        for spurt in 0..2 {
            for k in 0..40 {
                let input = voice(frame);
                frame += 1;
                if let (Output::Audio(packets), _) = t.process(input.clone(), false, -40.0) {
                    for p in packets {
                        let n = dec.decode(Some(&p), &mut out);
                        if spurt == 1 && k < 8 {
                            levels.push(level_db(&out[..n]) - level_db(&input));
                        }
                    }
                }
            }
            for _ in 0..HANGOVER + TRAILING_SILENCE + 25 {
                match t.process(vec![0.0; FRAME], false, -40.0).0 {
                    Output::Audio(packets) => packets.iter().for_each(|p| {
                        dec.decode(Some(p), &mut out);
                    }),
                    Output::Silence => {
                        dec.decode(Some(&crate::codec::SILENCE), &mut out);
                    }
                    Output::Nothing => {}
                }
            }
        }
        levels
    }

    #[test]
    #[ignore = "diagnostic: prints how a listener hears the start of a talk spurt"]
    fn spurt_start_levels() {
        for bitrate in [24_000, 64_000] {
            let levels = second_spurt_levels(&mut Transmitter::new(bitrate).unwrap());
            println!("{bitrate} bps: {:?}", levels.iter().map(|l| (l * 10.0).round() / 10.0).collect::<Vec<_>>());
        }
    }

    #[test]
    fn muting_closes_immediately() {
        let mut t = Transmitter::new(64_000).unwrap();
        t.process(tone(0.5), false, -40.0);
        t.process(tone(0.5), false, -40.0);
        t.process(tone(0.5), true, -40.0);
        assert!(!t.is_open());
    }
}
