//! Images through the Windows Imaging Component: decoding (PNG, JPEG, GIF, WebP, BMP…, with GIF and APNG
//! animation composited into whole frames), resizing, and PNG/JPEG encoding. WIC ships with Windows, so
//! none of these codecs need to be in the exe.

use std::time::Duration;

use egui::ColorImage;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToUInt32};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IStream};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{GUID, HSTRING, Interface};

/// An image as straight (not premultiplied) RGBA.
#[derive(Clone, Debug)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    pub fn from_color_image(image: &ColorImage) -> Rgba {
        let pixels = image.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
        Rgba { width: image.size[0] as u32, height: image.size[1] as u32, pixels }
    }

    pub fn to_color_image(&self) -> ColorImage {
        ColorImage::from_rgba_unmultiplied([self.width as usize, self.height as usize], &self.pixels)
    }

    /// The `w`×`h` area at (`x`, `y`), which must be inside the image.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Rgba {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row * self.width + x) * 4) as usize;
            pixels.extend_from_slice(&self.pixels[start..start + (w * 4) as usize]);
        }
        Rgba { width: w, height: h, pixels }
    }
}

type Result<T> = std::result::Result<T, String>;

fn err(e: windows::core::Error) -> String {
    e.message()
}

thread_local! {
    static FACTORY: Option<IWICImagingFactory> = unsafe {
        // Whatever this thread's COM apartment already is, WIC works in it.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
    };
}

fn with_factory<T>(f: impl FnOnce(&IWICImagingFactory) -> Result<T>) -> Result<T> {
    FACTORY.with(|factory| match factory {
        Some(factory) => f(factory),
        None => Err("Windows Imaging Component isn't available".into()),
    })
}

fn decoder(factory: &IWICImagingFactory, bytes: &[u8]) -> Result<IWICBitmapDecoder> {
    unsafe {
        let stream: IStream = SHCreateMemStream(Some(bytes)).ok_or("out of memory")?;
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).map_err(err)
    }
}

/// Converts any bitmap source to RGBA pixels.
fn pixels(source: &IWICBitmapSource) -> Result<Rgba> {
    unsafe {
        let converted = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, source).map_err(err)?;
        let (mut width, mut height) = (0, 0);
        converted.GetSize(&mut width, &mut height).map_err(err)?;
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        converted.CopyPixels(std::ptr::null(), width * 4, &mut pixels).map_err(err)?;
        Ok(Rgba { width, height, pixels })
    }
}

/// A metadata number (GIF frame positions, delays…), or `None` if it's absent.
fn metadata(reader: &IWICMetadataQueryReader, name: &str) -> Option<u32> {
    unsafe {
        let mut value = PROPVARIANT::default();
        reader.GetMetadataByName(&HSTRING::from(name), &mut value).ok()?;
        let n = PropVariantToUInt32(&value).ok();
        let _ = PropVariantClear(&mut value);
        n
    }
}

/// The first frame (for animations, as it's first shown).
pub fn decode(bytes: &[u8]) -> Result<Rgba> {
    if is_apng(bytes) {
        return decode_frames(bytes)?.into_iter().next().map(|(f, _)| f).ok_or_else(|| "empty animation".into());
    }
    with_factory(|factory| unsafe {
        let decoder = decoder(factory, bytes)?;
        if decoder.GetContainerFormat().map_err(err)? == GUID_ContainerFormatGif {
            return gif_frames(&decoder, 1)?.into_iter().next().map(|(f, _)| f).ok_or_else(|| "empty GIF".into());
        }
        pixels(&decoder.GetFrame(0).map_err(err)?.cast().map_err(err)?)
    })
}

/// Every frame with how long it's shown; a still image is one frame.
pub fn decode_frames(bytes: &[u8]) -> Result<Vec<(Rgba, Duration)>> {
    if is_apng(bytes) {
        return apng_frames(bytes);
    }
    with_factory(|factory| unsafe {
        let decoder = decoder(factory, bytes)?;
        if decoder.GetContainerFormat().map_err(err)? == GUID_ContainerFormatGif {
            return gif_frames(&decoder, usize::MAX);
        }
        let first = pixels(&decoder.GetFrame(0).map_err(err)?.cast().map_err(err)?)?;
        Ok(vec![(first, Duration::from_millis(100))])
    })
}

/// GIF frames are patches over the previous picture; this composites them into whole frames.
fn gif_frames(decoder: &IWICBitmapDecoder, limit: usize) -> Result<Vec<(Rgba, Duration)>> {
    unsafe {
        let count = decoder.GetFrameCount().map_err(err)? as usize;
        let screen = decoder.GetMetadataQueryReader().ok();
        let first = decoder.GetFrame(0).map_err(err)?;
        let (mut fw, mut fh) = (0, 0);
        first.GetSize(&mut fw, &mut fh).map_err(err)?;
        let width = screen.as_ref().and_then(|r| metadata(r, "/logscrdesc/Width")).filter(|&w| w > 0).unwrap_or(fw);
        let height = screen.as_ref().and_then(|r| metadata(r, "/logscrdesc/Height")).filter(|&h| h > 0).unwrap_or(fh);
        let mut canvas = vec![0u8; (width * height * 4) as usize];
        let mut frames = Vec::new();
        for i in 0..count.min(limit) {
            let frame = decoder.GetFrame(i as u32).map_err(err)?;
            let patch = pixels(&frame.cast().map_err(err)?)?;
            let meta = frame.GetMetadataQueryReader().ok();
            let get = |name| meta.as_ref().and_then(|m| metadata(m, name)).unwrap_or(0);
            let (left, top) = (get("/imgdesc/Left"), get("/imgdesc/Top"));
            // In hundredths of a second; like browsers, treat (nearly) zero as 0.1 s.
            let delay = match get("/grctlext/Delay") {
                0 | 1 => Duration::from_millis(100),
                d => Duration::from_millis(d as u64 * 10),
            };
            let disposal = get("/grctlext/Disposal");
            let previous = (disposal == 3).then(|| canvas.clone());
            for y in 0..patch.height {
                for x in 0..patch.width {
                    let (cx, cy) = (left + x, top + y);
                    let src = ((y * patch.width + x) * 4) as usize;
                    if cx < width && cy < height && patch.pixels[src + 3] != 0 {
                        let dst = ((cy * width + cx) * 4) as usize;
                        canvas[dst..dst + 4].copy_from_slice(&patch.pixels[src..src + 4]);
                    }
                }
            }
            frames.push((Rgba { width, height, pixels: canvas.clone() }, delay));
            match disposal {
                // Restore to background: the patch's area goes transparent.
                2 => clear(&mut canvas, width, height, left, top, patch.width, patch.height),
                3 => canvas = previous.unwrap(),
                _ => {}
            }
        }
        Ok(frames)
    }
}

fn clear(canvas: &mut [u8], width: u32, height: u32, left: u32, top: u32, w: u32, h: u32) {
    for y in top..(top + h).min(height) {
        let (start, end) = (((y * width + left.min(width)) * 4) as usize, ((y * width + (left + w).min(width)) * 4) as usize);
        canvas[start..end].fill(0);
    }
}

/// Scales to exactly `width`×`height`.
pub fn resize(image: &Rgba, width: u32, height: u32) -> Result<Rgba> {
    if (width, height) == (image.width, image.height) {
        return Ok(image.clone());
    }
    with_factory(|factory| unsafe {
        let bitmap = factory
            .CreateBitmapFromMemory(image.width, image.height, &GUID_WICPixelFormat32bppRGBA, image.width * 4, &image.pixels)
            .map_err(err)?;
        let scaler = factory.CreateBitmapScaler().map_err(err)?;
        scaler.Initialize(&bitmap, width.max(1), height.max(1), WICBitmapInterpolationModeHighQualityCubic).map_err(err)?;
        pixels(&scaler.cast().map_err(err)?)
    })
}

/// Scales to fit inside `max_w`×`max_h`, keeping the aspect ratio.
pub fn fit(image: &Rgba, max_w: u32, max_h: u32) -> Result<Rgba> {
    let scale = (max_w as f64 / image.width as f64).min(max_h as f64 / image.height as f64);
    let w = ((image.width as f64 * scale).round() as u32).max(1);
    let h = ((image.height as f64 * scale).round() as u32).max(1);
    resize(image, w, h)
}

pub fn encode_png(image: &Rgba) -> Result<Vec<u8>> {
    encode(image, &GUID_ContainerFormatPng, &GUID_WICPixelFormat32bppRGBA, None)
}

/// JPEG has no alpha; transparent areas come out black. `quality` is 0–1.
pub fn encode_jpeg(image: &Rgba, quality: f32) -> Result<Vec<u8>> {
    encode(image, &GUID_ContainerFormatJpeg, &GUID_WICPixelFormat24bppBGR, Some(quality))
}

fn encode(image: &Rgba, container: &GUID, format: &GUID, quality: Option<f32>) -> Result<Vec<u8>> {
    use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPBAG2};
    use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
    use windows::Win32::System::Variant::{VARIANT, VT_R4};
    with_factory(|factory| unsafe {
        let stream: IStream = SHCreateMemStream(None).ok_or("out of memory")?;
        let encoder = factory.CreateEncoder(container, std::ptr::null()).map_err(err)?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache).map_err(err)?;
        let mut frame = None;
        let mut options: Option<IPropertyBag2> = None;
        encoder.CreateNewFrame(&mut frame, &mut options).map_err(err)?;
        let frame = frame.ok_or("no frame")?;
        if let (Some(q), Some(options)) = (quality, &options) {
            let name = windows::core::w!("ImageQuality");
            let bag = PROPBAG2 { pstrName: windows::core::PWSTR(name.as_ptr() as *mut _), ..Default::default() };
            let mut value = VARIANT::default();
            (*value.Anonymous.Anonymous).vt = VT_R4;
            (*value.Anonymous.Anonymous).Anonymous.fltVal = q;
            options.Write(1, &bag, &value).map_err(err)?;
        }
        frame.Initialize(options.as_ref()).map_err(err)?;
        frame.SetSize(image.width, image.height).map_err(err)?;
        let mut pixel_format = *format;
        frame.SetPixelFormat(&mut pixel_format).map_err(err)?;
        // Hand WIC the RGBA pixels and let it convert them to whatever format the encoder settled on.
        let bitmap = factory
            .CreateBitmapFromMemory(image.width, image.height, &GUID_WICPixelFormat32bppRGBA, image.width * 4, &image.pixels)
            .map_err(err)?;
        let source = WICConvertBitmapSource(&pixel_format, &bitmap).map_err(err)?;
        frame.WriteSource(&source, std::ptr::null()).map_err(err)?;
        frame.Commit().map_err(err)?;
        encoder.Commit().map_err(err)?;

        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).map_err(err)?;
        stream.Seek(0, STREAM_SEEK_SET, None).map_err(err)?;
        let mut out = vec![0u8; stat.cbSize as usize];
        let mut read = 0;
        stream.Read(out.as_mut_ptr().cast(), out.len() as u32, Some(&mut read)).ok().map_err(err)?;
        out.truncate(read as usize);
        Ok(out)
    })
}

// MARK: APNG

/// WIC's PNG decoder only sees an APNG's default image, so the animation is read here: each frame's data
/// is rewrapped as a standalone PNG for WIC to decode, then composited.

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn chunks(bytes: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = PNG_SIGNATURE.len();
    std::iter::from_fn(move || {
        let len = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(at + 4..at + 8)?;
        let data = bytes.get(at + 8..at + 8 + len)?;
        at += 12 + len;
        Some((kind, data))
    })
}

fn is_apng(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_SIGNATURE) && chunks(bytes).take_while(|(k, _)| *k != b"IDAT").any(|(k, _)| k == b"acTL")
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = !0u32;
    for byte in parts.iter().flat_map(|p| p.iter()) {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

struct ApngFrame {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    delay: Duration,
    dispose: u8,
    blend: u8,
    data: Vec<Vec<u8>>,
}

fn apng_frames(bytes: &[u8]) -> Result<Vec<(Rgba, Duration)>> {
    let be = |d: &[u8], at: usize| u32::from_be_bytes(d[at..at + 4].try_into().unwrap());
    let mut header = None;
    // Chunks every frame needs (palette, transparency, colour space).
    let mut shared: Vec<(&[u8], &[u8])> = Vec::new();
    let mut frames: Vec<ApngFrame> = Vec::new();
    for (kind, data) in chunks(bytes) {
        match kind {
            b"IHDR" => header = Some(data),
            b"PLTE" | b"tRNS" | b"gAMA" | b"cHRM" | b"sRGB" | b"iCCP" | b"sBIT" => shared.push((kind, data)),
            b"fcTL" if data.len() >= 26 => {
                let (num, den) = (u16::from_be_bytes([data[20], data[21]]) as u64, u16::from_be_bytes([data[22], data[23]]) as u64);
                let den = if den == 0 { 100 } else { den };
                frames.push(ApngFrame {
                    width: be(data, 4),
                    height: be(data, 8),
                    x: be(data, 12),
                    y: be(data, 16),
                    delay: Duration::from_micros(num * 1_000_000 / den).max(Duration::from_millis(16)),
                    dispose: data[24],
                    blend: data[25],
                    data: Vec::new(),
                });
            }
            // IDAT before any fcTL is a default image that isn't part of the animation.
            b"IDAT" => {
                if let Some(f) = frames.last_mut() {
                    f.data.push(data.to_vec());
                }
            }
            b"fdAT" if data.len() > 4 => {
                if let Some(f) = frames.last_mut() {
                    f.data.push(data[4..].to_vec());
                }
            }
            _ => {}
        }
    }
    let header = header.filter(|h| h.len() == 13).ok_or("no PNG header")?;
    let (width, height) = (be(header, 0), be(header, 4));
    let mut canvas = vec![0u8; (width * height * 4) as usize];
    let mut out = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        if f.data.is_empty() || f.x + f.width > width || f.y + f.height > height {
            continue;
        }
        let mut png = PNG_SIGNATURE.to_vec();
        let mut ihdr = header.to_vec();
        ihdr[0..4].copy_from_slice(&f.width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&f.height.to_be_bytes());
        push_chunk(&mut png, b"IHDR", &ihdr);
        for (kind, data) in &shared {
            push_chunk(&mut png, kind, data);
        }
        for data in &f.data {
            push_chunk(&mut png, b"IDAT", data);
        }
        push_chunk(&mut png, b"IEND", &[]);
        let patch = with_factory(|factory| unsafe { pixels(&decoder(factory, &png)?.GetFrame(0).map_err(err)?.cast().map_err(err)?) })?;

        // "Restore previous" on the first frame means "clear".
        let dispose = if i == 0 && f.dispose == 2 { 1 } else { f.dispose };
        let previous = (dispose == 2).then(|| canvas.clone());
        for y in 0..patch.height {
            for x in 0..patch.width {
                let src = ((y * patch.width + x) * 4) as usize;
                let dst = (((f.y + y) * width + f.x + x) * 4) as usize;
                let s = &patch.pixels[src..src + 4];
                if f.blend == 0 {
                    canvas[dst..dst + 4].copy_from_slice(s);
                } else {
                    blend_over(&mut canvas[dst..dst + 4], s);
                }
            }
        }
        out.push((Rgba { width, height, pixels: canvas.clone() }, f.delay));
        match dispose {
            1 => clear(&mut canvas, width, height, f.x, f.y, f.width, f.height),
            2 => canvas = previous.unwrap(),
            _ => {}
        }
    }
    if out.is_empty() {
        return Err("no frames in the APNG".into());
    }
    Ok(out)
}

/// `src` over `dst`, both straight alpha.
fn blend_over(dst: &mut [u8], src: &[u8]) {
    let sa = src[3] as f32 / 255.0;
    if sa >= 1.0 {
        dst.copy_from_slice(src);
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let a = sa + da * (1.0 - sa);
    if a <= 0.0 {
        dst.fill(0);
        return;
    }
    for c in 0..3 {
        dst[c] = ((src[c] as f32 * sa + dst[c] as f32 * da * (1.0 - sa)) / a).round() as u8;
    }
    dst[3] = (a * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Rgba {
        Rgba { width: w, height: h, pixels: rgba.repeat((w * h) as usize) }
    }

    #[test]
    fn png_round_trips() {
        let png = encode_png(&solid(3, 2, [200, 30, 90, 255])).unwrap();
        let back = decode(&png).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(&back.pixels[..4], &[200, 30, 90, 255]);
    }

    #[test]
    fn jpeg_encodes() {
        let jpeg = encode_jpeg(&solid(16, 16, [10, 200, 10, 255]), 0.75).unwrap();
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        let back = decode(&jpeg).unwrap();
        assert!(back.pixels[1] > 180);
    }

    #[test]
    fn resizes_and_fits() {
        let img = solid(40, 20, [1, 2, 3, 255]);
        let r = resize(&img, 10, 7).unwrap();
        assert_eq!((r.width, r.height, r.pixels.len()), (10, 7, 280));
        let f = fit(&img, 10, 10).unwrap();
        assert_eq!((f.width, f.height), (10, 5));
    }

    #[test]
    fn crops() {
        let mut img = solid(4, 4, [0, 0, 0, 255]);
        img.pixels[(4 + 2) * 4] = 99; // (2, 1)
        let c = img.crop(2, 1, 2, 2);
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixels[0], 99);
    }

    /// LZW image data that codes each pixel as a literal, clearing the table before it grows past 3-bit codes.
    fn gif_image_data(indices: &[u8]) -> Vec<u8> {
        let (clear, end) = (4u32, 5u32);
        let mut codes = Vec::new();
        for pair in indices.chunks(2) {
            codes.push(clear);
            codes.extend(pair.iter().map(|&i| i as u32));
        }
        codes.push(end);
        let (mut bits, mut acc, mut packed) = (0, 0u32, Vec::new());
        for code in codes {
            acc |= code << bits;
            bits += 3;
            while bits >= 8 {
                packed.push(acc as u8);
                acc >>= 8;
                bits -= 8;
            }
        }
        if bits > 0 {
            packed.push(acc as u8);
        }
        let mut out = vec![2, packed.len() as u8];
        out.extend_from_slice(&packed);
        out.push(0);
        out
    }

    /// Two frames: a red 2×2 screen, then its top-left pixel painted blue.
    #[test]
    fn composites_gif_frames() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[2, 0, 2, 0, 0x80, 0, 0, 0xFF, 0, 0, 0, 0, 0xFF]); // 2×2, palette: red, blue
        gif.extend_from_slice(&[0x21, 0xF9, 4, 0, 10, 0, 0, 0]); // 0.1 s
        gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0]);
        gif.extend_from_slice(&gif_image_data(&[0, 0, 0, 0]));
        gif.extend_from_slice(&[0x21, 0xF9, 4, 0, 20, 0, 0, 0]); // 0.2 s
        gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        gif.extend_from_slice(&gif_image_data(&[1]));
        gif.push(0x3B);

        let frames = decode_frames(&gif).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].1, Duration::from_millis(200));
        let second = &frames[1].0;
        assert_eq!((second.width, second.height), (2, 2));
        assert_eq!(&second.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&second.pixels[4..8], &[255, 0, 0, 255]);
        assert_eq!(&decode(&gif).unwrap().pixels[0..4], &[255, 0, 0, 255]);
    }

    /// A two-frame APNG built from WIC-encoded PNGs: the second frame covers the first.
    #[test]
    fn decodes_apng_frames() {
        let frame = |c: [u8; 4]| {
            let png = encode_png(&solid(2, 2, c)).unwrap();
            let header = chunks(&png).find(|(k, _)| *k == b"IHDR").unwrap().1.to_vec();
            let data: Vec<u8> = chunks(&png).filter(|(k, _)| *k == b"IDAT").flat_map(|(_, d)| d.to_vec()).collect();
            (header, data)
        };
        let (header, red) = frame([255, 0, 0, 255]);
        let (_, blue) = frame([0, 0, 255, 255]);
        let fctl = |seq: u32| {
            let mut d = Vec::new();
            for v in [seq, 2, 2, 0, 0] {
                d.extend_from_slice(&v.to_be_bytes());
            }
            d.extend_from_slice(&[0, 1, 0, 10, 0, 0]); // 1/10 s, no dispose, source blend
            d
        };
        let mut apng = PNG_SIGNATURE.to_vec();
        push_chunk(&mut apng, b"IHDR", &header);
        push_chunk(&mut apng, b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
        push_chunk(&mut apng, b"fcTL", &fctl(0));
        push_chunk(&mut apng, b"IDAT", &red);
        push_chunk(&mut apng, b"fcTL", &fctl(1));
        let mut fdat = 2u32.to_be_bytes().to_vec();
        fdat.extend_from_slice(&blue);
        push_chunk(&mut apng, b"fdAT", &fdat);
        push_chunk(&mut apng, b"IEND", &[]);

        let frames = decode_frames(&apng).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(&frames[0].0.pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(&frames[1].0.pixels[..4], &[0, 0, 255, 255]);
        assert_eq!(frames[1].1, Duration::from_millis(100));
        // WIC's own decoder still reads it as a PNG (its default image).
        assert_eq!(&decode(&apng).unwrap().pixels[..4], &[255, 0, 0, 255]);
    }
}
