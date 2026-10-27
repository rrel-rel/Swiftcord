//! Swiftcord's platform-independent Discord client: REST, gateway, state cache, markdown and permissions.

pub mod cdn;
pub mod commands;
pub mod events;
pub mod gateway;
pub mod interactions;
pub mod links;
pub mod markdown;
pub mod mentions;
pub mod models;
pub mod permissions;
pub mod proto;
pub mod remote_auth;
pub mod rest;
pub mod search;
mod snowflake;
pub mod state;

pub use snowflake::Snowflake;

/// A snowflake for "now", used as a message nonce so optimistic copies sort where the real message will.
pub fn nonce_now() -> Snowflake {
    let ms = chrono::Utc::now().timestamp_millis() as u64;
    let discord_ms = ms.saturating_sub(1_420_070_400_000);
    Snowflake((discord_ms << 22) | (rand::random::<u32>() as u64 & 0x3f_ffff))
}
