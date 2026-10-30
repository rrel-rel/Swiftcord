//! The last session, saved at shutdown and shown at the next start until the gateway is ready: the app opens
//! on what it showed when it was closed instead of "Connecting to Discord…". Things that need Discord wait
//! for the connection (see `App::apply`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use swiftcord_core::state::{Snapshot, State};
use swiftcord_core::Snowflake;

use crate::app::UiState;

#[derive(Serialize, Deserialize)]
pub struct Saved {
    pub state: Snapshot,
    /// What was open: the server (none for Home), each server's channel, and the collapsed categories.
    pub guild: Option<Snowflake>,
    pub channel_for: Vec<(Snowflake, Snowflake)>,
    pub collapsed: Vec<Snowflake>,
}

fn path() -> PathBuf {
    crate::platform::data_dir().join("cache").join("session.json")
}

pub fn save(state: &State, ui: &UiState) {
    let saved = Saved {
        state: state.snapshot(),
        guild: ui.guild,
        channel_for: ui.channel_for.iter().map(|(g, c)| (*g, *c)).collect(),
        collapsed: ui.collapsed.iter().copied().collect(),
    };
    let path = path();
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("cache dir"))?;
        // Written beside and then moved over, so a crash mid-write can't leave half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
        std::fs::rename(&tmp, &path)
    })();
    match result {
        Ok(()) => tracing::info!("saved the session for the next start"),
        Err(e) => tracing::warn!("couldn't save the session: {e}"),
    }
}

/// The saved session, unless there's none or it's from a version that laid it out differently.
pub fn load() -> Option<Saved> {
    let bytes = std::fs::read(path()).ok()?;
    match serde_json::from_slice::<Saved>(&bytes) {
        Ok(saved) if saved.state.version == Snapshot::VERSION => Some(saved),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("couldn't read the saved session: {e}");
            None
        }
    }
}

/// Forgets the saved session (signing out, or a session Discord ended).
pub fn delete() {
    let _ = std::fs::remove_file(path());
}
