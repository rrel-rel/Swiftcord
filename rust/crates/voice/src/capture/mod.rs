//! Screen and window capture for Go Live, through GDI: it works everywhere (including software-rendered VMs),
//! needs no Direct3D, and scales to the stream size in the same blit.

/// Something that can be shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A whole monitor, by its desktop rectangle (left, top, right, bottom) in physical pixels.
    Monitor { rect: (i32, i32, i32, i32) },
    /// A top-level window, by handle.
    Window { hwnd: isize },
}

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub source: Source,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

/// A captured picture: BGRA rows, top to bottom, `width * 4` bytes each.
pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub bgra: &'a [u8],
}

/// The output size for a `w`×`h` source fitted into `max_w`×`max_h` (never enlarged), with even sides as
/// 4:2:0 video needs.
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (w, h) = (w.max(2), h.max(2));
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

#[cfg(windows)]
pub use win::{output_size, sources, Capturer};

#[cfg(windows)]
mod dda;
#[cfg(windows)]
mod wgc;

/// Scales a BGRA picture (`pitch` bytes per row) into `out` (`ow`×`oh`), keeping its shape: centred, with
/// black bars when the aspect differs, never enlarged. Bilinear, which is enough for the usual ≤2× reductions.
pub(crate) fn scale_into(src: &[u8], sw: usize, sh: usize, pitch: usize, out: &mut [u8], ow: usize, oh: usize) {
    if sw == 0 || sh == 0 {
        return;
    }
    let (dw, dh) = fit(sw as u32, sh as u32, ow as u32, oh as u32);
    let (dw, dh) = ((dw as usize).min(ow), (dh as usize).min(oh));
    let (dx, dy) = ((ow - dw) / 2, (oh - dh) / 2);
    if (dw, dh) != (ow, oh) {
        out.fill(0);
    }
    if (dw, dh) == (sw, sh) {
        for y in 0..dh {
            let o = ((dy + y) * ow + dx) * 4;
            out[o..o + dw * 4].copy_from_slice(&src[y * pitch..y * pitch + dw * 4]);
        }
        return;
    }
    // Source sample positions and 8-bit weights per output column and row.
    let taps = |dst: usize, src_len: usize| -> Vec<(usize, usize, u32)> {
        (0..dst)
            .map(|i| {
                let f = ((i as f32 + 0.5) * src_len as f32 / dst as f32 - 0.5).max(0.0);
                let i0 = (f as usize).min(src_len - 1);
                ((i0), (i0 + 1).min(src_len - 1), ((f - i0 as f32) * 256.0) as u32)
            })
            .collect()
    };
    let (xs, ys) = (taps(dw, sw), taps(dh, sh));
    for (y, &(y0, y1, wy)) in ys.iter().enumerate() {
        let (r0, r1) = (&src[y0 * pitch..], &src[y1 * pitch..]);
        let row = &mut out[((dy + y) * ow + dx) * 4..((dy + y) * ow + dx + dw) * 4];
        for (x, &(x0, x1, wx)) in xs.iter().enumerate() {
            for c in 0..3 {
                let top = r0[x0 * 4 + c] as u32 * (256 - wx) + r0[x1 * 4 + c] as u32 * wx;
                let bottom = r1[x0 * 4 + c] as u32 * (256 - wx) + r1[x1 * 4 + c] as u32 * wx;
                row[x * 4 + c] = ((top * (256 - wy) + bottom * wy) >> 16) as u8;
            }
            row[x * 4 + 3] = 255;
        }
    }
}

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::ffi::c_void;

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::WindowsAndMessaging::*;

    use super::{fit, Frame, Source, SourceInfo};

    /// Monitors first (primary first), then visible application windows.
    pub fn sources() -> Vec<SourceInfo> {
        let mut monitors: Vec<(RECT, bool)> = Vec::new();
        unsafe extern "system" fn monitor(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let list = unsafe { &mut *(data.0 as *mut Vec<(RECT, bool)>) };
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
                list.push((info.rcMonitor, info.dwFlags & MONITORINFOF_PRIMARY != 0));
            }
            true.into()
        }
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(monitor), LPARAM(&mut monitors as *mut _ as isize));
        }
        monitors.sort_by_key(|(r, primary)| (!primary, r.left, r.top));
        let mut out: Vec<SourceInfo> = monitors
            .iter()
            .enumerate()
            .map(|(i, (r, primary))| {
                let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
                SourceInfo {
                    source: Source::Monitor { rect: (r.left, r.top, r.right, r.bottom) },
                    label: format!("Screen {}{}", i + 1, if *primary { " (main)" } else { "" }),
                    width: w,
                    height: h,
                }
            })
            .collect();

        let mut windows: Vec<HWND> = Vec::new();
        unsafe extern "system" fn window(h: HWND, data: LPARAM) -> BOOL {
            unsafe { &mut *(data.0 as *mut Vec<HWND>) }.push(h);
            true.into()
        }
        unsafe {
            let _ = EnumWindows(Some(window), LPARAM(&mut windows as *mut _ as isize));
        }
        let me = std::process::id();
        for h in windows {
            if !shareable(h, me) {
                continue;
            }
            let Some(rect) = frame_bounds(h) else { continue };
            let mut title = [0u16; 256];
            let n = unsafe { GetWindowTextW(h, &mut title) };
            let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);
            out.push(SourceInfo {
                source: Source::Window { hwnd: h.0 as isize },
                label: title,
                width: (rect.right - rect.left) as u32,
                height: (rect.bottom - rect.top) as u32,
            });
        }
        out
    }

    /// Visible, titled, unowned application windows that aren't ours, aren't tool windows and aren't hidden
    /// by the shell (cloaked: other virtual desktops, suspended UWP apps).
    fn shareable(h: HWND, me: u32) -> bool {
        unsafe {
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || GetWindowTextLengthW(h) == 0 {
                return false;
            }
            if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                return false;
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == me {
                return false;
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4);
            cloaked == 0
        }
    }

    /// The visible window rectangle (GetWindowRect includes invisible resize borders).
    fn frame_bounds(h: HWND) -> Option<RECT> {
        let mut r = RECT::default();
        unsafe {
            if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut c_void, std::mem::size_of::<RECT>() as u32).is_err() {
                GetWindowRect(h, &mut r).ok()?;
            }
        }
        (r.right > r.left && r.bottom > r.top).then_some(r)
    }

    /// The size frames of `source` will have when captured at no more than `max_w`×`max_h`.
    pub fn output_size(source: &Source, max_w: u32, max_h: u32) -> Option<(u32, u32)> {
        let (w, h) = match source {
            Source::Monitor { rect } => ((rect.2 - rect.0) as u32, (rect.3 - rect.1) as u32),
            Source::Window { hwnd } => {
                let r = frame_bounds(HWND(*hwnd as *mut c_void))?;
                ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
            }
        };
        Some(fit(w, h, max_w, max_h))
    }

    /// A 32-bit top-down DIB selected into its own memory DC.
    struct Surface {
        dc: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u8,
        width: i32,
        height: i32,
    }

    impl Surface {
        fn new(width: i32, height: i32) -> Option<Surface> {
            unsafe {
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(Some(screen));
                ReleaseDC(None, screen);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
                let old = SelectObject(dc, bitmap.into());
                SetStretchBltMode(dc, HALFTONE);
                let _ = SetBrushOrgEx(dc, 0, 0, None);
                Some(Surface { dc, bitmap, old, bits: bits.cast(), width, height })
            }
        }

        fn pixels(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.bits, (self.width * self.height * 4) as usize) }
        }

        fn clear(&self) {
            unsafe { std::ptr::write_bytes(self.bits, 0, (self.width * self.height * 4) as usize) };
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap.into());
                let _ = DeleteDC(self.dc);
            }
        }
    }

    /// Grabs frames from one source, scaled into a fixed output size (a window that changes shape is
    /// letterboxed), with the mouse pointer drawn in. Uses Windows.Graphics.Capture, with GDI as the last
    /// resort (it works anywhere but reading the screen through it can be very slow). With
    /// `fullscreen_games`, a whole screen is first tried through Desktop Duplication, which can also see
    /// fullscreen-exclusive games; if that gives no picture within a few seconds, it falls back.
    pub struct Capturer {
        source: Source,
        width: usize,
        height: usize,
        backend: Backend,
        started: std::time::Instant,
    }

    enum Backend {
        Dda(super::dda::Dda),
        Wgc(super::wgc::Wgc),
        Gdi(GdiCapturer),
    }

    impl Capturer {
        /// Captures `source` at no more than `max_w`×`max_h`.
        pub fn new(source: Source, max_w: u32, max_h: u32, fullscreen_games: bool) -> Option<Capturer> {
            let (w, h) = output_size(&source, max_w, max_h)?;
            let (w_us, h_us) = (w as usize, h as usize);
            let dda = match &source {
                Source::Monitor { rect } if fullscreen_games => match super::dda::Dda::new(*rect, w_us, h_us) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        tracing::info!("Desktop Duplication unavailable ({e}); fullscreen games may not show");
                        None
                    }
                },
                _ => None,
            };
            let backend = match dda {
                Some(c) => {
                    tracing::info!("capturing {source:?} through Desktop Duplication");
                    Backend::Dda(c)
                }
                None => Self::usual(&source, w_us, h_us)?,
            };
            Some(Capturer { source, width: w_us, height: h_us, backend, started: std::time::Instant::now() })
        }

        /// Windows.Graphics.Capture, or GDI where that's unavailable.
        fn usual(source: &Source, w: usize, h: usize) -> Option<Backend> {
            match super::wgc::Wgc::new(source, w, h) {
                Ok(c) => Some(Backend::Wgc(c)),
                Err(e) => {
                    tracing::warn!("Windows.Graphics.Capture unavailable ({e}); capturing through GDI");
                    Some(Backend::Gdi(GdiCapturer::new(source.clone(), w as u32, h as u32)?))
                }
            }
        }

        pub fn size(&self) -> (usize, usize) {
            (self.width, self.height)
        }

        /// Which way it's capturing, for logs.
        pub fn method(&self) -> &'static str {
            match self.backend {
                Backend::Dda(_) => "Desktop Duplication",
                Backend::Wgc(_) => "Windows.Graphics.Capture",
                Backend::Gdi(_) => "GDI",
            }
        }

        /// The latest picture (the previous one again when nothing changed), or None once a shared window
        /// has closed.
        pub fn grab(&mut self) -> Option<Frame<'_>> {
            // Desktop Duplication that hasn't produced a picture in a few seconds won't: use the usual way.
            if matches!(&self.backend, Backend::Dda(d) if !d.pictured) && self.started.elapsed() > std::time::Duration::from_secs(3) {
                tracing::warn!("Desktop Duplication gave no picture; capturing the usual way instead");
                self.backend = Self::usual(&self.source, self.width, self.height)?;
            }
            match &mut self.backend {
                Backend::Dda(c) => Some(Frame { width: self.width, height: self.height, bgra: c.grab()? }),
                Backend::Wgc(c) => Some(Frame { width: self.width, height: self.height, bgra: c.grab()? }),
                Backend::Gdi(c) => c.grab(),
            }
        }
    }

    struct GdiCapturer {
        source: Source,
        out: Surface,
        /// Windows are rendered at full size first (PrintWindow can't scale).
        window: Option<Surface>,
        hotspots: HashMap<isize, (i32, i32)>,
    }

    // The raw pointers are only touched by whichever thread owns the capturer.
    unsafe impl Send for GdiCapturer {}

    impl GdiCapturer {
        fn new(source: Source, w: u32, h: u32) -> Option<GdiCapturer> {
            Some(GdiCapturer { source, out: Surface::new(w as i32, h as i32)?, window: None, hotspots: HashMap::new() })
        }

        fn grab(&mut self) -> Option<Frame<'_>> {
            let (ow, oh) = (self.out.width, self.out.height);
            // Where the source lands in the output, and the source's desktop origin and size (for the cursor).
            let (dest, origin, src_size) = match self.source.clone() {
                Source::Monitor { rect } => unsafe {
                    let (sw, sh) = (rect.2 - rect.0, rect.3 - rect.1);
                    let screen = GetDC(None);
                    let _ = StretchBlt(self.out.dc, 0, 0, ow, oh, Some(screen), rect.0, rect.1, sw, sh, SRCCOPY);
                    ReleaseDC(None, screen);
                    ((0, 0, ow, oh), (rect.0, rect.1), (sw, sh))
                },
                Source::Window { hwnd } => {
                    let h = HWND(hwnd as *mut c_void);
                    if !unsafe { IsWindow(Some(h)) }.as_bool() {
                        return None;
                    }
                    let mut wr = RECT::default();
                    unsafe { GetWindowRect(h, &mut wr) }.ok()?;
                    let fr = frame_bounds(h).unwrap_or(wr);
                    let (full_w, full_h) = (wr.right - wr.left, wr.bottom - wr.top);
                    let (sw, sh) = (fr.right - fr.left, fr.bottom - fr.top);
                    if self.window.as_ref().is_none_or(|s| (s.width, s.height) != (full_w, full_h)) {
                        self.window = Surface::new(full_w.max(1), full_h.max(1));
                    }
                    let win = self.window.as_ref()?;
                    // Letterbox: fit the visible window into the output, centred.
                    let (dw, dh) = fit(sw as u32, sh as u32, ow as u32, oh as u32);
                    let (dw, dh) = (dw as i32, dh as i32);
                    let (dx, dy) = ((ow - dw) / 2, (oh - dh) / 2);
                    unsafe {
                        // PW_RENDERFULLCONTENT (2) also captures DirectX and browser windows.
                        let _ = PrintWindow(h, win.dc, PRINT_WINDOW_FLAGS(2));
                        self.out.clear();
                        let _ = StretchBlt(self.out.dc, dx, dy, dw, dh, Some(win.dc), fr.left - wr.left, fr.top - wr.top, sw, sh, SRCCOPY);
                    }
                    ((dx, dy, dw, dh), (fr.left, fr.top), (sw, sh))
                }
            };
            self.draw_cursor(dest, origin, src_size);
            Some(Frame { width: ow as usize, height: oh as usize, bgra: self.out.pixels() })
        }

        fn draw_cursor(&mut self, dest: (i32, i32, i32, i32), origin: (i32, i32), src: (i32, i32)) {
            unsafe {
                let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
                if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid() {
                    return;
                }
                let key = ci.hCursor.0 as isize;
                let (hx, hy) = *self.hotspots.entry(key).or_insert_with(|| {
                    let mut ii = ICONINFO::default();
                    if GetIconInfo(ci.hCursor.into(), &mut ii).is_ok() {
                        let _ = DeleteObject(ii.hbmMask.into());
                        if !ii.hbmColor.is_invalid() {
                            let _ = DeleteObject(ii.hbmColor.into());
                        }
                        (ii.xHotspot as i32, ii.yHotspot as i32)
                    } else {
                        (0, 0)
                    }
                });
                let (sx, sy) = (dest.2 as f64 / src.0.max(1) as f64, dest.3 as f64 / src.1.max(1) as f64);
                let px = ci.ptScreenPos;
                let rel = POINT { x: px.x - origin.0, y: px.y - origin.1 };
                if rel.x < 0 || rel.y < 0 || rel.x >= src.0 || rel.y >= src.1 {
                    return;
                }
                let size = (32.0 * sx.max(0.5)).round() as i32;
                let x = dest.0 + ((rel.x - hx) as f64 * sx).round() as i32;
                let y = dest.1 + ((rel.y - hy) as f64 * sy).round() as i32;
                let _ = DrawIconEx(self.out.dc, x, y, ci.hCursor.into(), size, size, 0, None, DI_NORMAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_and_keeps_even_sides() {
        assert_eq!(fit(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(fit(2560, 1440, 1920, 1080), (1920, 1080));
        // Never enlarged; odd sides rounded down to even.
        assert_eq!(fit(801, 601, 1280, 720), (800, 600));
        // Tall windows are limited by height.
        assert_eq!(fit(1000, 2000, 1280, 720), (360, 720));
    }

    #[test]
    fn scales_and_letterboxes() {
        // 8×4 source: left half white, right half black, rows padded to 40 bytes.
        let pitch = 40;
        let mut src = vec![0u8; pitch * 4];
        for y in 0..4 {
            for x in 0..4 {
                src[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&[255; 4]);
            }
        }
        // Same size: copied.
        let mut out = vec![9u8; 8 * 4 * 4];
        scale_into(&src, 8, 4, pitch, &mut out, 8, 4);
        assert_eq!(&out[..4], &[255; 4]);
        assert_eq!(&out[7 * 4..7 * 4 + 3], &[0, 0, 0]);
        // Into a square: halved to 4×2, centred in 4×4 with bars above and below.
        let mut out = vec![9u8; 4 * 4 * 4];
        scale_into(&src, 8, 4, pitch, &mut out, 4, 4);
        let px = |x: usize, y: usize| out[(y * 4 + x) * 4];
        assert!((0..4).all(|x| px(x, 0) == 0 && px(x, 3) == 0), "bars");
        assert!(px(0, 1) > 200 && px(3, 1) < 60 && px(0, 2) > 200, "{:?}", &out[16..32]);
    }

    #[cfg(windows)]
    #[test]
    fn captures_the_main_screen() {
        let Some(screen) = sources().into_iter().find(|s| matches!(s.source, Source::Monitor { .. })) else { return };
        let mut c = Capturer::new(screen.source, 640, 360, false).expect("capturer");
        let f = c.grab().expect("frame");
        assert_eq!(f.bgra.len(), f.width * f.height * 4);
        assert!(f.width <= 640 && f.height <= 360);
    }

    /// Desktop Duplication gives a real picture of the main screen (the desktop isn't all black), several
    /// times in a row. Skipped where duplication isn't available (some remote sessions).
    /// With "Capture fullscreen games", a screen share always keeps giving frames: through Desktop Duplication
    /// where it works, otherwise falling back after a few seconds.
    #[cfg(windows)]
    #[test]
    #[ignore = "takes a few seconds; run by hand"]
    fn fullscreen_game_capture_falls_back_when_duplication_gives_nothing() {
        let Some(screen) = sources().into_iter().find(|s| matches!(s.source, Source::Monitor { .. })) else { return };
        let mut c = Capturer::new(screen.source, 640, 360, true).expect("capturer");
        let start = std::time::Instant::now();
        let mut lit = 0;
        while start.elapsed() < std::time::Duration::from_secs(5) {
            let f = c.grab().expect("frame");
            assert_eq!(f.bgra.len(), f.width * f.height * 4);
            lit = f.bgra.chunks(4).filter(|p| p[0] > 16 || p[1] > 16 || p[2] > 16).count();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        eprintln!("after 5 s: {}, {lit} lit pixels", c.method());
        assert!(lit > 640 * 360 / 20, "the share is black");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a display whose driver supports duplication; run by hand"]
    fn desktop_duplication_sees_the_screen() {
        let Some(screen) = sources().into_iter().find(|s| matches!(s.source, Source::Monitor { .. })) else { return };
        let Source::Monitor { rect } = screen.source else { return };
        let (w, h) = output_size(&screen.source, 640, 360).unwrap();
        let mut d = match super::dda::Dda::new(rect, w as usize, h as usize) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("desktop duplication unavailable here: {e}");
                return;
            }
        };
        let mut lit = 0;
        for _ in 0..20 {
            let f = d.grab().expect("frame");
            assert_eq!(f.len(), w as usize * h as usize * 4);
            lit = lit.max(f.chunks(4).filter(|p| p[0] > 16 || p[1] > 16 || p[2] > 16).count());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(lit > (w * h / 20) as usize, "the screen came out black ({lit} lit pixels)");
    }
}

#[cfg(all(test, windows))]
mod bench {
    /// `cargo test --release -p swiftcord-voice capture_encode_rate -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn capture_encode_rate() {
        use std::time::Instant;
        let screen = super::sources().into_iter().next().expect("a screen");
        for (mw, mh) in [(1280, 720), (1920, 1080)] {
            let mut c = super::Capturer::new(screen.source.clone(), mw, mh, false).unwrap();
            let (w, h) = c.size();
            let mut enc = crate::video::Encoder::new(w as u32, h as u32, 30, 3_000_000).unwrap();
            let (mut t_cap, mut t_conv, mut t_enc) = (0.0, 0.0, 0.0);
            let n = 60;
            for _ in 0..n {
                let t = Instant::now();
                let f = c.grab().unwrap();
                t_cap += t.elapsed().as_secs_f64();
                let t = Instant::now();
                let nv12 = crate::video::bgra_to_nv12(f.bgra, w, h);
                t_conv += t.elapsed().as_secs_f64();
                let t = Instant::now();
                enc.encode(&nv12).unwrap();
                t_enc += t.elapsed().as_secs_f64();
            }
            let ms = |t: f64| t * 1000.0 / n as f64;
            println!("{w}x{h}: capture {:.1} ms, convert {:.1} ms, encode {:.1} ms per frame", ms(t_cap), ms(t_conv), ms(t_enc));
        }
    }
}


#[cfg(all(test, windows))]
mod dump {
    /// Writes what the capturer sees to `SWIFTCORD_CAPTURE_DUMP` (a .bmp) for eyeballing.
    #[test]
    #[ignore]
    fn dump_frame() {
        let Ok(path) = std::env::var("SWIFTCORD_CAPTURE_DUMP") else { return };
        let screen = super::sources().into_iter().next().expect("a screen");
        let mut c = super::Capturer::new(screen.source, 1280, 720, false).unwrap();
        for _ in 0..10 {
            c.grab();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let f = c.grab().unwrap();
        let (w, h) = (f.width as u32, f.height as u32);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54 + w * h * 4).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&w.to_le_bytes());
        bmp.extend_from_slice(&(-(h as i32)).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(f.bgra);
        std::fs::write(path, bmp).unwrap();
    }
}
