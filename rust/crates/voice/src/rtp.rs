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
    seal_marked(cipher, payload_type, false, seq, timestamp, ssrc, counter, payload)
}

/// [`seal`] with the marker bit, which ends a video frame.
#[allow(clippy::too_many_arguments)]
pub fn seal_marked(cipher: &Cipher, payload_type: u8, marker: bool, seq: u16, timestamp: u32, ssrc: u32, counter: u32, payload: &[u8]) -> Option<Vec<u8>> {
    let mut header = [0u8; 12];
    header[0] = 0x80;
    header[1] = payload_type | if marker { 0x80 } else { 0 };
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

/// An encrypted RTCP generic NACK asking for specific lost packets to be retransmitted (RFC 4585): each entry
/// is a sequence number plus a bitmask of the 16 following ones.
pub fn seal_nack(cipher: &Cipher, our_ssrc: u32, media_ssrc: u32, lost: &[u16], counter: u32) -> Option<Vec<u8>> {
    let mut fci: Vec<(u16, u16)> = Vec::new();
    for &seq in lost {
        match fci.last_mut() {
            Some((pid, blp)) if seq.wrapping_sub(*pid).wrapping_sub(1) < 16 && seq != *pid => {
                *blp |= 1 << seq.wrapping_sub(*pid).wrapping_sub(1);
            }
            _ => fci.push((seq, 0)),
        }
    }
    if fci.is_empty() {
        return None;
    }
    let words = 2 + fci.len() as u16; // length field: total 32-bit words minus one
    let mut header = [0x81, 205, 0, 0, 0, 0, 0, 0];
    header[2..4].copy_from_slice(&words.to_be_bytes());
    header[4..8].copy_from_slice(&our_ssrc.to_be_bytes());
    let mut body = media_ssrc.to_be_bytes().to_vec();
    for (pid, blp) in fci {
        body.extend_from_slice(&pid.to_be_bytes());
        body.extend_from_slice(&blp.to_be_bytes());
    }
    if !cipher.seal(counter, &header, &mut body) {
        return None;
    }
    let mut out = header.to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&counter.to_be_bytes());
    Some(out)
}

/// What viewers (through the voice server) ask of the video we send.
#[derive(Debug, PartialEq, Eq)]
pub enum Feedback {
    /// Picture Loss Indication or Full Intra Request: send a keyframe.
    Keyframe,
    /// Retransmit these packets.
    Nack(Vec<u16>),
}

/// Decrypts an RTCP compound packet and pulls out keyframe requests and NACKs.
pub fn open_feedback(cipher: &Cipher, data: &[u8]) -> Vec<Feedback> {
    let mut out = Vec::new();
    // RTCP packet types are 200–207; RTP's second byte (marker + payload type) never is for Discord's types.
    if data.len() < 8 + TAG + 4 || !(200..=207).contains(&data[1]) {
        return out;
    }
    let nonce_at = data.len() - 4;
    let mut body = data[8..nonce_at].to_vec();
    let nonce: [u8; 4] = data[nonce_at..].try_into().unwrap_or_default();
    if !cipher.open(nonce, &data[..8], &mut body) {
        return out;
    }
    let mut plain = data[..8].to_vec();
    plain.extend_from_slice(&body);
    let mut rest = &plain[..];
    while rest.len() >= 4 {
        let (fmt, pt) = (rest[0] & 0x1f, rest[1]);
        let len = (u16::from_be_bytes([rest[2], rest[3]]) as usize + 1) * 4;
        let Some(packet) = rest.get(..len) else { break };
        match (pt, fmt) {
            (206, 1) | (206, 4) => out.push(Feedback::Keyframe),
            (205, 1) => {
                let mut lost = Vec::new();
                for fci in packet.get(12..).unwrap_or_default().chunks_exact(4) {
                    let pid = u16::from_be_bytes([fci[0], fci[1]]);
                    let blp = u16::from_be_bytes([fci[2], fci[3]]);
                    lost.push(pid);
                    lost.extend((0..16).filter(|b| blp & (1 << b) != 0).map(|b| pid.wrapping_add(b + 1)));
                }
                out.push(Feedback::Nack(lost));
            }
            _ => {}
        }
        rest = &rest[len..];
    }
    out
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
    // With the padding bit set, the (decrypted) payload ends in filler whose last byte gives its length. Video
    // uses it a lot, including padding-only packets for bandwidth probing.
    if data[0] & 0x20 != 0 {
        let pad = *buf.last()? as usize;
        if pad == 0 || pad > buf.len() {
            return None;
        }
        buf.truncate(buf.len() - pad);
    }
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
    fn nack_packs_neighbours_into_bitmasks() {
        let c = Cipher::new(Mode::Aes256Gcm, &[3u8; 32]).unwrap();
        let p = seal_nack(&c, 1, 2, &[100, 101, 103, 200], 0).unwrap();
        // Header + media SSRC + two FCI entries + tag + nonce.
        assert_eq!(p.len(), 8 + 4 + 8 + 16 + 4);
        assert_eq!(u16::from_be_bytes([p[2], p[3]]), 4);
        let Cipher::Aes(aes) = &c else { unreachable!() };
        let mut body = p[8..p.len() - 4].to_vec();
        aes.decrypt_in_place(&[0u8; 12].into(), &p[..8], &mut body).unwrap();
        // 100 with 101 (bit 0) and 103 (bit 2); then 200 alone.
        assert_eq!(&body[4..], &[0, 100, 0, 0b101, 0, 200, 0, 0]);
    }

    #[test]
    fn reads_back_feedback() {
        let c = Cipher::new(Mode::XChaCha20, &[4u8; 32]).unwrap();
        let nack = seal_nack(&c, 1, 2, &[100, 101, 103, 200], 5).unwrap();
        assert_eq!(open_feedback(&c, &nack), [Feedback::Nack(vec![100, 101, 103, 200])]);
        let pli = seal_pli(&c, 1, 2, 6).unwrap();
        assert_eq!(open_feedback(&c, &pli), [Feedback::Keyframe]);
        let marked = seal_marked(&c, 101, true, 1, 2, 3, 7, b"x").unwrap();
        assert!(open(&c, &marked).unwrap().marker);
    }

    #[test]
    fn strips_padding() {
        let c = Cipher::new(Mode::Aes256Gcm, &[2u8; 32]).unwrap();
        let mut header = vec![0xA0, 101, 0, 7, 0, 0, 0, 9, 0, 0, 0, 5];
        let mut body = b"frame".to_vec();
        body.extend_from_slice(&[0, 0, 3]);
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&1u32.to_be_bytes());
        let Cipher::Aes(aes) = &c else { unreachable!() };
        aes.encrypt_in_place(&nonce.into(), &header, &mut body).unwrap();
        header.extend_from_slice(&body);
        header.extend_from_slice(&1u32.to_be_bytes());
        assert_eq!(open(&c, &header).unwrap().payload, b"frame");
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
