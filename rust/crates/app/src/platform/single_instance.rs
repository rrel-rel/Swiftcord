//! One Swiftcord per exe: starting it again brings the running one's window back instead of opening a second
//! client (and a second connection to the account). Copies at different paths, such as an installed one and
//! a build, are separate instances.

use std::sync::OnceLock;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;

/// A name for this exe's instance, from its path.
pub fn key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        let path = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
        // FNV-1a: the same on every build, unlike std's hasher.
        let hash = path.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        format!("Swiftcord-{hash:016x}")
    })
}

/// Claims this exe's instance for the rest of the run. `false` if it's already running, in which case its
/// window has been asked to come back.
pub fn claim() -> bool {
    let name = HSTRING::from(format!(r"Local\{}", key()));
    let Ok(handle) = (unsafe { CreateMutexW(None, false, &name) }) else { return true };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        super::tray::show_running();
        return false;
    }
    // Held until the process ends.
    true
}
