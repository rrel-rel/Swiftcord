//! Launching at sign-in: an entry for this exe in the current user's Run key. Windows' own list is the setting
//! (not settings.json), so turning it off in Task Manager's Startup apps shows here too.

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ};

const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Where Task Manager records the Run entries it has disabled (the first byte odd: off).
const APPROVED: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
const NAME: PCWSTR = w!("Swiftcord");
/// Given when started at sign-in, which opens in the tray (if the settings say so).
pub const ARG: &str = "--startup";

fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" {ARG}", exe.display()))
}

/// The Run entry's command, if there is one.
fn listed() -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
    Some(String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

/// Brings an entry for this exe up to date (one made before `--startup` existed); entries for other copies
/// are left alone.
pub fn refresh() {
    let (Some(listed), Some(command)) = (listed(), command()) else { return };
    let exe = std::env::current_exe().map(|e| format!("\"{}\"", e.display())).unwrap_or_default();
    if listed != command && listed.to_lowercase().starts_with(&exe.to_lowercase()) {
        // Only the command: whether it's enabled stays as it was.
        let _ = write_command();
    }
}

fn write_command() -> Result<(), String> {
    let command = command().ok_or("couldn't find Swiftcord's exe")?;
    let command: Vec<u16> = command.encode_utf16().chain([0]).collect();
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(command.as_ptr().cast()), (command.len() * 2) as u32) }
        .ok()
        .map_err(|e| e.message())
}

/// Whether Swiftcord starts when the user signs in.
pub fn enabled() -> bool {
    let listed = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, None) }.is_ok();
    let mut flags = [0u8; 12];
    let mut len = flags.len() as u32;
    let disabled = unsafe { RegGetValueW(HKEY_CURRENT_USER, APPROVED, NAME, RRF_RT_REG_BINARY, None, Some(flags.as_mut_ptr().cast()), Some(&mut len)) }
        .is_ok()
        && flags[0] & 1 == 1;
    listed && !disabled
}

/// Adds this exe to the Run key (enabled, even if Task Manager had disabled it), or takes it out.
pub fn set(on: bool) -> Result<(), String> {
    unsafe {
        if on {
            write_command()?;
            // Twelve bytes: a flag (2 is enabled) and the time it changed (none).
            let approved = [2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, APPROVED, NAME, REG_BINARY.0, Some(approved.as_ptr().cast()), approved.len() as u32);
        } else {
            let result = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME);
            if result.is_err() && result != ERROR_FILE_NOT_FOUND {
                return Err(HSTRING::from(result.to_hresult().message()).to_string());
            }
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED, NAME);
        }
    }
    Ok(())
}
