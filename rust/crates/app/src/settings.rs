//! App preferences, saved as JSON in `%LOCALAPPDATA%\Swiftcord\settings.json`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// Voice activity threshold in dBFS.
    pub vad_threshold_db: f32,
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { input_device: None, output_device: None, vad_threshold_db: -50.0, notifications: true }
    }
}

fn path() -> std::path::PathBuf {
    crate::platform::data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::platform::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }
}
