//! Sound effects: new messages, incoming calls (ringing), the ringback while a DM call we started rings,
//! people joining or leaving our call, and Source's button sounds.
//!
//! Each can be replaced by a WAV file in the data folder's `sounds` folder (see [`FILES`]; a README there lists
//! them), picked up the next time it plays. Without one, the chimes are synthesized here (soft sine notes)
//! and the button sounds are silent: Source's are Valve's, so they don't ship with the app.
//!
//! Everything is mixed on one output stream, at the volume set in the settings, that's opened when a sound
//! plays and closed after a few idle seconds.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

const RATE: f32 = 48_000.0;
/// Rollovers overlap (sweeping over a list plays one per item) up to this many at once.
const MAX_ROLLOVERS: usize = 5;
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
    /// The pointer moved onto a list item (a DM, channel or server).
    ButtonRollover,
    ButtonPress,
    ButtonRelease,
}

/// The two kinds of sound, each with its own volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Buttons and list rollovers.
    Menu,
    /// Messages and calls.
    Notification,
}

impl Sound {
    pub fn kind(self) -> Kind {
        match self {
            Sound::ButtonRollover | Sound::ButtonPress | Sound::ButtonRelease => Kind::Menu,
            Sound::Message | Sound::Join | Sound::Leave | Sound::Ring | Sound::Ringback => Kind::Notification,
        }
    }
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
        // Source's: silent unless installed.
        Sound::ButtonRollover | Sound::ButtonPress | Sound::ButtonRelease => Vec::new(),
    }
}

/// Each sound's file in the sounds folder, and what it's for (for the README there).
pub const FILES: &[(Sound, &str, &str)] = &[
    (Sound::Message, "message.wav", "a new message (when a notification would show)"),
    (Sound::Join, "join.wav", "someone joins your call (or you join one)"),
    (Sound::Leave, "leave.wav", "someone leaves your call (or you leave)"),
    (Sound::Ring, "ring.wav", "an incoming call (loops while it rings)"),
    (Sound::Ringback, "ringback.wav", "a call you started, ringing the others (loops)"),
    (Sound::ButtonRollover, r"ui\buttonrollover.wav", "the pointer moves onto a DM, channel or server (played at half volume)"),
    (Sound::ButtonPress, r"ui\buttonclick.wav", "a button or list item is pressed"),
    (Sound::ButtonRelease, r"ui\buttonclickrelease.wav", "the mouse button comes back up"),
];

/// The folder of replacement sounds.
pub fn folder() -> PathBuf {
    crate::platform::data_dir().join("sounds")
}

fn path(sound: Sound) -> PathBuf {
    let file = FILES.iter().find(|(s, ..)| *s == sound).map_or("", |(_, f, _)| f);
    folder().join(file)
}

/// Creates the sounds folder, with a README listing the files that replace the sounds.
pub fn write_readme() {
    let mut text = String::from(
        "Sounds in this folder replace '08cord's own. Each is a WAV file (PCM at 8, 16, 24 or 32 bits, or 32-bit\r\n\
         float; mono or stereo; any sample rate). A file that's added, changed or removed is used from the next\r\n\
         time that sound plays. Without one, the built-in sound plays (the button sounds are silent).\r\n\r\n",
    );
    for (_, file, what) in FILES {
        text.push_str(&format!("{file:<28} {what}\r\n"));
    }
    text.push_str("\r\nThe button sounds are named after Source's UI sounds (sound\\ui in a Source game), which fit as they are.\r\n");
    let readme = folder().join("README.txt");
    if std::fs::read_to_string(&readme).ok().as_deref() != Some(text.as_str()) {
        let _ = std::fs::create_dir_all(folder().join("ui"));
        let _ = std::fs::write(readme, text);
    }
}

/// A sound from its file if there's one (and it can be read), else the built-in one; at the level it plays at.
fn load(sound: Sound) -> Vec<f32> {
    let path = path(sound);
    let pcm = match std::fs::read(&path) {
        Ok(bytes) => decode_wav(&bytes).unwrap_or_else(|| {
            tracing::warn!("{} isn't a WAV file that can be played; using the built-in sound", path.display());
            synth(sound)
        }),
        Err(_) => synth(sound),
    };
    // Heard all the time, sweeping over lists: at half volume.
    let gain = if sound == Sound::ButtonRollover { 0.5 } else { 1.0 };
    if gain == 1.0 { pcm } else { pcm.into_iter().map(|s| s * gain).collect() }
}

/// A WAV file (PCM at 8, 16, 24 or 32 bits, or 32-bit float; any number of channels, the first two used;
/// any rate) as interleaved stereo at [`RATE`].
pub(crate) fn decode_wav(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (mut format, mut data, mut extensible_kind) = (None, None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32_at(at + 4)? as usize;
        let body = at + 8;
        match bytes.get(at..at + 4)? {
            b"fmt " => {
                format = Some((u16_at(body)?, u16_at(body + 2)?, u32_at(body + 4)?, u16_at(body + 14)?));
                extensible_kind = u16_at(body + 24);
            }
            b"data" => data = bytes.get(body..(body + len).min(bytes.len())),
            _ => {}
        }
        // Chunks are padded to an even length.
        at = body + len + (len & 1);
    }
    let ((kind, channels, rate, bits), data) = (format?, data?);
    // WAVE_FORMAT_EXTENSIBLE (what editors write for 24-bit and up): the real format leads its sub-format GUID.
    let kind = if kind == 0xFFFE { extensible_kind? } else { kind };
    const PCM: u16 = 1;
    const FLOAT: u16 = 3;
    let sample: fn(&[u8]) -> f32 = match (kind, bits) {
        (PCM, 8) => |b| (b[0] as f32 - 128.0) / 128.0,
        (PCM, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        (PCM, 24) => |b| i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
        (PCM, 32) => |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (FLOAT, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0),
        _ => return None,
    };
    if channels == 0 || rate == 0 {
        return None;
    }
    let width = bits as usize / 8;
    let frames: Vec<[f32; 2]> = data
        .chunks_exact(width * channels as usize)
        .map(|f| {
            let s = |i: usize| sample(&f[width * i..width * (i + 1)]);
            if channels == 1 { [s(0), s(0)] } else { [s(0), s(1)] }
        })
        .collect();
    if frames.is_empty() {
        return None;
    }
    // Linear resampling to our rate.
    let step = rate as f32 / RATE;
    let len = (frames.len() as f32 / step) as usize;
    let mut out = Vec::with_capacity(len * 2);
    for i in 0..len {
        let pos = i as f32 * step;
        let (j, t) = (pos as usize, pos.fract());
        let (a, b) = (frames[j], frames[(j + 1).min(frames.len() - 1)]);
        out.extend([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
    }
    Some(out)
}

/// Button sounds waiting for the app to play them (they're worked out while drawing, where the player isn't).
static PENDING: Mutex<Vec<Sound>> = Mutex::new(Vec::new());

/// The sounds to play now.
pub fn take_pending() -> Vec<Sound> {
    std::mem::take(&mut *PENDING.lock())
}

/// Where the pointer was last frame: the list item it was over, and whether it pressed something.
#[derive(Clone, Copy, Default)]
struct ButtonPointer {
    item: Option<egui::Rect>,
    pressed: bool,
}

/// Source's sounds for the frame's (enabled) `buttons` and list `items` (DMs, channels, servers): a rollover
/// as the pointer moves onto a list item, a click when either is pressed, and the release when the mouse
/// comes back up.
pub fn button_feedback(ctx: &egui::Context, buttons: &[egui::Rect], items: &[egui::Rect]) {
    let (hover, pressed, released) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed(), i.pointer.primary_released()));
    // The last one drawn there is the one on top.
    let top = |rects: &[egui::Rect]| hover.and_then(|p| rects.iter().rev().find(|r| r.contains(p)).copied());
    let (button, item) = (top(buttons), top(items));
    let key = egui::Id::unique("vgui-button-pointer");
    let mut last: ButtonPointer = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
    let mut play = Vec::new();
    let same = |a: egui::Rect, b: egui::Rect| (a.min - b.min).length() < 0.5 && (a.max - b.max).length() < 0.5;
    if let Some(r) = item {
        if !last.item.is_some_and(|l| same(l, r)) {
            play.push(Sound::ButtonRollover);
        }
    }
    if pressed && (button.is_some() || item.is_some()) {
        play.push(Sound::ButtonPress);
        last.pressed = true;
    }
    if released && last.pressed {
        play.push(Sound::ButtonRelease);
        last.pressed = false;
    }
    last.item = item;
    ctx.data_mut(|d| d.insert_temp(key, last));
    if !play.is_empty() {
        PENDING.lock().extend(play);
        // Played at the start of the next frame, which might otherwise wait for the next input.
        ctx.request_repaint();
    }
}

struct Voice {
    sound: Sound,
    pcm: Arc<Vec<f32>>,
    pos: usize,
    looping: bool,
    /// Its own volume (a preview of a new setting), rather than the one set.
    volume: Option<f32>,
}

/// Adds a sound to those playing.
fn queue(voices: &mut Vec<Voice>, sound: Sound, pcm: Arc<Vec<f32>>) {
    if sound == Sound::ButtonRollover {
        // Each plays out; past the limit, the oldest (the furthest along) gives way.
        let playing = voices.iter().filter(|v| v.sound == sound).count();
        if playing >= MAX_ROLLOVERS {
            if let Some(oldest) = voices.iter().enumerate().filter(|(_, v)| v.sound == sound).max_by_key(|(_, v)| v.pos).map(|(i, _)| i) {
                voices.remove(oldest);
            }
        }
    } else {
        // The same sound twice at once (a burst of messages) just sounds louder; restart it instead.
        voices.retain(|v| v.sound != sound || v.looping);
    }
    voices.push(Voice { sound, pcm, pos: 0, looping: false, volume: None });
}

pub struct Sounds {
    voices: Arc<Mutex<Vec<Voice>>>,
    /// Each sound as loaded, and the modified time of its file then (none: there wasn't one).
    cache: HashMap<Sound, (Option<SystemTime>, Arc<Vec<f32>>)>,
    io: Option<(swiftcord_voice::AudioIo, Option<String>)>,
    idle_since: Option<Instant>,
    /// The volumes set (0 to 1) for menu and notification sounds, as f32 bits: the mixer reads them, so a
    /// change applies to what's playing too.
    volumes: Arc<[AtomicU32; 2]>,
}

impl Default for Sounds {
    fn default() -> Self {
        let full = || AtomicU32::new(1f32.to_bits());
        Self { voices: Default::default(), cache: HashMap::new(), io: None, idle_since: None, volumes: Arc::new([full(), full()]) }
    }
}

/// Where a kind's volume is kept in [`Sounds::volumes`].
fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Menu => 0,
        Kind::Notification => 1,
    }
}

impl Sounds {
    /// The sound, loaded again if its file has been added, changed or removed since.
    fn pcm(&mut self, sound: Sound) -> Arc<Vec<f32>> {
        let modified = std::fs::metadata(path(sound)).and_then(|m| m.modified()).ok();
        match self.cache.get(&sound) {
            Some((m, pcm)) if *m == modified => pcm.clone(),
            _ => {
                let pcm = Arc::new(load(sound));
                self.cache.insert(sound, (modified, pcm.clone()));
                pcm
            }
        }
    }

    /// The volume for one kind of sound, 0 to 1 (the settings).
    pub fn set_volume(&self, kind: Kind, volume: f32) {
        self.volumes[slot(kind)].store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Plays a sound once at `volume` rather than the one set: hearing a new setting before it's saved.
    pub fn preview(&mut self, sound: Sound, volume: f32, output: &Option<String>) {
        let pcm = self.pcm(sound);
        if pcm.is_empty() {
            return;
        }
        self.ensure_output(output);
        let mut voices = self.voices.lock();
        queue(&mut voices, sound, pcm);
        if let Some(v) = voices.last_mut() {
            v.volume = Some(volume.clamp(0.0, 1.0));
        }
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
        let volumes = self.volumes.clone();
        let fill = move |out: &mut [f32]| {
            out.fill(0.0);
            let set = |kind: Kind| f32::from_bits(volumes[slot(kind)].load(Ordering::Relaxed));
            let mut voices = voices.lock();
            for v in voices.iter_mut() {
                // Perceived loudness is about logarithmic: squared, the slider's middle sounds like half.
                let gain = v.volume.unwrap_or_else(|| set(v.sound.kind())).powi(2);
                let mut i = 0;
                while i < out.len() {
                    let n = (out.len() - i).min(v.pcm.len() - v.pos);
                    for (o, s) in out[i..i + n].iter_mut().zip(&v.pcm[v.pos..v.pos + n]) {
                        *o += s * gain;
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
        // A UI sound that isn't installed.
        if pcm.is_empty() {
            return;
        }
        self.ensure_output(output);
        queue(&mut self.voices.lock(), sound, pcm);
    }

    /// Starts or stops a looping sound (ringing).
    pub fn set_looping(&mut self, sound: Sound, on: bool, output: &Option<String>) {
        let playing = self.voices.lock().iter().any(|v| v.sound == sound && v.looping);
        if on && !playing {
            let pcm = self.pcm(sound);
            self.ensure_output(output);
            self.voices.lock().push(Voice { sound, pcm, pos: 0, looping: true, volume: None });
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

    /// A WAV as Source's UI sounds are: 16-bit PCM, mono, 44.1 kHz.
    fn wav(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut w = b"RIFF".to_vec();
        w.extend((36 + data.len() as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(channels.to_le_bytes());
        w.extend(rate.to_le_bytes());
        w.extend((rate * 2 * channels as u32).to_le_bytes());
        w.extend((2 * channels).to_le_bytes());
        w.extend(16u16.to_le_bytes());
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(data);
        w
    }

    #[test]
    fn wavs_become_stereo_at_our_rate() {
        let pcm = decode_wav(&wav(&[16384; 44100], 1, 44100)).unwrap();
        // A second at 44.1 kHz is a second at 48 kHz, two channels each.
        assert_eq!(pcm.len(), 48000 * 2);
        assert!((pcm[0] - 0.5).abs() < 1e-3 && (pcm[1] - 0.5).abs() < 1e-3);
        let stereo = decode_wav(&wav(&[16384, -16384, 16384, -16384], 2, 48000)).unwrap();
        assert_eq!(&stereo[..2], &[0.5, -0.5]);
        assert!(decode_wav(b"not a wav").is_none());
    }

    /// The same samples in each format editors write: 8-bit, 24-bit (extensible), 32-bit float.
    #[test]
    fn other_wav_formats_decode_too() {
        fn wav(kind: u16, bits: u16, extensible: bool, samples: &[u8]) -> Vec<u8> {
            let fmt_len: u32 = if extensible { 40 } else { 16 };
            let mut w = b"RIFF".to_vec();
            w.extend((4 + 8 + fmt_len + 8 + samples.len() as u32).to_le_bytes());
            w.extend(b"WAVEfmt ");
            w.extend(fmt_len.to_le_bytes());
            w.extend((if extensible { 0xFFFE } else { kind }).to_le_bytes());
            w.extend(1u16.to_le_bytes());
            w.extend(48000u32.to_le_bytes());
            w.extend((48000 * bits as u32 / 8).to_le_bytes());
            w.extend((bits / 8).to_le_bytes());
            w.extend(bits.to_le_bytes());
            if extensible {
                w.extend(22u16.to_le_bytes());
                w.extend(bits.to_le_bytes());
                w.extend(4u32.to_le_bytes());
                // The sub-format GUID starts with the real format.
                w.extend(kind.to_le_bytes());
                w.extend([0u8; 14]);
            }
            w.extend(b"data");
            w.extend((samples.len() as u32).to_le_bytes());
            w.extend(samples);
            w
        }
        let half = |pcm: Vec<f32>| assert!((pcm[0] - 0.5).abs() < 0.01 && (pcm[1] - 0.5).abs() < 0.01, "{:?}", &pcm[..2]);
        half(decode_wav(&wav(1, 8, false, &[192, 192])).unwrap());
        half(decode_wav(&wav(1, 24, true, &[0, 0, 0x40, 0, 0, 0x40])).unwrap());
        half(decode_wav(&wav(3, 32, false, &[0.5f32.to_le_bytes(), 0.5f32.to_le_bytes()].concat())).unwrap());
        // An unknown format is refused, not played as noise.
        assert!(decode_wav(&wav(2, 16, false, &[0, 0])).is_none());
    }

    #[test]
    fn rollovers_overlap_up_to_a_limit() {
        let pcm = Arc::new(vec![0.0; 100]);
        let mut voices = Vec::new();
        for i in 0..7 {
            queue(&mut voices, Sound::ButtonRollover, pcm.clone());
            voices.last_mut().unwrap().pos = 0;
            // Older ones are further along.
            for v in voices.iter_mut() {
                v.pos += 10;
            }
            assert_eq!(voices.len(), (i + 1).min(MAX_ROLLOVERS));
        }
        // The newest five remain: the furthest along is the oldest kept.
        assert_eq!(voices.iter().map(|v| v.pos).max(), Some(50));
        // Other sounds still restart rather than stack.
        queue(&mut voices, Sound::Message, pcm.clone());
        queue(&mut voices, Sound::Message, pcm.clone());
        assert_eq!(voices.iter().filter(|v| v.sound == Sound::Message).count(), 1);
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
