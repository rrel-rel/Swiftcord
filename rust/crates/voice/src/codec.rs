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
