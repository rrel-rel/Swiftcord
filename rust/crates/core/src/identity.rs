//! How the client describes itself to Discord: as the official Windows desktop client, in the gateway's
//! IDENTIFY properties, the `X-Super-Properties` header and the user agent of its requests.
//!
//! The app reads the versions from the desktop client installed on the machine at startup (see
//! `platform::discord_install` in the app) and [`set`]s them before connecting. [`Identity::default`] holds
//! the values last seen, used if there's no installed client to read.

use std::sync::OnceLock;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The versions the desktop client reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The desktop app's version (its `app-*` folder), e.g. "1.0.9266".
    pub client_version: String,
    pub chrome_version: String,
    pub electron_version: String,
    /// Windows' version as "10.0.<build>".
    pub os_version: String,
    pub native_build_number: u32,
    /// The web client's build, which changes with each web release.
    pub client_build_number: u32,
}

impl Default for Identity {
    /// The desktop client as installed when this was written (October 2026).
    fn default() -> Self {
        Identity {
            client_version: "1.0.9266".into(),
            chrome_version: "152.0.7977.167".into(),
            electron_version: "44.9.3".into(),
            os_version: "10.0.26200".into(),
            native_build_number: 97026,
            client_build_number: 657850,
        }
    }
}

impl Identity {
    /// The desktop client's user agent: Electron's Chrome one, with Discord's version in it.
    pub fn user_agent(&self) -> String {
        format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) discord/{} Chrome/{} Electron/{} Safari/537.36",
            self.client_version, self.chrome_version, self.electron_version
        )
    }

    /// The client properties sent in IDENTIFY and, base64-encoded, in `X-Super-Properties`.
    pub fn properties(&self, locale: &str) -> Value {
        json!({
            "os": "Windows",
            "browser": "Discord Client",
            "release_channel": "stable",
            "client_version": self.client_version,
            "os_version": self.os_version,
            "os_arch": "x64",
            "app_arch": "x64",
            "system_locale": locale,
            "has_client_mods": false,
            "browser_user_agent": self.user_agent(),
            "browser_version": self.electron_version,
            "client_build_number": self.client_build_number,
            "native_build_number": self.native_build_number,
            "client_event_source": null,
        })
    }
}

static CURRENT: OnceLock<Identity> = OnceLock::new();

/// Sets the identity for the rest of the run; only the first call counts, and it should come before anything
/// connects.
pub fn set(identity: Identity) {
    let _ = CURRENT.set(identity);
}

/// The identity in use ([`Identity::default`] if none was set).
pub fn current() -> &'static Identity {
    CURRENT.get_or_init(Identity::default)
}

pub fn user_agent() -> String {
    current().user_agent()
}

pub fn properties(locale: &str) -> Value {
    current().properties(locale)
}

/// The `X-Super-Properties` header value.
pub fn super_properties(locale: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(properties(locale).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_and_properties_carry_the_versions() {
        let id = Identity::default();
        let ua = id.user_agent();
        assert!(ua.contains(&format!("discord/{} ", id.client_version)));
        assert!(ua.contains(&format!("Chrome/{} ", id.chrome_version)));
        assert!(ua.contains(&format!("Electron/{} ", id.electron_version)));
        let props = id.properties("en-US");
        assert_eq!(props["browser_user_agent"], ua);
        assert_eq!(props["client_build_number"], id.client_build_number);
        assert_eq!(props["os_version"], id.os_version);
    }
}
