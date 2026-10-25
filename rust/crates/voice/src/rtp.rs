//! RTP packets and Discord's transport encryption (the "rtpsize" AEAD modes).
//!
//! Outgoing: `header (12) ‖ AEAD(payload, aad = header) ‖ nonce counter (4, big-endian)`.
//! Incoming packets may carry CSRCs and a header extension; the extension's 4-byte preamble is part of the
//! authenticated header while its body is encrypted with the payload and stripped after decrypting.

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes256Gcm;
use chacha20poly1305::XChaCha20Poly1305;

pub const OPUS_PAYLOAD_TYPE: u8 = 120;
const TAG: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Aes256Gcm,
    XChaCha20,
}

impl Mode {
    pub const PREFERRED: [(&'static str, Mode); 2] =
        [("aead_aes256_gcm_rtpsize", Mode::Aes256Gcm), ("aead_xchacha20_poly1305_rtpsize", Mode::XChaCha20)];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Aes256Gcm => "aead_aes256_gcm_rtpsize",
            Mode::XChaCha20 => "aead_xchacha20_poly1305_rtpsize",
        }
    }

    /// The best mode the server offers.
    pub fn choose(offered: &[String]) -> Option<Mode> {
        Self::PREFERRED.iter().find(|(n, _)| offered.iter().any(|o| o == n)).map(|(_, m)| *m)
    }
}

pub enum Cipher {
    Aes(Box<Aes256Gcm>),
    XChaCha(Box<XChaCha20Poly1305>),
}

impl Cipher {
    pub fn new(mode: Mode, key: &[u8]) -> Option<Cipher> {
        Some(match mode {
            Mode::Aes256Gcm => Cipher::Aes(Box::new(Aes256Gcm::new_from_slice(key).ok()?)),
            Mode::XChaCha20 => Cipher::XChaCha(Box::new(XChaCha20Poly1305::new_from_slice(key).ok()?)),
        })
    }

    fn seal(&self, counter: u32, aad: &[u8], buf: &mut Vec<u8>) -> bool {
        match self {
            Cipher::Aes(c) => {
                let mut nonce = [0u8; 12];
                nonce[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place(&nonce.into(), aad, buf).is_ok()
            }
            Cipher::XChaCha(c) => {
                let mut nonce = [0u8; 24];
                nonce[..4].copy_from_slice(&counter.to_be_bytes());
                c.encrypt_in_place(&nonce.into(), aad, buf).is_ok()
            }
        }
    }

    fn open(&self, nonce4: [u8; 4], aad: &[u8], buf: &mut Vec<u8>) -> bool {
        match self {
            Cipher::Aes(c) => {
                let mut nonce = [0u8; 12];
                nonce[..4].copy_from_slice(&nonce4);
                c.decrypt_in_place(&nonce.into(), aad, buf).is_ok()
            }
            Cipher::XChaCha(c) => {
                let mut nonce = [0u8; 24];
                nonce[..4].copy_from_slice(&nonce4);
                c.decrypt_in_place(&nonce.into(), aad, buf).is_ok()
            }
        }
    }
}

/// Builds an encrypted RTP packet.
pub fn seal(cipher: &Cipher, payload_type: u8, seq: u16, timestamp: u32, ssrc: u32, counter: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut header = [0u8; 12];
    header[0] = 0x80;
    header[1] = payload_type;
    header[2..4].copy_from_slice(&seq.to_be_bytes());
    header[4..8].copy_from_slice(&timestamp.to_be_bytes());
    header[8..12].copy_from_slice(&ssrc.to_be_bytes());
    let mut body = Vec::with_capacity(payload.len() + TAG);
    body.extend_from_slice(payload);
    if !cipher.seal(counter, &header, &mut body) {
        return None;
    }
    let mut out = Vec::with_capacity(12 + body.len() + 4);
    out.extend_from_slice(&header);
    out.extend_from_slice(&body);
    out.extend_from_slice(&counter.to_be_bytes());
    Some(out)
}

/// An encrypted RTCP Picture Loss Indication, asking the sender of `media_ssrc` for a keyframe. As with RTP,
/// the 8-byte header is authenticated in the clear and the rest encrypted, with the nonce counter appended.
pub fn seal_pli(cipher: &Cipher, our_ssrc: u32, media_ssrc: u32, counter: u32) -> Option<Vec<u8>> {
    // V=2, FMT=1 (PLI), PT=206 (payload-specific feedback), length 2 (in 32-bit words, minus one).
    let mut header = [0x81, 206, 0, 2, 0, 0, 0, 0];
    header[4..8].copy_from_slice(&our_ssrc.to_be_bytes());
    let mut body = media_ssrc.to_be_bytes().to_vec();
    if !cipher.seal(counter, &header, &mut body) {
        return None;
    }
    let mut out = header.to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&counter.to_be_bytes());
    Some(out)
}

#[derive(Debug)]
pub struct Packet {
    pub payload_type: u8,
    pub marker: bool,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

/// RTCP shares the socket; its second byte is a packet type in 200..=204.
pub fn is_rtcp(data: &[u8]) -> bool {
    data.len() >= 2 && (200..=204).contains(&data[1])
}

pub fn open(cipher: &Cipher, data: &[u8]) -> Option<Packet> {
    if data.len() < 12 + TAG + 4 || data[0] >> 6 != 2 || is_rtcp(data) {
        return None;
    }
    let csrc = (data[0] & 0x0f) as usize;
    let has_ext = data[0] & 0x10 != 0;
    let mut aad_len = 12 + csrc * 4;
    let mut ext_words = 0usize;
    if has_ext {
        let pre = data.get(aad_len..aad_len + 4)?;
        ext_words = u16::from_be_bytes([pre[2], pre[3]]) as usize;
        aad_len += 4;
    }
    let nonce_at = data.len() - 4;
    if nonce_at < aad_len + TAG {
        return None;
    }
    let mut buf = data[aad_len..nonce_at].to_vec();
    let nonce: [u8; 4] = data[nonce_at..].try_into().ok()?;
    if !cipher.open(nonce, &data[..aad_len], &mut buf) {
        return None;
    }
    let skip = ext_words * 4;
    if buf.len() < skip {
        return None;
    }
    buf.drain(..skip);
    Some(Packet {
        payload_type: data[1] & 0x7f,
        marker: data[1] & 0x80 != 0,
        seq: u16::from_be_bytes([data[2], data[3]]),
        timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        ssrc: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        payload: buf,
    })
}

/// The 74-byte IP discovery request.
pub fn discovery_request(ssrc: u32) -> [u8; 74] {
    let mut p = [0u8; 74];
    p[0..2].copy_from_slice(&1u16.to_be_bytes());
    p[2..4].copy_from_slice(&70u16.to_be_bytes());
    p[4..8].copy_from_slice(&ssrc.to_be_bytes());
    p
}

/// Our public address as seen by the voice server.
pub fn parse_discovery(resp: &[u8]) -> Option<(String, u16)> {
    if resp.len() < 74 || u16::from_be_bytes([resp[0], resp[1]]) != 2 {
        return None;
    }
    let ip = &resp[8..72];
    let end = ip.iter().position(|&b| b == 0).unwrap_or(ip.len());
    let ip = std::str::from_utf8(&ip[..end]).ok()?.to_owned();
    let port = u16::from_be_bytes([resp[72], resp[73]]);
    Some((ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_both_modes() {
        for mode in [Mode::Aes256Gcm, Mode::XChaCha20] {
            let c = Cipher::new(mode, &[7u8; 32]).unwrap();
            let pkt = seal(&c, OPUS_PAYLOAD_TYPE, 9, 960, 1234, 42, b"opus!").unwrap();
            let p = open(&c, &pkt).unwrap();
            assert_eq!((p.seq, p.timestamp, p.ssrc, p.payload_type), (9, 960, 1234, OPUS_PAYLOAD_TYPE));
            assert_eq!(p.payload, b"opus!");
            // Tampering fails authentication.
            let mut bad = pkt.clone();
            bad[13] ^= 1;
            assert!(open(&c, &bad).is_none());
        }
    }

    #[test]
    fn strips_encrypted_header_extension() {
        // Build a packet whose header has an extension preamble and whose encrypted body starts with one
        // extension word, as Discord's clients send.
        let c = Cipher::new(Mode::Aes256Gcm, &[1u8; 32]).unwrap();
        let mut header = vec![0x90, 120, 0, 1, 0, 0, 0, 0, 0, 0, 0, 5, 0xBE, 0xDE, 0, 1];
        let mut body = vec![0x10, 0xAA, 0, 0];
        body.extend_from_slice(b"audio");
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&3u32.to_be_bytes());
        let Cipher::Aes(aes) = &c else { unreachable!() };
        aes.encrypt_in_place(&nonce.into(), &header, &mut body).unwrap();
        header.extend_from_slice(&body);
        header.extend_from_slice(&3u32.to_be_bytes());
        let p = open(&c, &header).unwrap();
        assert_eq!(p.payload, b"audio");
        assert_eq!(p.ssrc, 5);
    }

    #[test]
    fn parses_discovery() {
        let mut r = [0u8; 74];
        r[1] = 2;
        r[8..17].copy_from_slice(b"203.0.113");
        r[72..74].copy_from_slice(&50000u16.to_be_bytes());
        assert_eq!(parse_discovery(&r), Some(("203.0.113".into(), 50000)));
    }
}
