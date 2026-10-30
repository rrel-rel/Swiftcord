pub mod captcha_webview;
pub mod credentials;
pub mod inline_player;
pub mod login_webview;
pub mod video_window;

use std::path::PathBuf;

/// `%LOCALAPPDATA%\Swiftcord`.
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

const APP_ID: &str = "Swiftcord.Rust";

/// Unpackaged apps need their AppUserModelID registered for toasts to show under their own name.
pub fn register_app_id() {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    let name: Vec<u16> = "Swiftcord".encode_utf16().chain([0]).collect();
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            &HSTRING::from("DisplayName"),
            REG_SZ.0,
            Some(name.as_ptr().cast()),
            (name.len() * 2) as u32,
        );
    }
}

/// A Windows toast. Runs on a worker thread because the WinRT call can block briefly.
pub fn notify(title: String, body: String) {
    std::thread::spawn(move || {
        let _ = tauri_winrt_notification::Toast::new(APP_ID)
            .title(&title)
            .text1(&body)
            .sound(Some(tauri_winrt_notification::Sound::Default))
            .show();
    });
}
