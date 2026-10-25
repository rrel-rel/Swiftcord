//! Received video: H.264 RTP depacketizing into Annex-B access units, decoding through Windows' Media
//! Foundation H.264 decoder (hardware-accelerated where available, nothing to ship), and NV12 → RGBA.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

pub const H264_PAYLOAD_TYPE: u8 = 101;
pub const H264_RTX_PAYLOAD_TYPE: u8 = 102;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// One complete encoded frame.
#[derive(Debug)]
pub struct EncodedFrame {
    /// Annex-B: each NAL unit preceded by a 4-byte start code, as WebRTC's depacketizer produces (and as
    /// DAVE frame decryption expects).
    pub data: Vec<u8>,
    pub timestamp: u32,
    pub keyframe: bool,
}

/// Gaps unfilled for this long are given up on (the frame is dropped and a keyframe requested).
const GIVE_UP: Duration = Duration::from_millis(300);
/// How often a missing packet is re-requested, and how many times.
const NACK_INTERVAL: Duration = Duration::from_millis(60);
const NACK_TRIES: u8 = 3;

#[derive(Default, Debug, Clone, Copy)]
pub struct VideoStats {
    pub packets: u64,
    pub retransmitted: u64,
    pub frames: u64,
    pub lost_frames: u64,
    pub nacked: u64,
}

/// Reassembles one video SSRC's RTP packets (RFC 6184: single NAL units, STAP-A and FU-A) into frames, in
/// sequence order. Packets may arrive out of order or be retransmitted; missing ones are reported for NACKs
/// and, if they don't turn up in time, the frame is dropped and later frames are held back until the next
/// keyframe (which the caller should request).
pub struct VideoReceiver {
    packets: BTreeMap<u64, (u32, bool, Vec<u8>)>,
    highest: Option<u64>,
    /// The first sequence number of the next frame.
    next: Option<u64>,
    gap_since: Option<(u64, Instant)>,
    nacked: HashMap<u64, (Instant, u8)>,
    /// Holding frames back until a keyframe.
    broken: bool,
    pub needs_keyframe: bool,
    pub stats: VideoStats,
}

impl Default for VideoReceiver {
    /// Joining a stream midway: frames are held back until the first keyframe, which should be requested.
    fn default() -> Self {
        Self {
            packets: BTreeMap::new(),
            highest: None,
            next: None,
            gap_since: None,
            nacked: HashMap::new(),
            broken: true,
            needs_keyframe: true,
            stats: VideoStats::default(),
        }
    }
}

impl VideoReceiver {
    fn unwrap_seq(&self, seq: u16) -> u64 {
        let Some(h) = self.highest else { return (1 << 16) + seq as u64 };
        let base = h & !0xffff;
        [base.wrapping_sub(1 << 16), base, base + (1 << 16)].into_iter().map(|b| b + seq as u64).min_by_key(|&c| c.abs_diff(h)).unwrap()
    }

    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: Vec<u8>, retransmitted: bool) {
        let ext = self.unwrap_seq(seq);
        self.stats.packets += 1;
        if retransmitted {
            self.stats.retransmitted += 1;
        }
        if self.next.is_some_and(|n| ext < n) {
            return; // Already played past it.
        }
        self.next.get_or_insert(ext);
        self.highest = Some(self.highest.map_or(ext, |h| h.max(ext)));
        self.nacked.remove(&ext);
        // Padding-only packets (bandwidth probes) carry no media but still occupy a sequence number.
        self.packets.insert(ext, (timestamp, marker, payload));
    }

    /// Emits completed frames, and gives up on gaps that have waited too long.
    pub fn poll(&mut self) -> Vec<EncodedFrame> {
        let mut out = Vec::new();
        while let Some(start) = self.next {
            // Walk forward from the start of the next frame to its marker packet.
            let mut s = start;
            let complete = loop {
                match self.packets.get(&s) {
                    Some(&(_, true, _)) => break true,
                    Some(_) => s += 1,
                    None => break false,
                }
            };
            if complete {
                let parts: Vec<(u32, bool, Vec<u8>)> = (start..=s).filter_map(|k| self.packets.remove(&k)).collect();
                self.next = Some(s + 1);
                self.gap_since = None;
                let ts = parts.last().map_or(0, |p| p.0);
                // Packets from an earlier timestamp would be the tail of a frame we joined midway; skip them.
                let payloads: Vec<Vec<u8>> = parts.into_iter().filter(|p| p.0 == ts && !p.2.is_empty()).map(|p| p.2).collect();
                if let Some(frame) = self.emit(&payloads, ts) {
                    out.push(frame);
                }
                continue;
            }
            // A gap at `s`. If nothing has arrived beyond it, we're simply waiting for the rest of the frame.
            if self.highest.is_none_or(|h| h <= s) {
                break;
            }
            let since = match self.gap_since {
                Some((g, t)) if g == s => t,
                _ => {
                    self.gap_since = Some((s, Instant::now()));
                    Instant::now()
                }
            };
            if since.elapsed() < GIVE_UP && self.packets.len() < 2000 {
                break;
            }
            // Give up: skip to just past the next frame boundary after the gap.
            self.stats.lost_frames += 1;
            self.broken = true;
            self.needs_keyframe = true;
            self.gap_since = None;
            let boundary = self.packets.range(s..).find(|(_, p)| p.1).map(|(&k, _)| k);
            match boundary {
                Some(b) => {
                    self.packets.retain(|&k, _| k > b);
                    self.next = Some(b + 1);
                }
                None => {
                    // No complete frame yet beyond the gap: restart from what's buffered.
                    self.next = self.packets.keys().next().copied();
                    if self.next.is_none() {
                        break;
                    }
                }
            }
        }
        self.nacked.retain(|&k, _| self.next.is_none_or(|n| k >= n));
        out
    }

    fn emit(&mut self, payloads: &[Vec<u8>], ts: u32) -> Option<EncodedFrame> {
        let Some((data, keyframe)) = assemble(payloads) else {
            return None;
        };
        if self.broken && !keyframe {
            return None;
        }
        if keyframe {
            self.broken = false;
            self.needs_keyframe = false;
        }
        self.stats.frames += 1;
        Some(EncodedFrame { data, timestamp: ts, keyframe })
    }

    /// Sequence numbers to NACK now: missing ones between the next frame and the newest packet, each asked for
    /// at most a few times.
    pub fn nacks(&mut self) -> Vec<u16> {
        let (Some(next), Some(high)) = (self.next, self.highest) else { return Vec::new() };
        let now = Instant::now();
        let mut out = Vec::new();
        for k in next..high {
            if out.len() >= 64 {
                break;
            }
            if self.packets.contains_key(&k) {
                continue;
            }
            let e = self.nacked.entry(k).or_insert((now - NACK_INTERVAL * 2, 0));
            if e.1 < NACK_TRIES && now.duration_since(e.0) >= NACK_INTERVAL {
                *e = (now, e.1 + 1);
                out.push(k as u16);
            }
        }
        self.stats.nacked += out.len() as u64;
        out
    }

    /// The decoder lost track (a dropped or undecodable frame): hold back until a keyframe.
    pub fn resync(&mut self) {
        self.broken = true;
        self.needs_keyframe = true;
    }
}

/// Packets of one frame → (Annex-B data, contains a keyframe).
fn assemble(packets: &[Vec<u8>]) -> Option<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut keyframe = false;
    let nal = |out: &mut Vec<u8>, data: &[u8], keyframe: &mut bool| {
        if let Some(&h) = data.first() {
            *keyframe |= matches!(h & 0x1f, 5 | 7);
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(data);
        }
    };
    let mut fu: Option<Vec<u8>> = None;
    for p in packets {
        let &h = p.first()?;
        match h & 0x1f {
            1..=23 => nal(&mut out, p, &mut keyframe),
            24 => {
                // STAP-A: (16-bit size, NAL)…
                let mut rest = &p[1..];
                while rest.len() >= 2 {
                    let size = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                    let unit = rest.get(2..2 + size)?;
                    nal(&mut out, unit, &mut keyframe);
                    rest = &rest[2 + size..];
                }
            }
            28 => {
                // FU-A: indicator, header (S E R type), fragment.
                let &fh = p.get(1)?;
                let (start, end) = (fh & 0x80 != 0, fh & 0x40 != 0);
                if start {
                    fu = Some(vec![(h & 0xe0) | (fh & 0x1f)]);
                }
                let buf = fu.as_mut()?;
                buf.extend_from_slice(&p[2..]);
                if end {
                    let unit = fu.take()?;
                    nal(&mut out, &unit, &mut keyframe);
                }
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some((out, keyframe))
}

/// A decoded picture, ready to upload.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// NV12 (Y plane, then interleaved UV at half resolution) in limited range → RGBA. `stride` is the row pitch
/// of both planes; the UV plane starts `uv_offset` bytes in.
pub fn nv12_to_rgba(src: &[u8], width: usize, height: usize, stride: usize, uv_offset: usize) -> Vec<u8> {
    // BT.709 for HD, BT.601 otherwise, as encoders usually tag them. Coefficients ×256.
    let (rv, gu, gv, bu) = if height >= 720 { (459, 55, 136, 541) } else { (409, 100, 208, 516) };
    let mut out = vec![255u8; width * height * 4];
    for y in 0..height {
        let yrow = &src[y * stride..];
        let uvrow = &src[uv_offset + (y / 2) * stride..];
        let orow = &mut out[y * width * 4..(y + 1) * width * 4];
        for x in 0..width {
            let c = (yrow[x] as i32 - 16) * 298;
            let u = uvrow[x & !1] as i32 - 128;
            let v = uvrow[(x & !1) + 1] as i32 - 128;
            let px = &mut orow[x * 4..x * 4 + 3];
            px[0] = ((c + rv * v + 128) >> 8).clamp(0, 255) as u8;
            px[1] = ((c - gu * u - gv * v + 128) >> 8).clamp(0, 255) as u8;
            px[2] = ((c + bu * u + 128) >> 8).clamp(0, 255) as u8;
        }
    }
    out
}

#[cfg(windows)]
pub use mf::Decoder;

#[cfg(windows)]
mod mf {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    use super::{nv12_to_rgba, Picture};

    /// Windows' built-in H.264 decoder MFT. Create and use it on one thread.
    pub struct Decoder {
        mft: IMFTransform,
        width: usize,
        height: usize,
        stride: usize,
        /// Rows in the decoder's padded Y plane (the UV plane starts after them).
        padded_height: usize,
        /// The picture inside the decoded frame, which is padded to whole 16-pixel macroblocks.
        visible: (usize, usize),
        provides_samples: bool,
        out_size: u32,
        time: i64,
    }

    impl Decoder {
        pub fn new() -> windows::core::Result<Decoder> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
                let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
                if let Ok(attrs) = mft.GetAttributes() {
                    // Output each frame as soon as it's decoded instead of buffering for reordering.
                    let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
                }
                let input = MFCreateMediaType()?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
                mft.SetInputType(0, &input, 0)?;
                let mut d = Decoder { mft, width: 0, height: 0, stride: 0, padded_height: 0, visible: (0, 0), provides_samples: false, out_size: 0, time: 0 };
                d.configure_output()?;
                d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
                Ok(d)
            }
        }

        /// Picks NV12 output and reads the current frame geometry (called again on format changes).
        fn configure_output(&mut self) -> windows::core::Result<()> {
            unsafe {
                let mut i = 0;
                loop {
                    let t = self.mft.GetOutputAvailableType(0, i)?;
                    if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                        self.mft.SetOutputType(0, &t, 0)?;
                        let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                        self.width = (size >> 32) as usize;
                        self.height = (size & 0xffff_ffff) as usize;
                        self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32 as usize).unwrap_or(self.width);
                        self.visible = aperture(&t).unwrap_or((self.width, self.height));
                        break;
                    }
                    i += 1;
                }
                let info = self.mft.GetOutputStreamInfo(0)?;
                self.provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
                self.out_size = info.cbSize;
            }
            Ok(())
        }

        /// Feeds one Annex-B access unit. Pictures that come out are converted for display only when `convert`
        /// is set (conversion costs far more than decoding, and a viewer that has fallen behind only needs the
        /// latest); every frame is still decoded, so later frames have their references.
        pub fn decode(&mut self, annexb: &[u8], convert: bool) -> windows::core::Result<Vec<Picture>> {
            unsafe {
                let buf = MFCreateMemoryBuffer(annexb.len() as u32)?;
                let mut ptr = std::ptr::null_mut();
                buf.Lock(&mut ptr, None, None)?;
                std::ptr::copy_nonoverlapping(annexb.as_ptr(), ptr, annexb.len());
                buf.Unlock()?;
                buf.SetCurrentLength(annexb.len() as u32)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buf)?;
                sample.SetSampleTime(self.time)?;
                self.time += 333_333;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            let mut pictures = Vec::new();
            while let Some(p) = self.drain_one(convert)? {
                pictures.extend(p);
            }
            Ok(pictures)
        }

        /// One output sample: `None` when the decoder needs more input, `Some(None)` for a picture not converted.
        fn drain_one(&mut self, convert: bool) -> windows::core::Result<Option<Option<Picture>>> {
            unsafe {
                let own_sample = if self.provides_samples {
                    None
                } else {
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(self.out_size.max((self.stride.max(1) * self.height.max(1) * 3 / 2) as u32))?)?;
                    Some(s)
                };
                let mut out = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: std::mem::ManuallyDrop::new(own_sample), dwStatus: 0, pEvents: std::mem::ManuallyDrop::new(None) }];
                let mut status = 0;
                let r = self.mft.ProcessOutput(0, &mut out, &mut status);
                let sample = std::mem::ManuallyDrop::take(&mut out[0].pSample);
                drop(std::mem::ManuallyDrop::take(&mut out[0].pEvents));
                match r {
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.configure_output()?;
                        return self.drain_one(convert);
                    }
                    Err(e) => return Err(e),
                    Ok(()) => {}
                }
                let Some(sample) = sample else { return Ok(None) };
                if !convert {
                    return Ok(Some(None));
                }
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut ptr, None, Some(&mut len))?;
                let data = std::slice::from_raw_parts(ptr, len as usize);
                let stride = self.stride.max(self.width);
                // The Y plane may be padded to a multiple of 16 rows; infer from the buffer size.
                self.padded_height = (len as usize * 2 / 3) / stride.max(1);
                let uv_offset = stride * self.padded_height.max(self.height);
                // Only the visible area: the padding columns and rows hold garbage.
                let (w, h) = (self.visible.0.min(self.width), self.visible.1.min(self.height));
                let picture = (data.len() >= uv_offset + stride * self.height / 2)
                    .then(|| Picture { width: w, height: h, rgba: nv12_to_rgba(data, w, h, stride, uv_offset) });
                buffer.Unlock()?;
                Ok(Some(picture))
            }
        }
    }

    /// The visible size of a decoded frame, from its display aperture.
    fn aperture(t: &IMFMediaType) -> Option<(usize, usize)> {
        for key in [MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_GEOMETRIC_APERTURE] {
            let mut area = [0u8; std::mem::size_of::<MFVideoArea>()];
            if unsafe { t.GetBlob(&key, &mut area, None) }.is_ok() {
                let area: MFVideoArea = unsafe { std::ptr::read_unaligned(area.as_ptr().cast()) };
                if area.Area.cx > 0 && area.Area.cy > 0 {
                    return Some((area.Area.cx as usize, area.Area.cy as usize));
                }
            }
        }
        None
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe {
                let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx() -> VideoReceiver {
        VideoReceiver::default()
    }

    #[test]
    fn single_and_fragmented_nals() {
        let mut d = rx();
        // SPS (type 7) alone, then an IDR slice split across two FU-A packets.
        d.push(1, 900, false, vec![0x67, 0xAA], false);
        d.push(2, 900, false, vec![0x7C, 0x85, 1, 2], false);
        d.push(3, 900, true, vec![0x7C, 0x45, 3, 4], false);
        let f = d.poll().pop().unwrap();
        assert!(f.keyframe);
        assert_eq!(f.data, [0, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x65, 1, 2, 3, 4]);
    }

    #[test]
    fn stap_a() {
        let mut d = rx();
        d.push(10, 5, false, vec![0x18, 0, 2, 0x67, 1, 0, 2, 0x68, 2], false);
        d.push(11, 5, true, vec![0x65, 9], false);
        let f = d.poll().pop().unwrap();
        assert_eq!(f.data, [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9]);
    }

    #[test]
    fn reordering_across_frames() {
        let mut d = rx();
        d.push(1, 1, true, vec![0x65, 0], false);
        // Frame 2's marker arrives before its first packet, and frame 3 before both.
        d.push(4, 3, true, vec![0x41, 3], false);
        d.push(3, 2, true, vec![0x7C, 0x41, 2], false);
        assert_eq!(d.poll().len(), 1, "only the keyframe so far; 2 is waiting for packet 2");
        d.push(2, 2, false, vec![0x7C, 0x81, 1], false);
        let frames = d.poll();
        assert_eq!(frames.iter().map(|f| f.timestamp).collect::<Vec<_>>(), [2, 3]);
        assert!(!d.needs_keyframe);
    }

    #[test]
    fn nacks_and_recovers_retransmissions() {
        let mut d = rx();
        d.push(1, 1, true, vec![0x65, 0], false);
        d.poll();
        d.push(2, 2, false, vec![0x7C, 0x81, 1], false);
        d.push(4, 2, true, vec![0x7C, 0x41, 3], false);
        assert!(d.poll().is_empty());
        assert_eq!(d.nacks(), [3]);
        assert!(d.nacks().is_empty(), "not re-requested immediately");
        d.push(3, 2, false, vec![0x7C, 0x01, 2], true);
        let f = d.poll().pop().unwrap();
        assert_eq!(f.data, [0, 0, 0, 1, 0x61, 1, 2, 3]);
        assert_eq!(d.stats.retransmitted, 1);
    }

    #[test]
    fn gives_up_on_lost_packets_and_waits_for_keyframe() {
        let mut d = rx();
        d.push(1, 1, true, vec![0x65, 0], false);
        d.poll();
        d.push(2, 2, false, vec![0x7C, 0x81, 1], false);
        d.push(4, 2, true, vec![0x7C, 0x41, 3], false);
        d.push(5, 3, true, vec![0x41, 4], false);
        d.poll();
        // As if we had waited out the gap.
        let (gap, _) = d.gap_since.unwrap();
        d.gap_since = Some((gap, Instant::now() - GIVE_UP));
        assert!(d.poll().is_empty(), "frame 2 lost; P-frame 3 held back");
        assert!(d.needs_keyframe);
        d.push(6, 4, true, vec![0x65, 5], false);
        assert!(d.poll().pop().unwrap().keyframe);
        assert_eq!(d.stats.lost_frames, 1);
    }

    #[test]
    fn wraps_sequence_numbers() {
        let mut d = rx();
        d.push(65535, 7, false, vec![0x7C, 0x85, 1], false);
        d.push(0, 7, true, vec![0x7C, 0x45, 2], false);
        assert_eq!(d.poll().pop().unwrap().data, [0, 0, 0, 1, 0x65, 1, 2]);
    }

    #[test]
    fn nv12_grey_and_colour() {
        // 2×2: Y = 126 (mid grey), neutral chroma.
        let src = [126, 126, 126, 126, 128, 128];
        let rgba = nv12_to_rgba(&src, 2, 2, 2, 4);
        assert!(rgba.chunks(4).all(|p| (p[0] as i32 - 128).abs() <= 2 && p[0] == p[1] && p[1] == p[2] && p[3] == 255));
        // Strong Cr pushes red up and green down.
        let src = [81, 81, 81, 81, 90, 240];
        let px = &nv12_to_rgba(&src, 2, 2, 2, 4)[..4];
        assert!(px[0] > 200 && px[1] < 60, "{px:?}");
    }

    /// A minimal H.264 stream of raw-pixel (I_PCM) macroblocks, so the decoder can be tested without a file.
    fn pcm_stream(mbs_w: usize, mbs_h: usize, luma: impl Fn(usize) -> u8) -> Vec<u8> {
        struct Bits {
            out: Vec<u8>,
            acc: u8,
            n: u8,
        }
        impl Bits {
            fn bit(&mut self, b: u32) {
                self.acc = (self.acc << 1) | (b as u8 & 1);
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc);
                    self.acc = 0;
                    self.n = 0;
                }
            }
            fn bits(&mut self, v: u32, n: u32) {
                for i in (0..n).rev() {
                    self.bit(v >> i);
                }
            }
            fn ue(&mut self, v: u32) {
                let v = v + 1;
                let len = 32 - v.leading_zeros();
                self.bits(0, len - 1);
                self.bits(v, len);
            }
            fn align_zero(&mut self) {
                while self.n != 0 {
                    self.bit(0);
                }
            }
            fn trailing(mut self) -> Vec<u8> {
                self.bit(1);
                self.align_zero();
                self.out
            }
        }
        fn nal(kind: u8, rbsp: Vec<u8>, out: &mut Vec<u8>) {
            out.extend_from_slice(&[0, 0, 0, 1, 0x60 | kind]);
            let mut zeros = 0;
            for b in rbsp {
                if zeros >= 2 && b <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                out.push(b);
                zeros = if b == 0 { zeros + 1 } else { 0 };
            }
        }
        let new = || Bits { out: Vec::new(), acc: 0, n: 0 };
        let mut out = Vec::new();
        // SPS: baseline, level 3.0.
        let mut s = new();
        s.bits(66, 8);
        s.bits(0xC0, 8);
        s.bits(30, 8);
        s.ue(0); // sps id
        s.ue(0); // log2_max_frame_num - 4
        s.ue(2); // pic_order_cnt_type
        s.ue(1); // num_ref_frames
        s.bit(0);
        s.ue(mbs_w as u32 - 1);
        s.ue(mbs_h as u32 - 1);
        s.bit(1); // frame_mbs_only
        s.bit(1); // direct_8x8_inference
        s.bit(0); // cropping
        s.bit(0); // vui
        nal(7, s.trailing(), &mut out);
        // PPS.
        let mut p = new();
        p.ue(0);
        p.ue(0);
        p.bit(0); // CAVLC
        p.bit(0);
        p.ue(0);
        p.ue(0);
        p.ue(0);
        p.bit(0);
        p.bits(0, 2);
        p.ue(0); // pic_init_qp - 26 (se 0)
        p.ue(0); // pic_init_qs - 26
        p.ue(0); // chroma_qp_index_offset
        p.bit(0); // deblocking_filter_control_present
        p.bit(0);
        p.bit(0);
        nal(8, p.trailing(), &mut out);
        // IDR slice of I_PCM macroblocks.
        let mut sl = new();
        sl.ue(0); // first_mb
        sl.ue(7); // I slice
        sl.ue(0); // pps id
        sl.bits(0, 4); // frame_num
        sl.ue(0); // idr_pic_id
        sl.bit(0); // no_output_of_prior_pics
        sl.bit(0); // long_term_reference
        sl.ue(0); // slice_qp_delta (se 0)
        for mb in 0..mbs_w * mbs_h {
            sl.ue(25); // I_PCM
            sl.align_zero();
            for _ in 0..256 {
                sl.bits(luma(mb) as u32, 8);
            }
            for _ in 0..128 {
                sl.bits(128, 8);
            }
        }
        nal(5, sl.trailing(), &mut out);
        out
    }

    #[cfg(windows)]
    #[test]
    fn media_foundation_decodes_h264() {
        let (w, h) = (4, 4); // 64×64 pixels
        let lum = |mb: usize| if mb % 2 == 0 { 40 } else { 200 };
        let stream = pcm_stream(w, h, lum);
        let mut dec = Decoder::new().expect("H.264 decoder");
        let mut pics = dec.decode(&stream, true).expect("decode");
        // Low-latency mode may still hold the first picture until more input arrives.
        if pics.is_empty() {
            pics = dec.decode(&stream, true).expect("decode");
        }
        let p = pics.first().expect("a picture");
        assert_eq!((p.width, p.height), (64, 64));
        let px = |x: usize, y: usize| p.rgba[(y * p.width + x) * 4];
        // Macroblock 0 is dark, macroblock 1 (to its right) bright.
        assert!(px(8, 8) < 60, "{}", px(8, 8));
        assert!(px(24, 8) > 180, "{}", px(24, 8));
    }
}
