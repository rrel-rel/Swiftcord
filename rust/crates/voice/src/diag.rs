//! Call audio diagnostics, for finding out why a call sounds wrong on someone's machine.
//!
//! Counters (clipping and peaks on the microphone and the playback mix, device dropouts, and what the jitter
//! buffer had to do) are logged every 30 seconds during a call and when it ends. With `SWIFTCORD_RECORD_VOICE=1`
//! set, the call's audio is also kept (up to five minutes of each) and written to WAV files in the log folder
//! when the call ends: the microphone as captured, what was sent (decoded as a listener decodes it), and what
//! was played.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

/// A sample at or beyond this is clipped (the source ran out of headroom).
const CLIP: f32 = 0.999;

#[derive(Default)]
pub struct Stats {
    pub mic_samples: AtomicU64,
    pub mic_clipped: AtomicU64,
    /// The loudest sample, as f32 bits (non-negative floats order like their bits).
    pub mic_peak: AtomicU32,
    pub mic_dropped: AtomicU64,
    pub capture_xruns: AtomicU64,
    pub render_xruns: AtomicU64,
    pub mix_samples: AtomicU64,
    /// Mixed samples loud enough for the soft clipper to bend.
    pub mix_bent: AtomicU64,
    pub mix_peak: AtomicU32,
    pub packets: AtomicU64,
    pub late: AtomicU64,
    pub fec_recovered: AtomicU64,
    pub covered: AtomicU64,
    pub skipped: AtomicU64,
    pub shortened: AtomicU64,
    pub underruns: AtomicU64,
    pub stalls: AtomicU64,
    pub max_target: AtomicU64,
}

pub static STATS: OnceLock<Stats> = OnceLock::new();

pub fn stats() -> &'static Stats {
    STATS.get_or_init(Stats::default)
}

pub fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

fn peak(at: &AtomicU32, v: f32) {
    at.fetch_max(v.abs().to_bits(), Ordering::Relaxed);
}

/// Counts raw microphone samples (all channels, as the device gave them).
pub fn mic(samples: &[f32]) {
    let s = stats();
    let (mut clipped, mut loudest) = (0, 0f32);
    for v in samples {
        let a = v.abs();
        clipped += (a >= CLIP) as u64;
        loudest = loudest.max(a);
    }
    add(&s.mic_samples, samples.len() as u64);
    add(&s.mic_clipped, clipped);
    peak(&s.mic_peak, loudest);
}

/// Counts the playback mix before soft clipping.
pub fn mix(samples: &[f32], knee: f32) {
    let s = stats();
    let (mut bent, mut loudest) = (0, 0f32);
    for v in samples {
        let a = v.abs();
        bent += (a > knee) as u64;
        loudest = loudest.max(a);
    }
    add(&s.mix_samples, samples.len() as u64);
    add(&s.mix_bent, bent);
    peak(&s.mix_peak, loudest);
}

static LAST_LOG: Mutex<Option<Instant>> = Mutex::new(None);

/// Logs the counters if 30 seconds have passed since last time (or now, when the call has ended, which also
/// resets them and writes any recording).
pub fn report(ended: bool) {
    {
        let mut last = LAST_LOG.lock();
        if !ended && last.is_some_and(|t| t.elapsed().as_secs() < 30) {
            return;
        }
        *last = if ended { None } else { Some(Instant::now()) };
    }
    let s = stats();
    let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
    let pct = |n: u64, of: u64| if of == 0 { 0.0 } else { n as f64 * 100.0 / of as f64 };
    let db = |bits: &AtomicU32| {
        let v = f32::from_bits(bits.load(Ordering::Relaxed));
        if v > 0.0 { 20.0 * v.log10() } else { f32::NEG_INFINITY }
    };
    if get(&s.mic_samples) + get(&s.mix_samples) + get(&s.packets) > 0 {
        tracing::info!(
            "voice audio{}: mic peak {:.1} dBFS, {:.3}% clipped, {} chunks dropped, {} capture dropouts | playback peak {:.1} dBFS, {:.3}% soft-clipped, {} render dropouts | received {} packets: {} late, {} recovered by FEC, {} covered, {} skips, {} catch-ups, {} underruns, {} stalls, buffer up to {} frames",
            if ended { " (call ended)" } else { "" },
            db(&s.mic_peak),
            pct(get(&s.mic_clipped), get(&s.mic_samples)),
            get(&s.mic_dropped),
            get(&s.capture_xruns),
            db(&s.mix_peak),
            pct(get(&s.mix_bent), get(&s.mix_samples)),
            get(&s.render_xruns),
            get(&s.packets),
            get(&s.late),
            get(&s.fec_recovered),
            get(&s.covered),
            get(&s.skipped),
            get(&s.shortened),
            get(&s.underruns),
            get(&s.stalls),
            get(&s.max_target),
        );
    }
    if ended {
        let fresh = Stats::default();
        for (a, b) in [
            (&s.mic_samples, &fresh.mic_samples),
            (&s.mic_clipped, &fresh.mic_clipped),
            (&s.mic_dropped, &fresh.mic_dropped),
            (&s.capture_xruns, &fresh.capture_xruns),
            (&s.render_xruns, &fresh.render_xruns),
            (&s.mix_samples, &fresh.mix_samples),
            (&s.mix_bent, &fresh.mix_bent),
            (&s.packets, &fresh.packets),
            (&s.late, &fresh.late),
            (&s.fec_recovered, &fresh.fec_recovered),
            (&s.covered, &fresh.covered),
            (&s.skipped, &fresh.skipped),
            (&s.shortened, &fresh.shortened),
            (&s.underruns, &fresh.underruns),
            (&s.stalls, &fresh.stalls),
            (&s.max_target, &fresh.max_target),
        ] {
            a.store(b.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        s.mic_peak.store(0, Ordering::Relaxed);
        s.mix_peak.store(0, Ordering::Relaxed);
        write_recordings();
    }
}

/// Up to five minutes of one channel at 48 kHz.
const MAX_RECORDED: usize = 48_000 * 300;

pub struct Recording {
    name: &'static str,
    channels: u16,
    samples: Mutex<Vec<f32>>,
}

pub static MIC: Recording = Recording { name: "mic", channels: 1, samples: Mutex::new(Vec::new()) };
pub static SENT: Recording = Recording { name: "sent", channels: 1, samples: Mutex::new(Vec::new()) };
pub static PLAYED: Recording = Recording { name: "played", channels: 2, samples: Mutex::new(Vec::new()) };

static FOLDER: OnceLock<PathBuf> = OnceLock::new();

/// Where recordings go (the log folder); without it, nothing is recorded.
pub fn set_folder(folder: PathBuf) {
    let _ = FOLDER.set(folder);
}

pub fn recording() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| FOLDER.get().is_some() && std::env::var("SWIFTCORD_RECORD_VOICE").is_ok_and(|v| v == "1"))
}

impl Recording {
    pub fn add(&self, samples: &[f32]) {
        if !recording() {
            return;
        }
        let mut kept = self.samples.lock();
        let room = (MAX_RECORDED * self.channels as usize).saturating_sub(kept.len());
        kept.extend_from_slice(&samples[..samples.len().min(room)]);
    }
}

fn write_recordings() {
    let Some(folder) = FOLDER.get().filter(|_| recording()) else { return };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    for r in [&MIC, &SENT, &PLAYED] {
        let samples = std::mem::take(&mut *r.samples.lock());
        if samples.is_empty() {
            continue;
        }
        let path = folder.join(format!("voice-{stamp}-{}.wav", r.name));
        match std::fs::write(&path, wav(&samples, r.channels)) {
            Ok(()) => tracing::info!("recorded {}", path.display()),
            Err(e) => tracing::warn!("couldn't write {}: {e}", path.display()),
        }
    }
}

/// 32-bit float WAV at 48 kHz (float, so clipping in the source shows as it was, not as ours).
fn wav(samples: &[f32], channels: u16) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut w = b"RIFF".to_vec();
    w.extend((36 + data.len() as u32).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(3u16.to_le_bytes());
    w.extend(channels.to_le_bytes());
    w.extend(48_000u32.to_le_bytes());
    w.extend((48_000 * 4 * channels as u32).to_le_bytes());
    w.extend((4 * channels).to_le_bytes());
    w.extend(32u16.to_le_bytes());
    w.extend(b"data");
    w.extend((data.len() as u32).to_le_bytes());
    w.extend(data);
    w
}
