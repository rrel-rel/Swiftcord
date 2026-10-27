//! Decoding Ogg Opus files (Discord voice messages) to 48 kHz stereo with the libopus a call already uses.

use crate::codec::{Decoder, FRAME};

/// The Opus packets in an Ogg stream, in order (the first two are the OpusHead and OpusTags headers).
fn packets(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut at = 0;
    while at + 27 <= data.len() {
        if &data[at..at + 4] != b"OggS" {
            return Err("not an Ogg file".into());
        }
        let segments = data[at + 26] as usize;
        let table = data.get(at + 27..at + 27 + segments).ok_or("truncated Ogg page")?;
        let mut body = at + 27 + segments;
        for &lace in table {
            let seg = data.get(body..body + lace as usize).ok_or("truncated Ogg page")?;
            partial.extend_from_slice(seg);
            body += lace as usize;
            // A lacing value under 255 ends a packet; 255 continues it (possibly onto the next page).
            if lace < 255 {
                out.push(std::mem::take(&mut partial));
            }
        }
        at = body;
    }
    Ok(out)
}

/// Decodes an Ogg Opus file to interleaved 48 kHz stereo.
pub fn decode_ogg_opus(data: &[u8]) -> Result<Vec<f32>, String> {
    let packets = packets(data)?;
    let head = packets.first().filter(|h| h.starts_with(b"OpusHead") && h.len() >= 19).ok_or("not an Opus file")?;
    // Samples (at 48 kHz) the encoder added at the start, to be dropped.
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
    let mut decoder = Decoder::new().ok_or("no Opus decoder")?;
    // The longest Opus packet is 120 ms.
    let mut buf = vec![0f32; FRAME * 6 * 2];
    let mut pcm = Vec::new();
    for p in packets.iter().skip(2).filter(|p| !p.is_empty()) {
        let n = decoder.decode(Some(p), &mut buf);
        pcm.extend_from_slice(&buf[..n]);
    }
    pcm.drain(..(pre_skip * 2).min(pcm.len()));
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::Encoder;

    /// One Ogg page holding `packets` (each under 255 × 255 bytes).
    fn page(packets: &[&[u8]]) -> Vec<u8> {
        let mut lacing = Vec::new();
        for p in packets {
            lacing.extend(std::iter::repeat_n(255u8, p.len() / 255));
            lacing.push((p.len() % 255) as u8);
        }
        let mut out = b"OggS".to_vec();
        out.extend_from_slice(&[0; 22]);
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        for p in packets {
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn decodes_a_small_file() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 1]);
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&[0; 7]);
        let mut enc = Encoder::new(64_000).unwrap();
        let mut frames = Vec::new();
        for f in 0..10 {
            let pcm: Vec<f32> = (0..FRAME).map(|i| ((f * FRAME + i) as f32 * 0.05).sin() * 0.5).collect();
            let mut buf = [0u8; 1400];
            let n = enc.encode(&pcm, &mut buf).unwrap();
            frames.push(buf[..n].to_vec());
        }
        let mut file = page(&[&head]);
        file.extend(page(&[b"OpusTags"]));
        // A packet split across two pages, as long ones are.
        let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
        file.extend(page(&refs[..5]));
        file.extend(page(&refs[5..]));
        let pcm = decode_ogg_opus(&file).unwrap();
        assert_eq!(pcm.len(), (10 * FRAME - 312) * 2);
        assert!(pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32 > 0.01);
        assert!(decode_ogg_opus(b"RIFF....").is_err());
    }

    /// Decodes a real voice message file from `SWIFTCORD_TEST_OGG` and prints its length.
    #[test]
    #[ignore]
    fn decodes_a_real_voice_message() {
        let path = std::env::var("SWIFTCORD_TEST_OGG").expect("SWIFTCORD_TEST_OGG");
        let pcm = decode_ogg_opus(&std::fs::read(path).unwrap()).unwrap();
        let rms = (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt();
        println!("{:.2} s, rms {rms:.3}", pcm.len() as f64 / 2.0 / 48_000.0);
        assert!(rms > 0.001);
    }

    #[test]
    fn joins_packets_continued_across_pages() {
        let big = vec![7u8; 300];
        // 255 bytes end the first page (lacing 255: continues), the rest start the next.
        let mut file = b"OggS".to_vec();
        file.extend_from_slice(&[0; 22]);
        file.extend_from_slice(&[1, 255]);
        file.extend_from_slice(&big[..255]);
        file.extend(page(&[&big[255..]]));
        assert_eq!(packets(&file).unwrap(), vec![big]);
    }
}
