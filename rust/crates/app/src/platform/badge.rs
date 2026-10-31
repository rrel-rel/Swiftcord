//! Unread mention badges, as Discord shows them: a red count over the taskbar button, and a red dot on the
//! tray icon. Icons are drawn here: a circle by hand, the count with GDI.

use std::sync::OnceLock;

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON};

/// Discord's mention red.
const RED: [u8; 3] = [0xf2, 0x3f, 0x43];

/// Shows `mentions` over the window's taskbar button (nothing for none). On the UI thread.
pub fn set_taskbar(hwnd: HWND, mentions: u32) {
    thread_local! {
        static TASKBAR: Option<ITaskbarList3> = unsafe {
            // Usually already done by winit; harmless if so.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok().filter(|t| t.HrInit().is_ok())
        };
    }
    TASKBAR.with(|taskbar| {
        let Some(taskbar) = taskbar else { return };
        let icon = (mentions > 0).then(|| count_icon(mentions)).flatten();
        let description = match mentions {
            0 => HSTRING::new(),
            1 => HSTRING::from("1 unread mention"),
            n => HSTRING::from(format!("{n} unread mentions")),
        };
        unsafe {
            let _ = taskbar.SetOverlayIcon(hwnd, icon.unwrap_or_default(), &description);
            // The taskbar keeps a copy.
            if let Some(icon) = icon {
                let _ = DestroyIcon(icon);
            }
        }
    });
}

/// The small icon size (the taskbar's overlay and the tray's), in pixels.
fn small_size() -> usize {
    (unsafe { GetSystemMetrics(SM_CXSMICON) }).max(16) as usize
}

fn count_icon(count: u32) -> Option<HICON> {
    let size = small_size();
    icon(size, &count_pixels(size, count))
}

/// A red circle with the count in it ("9+" past nine).
fn count_pixels(size: usize, count: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; size * size * 4];
    let c = size as f32 / 2.0;
    circle(&mut rgba, size, c, c, c, RED);
    let text = if count > 9 { "9+".to_owned() } else { count.to_string() };
    let coverage = text_coverage(size, &text, (size as f32 * if count > 9 { 0.62 } else { 0.78 }) as i32);
    for (px, cov) in rgba.chunks_exact_mut(4).zip(coverage) {
        let a = cov as f32 / 255.0;
        for ch in 0..3 {
            px[ch] = (px[ch] as f32 * (1.0 - a) + 255.0 * a) as u8;
        }
    }
    rgba
}

/// The app's icon at tray size, with a red dot in the bottom-right corner (cut out of the icon so it stands
/// out) when there are mentions.
pub fn tray_icon(mentions: u32) -> Option<HICON> {
    let size = small_size();
    icon(size, &tray_pixels(size, mentions)?)
}

fn tray_pixels(size: usize, mentions: u32) -> Option<Vec<u8>> {
    static BASE: OnceLock<Option<super::wic::Rgba>> = OnceLock::new();
    // Scaled each time: the size changes with the display's scale.
    let base = BASE.get_or_init(|| super::wic::decode(include_bytes!("../../assets/icon.png")).ok()).as_ref()?;
    let mut rgba = super::wic::resize(base, size as u32, size as u32).ok()?.pixels;
    if mentions > 0 {
        let r = size as f32 * 0.24;
        let (x, y) = (size as f32 - r, size as f32 - r);
        // A transparent ring around the dot, then the dot.
        let gap = (size as f32 * 0.08).max(1.0);
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let (px_x, px_y) = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
            let d = ((px_x - x).powi(2) + (px_y - y).powi(2)).sqrt() - (r + gap);
            px[3] = (px[3] as f32 * (d + 0.5).clamp(0.0, 1.0)) as u8;
        }
        circle(&mut rgba, size, x, y, r, RED);
    }
    Some(rgba)
}

/// Paints an antialiased circle over `rgba` (straight alpha).
fn circle(rgba: &mut [u8], size: usize, cx: f32, cy: f32, r: f32, color: [u8; 3]) {
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i % size) as f32 + 0.5, (i / size) as f32 + 0.5);
        let a = (r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        let under = px[3] as f32 / 255.0;
        let out = a + under * (1.0 - a);
        for ch in 0..3 {
            px[ch] = ((color[ch] as f32 * a + px[ch] as f32 * under * (1.0 - a)) / out) as u8;
        }
        px[3] = (out * 255.0) as u8;
    }
}

/// How much of each pixel `text` covers, centred in a `size` square, in bold Segoe UI `height` pixels tall.
fn text_coverage(size: usize, text: &str, height: i32) -> Vec<u8> {
    let mut out = vec![0u8; size * size];
    unsafe {
        let dc = CreateCompatibleDC(None);
        let info = dib_info(size);
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return out;
        };
        let old_bitmap = SelectObject(dc, bitmap.into());
        let font = CreateFontW(
            -height,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font.into());
        SetTextColor(dc, windows::Win32::Foundation::COLORREF(0x00ffffff));
        SetBkMode(dc, TRANSPARENT);
        let mut rect = RECT { left: 0, top: 0, right: size as i32, bottom: size as i32 };
        let mut text: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(dc, &mut text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits as *const u8, size * size * 4);
        for (o, px) in out.iter_mut().zip(pixels.chunks_exact(4)) {
            // White on black: any channel is the coverage (the green one, for ClearType's sake).
            *o = px[1];
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
    }
    out
}

fn dib_info(size: usize) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            // Negative: top-down rows.
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// An icon from straight-alpha RGBA pixels.
fn icon(size: usize, rgba: &[u8]) -> Option<HICON> {
    unsafe {
        let info = dib_info(size);
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, size * size * 4);
        for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        // With a 32-bit colour bitmap, the mask is only a formality.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() }).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the badges at 125% (20 px) and 100% (16 px), scaled up 8 times, into the temp folder to look at.
    #[test]
    #[ignore]
    fn draws_the_badges() {
        let dir = std::env::temp_dir().join("swiftcord-badges");
        std::fs::create_dir_all(&dir).unwrap();
        for size in [16usize, 20] {
            let cases = [("count-1", count_pixels(size, 1)), ("count-7", count_pixels(size, 7)), ("count-12", count_pixels(size, 12)), ("tray", tray_pixels(size, 3).unwrap())];
            for (name, rgba) in cases {
                // Over the taskbar's grey, as it would be seen.
                let mut big = vec![0u8; size * 8 * size * 8 * 4];
                for (i, px) in big.chunks_exact_mut(4).enumerate() {
                    let (x, y) = ((i % (size * 8)) / 8, (i / (size * 8)) / 8);
                    let s = &rgba[(y * size + x) * 4..][..4];
                    let a = s[3] as f32 / 255.0;
                    for ch in 0..3 {
                        px[ch] = (s[ch] as f32 * a + 0x30 as f32 * (1.0 - a)) as u8;
                    }
                    px[3] = 255;
                }
                let png = crate::platform::wic::encode_png(&crate::platform::wic::Rgba { width: (size * 8) as u32, height: (size * 8) as u32, pixels: big }).unwrap();
                std::fs::write(dir.join(format!("{name}-{size}.png")), png).unwrap();
            }
        }
        println!("{}", dir.display());
    }
}
