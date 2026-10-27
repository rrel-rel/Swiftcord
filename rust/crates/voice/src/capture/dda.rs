//! DXGI Desktop Duplication: a monitor's picture as the display shows it. Unlike Windows.Graphics.Capture,
//! which only sees what the compositor draws, this can also see fullscreen-exclusive games. It's opt-in
//! ("Capture fullscreen games") because it's less dependable: on some setups it delivers nothing, in which
//! case the capturer falls back, and two at once can crash inside the driver, so only one may exist.
//! Duplication frames don't include the pointer, so it's drawn in from the shape and position Windows reports.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::*;

use super::scale_into;

/// The pointer as last reported: its shape (BGRA, with a mask for XOR pixels) and where it is on the monitor.
#[derive(Default)]
struct Pointer {
    visible: bool,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    /// BGRA with straight alpha; pixels that invert the screen are marked in `invert`.
    bgra: Vec<u8>,
    invert: Vec<bool>,
}

/// Set while a duplication exists. Two at once (even of different capturers) can crash inside the graphics
/// driver, so a second capture falls back to Windows.Graphics.Capture instead.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Holds [`ACTIVE`] for as long as it lives.
struct Claim;

impl Claim {
    fn take() -> Result<Claim> {
        if ACTIVE.swap(true, Ordering::AcqRel) {
            return Err(windows::core::Error::new(DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, "another capture is already duplicating the desktop"));
        }
        Ok(Claim)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

pub struct Dda {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    /// The monitor's size.
    source: (usize, usize),
    staging: Option<ID3D11Texture2D>,
    /// The latest picture without the pointer, so the pointer can move without a new frame.
    clean: Vec<u8>,
    out: Vec<u8>,
    width: usize,
    height: usize,
    pointer: Pointer,
    /// When duplication was last (re)started, so a failing one isn't retried every frame.
    retry_at: std::time::Instant,
    /// Duplication just (re)started: its first frame (the whole screen) is waited for, since later frames
    /// only come when something changes.
    fresh: bool,
    /// A picture has arrived (if none does, the capturer falls back to another method).
    pub pictured: bool,
    /// Declared last: released after the duplication and device are.
    _claim: Claim,
}

/// The adapter and output showing the monitor at `rect` (desktop coordinates).
fn find_output(rect: (i32, i32, i32, i32)) -> Result<(IDXGIAdapter1, IDXGIOutput1)> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut a = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a) } {
        let mut o = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o) } {
            let desc = unsafe { output.GetDesc()? };
            let r = desc.DesktopCoordinates;
            if (r.left, r.top, r.right, r.bottom) == rect {
                return Ok((adapter, output.cast()?));
            }
            o += 1;
        }
        a += 1;
    }
    Err(windows::core::Error::new(DXGI_ERROR_NOT_FOUND, "no display output matches that monitor"))
}

impl Dda {
    pub fn new(rect: (i32, i32, i32, i32), width: usize, height: usize) -> Result<Dda> {
        let claim = Claim::take()?;
        let (adapter, output) = find_output(rect)?;
        let (mut device, mut context) = (None, None);
        unsafe {
            // Duplication must run on the GPU driving the display.
            D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        }
        let (device, context) = (device.unwrap(), context.unwrap());
        let duplication = unsafe { output.DuplicateOutput(&device)? };
        let source = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
        Ok(Dda {
            device,
            context,
            output,
            duplication: Some(duplication),
            source,
            staging: None,
            clean: vec![0; width * height * 4],
            out: vec![0; width * height * 4],
            width,
            height,
            pointer: Pointer::default(),
            retry_at: std::time::Instant::now(),
            fresh: true,
            pictured: false,
            _claim: claim,
        })
    }

    /// The newest picture scaled into the output, with the pointer (the previous picture if nothing changed).
    pub fn grab(&mut self) -> Option<&[u8]> {
        if self.duplication.is_none() && self.retry_at.elapsed() > std::time::Duration::from_millis(250) {
            // Lost (a game switched display modes, the desktop switched, …): start again.
            self.retry_at = std::time::Instant::now();
            self.duplication = unsafe { self.output.DuplicateOutput(&self.device) }.ok();
            self.fresh = true;
        }
        if let Some(dup) = self.duplication.clone() {
            let (mut info, mut resource) = (DXGI_OUTDUPL_FRAME_INFO::default(), None);
            let wait = if self.fresh { 500 } else { 0 };
            match unsafe { dup.AcquireNextFrame(wait, &mut info, &mut resource) } {
                Ok(()) => {
                    self.fresh = false;
                    if let Err(e) = self.take(&dup, &info, resource) {
                        tracing::debug!("desktop duplication frame failed: {e}");
                    }
                    unsafe {
                        let _ = dup.ReleaseFrame();
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(e) => {
                    tracing::info!("desktop duplication lost ({e}); restarting it");
                    self.duplication = None;
                }
            }
        }
        self.out.copy_from_slice(&self.clean);
        self.draw_pointer();
        Some(&self.out)
    }

    fn take(&mut self, dup: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO, resource: Option<IDXGIResource>) -> Result<()> {
        if info.LastMouseUpdateTime != 0 {
            self.pointer.visible = info.PointerPosition.Visible.as_bool();
            self.pointer.x = info.PointerPosition.Position.x;
            self.pointer.y = info.PointerPosition.Position.y;
        }
        if info.PointerShapeBufferSize > 0 {
            self.read_pointer_shape(dup, info.PointerShapeBufferSize)?;
        }
        // No new picture (only the pointer moved).
        if info.LastPresentTime == 0 {
            return Ok(());
        }
        let Some(resource) = resource else { return Ok(()) };
        let texture: ID3D11Texture2D = resource.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        if self.staging.is_none() {
            let sd = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut tex = None;
            unsafe { self.device.CreateTexture2D(&sd, None, Some(&mut tex))? };
            self.staging = tex;
        }
        let Some(staging) = &self.staging else { return Ok(()) };
        unsafe {
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * desc.Height as usize);
            self.source = (desc.Width as usize, desc.Height as usize);
            scale_into(src, desc.Width as usize, desc.Height as usize, pitch, &mut self.clean, self.width, self.height);
            self.context.Unmap(staging, 0);
        }
        self.pictured = true;
        Ok(())
    }

    /// Reads a new pointer shape into BGRA (colour, masked colour and monochrome kinds).
    fn read_pointer_shape(&mut self, dup: &IDXGIOutputDuplication, size: u32) -> Result<()> {
        let mut buf = vec![0u8; size as usize];
        let (mut needed, mut info) = (0u32, DXGI_OUTDUPL_POINTER_SHAPE_INFO::default());
        unsafe { dup.GetFramePointerShape(size, buf.as_mut_ptr().cast(), &mut needed, &mut info)? };
        let (w, pitch) = (info.Width as usize, info.Pitch as usize);
        let p = &mut self.pointer;
        if info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
            // An AND mask then an XOR mask, one bit per pixel, each half the height.
            let h = info.Height as usize / 2;
            (p.width, p.height) = (w, h);
            p.bgra = vec![0; w * h * 4];
            p.invert = vec![false; w * h];
            for y in 0..h {
                for x in 0..w {
                    let bit = |row: usize| buf.get(row * pitch + x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
                    let (and, xor) = (bit(y), bit(y + h));
                    let i = y * w + x;
                    match (and, xor) {
                        (false, false) => p.bgra[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]),
                        (false, true) => p.bgra[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255]),
                        (true, true) => p.invert[i] = true,
                        (true, false) => {}
                    }
                }
            }
        } else {
            let h = info.Height as usize;
            (p.width, p.height) = (w, h);
            p.bgra = vec![0; w * h * 4];
            p.invert = vec![false; w * h];
            let masked = info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32;
            for y in 0..h {
                for x in 0..w {
                    let Some(px) = buf.get(y * pitch + x * 4..y * pitch + x * 4 + 4) else { continue };
                    let i = y * w + x;
                    if masked {
                        // Alpha 0: replace the screen with the colour; 0xFF: XOR (treated as invert when non-black).
                        if px[3] == 0 {
                            p.bgra[i * 4..i * 4 + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                        } else {
                            p.invert[i] = px[..3] != [0, 0, 0];
                        }
                    } else {
                        p.bgra[i * 4..i * 4 + 4].copy_from_slice(px);
                    }
                }
            }
        }
        Ok(())
    }

    /// Draws the pointer into the output at the output's scale.
    fn draw_pointer(&mut self) {
        let p = &self.pointer;
        if !p.visible || p.width == 0 {
            return;
        }
        let (sx, sy) = (self.width as f32 / self.source.0.max(1) as f32, self.height as f32 / self.source.1.max(1) as f32);
        let (dw, dh) = (((p.width as f32 * sx).ceil() as usize).max(1), ((p.height as f32 * sy).ceil() as usize).max(1));
        let (ox, oy) = ((p.x as f32 * sx) as i64, (p.y as f32 * sy) as i64);
        for dy in 0..dh {
            for dx in 0..dw {
                let (x, y) = (ox + dx as i64, oy + dy as i64);
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
                    continue;
                }
                let (px, py) = (((dx as f32 / sx) as usize).min(p.width - 1), ((dy as f32 / sy) as usize).min(p.height - 1));
                let i = py * p.width + px;
                let o = (y as usize * self.width + x as usize) * 4;
                if p.invert[i] {
                    for c in 0..3 {
                        self.out[o + c] = 255 - self.out[o + c];
                    }
                    continue;
                }
                let a = p.bgra[i * 4 + 3] as u32;
                if a == 0 {
                    continue;
                }
                for c in 0..3 {
                    let (s, d) = (p.bgra[i * 4 + c] as u32, self.out[o + c] as u32);
                    self.out[o + c] = ((s * a + d * (255 - a)) / 255) as u8;
                }
            }
        }
    }
}
