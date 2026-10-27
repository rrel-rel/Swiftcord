//! Webcams, through Media Foundation: listing them, and reading frames as packed NV12 for the H.264 encoder.

/// Camera video is sent at most this size (the camera's own aspect ratio is kept).
pub const MAX_WIDTH: u32 = 1280;
pub const MAX_HEIGHT: u32 = 720;
pub const FPS: u32 = 30;
/// Bits per second for camera video.
pub const BITRATE: u32 = 1_500_000;

/// The size to send a `width`×`height` camera at: scaled down to fit [`MAX_WIDTH`]×[`MAX_HEIGHT`], even sides.
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    let scale = (MAX_WIDTH as f64 / width.max(1) as f64).min(MAX_HEIGHT as f64 / height.max(1) as f64).min(1.0);
    let even = |v: f64| ((v as u32) & !1).max(2);
    (even(width as f64 * scale), even(height as f64 * scale))
}

#[cfg(windows)]
pub use mf::{list, Camera};

#[cfg(not(windows))]
pub fn list() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
mod mf {
    use windows::core::PWSTR;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    fn startup() -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)
        }
    }

    /// The video capture devices, with their names.
    fn devices() -> windows::core::Result<Vec<(String, IMFActivate)>> {
        startup()?;
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
            attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attrs, &mut list, &mut count)?;
            let mut out = Vec::new();
            for i in 0..count as usize {
                if let Some(a) = (*list.add(i)).take() {
                    let mut name = PWSTR::null();
                    let mut len = 0u32;
                    let label = if a.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &mut name, &mut len).is_ok() {
                        let s = name.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(name.0 as *const _));
                        s
                    } else {
                        format!("Camera {}", i + 1)
                    };
                    out.push((label, a));
                }
            }
            CoTaskMemFree(Some(list as *const _));
            Ok(out)
        }
    }

    /// Webcam names, for settings.
    pub fn list() -> Vec<String> {
        devices().map(|d| d.into_iter().map(|(n, _)| n).collect()).unwrap_or_default()
    }

    /// An open webcam delivering NV12 frames of [`Camera::size`]. Use it on one thread.
    pub struct Camera {
        reader: IMFSourceReader,
        source: IMFMediaSource,
        width: u32,
        height: u32,
        stride: usize,
    }

    impl Camera {
        /// Opens the named camera, or the first one.
        pub fn open(name: Option<&str>) -> Result<Camera, String> {
            let mut devices = devices().map_err(|e| e.to_string())?;
            if devices.is_empty() {
                return Err("No camera found.".into());
            }
            let index = name.and_then(|n| devices.iter().position(|(d, _)| d == n)).unwrap_or(0);
            let (label, activate) = devices.swap_remove(index);
            unsafe {
                let source: IMFMediaSource = activate.ActivateObject().map_err(|e| format!("Couldn't open {label}: {e}"))?;
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
                let attrs = attrs.ok_or("no attributes")?;
                // Lets the reader convert and scale to the NV12 size we ask for.
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
                let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs).map_err(|e| format!("Couldn't read {label}: {e}"))?;
                let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
                reader.SetStreamSelection(VIDEO, true).map_err(|e| e.to_string())?;
                let native = reader.GetNativeMediaType(VIDEO, 0).map_err(|e| e.to_string())?;
                let size = native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(((640u64) << 32) | 480);
                let (width, height) = super::fit((size >> 32) as u32, size as u32);
                let want = MFCreateMediaType().map_err(|e| e.to_string())?;
                want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
                want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| e.to_string())?;
                want.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64).map_err(|e| e.to_string())?;
                want.SetUINT64(&MF_MT_FRAME_RATE, ((super::FPS as u64) << 32) | 1).map_err(|e| e.to_string())?;
                reader.SetCurrentMediaType(VIDEO, None, &want).map_err(|e| format!("{label} can't give video in a usable format: {e}"))?;
                let got = reader.GetCurrentMediaType(VIDEO).map_err(|e| e.to_string())?;
                let size = got.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(((width as u64) << 32) | height as u64);
                let (width, height) = ((size >> 32) as u32, size as u32);
                let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| (s as i32).unsigned_abs() as usize).unwrap_or(width as usize);
                tracing::info!("camera {label}: {width}x{height}");
                Ok(Camera { reader, source, width, height, stride: stride.max(width as usize) })
            }
        }

        pub fn size(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        /// The next frame as packed NV12 (waits for the camera); `None` when the camera stopped.
        pub fn read(&mut self) -> Option<Vec<u8>> {
            unsafe {
                loop {
                    let (mut flags, mut sample) = (0u32, None);
                    self.reader.ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample)).ok()?;
                    if flags & (MF_SOURCE_READERF_ERROR.0 as u32 | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                        return None;
                    }
                    let Some(sample) = sample else { continue };
                    let buffer = sample.ConvertToContiguousBuffer().ok()?;
                    let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
                    buffer.Lock(&mut ptr, None, Some(&mut len)).ok()?;
                    let data = std::slice::from_raw_parts(ptr, len as usize);
                    let frame = pack_nv12(data, self.width as usize, self.height as usize, self.stride);
                    let _ = buffer.Unlock();
                    return frame;
                }
            }
        }
    }

    impl Drop for Camera {
        fn drop(&mut self) {
            unsafe {
                let _ = self.source.Shutdown();
            }
        }
    }

    /// Copies NV12 with row padding (`stride`) into a tightly packed frame.
    fn pack_nv12(data: &[u8], width: usize, height: usize, stride: usize) -> Option<Vec<u8>> {
        if stride == width {
            return data.get(..width * height * 3 / 2).map(<[u8]>::to_vec);
        }
        let rows_y = data.len() * 2 / 3 / stride;
        let uv = stride * rows_y.max(height);
        let mut out = Vec::with_capacity(width * height * 3 / 2);
        for y in 0..height {
            out.extend_from_slice(data.get(y * stride..y * stride + width)?);
        }
        for y in 0..height / 2 {
            out.extend_from_slice(data.get(uv + y * stride..uv + y * stride + width)?);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_within_720p_keeping_the_shape() {
        assert_eq!(fit(1920, 1080), (1280, 720));
        assert_eq!(fit(640, 480), (640, 480));
        assert_eq!(fit(1440, 1080), (960, 720));
        let (w, h) = fit(1281, 721);
        assert!(w <= 1280 && h <= 720 && w % 2 == 0 && h % 2 == 0);
    }

    #[cfg(windows)]
    #[test]
    fn listing_cameras_works_without_any() {
        // Machines without a camera get an empty list rather than an error.
        let _ = list();
    }
}
