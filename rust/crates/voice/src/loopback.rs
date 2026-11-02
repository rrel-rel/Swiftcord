//! What the PC is playing, for Go Live's audio: WASAPI process loopback, which can leave out or pick out one
//! process tree. A whole screen hears everything but ourselves (so the call isn't sent back into itself); a
//! window hears only the program that owns it.

/// Whose sound to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Everything except our own process tree.
    AllButUs,
    /// This process and its children.
    Process(u32),
}

/// Stereo samples per second, as Opus wants them.
pub const RATE: u32 = 48_000;

/// Interleaved 16-bit stereo to the floats the encoder takes.
pub fn to_float(pcm: &[u8], out: &mut Vec<f32>) {
    out.extend(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0));
}

#[cfg(windows)]
pub use win::{target_for, Capture};

#[cfg(windows)]
mod win {
    use std::time::Duration;

    use windows::core::imp::IAgileObject;
    use windows::core::{implement, IUnknown, Interface, Ref, HRESULT};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, SetEvent, WaitForSingleObject};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    use super::{Target, RATE};
    use crate::capture::Source;

    /// What to listen to while sharing `source`.
    pub fn target_for(source: &Source) -> Target {
        match source {
            Source::Monitor { .. } => Target::AllButUs,
            Source::Window { hwnd } => {
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(HWND(*hwnd as _), Some(&mut pid)) };
                if pid == 0 { Target::AllButUs } else { Target::Process(pid) }
            }
        }
    }

    /// Tells the waiting thread that activation finished. Process loopback insists on an agile handler.
    #[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
    struct Activated(HANDLE);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        fn ActivateCompleted(&self, _: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
            unsafe { SetEvent(self.0) }
        }
    }

    impl windows::core::imp::IAgileObject_Impl for Activated_Impl {}

    /// PROPVARIANT's layout when it holds a blob, the one form activation parameters come in.
    #[repr(C)]
    struct BlobVariant {
        vt: u16,
        reserved: [u16; 3],
        size: u32,
        data: *const AUDIOCLIENT_ACTIVATION_PARAMS,
    }
    const VT_BLOB: u16 = 65;

    pub struct Capture {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        event: HANDLE,
    }

    // COM is initialised for the multithreaded apartment on the thread that uses it.
    unsafe impl Send for Capture {}

    impl Capture {
        /// Starts capturing. Fails on Windows versions older than process loopback (before Windows 10 2004-ish).
        pub fn start(target: Target) -> Result<Capture, String> {
            unsafe { Self::open(target) }.map_err(|e| e.to_string())
        }

        unsafe fn open(target: Target) -> windows::core::Result<Capture> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let (pid, mode) = match target {
                    Target::AllButUs => (GetCurrentProcessId(), PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE),
                    Target::Process(pid) => (pid, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE),
                };
                let params = AUDIOCLIENT_ACTIVATION_PARAMS {
                    ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                    Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 { ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS { TargetProcessId: pid, ProcessLoopbackMode: mode } },
                };
                let blob = BlobVariant { vt: VT_BLOB, reserved: [0; 3], size: std::mem::size_of_val(&params) as u32, data: &params };
                let done = CreateEventW(None, false, false, None)?;
                let handler: IActivateAudioInterfaceCompletionHandler = Activated(done).into();
                let operation = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&blob as *const BlobVariant as *const PROPVARIANT), &handler);
                let operation = match operation {
                    Ok(o) => o,
                    Err(e) => {
                        let _ = CloseHandle(done);
                        return Err(e);
                    }
                };
                // Timed out, the handler may still fire later, so it keeps its event.
                if WaitForSingleObject(done, 5000) != WAIT_OBJECT_0 {
                    return Err(windows::core::Error::new(HRESULT(0x8000_000Au32 as i32), "audio capture didn't start in time"));
                }
                let _ = CloseHandle(done);
                let mut result = HRESULT(0);
                let mut unknown: Option<IUnknown> = None;
                operation.GetActivateResult(&mut result, &mut unknown)?;
                result.ok()?;
                let client: IAudioClient = unknown.ok_or_else(windows::core::Error::empty)?.cast()?;

                // Process loopback has no mix format of its own: it's converted to whatever is asked for.
                let format = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_PCM as u16,
                    nChannels: 2,
                    nSamplesPerSec: RATE,
                    nAvgBytesPerSec: RATE * 4,
                    nBlockAlign: 4,
                    wBitsPerSample: 16,
                    cbSize: 0,
                };
                let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                // 200 ms, in 100 ns units.
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &format, None)?;
                let event = CreateEventW(None, false, false, None)?;
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;
                Ok(Capture { client, capture, event })
            }
        }

        /// Waits up to `timeout` for sound and appends it to `out` as interleaved 16-bit stereo. False if the
        /// capture has failed.
        pub fn read(&mut self, timeout: Duration, out: &mut Vec<u8>) -> bool {
            unsafe {
                let _ = WaitForSingleObject(self.event, timeout.as_millis() as u32);
                loop {
                    match self.capture.GetNextPacketSize() {
                        Ok(0) => return true,
                        Ok(_) => {}
                        Err(_) => return false,
                    }
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    if self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                        return false;
                    }
                    let len = frames as usize * 4;
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        out.resize(out.len() + len, 0);
                    } else {
                        out.extend_from_slice(std::slice::from_raw_parts(data, len));
                    }
                    if self.capture.ReleaseBuffer(frames).is_err() {
                        return false;
                    }
                }
            }
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_float;

    #[test]
    fn pcm_becomes_floats() {
        let mut out = Vec::new();
        to_float(&[0, 0, 0, 0x40, 0, 0x80, 0xff], &mut out);
        assert_eq!(out, [0.0, 0.5, -1.0]);
    }
}
