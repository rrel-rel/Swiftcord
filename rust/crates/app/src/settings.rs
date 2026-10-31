//! App preferences, saved as JSON in `%LOCALAPPDATA%\Swiftcord\settings.json`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// The webcam for calls (`None`: the first one).
    pub camera_device: Option<String>,
    /// Voice activity threshold in dBFS.
    pub vad_threshold_db: f32,
    pub notifications: bool,
    /// The member list beside server channels is closed.
    pub hide_members: bool,
    /// Volume of Go Live streams being watched (1.0 is as sent; up to 2.0).
    pub stream_volume: f32,
    /// Keep hearing a watched stream while deafened.
    pub stream_audio_when_deafened: bool,
    /// Remove background noise from the microphone.
    pub noise_suppression: bool,
    /// Sounds for new messages (when a notification would show).
    pub message_sounds: bool,
    /// Sounds for calls: ringing, and people joining or leaving yours.
    pub call_sounds: bool,
    /// How loud people are in calls, by user (1.0 as sent; 0–2), as set from their voice menu.
    pub user_volumes: std::collections::HashMap<u64, f32>,
    /// People muted for us alone in calls.
    pub muted_users: std::collections::HashSet<u64>,
    /// Launched at sign-in, open in the tray rather than with the window up.
    pub start_minimized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            camera_device: None,
            vad_threshold_db: -50.0,
            notifications: true,
            hide_members: false,
            stream_volume: 1.0,
            stream_audio_when_deafened: false,
            noise_suppression: true,
            message_sounds: true,
            call_sounds: true,
            user_volumes: Default::default(),
            muted_users: Default::default(),
            start_minimized: true,
        }
    }
}

impl Settings {
    /// The gain to play someone at: silent when muted for us, else their volume.
    pub fn user_gain(&self, user: u64) -> f32 {
        if self.muted_users.contains(&user) {
            0.0
        } else {
            self.user_volumes.get(&user).copied().unwrap_or(1.0)
        }
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
