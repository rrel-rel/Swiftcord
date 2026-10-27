//! Notification sounds: new messages, incoming calls (ringing), the ringback while a DM call we started
//! rings, and people joining or leaving our call. The sounds are synthesized here (soft sine chimes), so
//! there are no audio files, and they're mixed on one output stream that's opened when a sound plays and
//! closed after a few idle seconds.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

const RATE: f32 = 48_000.0;
/// The output stream closes after this long with nothing playing.
const IDLE: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    Message,
    /// Someone (or we) joined the call.
    Join,
    /// Someone (or we) left the call.
    Leave,
    /// An incoming call (loops).
    Ring,
    /// A DM call we started, ringing the others (loops).
    Ringback,
}

/// Notes as (start in seconds, frequency in Hz, length in seconds), mixed with a soft bell envelope.
fn notes(notes: &[(f32, f32, f32)], total: f32, volume: f32) -> Vec<f32> {
    let len = (total * RATE) as usize;
    let mut mono = vec![0f32; len];
    for &(start, freq, dur) in notes {
        let (from, n) = ((start * RATE) as usize, (dur * RATE) as usize);
        for i in 0..n.min(len.saturating_sub(from)) {
            let t = i as f32 / RATE;
            // 5 ms attack (no click), then an exponential decay to silence by the note's end.
            let attack = (t / 0.005).min(1.0);
            let decay = (-5.0 * t / dur).exp() * (1.0 - i as f32 / n as f32);
            // A quiet octave above gives it a bell-like colour.
            let wave = (TAU * freq * t).sin() + 0.25 * (TAU * 2.0 * freq * t).sin();
            mono[from + i] += wave * attack * decay * volume;
        }
    }
    mono.iter().flat_map(|&s| [s, s]).collect()
}

fn synth(sound: Sound) -> Vec<f32> {
    match sound {
        // Two quick rising notes.
        Sound::Message => notes(&[(0.0, 880.0, 0.18), (0.07, 1318.5, 0.25)], 0.35, 0.18),
        Sound::Join => notes(&[(0.0, 659.3, 0.2), (0.1, 987.8, 0.3)], 0.42, 0.2),
        Sound::Leave => notes(&[(0.0, 987.8, 0.2), (0.1, 659.3, 0.3)], 0.42, 0.2),
        // A three-note chime, then a pause, repeated.
        Sound::Ring => notes(&[(0.0, 783.99, 0.35), (0.15, 987.77, 0.35), (0.3, 1174.66, 0.5)], 2.0, 0.22),
        // Softer and lower: two long tones, then a pause.
        Sound::Ringback => notes(&[(0.0, 523.25, 0.6), (0.6, 659.25, 0.8)], 3.0, 0.12),
    }
}

struct Voice {
    sound: Sound,
    pcm: Arc<Vec<f32>>,
    pos: usize,
    looping: bool,
}

#[derive(Default)]
pub struct Sounds {
    voices: Arc<Mutex<Vec<Voice>>>,
    cache: HashMap<Sound, Arc<Vec<f32>>>,
    io: Option<(swiftcord_voice::AudioIo, Option<String>)>,
    idle_since: Option<Instant>,
}

impl Sounds {
    fn pcm(&mut self, sound: Sound) -> Arc<Vec<f32>> {
        self.cache.entry(sound).or_insert_with(|| Arc::new(synth(sound))).clone()
    }

    /// Opens the output (on `output`, the chosen device) if it isn't already.
    fn ensure_output(&mut self, output: &Option<String>) {
        if self.io.as_ref().is_some_and(|(_, o)| o != output) {
            self.io = None;
        }
        if self.io.is_some() {
            return;
        }
        let voices = self.voices.clone();
        let fill = move |out: &mut [f32]| {
            out.fill(0.0);
            let mut voices = voices.lock();
            for v in voices.iter_mut() {
                let mut i = 0;
                while i < out.len() {
                    let n = (out.len() - i).min(v.pcm.len() - v.pos);
                    for (o, s) in out[i..i + n].iter_mut().zip(&v.pcm[v.pos..v.pos + n]) {
                        *o += s;
                    }
                    v.pos += n;
                    i += n;
                    if v.pos >= v.pcm.len() {
                        if !v.looping {
                            break;
                        }
                        v.pos = 0;
                    }
                }
            }
            voices.retain(|v| v.looping || v.pos < v.pcm.len());
        };
        match swiftcord_voice::play(output.clone(), fill) {
            Ok(io) => self.io = Some((io, output.clone())),
            Err(e) => tracing::warn!("notification sounds unavailable: {e}"),
        }
    }

    /// Plays a sound once.
    pub fn play(&mut self, sound: Sound, output: &Option<String>) {
        let pcm = self.pcm(sound);
        self.ensure_output(output);
        let mut voices = self.voices.lock();
        // The same sound twice at once (a burst of messages) just sounds louder; restart it instead.
        voices.retain(|v| v.sound != sound || v.looping);
        voices.push(Voice { sound, pcm, pos: 0, looping: false });
    }

    /// Starts or stops a looping sound (ringing).
    pub fn set_looping(&mut self, sound: Sound, on: bool, output: &Option<String>) {
        let playing = self.voices.lock().iter().any(|v| v.sound == sound && v.looping);
        if on && !playing {
            let pcm = self.pcm(sound);
            self.ensure_output(output);
            self.voices.lock().push(Voice { sound, pcm, pos: 0, looping: true });
        } else if !on && playing {
            self.voices.lock().retain(|v| !(v.sound == sound && v.looping));
        }
    }

    /// Closes the output once nothing has played for a while.
    pub fn tick(&mut self) {
        if self.io.is_none() {
            return;
        }
        if !self.voices.lock().is_empty() {
            self.idle_since = None;
        } else if self.idle_since.get_or_insert_with(Instant::now).elapsed() > IDLE {
            self.io = None;
            self.idle_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays each sound on the default speakers (run by hand, with something measuring the output).
    #[test]
    #[ignore]
    fn plays_on_the_speakers() {
        let mut s = Sounds::default();
        for sound in [Sound::Message, Sound::Join, Sound::Leave] {
            s.play(sound, &None);
            std::thread::sleep(Duration::from_millis(700));
        }
        s.set_looping(Sound::Ring, true, &None);
        std::thread::sleep(Duration::from_millis(2500));
        s.set_looping(Sound::Ring, false, &None);
        std::thread::sleep(Duration::from_millis(300));
        assert!(s.voices.lock().is_empty());
    }

    #[test]
    fn sounds_start_and_end_silently_and_never_clip() {
        for s in [Sound::Message, Sound::Join, Sound::Leave, Sound::Ring, Sound::Ringback] {
            let pcm = synth(s);
            assert!(pcm.len() > 1000);
            assert!(pcm[0].abs() < 1e-3, "{s:?} starts at {}", pcm[0]);
            assert!(pcm[pcm.len() - 2..].iter().all(|v| v.abs() < 1e-3), "{s:?} ends silent (loops cleanly)");
            let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05 && peak < 0.6, "{s:?} peak {peak}");
        }
    }
}
