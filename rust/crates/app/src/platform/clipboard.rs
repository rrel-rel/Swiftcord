//! The Windows clipboard: text, and images (as PNG, which keeps transparency, and as a DIB, which every
//! app understands). Images go through WIC (`super::wic`).

use std::sync::Arc;

use egui::ColorImage;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::core::w;

use super::wic;

/// Holds the clipboard open; another app may have it for a moment, so opening retries briefly.
struct Open;

impl Open {
    fn new() -> Option<Open> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }

    fn read(&self, format: u32) -> Option<Vec<u8>> {
        unsafe {
            IsClipboardFormatAvailable(format).ok()?;
            let handle = HGLOBAL(GetClipboardData(format).ok()?.0);
            let ptr = GlobalLock(handle) as *const u8;
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr, GlobalSize(handle)).to_vec();
            let _ = GlobalUnlock(handle);
            Some(bytes)
        }
    }

    fn write(&self, format: u32, bytes: &[u8]) {
        unsafe {
            let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return };
            let ptr = GlobalLock(handle) as *mut u8;
            if ptr.is_null() {
                return;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(handle);
            // The clipboard owns the memory from here on.
            let _ = SetClipboardData(format, Some(HANDLE(handle.0)));
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

pub fn get_text() -> Option<String> {
    let bytes = Open::new()?.read(CF_UNICODETEXT.0 as u32)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

pub fn set_text(text: &str) {
    let Some(open) = Open::new() else { return };
    let mut bytes: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    bytes.shrink_to_fit();
    let _ = unsafe { EmptyClipboard() };
    open.write(CF_UNICODETEXT.0 as u32, &bytes);
}

pub fn get_image() -> Option<Arc<ColorImage>> {
    let open = Open::new()?;
    // Browsers and screenshot tools put a PNG up; a DIB is what's always there for an image.
    let png = open.read(png_format());
    let dib = if png.is_none() { open.read(CF_DIB.0 as u32) } else { None };
    drop(open);
    let image = match (png, dib) {
        (Some(png), _) => wic::decode(&png).ok()?,
        (None, Some(dib)) => wic::decode(&bmp_from_dib(&dib)?).ok()?,
        _ => return None,
    };
    Some(Arc::new(image.to_color_image()))
}

pub fn set_image(image: &ColorImage) {
    let rgba = wic::Rgba::from_color_image(image);
    let png = wic::encode_png(&rgba).ok();
    let Some(open) = Open::new() else { return };
    let _ = unsafe { EmptyClipboard() };
    if let Some(png) = png {
        open.write(png_format(), &png);
    }
    open.write(CF_DIB.0 as u32, &dib_from_rgba(&rgba));
}

/// A DIB with a file header in front is a BMP file, which WIC decodes.
fn bmp_from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| dib.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let header_size = u32_at(0)?;
    let bit_count = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)?;
    // BI_BITFIELDS masks follow a plain BITMAPINFOHEADER; later headers include them.
    let masks = match compression {
        3 if header_size == 40 => 12,
        6 if header_size == 40 => 16,
        _ => 0,
    };
    let palette = if bit_count <= 8 { if colors_used == 0 { 1 << bit_count } else { colors_used } } else { colors_used } * 4;
    let offset = 14 + header_size + masks + palette;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

/// A 32-bit bottom-up DIB (the layout every app reads).
fn dib_from_rgba(image: &wic::Rgba) -> Vec<u8> {
    let (w, h) = (image.width, image.height);
    let mut dib = Vec::with_capacity(40 + (w * h * 4) as usize);
    for v in [40u32, w, h] {
        dib.extend_from_slice(&v.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]); // BI_RGB, and the rest left to defaults
    for row in (0..h).rev() {
        for px in image.pixels[(row * w * 4) as usize..((row + 1) * w * 4) as usize].chunks_exact(4) {
            dib.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    dib
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dibs_decode_through_wic() {
        let image = wic::Rgba { width: 2, height: 2, pixels: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 255] };
        let back = wic::decode(&bmp_from_dib(&dib_from_rgba(&image)).unwrap()).unwrap();
        assert_eq!((back.width, back.height), (2, 2));
        assert_eq!(back.pixels, image.pixels);
    }
}
