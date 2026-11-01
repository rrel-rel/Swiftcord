//! Offline mode (`SWIFTCORD_OFFLINE=1`): nothing connects to Discord or fetches from the internet (no gateway,
//! API, QR sign-in, voice, media or clip downloads), so the app can be developed and tested without touching
//! the account. Every network entry point checks [`is_on`]; the app shows the session saved last time, from
//! disk, and anything that needs Discord does nothing, as before the connection is ready.

use std::sync::atomic::{AtomicBool, Ordering};

static OFFLINE: AtomicBool = AtomicBool::new(false);

/// Turns offline mode on for the rest of the run (before anything connects).
pub fn enable() {
    OFFLINE.store(true, Ordering::Relaxed);
}

pub fn is_on() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}
