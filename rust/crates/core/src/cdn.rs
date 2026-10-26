//! Discord CDN URLs.

use crate::models::{Channel, Guild, User};
use crate::Snowflake;

const CDN: &str = "https://cdn.discordapp.com";

fn ext(hash: &str, animated_ok: bool) -> &'static str {
    if animated_ok && hash.starts_with("a_") { "gif" } else { "webp" }
}

pub fn avatar(user: &User, size: u32) -> String {
    match &user.avatar {
        Some(h) => format!("{CDN}/avatars/{}/{h}.{}?size={size}", user.id, ext(h, false)),
        None => default_avatar(user),
    }
}

pub fn default_avatar(user: &User) -> String {
    // New usernames index by ID; legacy discriminators by discriminator.
    let index = match user.discriminator.as_deref().and_then(|d| d.parse::<u64>().ok()) {
        Some(d) if d != 0 => d % 5,
        _ => (user.id.0 >> 22) % 6,
    };
    format!("{CDN}/embed/avatars/{index}.png")
}

pub fn guild_icon(guild: &Guild, size: u32) -> Option<String> {
    guild.icon.as_ref().map(|h| format!("{CDN}/icons/{}/{h}.{}?size={size}", guild.id, ext(h, false)))
}

pub fn group_dm_icon(channel: &Channel, size: u32) -> Option<String> {
    channel.icon.as_ref().map(|h| format!("{CDN}/channel-icons/{}/{h}.webp?size={size}", channel.id))
}

pub fn emoji(id: Snowflake, animated: bool, size: u32) -> String {
    format!("{CDN}/emojis/{id}.{}?size={size}", if animated { "gif" } else { "webp" })
}

/// A sticker as an image: still for APNG ones (see [`sticker_file`] for the animation), `None` for Lottie
/// ones, which have no image.
pub fn sticker(id: Snowflake, format_type: u8) -> Option<String> {
    match format_type {
        1 => Some(format!("https://media.discordapp.net/stickers/{id}.png?size=160")),
        2 => Some(format!("https://media.discordapp.net/stickers/{id}.png?size=160&passthrough=false")),
        4 => Some(format!("https://media.discordapp.net/stickers/{id}.gif?size=160")),
        _ => None,
    }
}

/// An animated sticker's file: the APNG, or the Lottie animation (JSON).
pub fn sticker_file(id: Snowflake, format_type: u8) -> Option<String> {
    match format_type {
        2 => Some(format!("https://media.discordapp.net/stickers/{id}.png?size=160")),
        3 => Some(format!("{CDN}/stickers/{id}.json")),
        _ => None,
    }
}

/// A resized copy of an image from Discord's media proxy. Other hosts don't understand the parameters, so
/// their URLs are left alone.
pub fn media_proxy(proxy_url: &str, width: u32, height: u32) -> String {
    let host = proxy_url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
    if !(host == "media.discordapp.net" || host.ends_with(".discordapp.net") || host == "cdn.discordapp.com") {
        return proxy_url.to_owned();
    }
    let sep = if proxy_url.contains('?') { '&' } else { '?' };
    format!("{proxy_url}{sep}width={width}&height={height}&format=webp")
}

pub fn twemoji(emoji: &str) -> String {
    format!(
        "https://cdn.jsdelivr.net/gh/jdecked/twemoji@17.0.3/assets/72x72/{}.png",
        crate::markdown::twemoji_name(emoji)
    )
}
