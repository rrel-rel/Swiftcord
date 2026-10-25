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
}
