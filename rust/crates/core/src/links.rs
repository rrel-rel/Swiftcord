//! Discord links that should open inside the client.

use crate::Snowflake;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageLink {
    /// `None` for DMs (`@me`).
    pub guild: Option<Snowflake>,
    pub channel: Snowflake,
    pub message: Option<Snowflake>,
}

/// Parses `https://discord.com/channels/{guild or @me}/{channel}[/{message}]`, including the ptb, canary
/// and legacy discordapp.com hosts.
pub fn parse(url: &str) -> Option<MessageLink> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !matches!(host, "discord.com" | "www.discord.com" | "ptb.discord.com" | "canary.discord.com" | "discordapp.com" | "www.discordapp.com") {
        return None;
    }
    let path = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut parts = path.split('/');
    if parts.next()? != "channels" {
        return None;
    }
    let guild = match parts.next()? {
        "@me" => None,
        g => Some(g.parse().ok()?),
    };
    let channel = parts.next()?.parse().ok()?;
    let message = match parts.next() {
        Some(m) => Some(m.parse().ok()?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(MessageLink { guild, channel, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links() {
        assert_eq!(
            parse("https://discord.com/channels/1/2/3"),
            Some(MessageLink { guild: Some(Snowflake(1)), channel: Snowflake(2), message: Some(Snowflake(3)) })
        );
        assert_eq!(
            parse("https://canary.discord.com/channels/@me/2"),
            Some(MessageLink { guild: None, channel: Snowflake(2), message: None })
        );
        assert_eq!(parse("https://discord.com/channels/1/2/3?x=1").map(|l| l.message), Some(Some(Snowflake(3))));
        assert_eq!(parse("https://discord.com/invite/abc"), None);
        assert_eq!(parse("https://evil.com/channels/1/2/3"), None);
    }
}
