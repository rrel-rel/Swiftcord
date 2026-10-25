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
        })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Processes one mono 20 ms frame. The second value reports the gate opening (`Some(true)`) or the
    /// talk spurt fully ending (`Some(false)`, after the trailing silence).
    pub fn process(&mut self, mut frame: Vec<f32>, muted: bool, threshold_db: f32) -> (Output, Option<bool>) {
        debug_assert_eq!(frame.len(), FRAME);
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
    fn muting_closes_immediately() {
        let mut t = Transmitter::new(64_000).unwrap();
        t.process(tone(0.5), false, -40.0);
        t.process(tone(0.5), false, -40.0);
        t.process(tone(0.5), true, -40.0);
        assert!(!t.is_open());
    }
}
