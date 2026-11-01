//! Opus at 48 kHz in 20 ms frames, through the transpiled libopus.

use unsafe_libopus as opus;

pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = 960;
/// Opus's three-byte silence frame, sent a few times after speaking stops so decoders don't interpolate.
pub const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

pub struct Encoder(*mut opus::OpusEncoder, i32);

// The encoder is only ever used from one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    /// A mono voice encoder.
    pub fn new(bitrate: i32) -> Option<Encoder> {
        let mut err = 0;
        let enc = unsafe { opus::opus_encoder_create(SAMPLE_RATE as i32, 1, opus::OPUS_APPLICATION_VOIP, &mut err) };
        if enc.is_null() || err != opus::OPUS_OK {
            return None;
        }
        unsafe {
            opus::opus_encoder_ctl!(enc, opus::OPUS_SET_BITRATE_REQUEST, bitrate);
            opus::opus_encoder_ctl!(enc, opus::OPUS_SET_INBAND_FEC_REQUEST, 1);
            opus::opus_encoder_ctl!(enc, opus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10);
        }
        Some(Encoder(enc, bitrate))
    }

    /// Forgets the previous audio, so a new talk spurt doesn't start from stale state (an audible click).
    pub fn reset(&mut self) {
        if let Some(fresh) = Encoder::new(self.1) {
            *self = fresh;
        }
    }

    /// Encodes one mono frame of `FRAME` samples.
    pub fn encode(&mut self, pcm: &[f32], out: &mut [u8]) -> Option<usize> {
        debug_assert_eq!(pcm.len(), FRAME);
        let n = unsafe { opus::opus_encode_float(self.0, pcm.as_ptr(), FRAME as i32, out.as_mut_ptr(), out.len() as i32) };
        (n > 0).then_some(n as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus::opus_encoder_destroy(self.0) }
    }
}

pub struct Decoder(*mut opus::OpusDecoder);

unsafe impl Send for Decoder {}

impl Decoder {
    /// A stereo decoder (mono packets decode to identical channels).
    pub fn new() -> Option<Decoder> {
        let mut err = 0;
        let dec = unsafe { opus::opus_decoder_create(SAMPLE_RATE as i32, 2, &mut err) };
        (!dec.is_null() && err == opus::OPUS_OK).then_some(Decoder(dec))
    }

    /// Decodes a packet into interleaved stereo; `None` as the packet conceals a lost one.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> usize {
        let (ptr, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        let n = unsafe { opus::opus_decode_float(self.0, ptr, len, out.as_mut_ptr(), (out.len() / 2) as i32, 0) };
        if n > 0 { n as usize * 2 } else { 0 }
    }

    /// Recovers the frame before `next_packet` from the copy of it that voice (SILK and hybrid) packets carry
    /// (in-band forward error correction), into interleaved stereo. Only call it when [`carries_fec`] says
    /// the packet has one: otherwise libopus falls back to its own concealment.
    pub fn decode_fec(&mut self, next_packet: &[u8], out: &mut [f32]) -> usize {
        // The copy is of a frame as long as the packet carrying it; asked for more, libopus would make up the
        // difference by concealment, stretching time.
        let duration = unsafe { opus::opus_packet_get_nb_samples(next_packet.as_ptr(), next_packet.len() as i32, SAMPLE_RATE as i32) };
        let frames = (out.len() / 2).min(if duration > 0 { duration as usize } else { FRAME }) as i32;
        let n = unsafe { opus::opus_decode_float(self.0, next_packet.as_ptr(), next_packet.len() as i32, out.as_mut_ptr(), frames, 1) };
        if n > 0 { n as usize * 2 } else { 0 }
    }

    /// A copy of the decoder's whole state, to go back to with [`Decoder::restore`]. libopus keeps its state in
    /// one block with offsets rather than pointers inside, so it can be copied as bytes.
    pub fn snapshot(&self) -> Vec<u8> {
        let size = unsafe { opus::opus_decoder_get_size(2) } as usize;
        unsafe { std::slice::from_raw_parts(self.0 as *const u8, size) }.to_vec()
    }

    /// Puts back a state from [`Decoder::snapshot`] of this decoder.
    pub fn restore(&mut self, state: &[u8]) {
        let size = unsafe { opus::opus_decoder_get_size(2) } as usize;
        if state.len() == size {
            unsafe { std::ptr::copy_nonoverlapping(state.as_ptr(), self.0 as *mut u8, size) };
        }
    }
}

/// Whether an Opus packet can carry a copy of the frame before it (forward error correction): only voice
/// modes do, SILK (configurations 0–11) and hybrid (12–15), not CELT (16–31).
pub fn carries_fec(packet: &[u8]) -> bool {
    packet.first().is_some_and(|toc| toc >> 3 < 16) && packet != SILENCE
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus::opus_decoder_destroy(self.0) }
    }
}

/// Measurements shared by the tests.
#[cfg(test)]
pub(crate) mod tests_support {
    /// What of `y` isn't made of the frequencies `freqs` (each fitted with any amplitude and phase), against
    /// its whole power, in dB: distortion and noise (THD+N), whatever the delay.
    pub(crate) fn thd_n(y: &[f32], freqs: &[f32]) -> f32 {
        let n = y.len();
        let tau = std::f64::consts::TAU;
        // Least squares on sin/cos pairs (near-orthogonal over many periods): project, subtract, repeat.
        let mut residual: Vec<f64> = y.iter().map(|&v| v as f64).collect();
        for _ in 0..3 {
            for &f in freqs {
                let w = tau * f as f64 / 48_000.0;
                let (mut s, mut c) = (0.0, 0.0);
                for (i, r) in residual.iter().enumerate() {
                    s += r * (w * i as f64).sin();
                    c += r * (w * i as f64).cos();
                }
                let (s, c) = (2.0 * s / n as f64, 2.0 * c / n as f64);
                for (i, r) in residual.iter_mut().enumerate() {
                    *r -= s * (w * i as f64).sin() + c * (w * i as f64).cos();
                }
            }
        }
        let rest: f64 = residual.iter().map(|v| v * v).sum();
        let all: f64 = y.iter().map(|&v| (v as f64).powi(2)).sum();
        (10.0 * (rest / all).log10()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_tone() {
        let mut enc = Encoder::new(64_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut packet = [0u8; 1500];
        let mut out = vec![0f32; FRAME * 2];
        let mut energy = 0.0;
        for f in 0..10 {
            let pcm: Vec<f32> = (0..FRAME).map(|i| ((f * FRAME + i) as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
            let n = enc.encode(&pcm, &mut packet).unwrap();
            let samples = dec.decode(Some(&packet[..n]), &mut out);
            assert_eq!(samples, FRAME * 2);
            energy = out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32;
        }
        // The decoded tone should carry roughly the input's energy (0.125 for a 0.5 sine).
        assert!(energy > 0.05, "decoded energy {energy}");
        // Concealment produces a frame too.
        assert_eq!(dec.decode(None, &mut out), FRAME * 2);
    }

    /// Distortion through the codec alone (diagnostic; prints).
    #[test]
    #[ignore]
    fn diag_codec_distortion() {
        let run = |name: &str, freqs: &[f32], amp: f32, bitrate: i32| {
            let tau = std::f32::consts::TAU;
            let signal = |i: usize| freqs.iter().enumerate().map(|(h, f)| (i as f32 * f * tau / 48_000.0).sin() * amp / (h + 1) as f32).sum::<f32>();
            let mut enc = Encoder::new(bitrate).unwrap();
            let mut dec = Decoder::new().unwrap();
            let mut packet = [0u8; 1500];
            let mut out = vec![0f32; FRAME * 2];
            let input: Vec<f32> = (0..100 * FRAME).map(signal).collect();
            let mut decoded = Vec::new();
            for f in 0..100 {
                let n = enc.encode(&input[f * FRAME..(f + 1) * FRAME], &mut packet).unwrap();
                let k = dec.decode(Some(&packet[..n]), &mut out);
                decoded.extend(out[..k].chunks_exact(2).map(|c| c[0]));
            }
            let window = &decoded[20 * FRAME..80 * FRAME];
            let thd_n = super::tests_support::thd_n;
            println!("{name:<26} {bitrate:>6} bps: THD+N {:6.1} dB (input {:6.1} dB)", thd_n(window, freqs), thd_n(&input[20 * FRAME..80 * FRAME], freqs));
        };
        for bitrate in [32_000, 64_000] {
            run("440 Hz sine, -6 dBFS", &[440.0], 0.5, bitrate);
            run("1 kHz sine, -20 dBFS", &[1000.0], 0.1, bitrate);
            run("3 kHz sine, -20 dBFS", &[3000.0], 0.1, bitrate);
            let harmonics: Vec<f32> = (1..20).map(|h| 150.0 * h as f32).collect();
            run("150 Hz + harmonics", &harmonics, 0.3, bitrate);
        }
    }

    /// Decodes the official Opus test vectors (RFC 8251; the folder in OPUS_VECTORS) and compares with the
    /// reference decoder: each packet's final range (bit-exact parsing) and the output (diagnostic; prints).
    #[test]
    #[ignore]
    fn diag_conformance_vectors() {
        let dir = std::path::PathBuf::from(std::env::var("OPUS_VECTORS").expect("OPUS_VECTORS"));
        for v in 1..=12 {
            let bits = std::fs::read(dir.join(format!("testvector{v:02}.bit"))).unwrap();
            let reference = std::fs::read(dir.join(format!("testvector{v:02}.dec"))).unwrap();
            let reference: Vec<i16> = reference.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            let mut err = 0;
            let dec = unsafe { opus::opus_decoder_create(48_000, 2, &mut err) };
            let mut out = vec![0i16; 5760 * 2];
            let mut decoded: Vec<i16> = Vec::new();
            let (mut at, mut packets, mut range_mismatches) = (0, 0, 0);
            while at + 8 <= bits.len() {
                let len = u32::from_be_bytes(bits[at..at + 4].try_into().unwrap()) as usize;
                let range = u32::from_be_bytes(bits[at + 4..at + 8].try_into().unwrap());
                let data = &bits[at + 8..at + 8 + len];
                at += 8 + len;
                let n = unsafe { opus::opus_decode(dec, if len == 0 { std::ptr::null() } else { data.as_ptr() }, len as i32, out.as_mut_ptr(), 5760, 0) };
                if n > 0 {
                    decoded.extend_from_slice(&out[..n as usize * 2]);
                }
                let mut ours = 0u32;
                unsafe { opus::opus_decoder_ctl!(dec, opus::OPUS_GET_FINAL_RANGE_REQUEST, &mut ours) };
                packets += 1;
                if len > 0 && ours != range {
                    range_mismatches += 1;
                }
            }
            unsafe { opus::opus_decoder_destroy(dec) };
            let n = decoded.len().min(reference.len());
            let (mut sig, mut diff) = (0f64, 0f64);
            for i in 0..n {
                sig += (reference[i] as f64).powi(2);
                diff += (reference[i] as f64 - decoded[i] as f64).powi(2);
            }
            let snr = if diff == 0.0 { f64::INFINITY } else { 10.0 * (sig / diff).log10() };
            println!("vector {v:02}: {packets:>5} packets, {range_mismatches:>4} final-range mismatches, SNR vs reference {snr:6.1} dB, length {} vs {}", decoded.len(), reference.len());
        }
    }

    /// What restarting the encoder for each talk spurt does to how a listener (one decoder for the whole call)
    /// hears the next spurt, against carrying the encoder on (diagnostic; prints).
    #[test]
    #[ignore]
    fn diag_encoder_reset_between_spurts() {
        let freqs: Vec<f32> = (1..=10).map(|h| 150.0 * h as f32).collect();
        let tau = std::f32::consts::TAU;
        let voice = |i: usize| freqs.iter().enumerate().map(|(h, f)| (i as f32 * f * tau / 48_000.0).sin() * 0.25 / (h + 1) as f32).sum::<f32>();
        for reset in [false, true] {
            let mut enc = Encoder::new(64_000).unwrap();
            let mut dec = Decoder::new().unwrap();
            let mut packet = [0u8; 1500];
            let mut out = vec![0f32; FRAME * 2];
            let mut decode = |dec: &mut Decoder, p: &[u8]| {
                let k = dec.decode(Some(p), &mut out);
                out[..k].chunks_exact(2).map(|c| c[0]).collect::<Vec<f32>>()
            };
            // A first spurt of a second, five silence frames (as sent), then the next spurt.
            for f in 0..50 {
                let pcm: Vec<f32> = (0..FRAME).map(|i| voice(f * FRAME + i)).collect();
                let n = enc.encode(&pcm, &mut packet).unwrap();
                decode(&mut dec, &packet[..n]);
            }
            for _ in 0..5 {
                decode(&mut dec, &SILENCE);
            }
            if reset {
                enc.reset();
            }
            let mut second = Vec::new();
            for f in 0..25 {
                let pcm: Vec<f32> = (0..FRAME).map(|i| voice((f + 100) * FRAME + i)).collect();
                let n = enc.encode(&pcm, &mut packet).unwrap();
                second.extend(decode(&mut dec, &packet[..n]));
            }
            let thd = |from: usize, to: usize| super::tests_support::thd_n(&second[from * FRAME..to * FRAME], &freqs);
            let peak = second.iter().fold(0f32, |m, v| m.max(v.abs()));
            println!(
                "encoder {}: next spurt THD+N 20-100 ms {:6.1} dB, 100-200 ms {:6.1} dB, 200-500 ms {:6.1} dB, peak {peak:.2}",
                if reset { "restarted " } else { "carried on" },
                thd(1, 5),
                thd(5, 10),
                thd(10, 25)
            );
        }
    }

    #[test]
    fn restoring_a_snapshot_undoes_decoding() {
        let mut enc = Encoder::new(64_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut packet = [0u8; 1500];
        let mut out = vec![0f32; FRAME * 2];
        let tone = |f: usize| (0..FRAME).map(|i| ((f * FRAME + i) as f32 * 0.05).sin() * 0.4).collect::<Vec<f32>>();
        let packets: Vec<Vec<u8>> = (0..6)
            .map(|f| {
                let n = enc.encode(&tone(f), &mut packet).unwrap();
                packet[..n].to_vec()
            })
            .collect();
        for p in &packets[..4] {
            dec.decode(Some(p), &mut out);
        }
        let saved = dec.snapshot();
        dec.decode(Some(&packets[4]), &mut out);
        let first = out.clone();
        // Something else happens (here, concealment), then the state is put back: decoding again gives the
        // same result as the first time.
        dec.decode(None, &mut out);
        dec.restore(&saved);
        dec.decode(Some(&packets[4]), &mut out);
        assert_eq!(out, first);
    }
}
