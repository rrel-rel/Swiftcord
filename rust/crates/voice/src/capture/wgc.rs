//! Windows.Graphics.Capture: the compositor hands us each new frame of a monitor or window (even one that's
//! covered), with the pointer drawn in. Frames are copied to the CPU and scaled into the output.

use windows::core::{factory, Interface, Result};
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use super::{scale_into, Source};

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

pub struct Wgc {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d3d: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    window: Option<HWND>,
    out: Vec<u8>,
    width: usize,
    height: usize,
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut last = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        match try_device(driver) {
            Ok(d) => return Ok(d),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap())
}

fn try_device(driver: D3D_DRIVER_TYPE) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
    }
    Ok((device.unwrap(), context.unwrap()))
}

impl Wgc {
    pub fn new(source: &Source, width: usize, height: usize) -> Result<Wgc> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let (device, context) = create_device()?;
        let dxgi: IDXGIDevice = device.cast()?;
        let d3d: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let (item, window): (GraphicsCaptureItem, _) = match source {
            Source::Monitor { rect } => {
                let m = unsafe { MonitorFromPoint(POINT { x: (rect.0 + rect.2) / 2, y: (rect.1 + rect.3) / 2 }, MONITOR_DEFAULTTONEAREST) };
                (unsafe { interop.CreateForMonitor(m)? }, None)
            }
            Source::Window { hwnd } => {
                let h = HWND(*hwnd as *mut core::ffi::c_void);
                (unsafe { interop.CreateForWindow(h)? }, Some(h))
            }
        };
        // Without this permission, Windows outlines what's being captured in yellow.
        let _ = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless).and_then(|op| op.join());
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, FORMAT, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture()?;
        Ok(Wgc { device, context, d3d, pool, session, pool_size: size, staging: None, window, out: vec![0; width * height * 4], width, height })
    }

    /// The newest picture scaled into the output (the previous one again if nothing changed); None once a
    /// shared window has closed.
    pub fn grab(&mut self) -> Option<&[u8]> {
        if let Some(h) = self.window {
            if !unsafe { IsWindow(Some(h)) }.as_bool() {
                return None;
            }
        }
        let mut newest = None;
        while let Ok(f) = self.pool.TryGetNextFrame() {
            newest = Some(f);
        }
        if let Some(frame) = newest {
            if let Err(e) = self.copy(&frame) {
                tracing::debug!("capture frame copy failed: {e}");
            }
        }
        Some(&self.out)
    }

    fn copy(&mut self, frame: &Direct3D11CaptureFrame) -> Result<()> {
        let content = frame.ContentSize()?;
        if (content.Width, content.Height) != (self.pool_size.Width, self.pool_size.Height) {
            // The window changed size: later frames come at the new size.
            self.pool.Recreate(&self.d3d, FORMAT, 2, content)?;
            self.pool_size = content;
        }
        let texture: ID3D11Texture2D = unsafe { frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()? };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        if self.staging.as_ref().is_none_or(|s| (s.1, s.2) != (desc.Width, desc.Height)) {
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
            self.staging = tex.map(|t| (t, desc.Width, desc.Height));
        }
        let Some((staging, _, _)) = &self.staging else { return Ok(()) };
        let (cw, ch) = ((content.Width.max(1) as u32).min(desc.Width) as usize, (content.Height.max(1) as u32).min(desc.Height) as usize);
        unsafe {
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * desc.Height as usize);
            scale_into(src, cw, ch, pitch, &mut self.out, self.width, self.height);
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }
}

impl Drop for Wgc {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
