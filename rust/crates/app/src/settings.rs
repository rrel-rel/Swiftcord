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
    /// Play the custom join sounds people set in Discord.
    pub join_sounds: bool,
    /// How loud people are in calls, by user (1.0 as sent; 0–2), as set from their voice menu.
    pub user_volumes: std::collections::HashMap<u64, f32>,
    /// People muted for us alone in calls.
    pub muted_users: std::collections::HashSet<u64>,
    /// Launched at sign-in, open in the tray rather than with the window up.
    pub start_minimized: bool,
    /// The volume of message sounds, 0 to 1. (It was the one volume for every sound, then for calls too.)
    #[serde(alias = "sound_volume", alias = "notification_volume")]
    pub message_volume: f32,
    /// The volume of call sounds (ringing, people joining and leaving), 0 to 1.
    pub call_volume: f32,
    /// The volume of clicks on buttons and list items, 0 to 1. (It was for rollovers too.)
    #[serde(alias = "menu_volume")]
    pub click_volume: f32,
    /// The volume of the sound as the pointer moves onto a list item, 0 to 1.
    pub rollover_volume: f32,
    /// The colour palette, by name (see `theme::names`).
    pub theme: String,
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
            join_sounds: true,
            user_volumes: Default::default(),
            muted_users: Default::default(),
            start_minimized: true,
            message_volume: 1.0,
            call_volume: 1.0,
            click_volume: 1.0,
            rollover_volume: 1.0,
            theme: "Steam".into(),
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
        std::fs::read_to_string(path()).ok().and_then(|s| Settings::parse(&s)).unwrap_or_default()
    }

    /// Settings from before calls and rollovers had their own volumes keep the levels those sounds had.
    fn parse(json: &str) -> Option<Settings> {
        let mut settings: Settings = serde_json::from_str(json).ok()?;
        let fields: serde_json::Value = serde_json::from_str(json).ok()?;
        if fields.get("call_volume").is_none() {
            settings.call_volume = settings.message_volume;
        }
        if fields.get("rollover_volume").is_none() {
            settings.rollover_volume = settings.click_volume;
        }
        Some(settings)
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::platform::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_volumes_carry_over() {
        let s = Settings::parse(r#"{"menu_volume": 0.25, "notification_volume": 0.5}"#).unwrap();
        assert_eq!((s.message_volume, s.call_volume, s.click_volume, s.rollover_volume), (0.5, 0.5, 0.25, 0.25));
        let s = Settings::parse(r#"{"sound_volume": 0.5}"#).unwrap();
        assert_eq!((s.message_volume, s.call_volume, s.click_volume, s.rollover_volume), (0.5, 0.5, 1.0, 1.0));
    }

    #[test]
    fn saved_volumes_stay_separate() {
        let saved = Settings { message_volume: 0.1, call_volume: 0.2, click_volume: 0.3, rollover_volume: 0.4, ..Settings::default() };
        let s = Settings::parse(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!((s.message_volume, s.call_volume, s.click_volume, s.rollover_volume), (0.1, 0.2, 0.3, 0.4));
    }
}
