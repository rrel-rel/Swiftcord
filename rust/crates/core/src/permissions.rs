//! Discord's permission resolution: base guild permissions from roles, then channel overwrites in the
//! order @everyone → roles (combined) → member.

use crate::models::{Channel, Guild};
use crate::Snowflake;

pub mod perm {
    pub const ADMINISTRATOR: u64 = 1 << 3;
    pub const ADD_REACTIONS: u64 = 1 << 6;
    pub const STREAM: u64 = 1 << 9;
    pub const VIEW_CHANNEL: u64 = 1 << 10;
    pub const SEND_MESSAGES: u64 = 1 << 11;
    /// Slash commands.
    pub const USE_APPLICATION_COMMANDS: u64 = 1 << 31;
    pub const SEND_TTS_MESSAGES: u64 = 1 << 12;
    pub const MANAGE_MESSAGES: u64 = 1 << 13;
    pub const EMBED_LINKS: u64 = 1 << 14;
    pub const ATTACH_FILES: u64 = 1 << 15;
    pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
    pub const MENTION_EVERYONE: u64 = 1 << 17;
    pub const CONNECT: u64 = 1 << 20;
    pub const SPEAK: u64 = 1 << 21;
    pub const MUTE_MEMBERS: u64 = 1 << 22;
    pub const DEAFEN_MEMBERS: u64 = 1 << 23;
    pub const MOVE_MEMBERS: u64 = 1 << 24;
    pub const USE_VAD: u64 = 1 << 25;
    pub const CHANGE_NICKNAME: u64 = 1 << 26;
    pub const MANAGE_THREADS: u64 = 1 << 34;
    pub const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
    pub const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
    pub const PIN_MESSAGES: u64 = 1 << 51;
    pub const ALL: u64 = u64::MAX;
}

pub fn compute_base(guild: &Guild, user_id: Snowflake, member_roles: &[Snowflake]) -> u64 {
    if guild.owner_id == user_id {
        return perm::ALL;
    }
    let mut p = guild.roles.iter().find(|r| r.id == guild.id).map_or(0, |r| r.permissions);
    for role in &guild.roles {
        if member_roles.contains(&role.id) {
            p |= role.permissions;
        }
    }
    if p & perm::ADMINISTRATOR != 0 { perm::ALL } else { p }
}

pub fn compute_overwrites(
    base: u64,
    channel: &Channel,
    guild_id: Snowflake,
    user_id: Snowflake,
    member_roles: &[Snowflake],
) -> u64 {
    if base & perm::ADMINISTRATOR != 0 {
        return perm::ALL;
    }
    let apply = |p: u64, allow: u64, deny: u64| (p & !deny) | allow;
    let ows = &channel.permission_overwrites;
    let mut p = base;
    if let Some(e) = ows.iter().find(|o| o.id == guild_id) {
        p = apply(p, e.allow, e.deny);
    }
    let (mut allow, mut deny) = (0, 0);
    for o in ows.iter().filter(|o| o.kind == 0 && o.id != guild_id && member_roles.contains(&o.id)) {
        allow |= o.allow;
        deny |= o.deny;
    }
    p = apply(p, allow, deny);
    if let Some(m) = ows.iter().find(|o| o.kind == 1 && o.id == user_id) {
        p = apply(p, m.allow, m.deny);
    }
    p
}

/// Full permissions for a member in a guild channel. Threads should be resolved against their parent.
pub fn compute(guild: &Guild, channel: &Channel, user_id: Snowflake, member_roles: &[Snowflake]) -> u64 {
    let base = compute_base(guild, user_id, member_roles);
    let mut p = compute_overwrites(base, channel, guild.id, user_id, member_roles);
    // Without View Channel no other channel permission applies.
    if p & perm::VIEW_CHANNEL == 0 {
        return 0;
    }
    // Without Send Messages, permissions that depend on it are implicitly denied.
    if p & perm::SEND_MESSAGES == 0 && !channel.is_thread() {
        p &= !(perm::MENTION_EVERYONE | perm::SEND_TTS_MESSAGES | perm::ATTACH_FILES | perm::EMBED_LINKS);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Overwrite, Role};

    fn guild() -> Guild {
        Guild {
            id: Snowflake(1),
            owner_id: Snowflake(99),
            roles: vec![
                Role { id: Snowflake(1), permissions: perm::VIEW_CHANNEL | perm::SEND_MESSAGES, ..Default::default() },
                Role { id: Snowflake(2), permissions: perm::MANAGE_MESSAGES, ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn owner_has_everything() {
        assert_eq!(compute_base(&guild(), Snowflake(99), &[]), perm::ALL);
    }

    #[test]
    fn overwrites_apply_in_order() {
        let g = guild();
        let channel = Channel {
            permission_overwrites: vec![
                // @everyone can't send…
                Overwrite { id: Snowflake(1), kind: 0, allow: 0, deny: perm::SEND_MESSAGES },
                // …but role 2 can.
                Overwrite { id: Snowflake(2), kind: 0, allow: perm::SEND_MESSAGES, deny: 0 },
                // And this member can't view at all.
                Overwrite { id: Snowflake(7), kind: 1, allow: 0, deny: perm::VIEW_CHANNEL },
            ],
            ..Default::default()
        };
        assert_eq!(compute(&g, &channel, Snowflake(5), &[]) & perm::SEND_MESSAGES, 0);
        assert_ne!(compute(&g, &channel, Snowflake(5), &[Snowflake(2)]) & perm::SEND_MESSAGES, 0);
        assert_eq!(compute(&g, &channel, Snowflake(7), &[Snowflake(2)]), 0);
    }
}
