pub mod captcha_webview;
pub mod clipboard;
pub mod credentials;
pub mod d3d11;
pub mod discord_install;
pub mod inline_player;
pub mod login_webview;
pub mod startup;
pub mod toast;
pub mod tray;
pub mod video_window;
pub mod webview_window;
pub mod wic;

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

/// A one-off Windows toast (message toasts are kept per channel; see `crate::notifications`).
pub fn notify(tag: &str, title: String, body: String) {
    toast::show(tag, title, body, true);
}
