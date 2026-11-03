pub mod badge;
pub mod captcha_webview;
pub mod clipboard;
pub mod credentials;
pub mod d3d11;
pub mod discord_install;
pub mod inline_player;
pub mod login_webview;
pub mod single_instance;
pub mod startup;
pub mod toast;
pub mod tray;
pub mod video_window;
pub mod webview_window;
pub mod wic;

use std::path::PathBuf;

/// `%LOCALAPPDATA%\Swiftcord`.
/// This process's CPU time so far (in 100 ns units, added up across processors) and memory in use (bytes).
#[cfg(windows)]
pub fn process_usage() -> (u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    unsafe {
        let process = GetCurrentProcess();
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let cpu = if GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user).is_ok() { ticks(kernel) + ticks(user) } else { 0 };
        let mut memory = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        let used = if GetProcessMemoryInfo(process, &mut memory, memory.cb).is_ok() { memory.WorkingSetSize as u64 } else { 0 };
        (cpu, used)
    }
}

#[cfg(not(windows))]
pub fn process_usage() -> (u64, u64) {
    (0, 0)
}

pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "Swiftcord")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("Swiftcord"))
}

pub fn open_url(url: &str) {
    // Only web links; never hand arbitrary schemes to the shell.
    if url.starts_with("https://") || url.starts_with("http://") {
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }
}

/// Puts the UI thread in a single-threaded COM apartment before anything else can. winit's window needs one
/// for drag and drop (it calls OleInitialize) and panics if the thread has joined the multithreaded one
/// first, which setting up WIC does (drawing the tray icon, before the window exists). In this apartment,
/// those later attempts just fail harmlessly.
pub fn init_ui_thread() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// How long since the user last used the keyboard or mouse (anywhere, not just in Swiftcord).
pub fn idle_time() -> std::time::Duration {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return std::time::Duration::ZERO;
    }
    // Both wrap after 49 days; the difference doesn't.
    std::time::Duration::from_millis(unsafe { GetTickCount() }.wrapping_sub(info.dwTime) as u64)
}

/// The app's name, as shown. Folders, the saved sign-in and Windows' records still say Swiftcord (the
/// name it had first), so renaming it kept everyone's settings and sign-in.
pub const APP_NAME: &str = "'08cord";

/// Shows a folder in Explorer.
pub fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

const APP_ID: &str = "Swiftcord.Rust";

/// Unpackaged apps need their AppUserModelID registered for toasts to show under their own name and icon
/// (which Windows reads from a file: the app's, written next to the settings).
pub fn register_app_id() {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    let icon = data_dir().join("icon.png");
    let png: &[u8] = include_bytes!("../../assets/icon.png");
    if std::fs::read(&icon).ok().as_deref() != Some(png) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(&icon, png);
    }
    let set = |name: &str, value: &str| {
        let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
        unsafe {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &key, &HSTRING::from(name), REG_SZ.0, Some(value.as_ptr().cast()), (value.len() * 2) as u32);
        }
    };
    set("DisplayName", APP_NAME);
    set("IconUri", &icon.to_string_lossy());
}

/// A one-off Windows toast (message toasts are kept per channel; see `crate::notifications`). Clicking it
/// opens `channel`.
pub fn notify(tag: &str, title: String, body: String, channel: u64) {
    toast::show(tag, title, body, true, channel);
}
