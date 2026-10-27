//! In-memory cache of everything the client knows, kept current from gateway events. It's owned by one
//! thread (the UI); events are delivered to it rather than it being shared behind a lock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::events::*;
use crate::models::*;
use crate::permissions::{self, perm};
use crate::Snowflake;

const TYPING_DURATION: Duration = Duration::from_secs(10);
/// Messages kept for channels that aren't being viewed.
const MESSAGE_CACHE_SIZE: usize = 500;

/// A server rail entry: a lone server or a folder of them.
#[derive(Debug, Clone)]
pub enum RailItem {
    Guild(Snowflake),
    Folder { id: i64, name: Option<String>, color: Option<u32>, guilds: Vec<Snowflake> },
}

/// What a client might react to after an event, beyond redrawing.
#[derive(Debug)]
pub enum Notice {
    /// Carries a copy of the message, since channels without loaded history don't keep it.
    MessageCreated { message: Box<Message>, notify: bool },
    /// The current user's voice state changed (joined, moved, left, muted…).
    OwnVoiceState,
    VoiceServer(VoiceServerUpdate),
    /// Favourite GIFs, stickers or emoji changed (possibly on another device).
    FrecencyChanged,
    /// A DM call started ringing the current user.
    Ring(Snowflake),
    /// A DM call stopped ringing the current user (answered elsewhere, declined, or ended).
    RingStopped(Snowflake),
    /// Raw events for other subsystems.
    Other(String, Box<serde_json::value::RawValue>),
}

#[derive(Default)]
pub struct ChannelMessages {
    /// Oldest first.
    pub list: BTreeMap<Snowflake, Message>,
    /// The oldest message has been loaded.
    pub reached_start: bool,
    /// History has been fetched at least once since the channel's cache was last reset.
    pub loaded: bool,
    /// Showing older history (e.g. after jumping to a linked message) without the latest messages, which are
    /// loaded as the user scrolls down. New messages aren't added meanwhile, to avoid a gap.
    pub has_newer: bool,
}

/// A row of a member list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListRow {
    /// A group header: a hoisted role's id, or "online" / "offline".
    Group(String),
    Member(Snowflake),
}

/// A channel's member list, as far as it has been synced: rows outside the subscribed ranges are `None`.
#[derive(Debug, Default)]
pub struct MemberList {
    pub id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub groups: Vec<MemberListGroup>,
    pub rows: Vec<Option<ListRow>>,
}

impl MemberList {
    pub fn group_count(&self, id: &str) -> u32 {
        self.groups.iter().find(|g| g.id == id).map_or(0, |g| g.count)
    }

    fn apply(&mut self, op: &MemberListOp) {
        let row = |item: &MemberListItem| match item {
            MemberListItem::Group(g) => ListRow::Group(g.id.clone()),
            MemberListItem::Member(m) => ListRow::Member(m.user.as_ref().map(|u| u.id).unwrap_or_default()),
        };
        let rows = &mut self.rows;
        let grow = |rows: &mut Vec<Option<ListRow>>, len: usize| {
            if rows.len() < len {
                rows.resize(len, None);
            }
        };
        match (op.op.as_str(), op.range, op.index, &op.item) {
            ("SYNC", Some([start, _]), ..) => {
                grow(rows, start + op.items.len());
                for (i, item) in op.items.iter().enumerate() {
                    rows[start + i] = Some(row(item));
                }
            }
            ("INSERT", _, Some(i), Some(item)) => {
                grow(rows, i);
                rows.insert(i, Some(row(item)));
            }
            ("UPDATE", _, Some(i), Some(item)) => {
                grow(rows, i + 1);
                rows[i] = Some(row(item));
            }
            ("DELETE", _, Some(i), _) if i < rows.len() => {
                rows.remove(i);
            }
            ("INVALIDATE", Some([start, end]), ..) => {
                for r in rows.iter_mut().take(end + 1).skip(start) {
                    *r = None;
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub struct State {
    pub me: Option<User>,
    /// This gateway session's ID (interactions with apps name it).
    pub session_id: String,
    /// Bumped when anything affecting channel lists or permissions changes, for UI caches.
    pub revision: u64,
    guild_order: Vec<Snowflake>,
    folders: Option<Vec<GuildFolder>>,
    guilds: HashMap<Snowflake, Guild>,
    channels: HashMap<Snowflake, Channel>,
    users: HashMap<Snowflake, User>,
    members: HashMap<Snowflake, HashMap<Snowflake, Member>>,
    messages: HashMap<Snowflake, ChannelMessages>,
    read_states: HashMap<Snowflake, ReadStateEntry>,
    presences: HashMap<Snowflake, PresenceStatus>,
    /// People online only from a phone.
    on_mobile: HashSet<Snowflake>,
    /// What people are doing ("Playing X", custom statuses), for those who are doing something.
    activities: HashMap<Snowflake, String>,
    /// Our custom status, to send back when our status changes (presence updates replace activities).
    own_custom_status: Vec<Activity>,
    /// The member list of the channel being viewed, by guild.
    member_lists: HashMap<Snowflake, MemberList>,
    /// Notification settings by guild; DM settings are under `Snowflake::NONE`.
    guild_settings: HashMap<Snowflake, UserGuildSettings>,
    typing: HashMap<Snowflake, HashMap<Snowflake, Instant>>,
    /// By guild (`Snowflake::NONE` for DM calls), then user.
    voice_states: HashMap<Snowflake, HashMap<Snowflake, VoiceState>>,
    /// Active DM and group DM calls by channel.
    calls: HashMap<Snowflake, CallEvent>,
}

impl State {
    // MARK: Queries

    pub fn guild(&self, id: Snowflake) -> Option<&Guild> {
        self.guilds.get(&id)
    }

    pub fn channel(&self, id: Snowflake) -> Option<&Channel> {
        self.channels.get(&id)
    }

    pub fn user(&self, id: Snowflake) -> Option<&User> {
        self.users.get(&id)
    }

    pub fn member(&self, guild: Snowflake, user: Snowflake) -> Option<&Member> {
        self.members.get(&guild)?.get(&user)
    }

    pub fn presence(&self, user: Snowflake) -> PresenceStatus {
        self.presences.get(&user).copied().unwrap_or_default()
    }

    /// Online only from a phone (shown with a phone instead of a dot).
    pub fn on_mobile(&self, user: Snowflake) -> bool {
        self.on_mobile.contains(&user)
    }

    pub fn activity(&self, user: Snowflake) -> Option<&str> {
        self.activities.get(&user).map(String::as_str)
    }

    pub fn member_list(&self, guild: Snowflake) -> Option<&MemberList> {
        self.member_lists.get(&guild)
    }

    pub fn messages(&self, channel: Snowflake) -> Option<&ChannelMessages> {
        self.messages.get(&channel)
    }

    pub fn message(&self, channel: Snowflake, id: Snowflake) -> Option<&Message> {
        self.messages.get(&channel)?.list.get(&id)
    }

    pub fn message_mut(&mut self, channel: Snowflake, id: Snowflake) -> Option<&mut Message> {
        self.messages.get_mut(&channel)?.list.get_mut(&id)
    }

    /// Servers in rail order: unplaced servers (joined since folders were saved) first, newest first, then
    /// the saved folders.
    pub fn rail(&self) -> Vec<RailItem> {
        let Some(folders) = &self.folders else {
            return self.guild_order.iter().filter(|id| self.guilds.contains_key(id)).map(|&id| RailItem::Guild(id)).collect();
        };
        let placed: HashSet<Snowflake> = folders.iter().flat_map(|f| f.guild_ids.iter().copied()).collect();
        let mut unplaced: Vec<Snowflake> =
            self.guild_order.iter().copied().filter(|id| self.guilds.contains_key(id) && !placed.contains(id)).collect();
        unplaced.sort_by_key(|id| std::cmp::Reverse(self.guilds[id].joined_at.unwrap_or_else(|| id.created_at())));
        let mut out: Vec<RailItem> = unplaced.into_iter().map(RailItem::Guild).collect();
        for f in folders {
            let guilds: Vec<Snowflake> = f.guild_ids.iter().copied().filter(|id| self.guilds.contains_key(id)).collect();
            match (f.id, guilds.len()) {
                (_, 0) => {}
                (Some(id), _) => out.push(RailItem::Folder { id, name: f.name.clone(), color: f.color, guilds }),
                (None, _) => out.extend(guilds.into_iter().map(RailItem::Guild)),
            }
        }
        out
    }

    /// DMs and group DMs, most recently active first.
    pub fn private_channels(&self) -> Vec<&Channel> {
        let mut v: Vec<&Channel> = self.channels.values().filter(|c| c.is_private()).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
        v
    }

    /// The guild's channel sidebar: uncategorized channels first, then each category with its children. Only
    /// viewable channels are included and empty categories are omitted.
    pub fn channel_tree(&self, guild: Snowflake) -> Vec<(Option<&Channel>, Vec<&Channel>)> {
        let visible: Vec<&Channel> = self
            .channels
            .values()
            .filter(|c| c.guild_id == Some(guild) && !c.is_thread() && c.kind != ChannelType::Category)
            .filter(|c| self.permissions_for(c) & perm::VIEW_CHANNEL != 0)
            .collect();
        let mut categories: Vec<&Channel> =
            self.channels.values().filter(|c| c.guild_id == Some(guild) && c.kind == ChannelType::Category).collect();
        categories.sort_by_key(|c| (c.position, c.id));
        let category_ids: HashSet<Snowflake> = categories.iter().map(|c| c.id).collect();

        // Within a group, text-like channels come before voice channels, then by position.
        let children = |parent: Option<Snowflake>| {
            let mut v: Vec<&Channel> = visible
                .iter()
                .copied()
                .filter(|c| match parent {
                    None => c.parent_id.is_none_or(|p| !category_ids.contains(&p)),
                    Some(p) => c.parent_id == Some(p),
                })
                .collect();
            v.sort_by_key(|c| (c.is_voice(), c.position, c.id));
            v
        };

        let mut out = Vec::new();
        let top = children(None);
        if !top.is_empty() {
            out.push((None, top));
        }
        for cat in categories {
            let kids = children(Some(cat.id));
            if !kids.is_empty() {
                out.push((Some(cat), kids));
            }
        }
        out
    }

    /// The current user's permissions in a channel; everything for DMs.
    pub fn permissions(&self, channel: Snowflake) -> u64 {
        self.channels.get(&channel).map_or(0, |c| self.permissions_for(c))
    }

    /// Stores our own member after changing our server profile (the gateway also says, but later).
    pub fn set_my_member(&mut self, guild: Snowflake, mut member: Member) {
        let Some(me) = self.me.clone() else { return };
        member.user.get_or_insert(me.clone());
        self.members.entry(guild).or_default().insert(me.id, member);
        self.revision += 1;
    }

    /// The current user's server-wide permissions (from roles, before any channel's overwrites).
    pub fn guild_permissions(&self, guild_id: Snowflake) -> u64 {
        let (Some(guild), Some(me)) = (self.guilds.get(&guild_id), &self.me) else { return 0 };
        let roles = self.member(guild_id, me.id).map(|m| m.roles.as_slice()).unwrap_or(&[]);
        permissions::compute_base(guild, me.id, roles)
    }

    fn permissions_for(&self, channel: &Channel) -> u64 {
        let Some(guild_id) = channel.guild_id else { return perm::ALL };
        let (Some(guild), Some(me)) = (self.guilds.get(&guild_id), &self.me) else { return 0 };
        // Threads inherit permissions from their parent channel.
        let thread = channel.is_thread().then_some(channel);
        let channel = match channel.parent_id {
            Some(p) if channel.is_thread() => self.channels.get(&p).unwrap_or(channel),
            _ => channel,
        };
        let roles = self.member(guild_id, me.id).map(|m| m.roles.as_slice()).unwrap_or(&[]);
        let mut perms = permissions::compute(guild, channel, me.id, roles);
        // In a thread, sending is governed by Send Messages in Threads, and a locked thread only by moderators.
        if let Some(t) = thread {
            let locked = t.thread_metadata.as_ref().is_some_and(|m| m.locked);
            let can_send = perms & perm::SEND_MESSAGES_IN_THREADS != 0 && (!locked || perms & perm::MANAGE_THREADS != 0);
            perms = if can_send { perms | perm::SEND_MESSAGES } else { perms & !perm::SEND_MESSAGES };
        }
        perms
    }

    pub fn is_unread(&self, channel: Snowflake) -> bool {
        let Some(last) = self.channels.get(&channel).and_then(|c| c.last_message_id) else { return false };
        // Without a read state the channel has never been opened; treat it as read to avoid a sea of badges.
        self.read_states.get(&channel).and_then(|r| r.last_message_id).is_some_and(|read| last > read)
    }

    pub fn mention_count(&self, channel: Snowflake) -> u32 {
        self.read_states.get(&channel).map_or(0, |r| r.mention_count)
    }

    pub fn guild_unread(&self, guild: Snowflake) -> bool {
        self.channels.values().any(|c| {
            c.guild_id == Some(guild)
                && c.is_text_based()
                && !c.is_thread()
                && self.is_unread(c.id)
                && !self.is_channel_muted(c)
                && self.permissions_for(c) & perm::VIEW_CHANNEL != 0
        })
    }

    pub fn guild_mentions(&self, guild: Snowflake) -> u32 {
        self.channels.values().filter(|c| c.guild_id == Some(guild)).map(|c| self.mention_count(c.id)).sum()
    }

    /// Unread flag and mention count for every guild, in one pass over the channels.
    pub fn guild_badges(&self) -> HashMap<Snowflake, (bool, u32)> {
        let mut out: HashMap<Snowflake, (bool, u32)> = HashMap::new();
        for c in self.channels.values() {
            let Some(g) = c.guild_id else { continue };
            let mentions = self.mention_count(c.id);
            let unread = c.is_text_based() && !c.is_thread() && self.is_unread(c.id) && !self.is_channel_muted(c);
            if mentions == 0 && !unread {
                continue;
            }
            if unread && self.permissions_for(c) & perm::VIEW_CHANNEL == 0 {
                continue;
            }
            let e = out.entry(g).or_default();
            e.0 |= unread;
            e.1 += mentions;
        }
        out
    }

    /// Total mentions across DMs, for the Home button.
    pub fn dm_mentions(&self) -> u32 {
        self.channels.values().filter(|c| c.is_private()).map(|c| self.mention_count(c.id)).sum()
    }

    pub fn is_guild_muted(&self, guild: Snowflake) -> bool {
        self.guild_settings.get(&guild).is_some_and(|s| s.is_muted())
    }

    pub fn is_channel_muted(&self, channel: &Channel) -> bool {
        let settings = self.guild_settings.get(&channel.guild_id.unwrap_or_default());
        self.channel_and_parents(channel)
            .any(|c| settings.is_some_and(|s| s.channel_overrides.iter().any(|o| o.channel_id == c.id && o.is_muted())))
    }

    fn channel_and_parents<'a>(&'a self, channel: &'a Channel) -> impl Iterator<Item = &'a Channel> + 'a {
        std::iter::successors(Some(channel), |c| c.parent_id.and_then(|p| self.channels.get(&p)))
    }

    /// Display name in context: server nickname, then global name, then username.
    pub fn display_name(&self, user: Snowflake, guild: Option<Snowflake>) -> String {
        if let Some(nick) = guild.and_then(|g| self.member(g, user)).and_then(|m| m.nick.clone()).filter(|n| !n.is_empty()) {
            return nick;
        }
        self.users.get(&user).map_or_else(|| "Unknown User".to_owned(), |u| u.display_name().to_owned())
    }

    /// Someone's avatar as shown in a server: their server profile's, if they set one, else their own.
    pub fn avatar_url(&self, user: &User, guild: Option<Snowflake>, size: u32) -> String {
        let server = guild.and_then(|g| Some((g, self.member(g, user.id)?.avatar.as_deref().filter(|h| !h.is_empty())?)));
        match server {
            Some((g, hash)) => crate::cdn::member_avatar(g, user.id, hash, size),
            None => crate::cdn::avatar(user, size),
        }
    }

    /// The colour of the member's highest coloured role.
    pub fn role_color(&self, guild: Snowflake, user: Snowflake) -> Option<u32> {
        let g = self.guilds.get(&guild)?;
        let m = self.member(guild, user)?;
        g.roles.iter().filter(|r| r.color != 0 && m.roles.contains(&r.id)).max_by_key(|r| r.position).map(|r| r.color)
    }

    pub fn typing_users(&self, channel: Snowflake) -> Vec<Snowflake> {
        let now = Instant::now();
        self.typing.get(&channel).map_or_else(Vec::new, |t| t.iter().filter(|&(_, &until)| until > now).map(|(&u, _)| u).collect())
    }

    /// Users in a voice channel.
    pub fn voice_members(&self, channel: &Channel) -> Vec<&VoiceState> {
        let mut v: Vec<&VoiceState> = self
            .voice_states
            .get(&channel.guild_id.unwrap_or_default())
            .map(|m| m.values().filter(|s| s.channel_id == Some(channel.id)).collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.user_id);
        v
    }

    /// People matching a name prefix, for mention suggestions: a guild's cached members, or a DM's participants.
    /// Prefix matches on any name rank first, then substring matches; at most `limit`.
    pub fn search_people<'a>(&'a self, channel: &'a Channel, query: &str, limit: usize) -> Vec<&'a User> {
        let q = query.to_lowercase();
        let candidates: Vec<(&User, Option<&str>)> = match channel.guild_id {
            // Only members who can see the channel (threads use their parent's permissions).
            Some(g) => {
                let guild = self.guilds.get(&g);
                let perm_channel = match channel.parent_id {
                    Some(p) if channel.is_thread() => self.channels.get(&p).unwrap_or(channel),
                    _ => channel,
                };
                self.members
                    .get(&g)
                    .map(|m| {
                        m.iter()
                            .filter(|(id, mem)| {
                                guild.is_some_and(|gd| permissions::compute(gd, perm_channel, **id, &mem.roles) & perm::VIEW_CHANNEL != 0)
                            })
                            .filter_map(|(id, mem)| Some((self.users.get(id)?, mem.nick.as_deref())))
                            .collect()
                    })
                    .unwrap_or_default()
            }
            None => channel.recipients.iter().chain(self.me.iter()).map(|u| (u, None)).collect(),
        };
        let mut scored: Vec<(u8, &User)> = candidates
            .into_iter()
            .filter(|(u, _)| !u.bot || !q.is_empty())
            .filter_map(|(u, nick)| {
                let names = [Some(u.username.as_str()), u.global_name.as_deref(), nick];
                let lower: Vec<String> = names.iter().flatten().map(|n| n.to_lowercase()).collect();
                let score = if lower.iter().any(|n| n.starts_with(&q)) {
                    0
                } else if lower.iter().any(|n| n.contains(&q)) {
                    1
                } else {
                    return None;
                };
                Some((score, u))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.username.cmp(&b.1.username)));
        scored.dedup_by_key(|(_, u)| u.id);
        scored.into_iter().take(limit).map(|(_, u)| u).collect()
    }

    /// The active call in a DM or group DM, if any.
    pub fn call(&self, channel: Snowflake) -> Option<&CallEvent> {
        self.calls.get(&channel)
    }

    /// DM calls currently ringing the current user.
    pub fn ringing_me(&self) -> Vec<Snowflake> {
        let Some(me) = self.me.as_ref().map(|m| m.id) else { return Vec::new() };
        self.calls.values().filter(|c| c.ringing.contains(&me)).map(|c| c.channel_id).collect()
    }

    pub fn my_voice_state(&self) -> Option<&VoiceState> {
        let me = self.me.as_ref()?.id;
        self.voice_states.values().find_map(|m| m.get(&me)).filter(|s| s.channel_id.is_some())
    }

    pub fn voice_state(&self, guild: Option<Snowflake>, user: Snowflake) -> Option<&VoiceState> {
        self.voice_states.get(&guild.unwrap_or_default())?.get(&user)
    }

    /// Whether a new message should raise a desktop notification under the user's Discord settings:
    /// channel (then category, then server) level and mutes, @everyone/role suppression and @silent. A mute
    /// still lets direct @mentions through, as in Discord.
    pub fn should_notify(&self, message: &Message) -> bool {
        let Some(me) = &self.me else { return false };
        if message.author.id == me.id || message.flags & message_flags::SUPPRESS_NOTIFICATIONS != 0 {
            return false;
        }
        let Some(channel) = self.channels.get(&message.channel_id) else { return false };
        let settings = self.guild_settings.get(&channel.guild_id.unwrap_or_default());
        let overrides: Vec<&ChannelOverride> = self
            .channel_and_parents(channel)
            .filter_map(|c| settings?.channel_overrides.iter().find(|o| o.channel_id == c.id))
            .collect();
        let direct = message.mentions.iter().any(|u| u.id == me.id);
        if overrides.iter().any(|o| o.is_muted()) || settings.is_some_and(|s| s.is_muted()) {
            return direct;
        }
        if channel.is_private() {
            return true;
        }
        const INHERIT: u8 = 3;
        let mut level = overrides.iter().map(|o| o.message_notifications).find(|&l| l != INHERIT).unwrap_or(INHERIT);
        if level == INHERIT {
            level = settings.map_or(INHERIT, |s| s.message_notifications);
        }
        if level == INHERIT {
            level = channel.guild_id.and_then(|g| self.guilds.get(&g)).map_or(0, |g| g.default_message_notifications);
        }
        match level {
            0 => true,
            1 => {
                direct
                    || (message.mention_everyone && !settings.is_some_and(|s| s.suppress_everyone))
                    || (!settings.is_some_and(|s| s.suppress_roles)
                        && channel.guild_id.and_then(|g| self.member(g, me.id)).is_some_and(|m| {
                            message.mention_roles.iter().any(|r| m.roles.contains(r))
                        }))
            }
            _ => false,
        }
    }

    pub fn mentions_me(&self, message: &Message) -> bool {
        let Some(me) = &self.me else { return false };
        if message.mentions.iter().any(|u| u.id == me.id) || message.mention_everyone {
            return true;
        }
        message
            .guild_id
            .or_else(|| self.channels.get(&message.channel_id).and_then(|c| c.guild_id))
            .and_then(|g| self.member(g, me.id))
            .is_some_and(|m| message.mention_roles.iter().any(|r| m.roles.contains(r)))
    }

    // MARK: Mutations from REST and the UI

    /// Adds a page of history (newest first, as the API returns it).
    pub fn add_history(&mut self, channel: Snowflake, page: Vec<Message>, requested: usize) {
        let reached_start = page.len() < requested;
        for m in &page {
            self.cache_author(m);
        }
        let entry = self.messages.entry(channel).or_default();
        entry.loaded = true;
        entry.reached_start |= reached_start;
        for m in page {
            entry.list.insert(m.id, m);
        }
    }

    /// Replaces a channel's history with a page centred on a message (oldest first or newest first).
    pub fn set_around(&mut self, channel: Snowflake, page: Vec<Message>) {
        let latest = self.channels.get(&channel).and_then(|c| c.last_message_id);
        for m in &page {
            self.cache_author(m);
        }
        let has_newer = latest.is_some_and(|l| !page.iter().any(|m| m.id >= l));
        let list = page.into_iter().map(|m| (m.id, m)).collect();
        self.messages.insert(channel, ChannelMessages { list, reached_start: false, loaded: true, has_newer });
    }

    /// Adds a page of newer messages (fetched with `after`, which the API returns newest first).
    pub fn add_newer(&mut self, channel: Snowflake, page: Vec<Message>, requested: usize) {
        for m in &page {
            self.cache_author(m);
        }
        let Some(entry) = self.messages.get_mut(&channel) else { return };
        if page.len() < requested {
            entry.has_newer = false;
        }
        for m in page {
            entry.list.insert(m.id, m);
        }
    }

    /// Drops a channel's cached history so the next view refetches it, e.g. after a reconnect.
    pub fn reset_history(&mut self, channel: Snowflake) {
        self.messages.remove(&channel);
    }

    /// Inserts an optimistic copy of a message being sent, keyed by its nonce.
    pub fn add_pending(&mut self, message: Message) {
        self.messages.entry(message.channel_id).or_default().list.insert(message.id, message);
    }

    pub fn remove_message(&mut self, channel: Snowflake, id: Snowflake) {
        if let Some(m) = self.messages.get_mut(&channel) {
            m.list.remove(&id);
        }
    }

    pub fn mark_read(&mut self, channel: Snowflake, message: Snowflake) {
        let entry = self.read_states.entry(channel).or_insert_with(|| ReadStateEntry { id: channel, ..Default::default() });
        entry.last_message_id = Some(entry.last_message_id.map_or(message, |l| l.max(message)));
        entry.mention_count = 0;
    }

    pub fn add_channel(&mut self, mut channel: Channel) {
        for r in &channel.recipients {
            self.users.insert(r.id, r.clone());
        }
        if let Some(old) = self.channels.get(&channel.id) {
            // Thread payloads often leave out our membership and the latest message.
            if channel.member.is_none() {
                channel.member = old.member.clone();
            }
            if channel.last_message_id.is_none() || old.last_message_id > channel.last_message_id {
                channel.last_message_id = old.last_message_id;
            }
        }
        if channel.is_thread() {
            self.link_thread(&channel);
        }
        self.channels.insert(channel.id, channel);
        self.revision += 1;
    }

    /// Threads started from a message share its ID; keep that message's thread chip current.
    fn link_thread(&mut self, thread: &Channel) {
        let Some(parent) = thread.parent_id else { return };
        if let Some(m) = self.message_mut(parent, thread.id) {
            m.thread = Some(Box::new(thread.clone()));
        }
    }

    /// Active threads under a channel, most recently active first.
    pub fn threads(&self, parent: Snowflake) -> Vec<&Channel> {
        let mut v: Vec<&Channel> =
            self.channels.values().filter(|c| c.is_thread() && c.parent_id == Some(parent) && !c.is_archived()).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_message_id.unwrap_or(c.id)));
        v
    }

    /// Active threads we've joined under a channel, as the sidebar lists them.
    pub fn joined_threads(&self, parent: Snowflake) -> Vec<&Channel> {
        let mut v: Vec<&Channel> = self.threads(parent).into_iter().filter(|c| c.member.is_some()).collect();
        v.sort_by_key(|c| c.id);
        v
    }

    /// Records joining or leaving a thread before Discord confirms it.
    pub fn set_thread_joined(&mut self, thread: Snowflake, joined: bool) {
        let me = self.me.as_ref().map(|m| m.id);
        if let Some(c) = self.channels.get_mut(&thread) {
            c.member = joined.then(|| ThreadMember { id: Some(thread), user_id: me, join_timestamp: None });
            self.revision += 1;
        }
    }

    pub fn set_folders(&mut self, folders: Vec<GuildFolder>) {
        self.folders = Some(folders);
    }

    // MARK: Gateway events

    pub fn apply(&mut self, event: Event) -> Vec<Notice> {
        let mut notices = Vec::new();
        match event {
            Event::Ready(r) => self.on_ready(*r),
            Event::ReadySupplemental(s) => {
                for p in s.merged_presences.friends {
                    self.set_presence(p.user_id(), &p.status, &p.activities, &p.client_status);
                }
                for g in s.guilds {
                    let states = self.voice_states.entry(g.id).or_default();
                    for mut vs in g.voice_states {
                        vs.guild_id = Some(g.id);
                        states.insert(vs.user_id, vs);
                    }
                }
            }
            Event::Resumed => {}
            Event::GuildCreate(g) => self.add_guild(*g),
            Event::GuildUpdate(u) => {
                if let Some(g) = self.guilds.get_mut(&u.id) {
                    if let Some(n) = u.name {
                        g.name = n;
                    }
                    g.icon = u.icon.or(g.icon.take());
                    if let Some(o) = u.owner_id {
                        g.owner_id = o;
                    }
                    if let Some(r) = u.roles {
                        g.roles = r;
                    }
                    if let Some(e) = u.emojis {
                        g.emojis = e;
                    }
                    if let Some(d) = u.default_message_notifications {
                        g.default_message_notifications = d;
                    }
                    self.revision += 1;
                }
            }
            Event::GuildDelete(d) => {
                if d.unavailable == Some(true) {
                    if let Some(g) = self.guilds.get_mut(&d.id) {
                        g.unavailable = true;
                    }
                } else {
                    self.guilds.remove(&d.id);
                    self.guild_order.retain(|&id| id != d.id);
                    self.members.remove(&d.id);
                    self.member_lists.remove(&d.id);
                    self.voice_states.remove(&d.id);
                    let gone: Vec<Snowflake> =
                        self.channels.values().filter(|c| c.guild_id == Some(d.id)).map(|c| c.id).collect();
                    for id in gone {
                        self.channels.remove(&id);
                        self.messages.remove(&id);
                        self.read_states.remove(&id);
                    }
                }
                self.revision += 1;
            }
            Event::GuildRoleUpsert(e) | Event::GuildRoleDelete(e) if !self.guilds.contains_key(&e.guild_id) => {}
            Event::GuildRoleUpsert(e) => {
                if let (Some(g), Some(role)) = (self.guilds.get_mut(&e.guild_id), e.role) {
                    g.roles.retain(|r| r.id != role.id);
                    g.roles.push(role);
                    self.revision += 1;
                }
            }
            Event::GuildRoleDelete(e) => {
                if let (Some(g), Some(id)) = (self.guilds.get_mut(&e.guild_id), e.role_id) {
                    g.roles.retain(|r| r.id != id);
                    self.revision += 1;
                }
            }
            Event::GuildMemberUpdate(e) => {
                let is_me = self.me.as_ref().is_some_and(|m| m.id == e.user.id);
                self.users.insert(e.user.id, e.user.clone());
                let m = self.members.entry(e.guild_id).or_default().entry(e.user.id).or_default();
                m.user = Some(e.user);
                m.nick = e.nick;
                m.avatar = e.avatar;
                m.roles = e.roles;
                m.joined_at = e.joined_at.or(m.joined_at);
                m.communication_disabled_until = e.communication_disabled_until;
                if is_me {
                    self.revision += 1;
                }
            }
            Event::GuildEmojisUpdate(e) => {
                if let Some(g) = self.guilds.get_mut(&e.guild_id) {
                    g.emojis = e.emojis;
                }
            }
            Event::GuildStickersUpdate(e) => {
                if let Some(g) = self.guilds.get_mut(&e.guild_id) {
                    g.stickers = e.stickers;
                }
            }
            Event::ChannelCreate(c) => self.add_channel(*c),
            Event::ChannelUpdate(mut c) => {
                if let Some(old) = self.channels.get(&c.id) {
                    c.guild_id = c.guild_id.or(old.guild_id);
                    // The update may omit the last message ID; don't regress unread state.
                    if c.last_message_id.is_none() || old.last_message_id > c.last_message_id {
                        c.last_message_id = old.last_message_id;
                    }
                    if c.recipients.is_empty() {
                        c.recipients = old.recipients.clone();
                    }
                }
                self.add_channel(*c);
            }
            Event::ThreadListSync(sync) => {
                let joined: HashSet<Snowflake> = sync.members.iter().filter_map(|m| m.id).collect();
                for mut t in sync.threads {
                    t.guild_id = t.guild_id.or(Some(sync.guild_id));
                    if joined.contains(&t.id) {
                        t.member = sync.members.iter().find(|m| m.id == Some(t.id)).cloned();
                    }
                    self.add_channel(t);
                }
            }
            Event::ThreadMemberUpdate(m) => {
                if let Some(id) = m.id {
                    if let Some(c) = self.channels.get_mut(&id) {
                        c.member = Some(m);
                        self.revision += 1;
                    }
                }
            }
            Event::ThreadMembersUpdate(u) => {
                let me = self.me.as_ref().map(|m| m.id);
                if let Some(c) = self.channels.get_mut(&u.id) {
                    c.member_count = u.member_count.or(c.member_count);
                    if let Some(m) = u.added_members.into_iter().find(|m| m.user_id.is_some() && m.user_id == me) {
                        c.member = Some(m);
                    }
                    if me.is_some_and(|me| u.removed_member_ids.contains(&me)) {
                        c.member = None;
                    }
                    self.revision += 1;
                }
            }
            Event::ChannelDelete(c) => {
                self.channels.remove(&c.id);
                self.messages.remove(&c.id);
                self.read_states.remove(&c.id);
                self.typing.remove(&c.id);
                self.revision += 1;
            }
            Event::MessageCreate(m) => notices.push(self.on_message_create(*m)),
            Event::MessageUpdate(u) => self.on_message_update(*u),
            Event::MessageDelete(d) => self.remove_message(d.channel_id, d.id),
            Event::MessageDeleteBulk(d) => {
                for id in d.ids {
                    self.remove_message(d.channel_id, id);
                }
            }
            Event::MessageAck(a) => {
                let e = self.read_states.entry(a.channel_id).or_insert_with(|| ReadStateEntry { id: a.channel_id, ..Default::default() });
                e.last_message_id = Some(a.message_id);
                e.mention_count = a.mention_count.unwrap_or(0);
            }
            Event::ReactionAdd(e) => self.on_reaction(e, true),
            Event::ReactionRemove(e) => self.on_reaction(e, false),
            Event::ReactionRemoveAll(e) => {
                if let Some(m) = self.message_mut(e.channel_id, e.message_id) {
                    match e.emoji {
                        Some(emoji) => m.reactions.retain(|r| !r.emoji.same_as(&emoji)),
                        None => m.reactions.clear(),
                    }
                }
            }
            Event::TypingStart(t) => {
                if self.me.as_ref().is_some_and(|m| m.id == t.user_id) {
                    return notices;
                }
                if let (Some(g), Some(member)) = (t.guild_id, t.member) {
                    if let Some(u) = &member.user {
                        self.users.insert(u.id, u.clone());
                    }
                    self.members.entry(g).or_default().insert(t.user_id, member);
                }
                // The local clock rather than the event timestamp, so clock skew can't hide indicators.
                self.typing.entry(t.channel_id).or_default().insert(t.user_id, Instant::now() + TYPING_DURATION);
            }
            Event::PresenceUpdate(p) => self.set_presence(p.user.id, &p.status, &p.activities, &p.client_status),
            Event::SessionsReplace(sessions) => self.set_own_presence(&sessions),
            Event::MemberListUpdate(u) => self.on_member_list(*u),
            Event::UserUpdate(u) => {
                if self.me.as_ref().is_some_and(|m| m.id == u.id) {
                    self.users.insert(u.id, (*u).clone());
                    self.me = Some(*u);
                }
            }
            Event::UserSettingsProtoUpdate(u) => {
                if u.settings.kind == 1 {
                    if let Some(f) = crate::proto::read_guild_folders(&u.settings.proto) {
                        self.folders = Some(f);
                    }
                } else if u.settings.kind == 2 {
                    notices.push(Notice::FrecencyChanged);
                }
            }
            Event::UserGuildSettingsUpdate(s) => {
                self.guild_settings.insert(s.guild_id.unwrap_or_default(), *s);
            }
            Event::VoiceStateUpdate(vs) => {
                let is_me = self.me.as_ref().is_some_and(|m| m.id == vs.user_id);
                if let (Some(g), Some(member)) = (vs.guild_id, &vs.member) {
                    if let Some(u) = &member.user {
                        self.users.insert(u.id, u.clone());
                    }
                    self.members.entry(g).or_default().insert(vs.user_id, member.clone());
                }
                let states = self.voice_states.entry(vs.guild_id.unwrap_or_default()).or_default();
                if vs.channel_id.is_some() {
                    states.insert(vs.user_id, *vs);
                } else {
                    states.remove(&vs.user_id);
                }
                if is_me {
                    notices.push(Notice::OwnVoiceState);
                }
            }
            Event::VoiceServerUpdate(v) => notices.push(Notice::VoiceServer(v)),
            Event::CallUpdate(call) => {
                let me = self.me.as_ref().map(|m| m.id);
                let was = self.calls.get(&call.channel_id).is_some_and(|c| me.is_some_and(|m| c.ringing.contains(&m)));
                let now = me.is_some_and(|m| call.ringing.contains(&m));
                let states = self.voice_states.entry(Snowflake::NONE).or_default();
                for vs in &call.voice_states {
                    if vs.channel_id.is_some() {
                        states.insert(vs.user_id, vs.clone());
                    }
                }
                let channel = call.channel_id;
                self.calls.insert(channel, *call);
                match (was, now) {
                    (false, true) => notices.push(Notice::Ring(channel)),
                    (true, false) => notices.push(Notice::RingStopped(channel)),
                    _ => {}
                }
            }
            Event::GuildMembersChunk(chunk) => {
                let members = self.members.entry(chunk.guild_id).or_default();
                for m in chunk.members {
                    let Some(uid) = m.user_id() else { continue };
                    if let Some(u) = &m.user {
                        self.users.insert(u.id, u.clone());
                    }
                    members.insert(uid, m);
                }
            }
            Event::CallDelete(channel) => {
                let me = self.me.as_ref().map(|m| m.id);
                if self.calls.remove(&channel).is_some_and(|c| me.is_some_and(|m| c.ringing.contains(&m))) {
                    notices.push(Notice::RingStopped(channel));
                }
                if let Some(states) = self.voice_states.get_mut(&Snowflake::NONE) {
                    states.retain(|_, s| s.channel_id != Some(channel));
                }
            }
            Event::Other(name, data) => notices.push(Notice::Other(name, data)),
        }
        notices
    }

    fn on_ready(&mut self, r: Ready) {
        *self = State { revision: self.revision + 1, session_id: r.session_id.clone(), ..State::default() };
        self.users.insert(r.user.id, r.user.clone());
        self.me = Some(r.user);
        for u in r.users {
            self.users.insert(u.id, u);
        }
        for (i, mut g) in r.guilds.into_iter().enumerate() {
            if let Some(members) = r.merged_members.get(i) {
                g.members.extend(members.iter().cloned());
            }
            self.add_guild(g);
        }
        for mut c in r.private_channels {
            if c.recipients.is_empty() {
                if let Some(ids) = &c.recipient_ids {
                    c.recipients = ids.iter().filter_map(|id| self.users.get(id).cloned()).collect();
                }
            }
            for u in &c.recipients {
                self.users.insert(u.id, u.clone());
            }
            self.channels.insert(c.id, c);
        }
        for e in r.read_state.into_iter().filter(|e| e.read_state_type == 0) {
            self.read_states.insert(e.id, e);
        }
        for s in r.user_guild_settings {
            self.guild_settings.insert(s.guild_id.unwrap_or_default(), s);
        }
        self.folders = r
            .user_settings_proto
            .as_deref()
            .filter(|p| !p.is_empty())
            .and_then(crate::proto::read_guild_folders)
            .or_else(|| r.user_settings.and_then(|s| s.guild_folders));
        self.set_own_presence(&r.sessions);
    }

    fn set_presence(&mut self, user: Snowflake, status: &str, activities: &[Activity], devices: &ClientStatus) {
        self.presences.insert(user, PresenceStatus::parse(status));
        if devices.mobile_only(status) {
            self.on_mobile.insert(user);
        } else {
            self.on_mobile.remove(&user);
        }
        match activity_summary(activities) {
            Some(a) => self.activities.insert(user, a),
            None => self.activities.remove(&user),
        };
    }

    /// Our own status comes from our sessions rather than PRESENCE_UPDATE: the combined "all" session if
    /// there is one. Invisible reads as offline.
    fn set_own_presence(&mut self, sessions: &[SessionInfo]) {
        let Some(me) = self.me.as_ref().map(|m| m.id) else { return };
        if let Some(s) = sessions.iter().find(|s| s.session_id == "all").or(sessions.first()) {
            self.set_presence(me, &s.status, &s.activities, &ClientStatus::default());
            self.own_custom_status = s.activities.iter().filter(|a| a.kind == Activity::CUSTOM).cloned().collect();
        }
    }

    /// Shows a status we just picked before Discord confirms it with SESSIONS_REPLACE.
    pub fn set_own_status(&mut self, status: PresenceStatus) {
        if let Some(me) = self.me.as_ref().map(|m| m.id) {
            self.presences.insert(me, status);
        }
    }

    pub fn own_custom_status(&self) -> &[Activity] {
        &self.own_custom_status
    }

    /// Our status: `Offline` when invisible.
    pub fn own_status(&self) -> PresenceStatus {
        self.me.as_ref().map_or(PresenceStatus::Offline, |m| self.presence(m.id))
    }

    fn on_member_list(&mut self, u: MemberListUpdate) {
        // Members (and their presences) are worth keeping whichever list they came in.
        let items = u.ops.iter().flat_map(|op| op.items.iter().chain(&op.item));
        let members: Vec<&ListMember> =
            items.filter_map(|i| if let MemberListItem::Member(m) = i { Some(&**m) } else { None }).collect();
        let mut presences = Vec::new();
        let cache = self.members.entry(u.guild_id).or_default();
        for m in members {
            let Some(user) = &m.user else { continue };
            self.users.insert(user.id, user.clone());
            cache.insert(
                user.id,
                Member {
                    user: Some(user.clone()),
                    user_id: None,
                    nick: m.nick.clone(),
                    avatar: m.avatar.clone(),
                    roles: m.roles.clone(),
                    joined_at: m.joined_at,
                    communication_disabled_until: m.communication_disabled_until,
                },
            );
            if let Some(p) = &m.presence {
                presences.push((user.id, p.status.clone(), p.activities.clone(), p.client_status.clone()));
            }
        }
        let me = self.me.as_ref().map(|m| m.id);
        for (user, status, activities, devices) in presences.into_iter().filter(|(user, ..)| Some(*user) != me) {
            self.set_presence(user, &status, &activities, &devices);
        }

        // Only one channel per guild is subscribed. After switching channels, the old list's updates are
        // ignored until the new one syncs.
        let list = self.member_lists.entry(u.guild_id).or_default();
        if list.id != u.id {
            if !list.id.is_empty() && !u.ops.iter().any(|o| o.op == "SYNC") {
                return;
            }
            *list = MemberList { id: u.id, ..Default::default() };
        }
        for op in &u.ops {
            list.apply(op);
        }
        list.member_count = u.member_count;
        list.online_count = u.online_count;
        list.groups = u.groups;
        // Each non-empty group is a header row and its members.
        let len = list.groups.iter().filter(|g| g.count > 0).map(|g| g.count as usize + 1).sum();
        list.rows.resize(len, None);
    }

    fn add_guild(&mut self, mut g: Guild) {
        g.normalize();
        if !self.guild_order.contains(&g.id) {
            self.guild_order.push(g.id);
        }
        if !g.unavailable {
            for c in g.channels.drain(..).chain(g.threads.drain(..)) {
                self.channels.insert(c.id, c);
            }
            let members = self.members.entry(g.id).or_default();
            for m in g.members.drain(..) {
                let Some(uid) = m.user_id() else { continue };
                if let Some(u) = &m.user {
                    self.users.insert(u.id, u.clone());
                }
                members.insert(uid, m);
            }
            let states = self.voice_states.entry(g.id).or_default();
            for mut vs in g.voice_states.drain(..) {
                vs.guild_id = Some(g.id);
                states.insert(vs.user_id, vs);
            }
        }
        self.guilds.insert(g.id, g);
        self.revision += 1;
    }

    /// Remembers a message's author (and their server profile), e.g. for messages shown outside a channel.
    pub fn cache_author(&mut self, m: &Message) {
        if m.author.id.is_none() {
            return;
        }
        self.users.insert(m.author.id, m.author.clone());
        // People it mentions, so the mentions show names (without replacing fresher copies).
        for u in &m.mentions {
            self.users.entry(u.id).or_insert_with(|| u.clone());
        }
        if let (Some(g), Some(member)) = (m.guild_id, &m.member) {
            let mut member = member.clone();
            member.user.get_or_insert_with(|| m.author.clone());
            self.members.entry(g).or_default().insert(m.author.id, member);
        }
    }

    fn on_message_create(&mut self, mut m: Message) -> Notice {
        self.cache_author(&m);
        if m.guild_id.is_none() {
            m.guild_id = self.channels.get(&m.channel_id).and_then(|c| c.guild_id);
        }
        let notify = self.should_notify(&m);
        let mentions_me = self.mentions_me(&m) || self.channels.get(&m.channel_id).is_some_and(|c| c.is_private());
        let (channel_id, message_id) = (m.channel_id, m.id);
        let copy = Box::new(m.clone());

        let mut thread = None;
        if let Some(c) = self.channels.get_mut(&channel_id) {
            c.last_message_id = Some(message_id);
            if c.is_thread() {
                c.message_count = Some(c.message_count.unwrap_or(0) + 1);
                thread = Some(c.clone());
            }
        }
        if let Some(t) = thread {
            self.link_thread(&t);
        }
        if let Some(t) = self.typing.get_mut(&channel_id) {
            t.remove(&m.author.id);
        }
        if let Some(me) = &self.me {
            let me = me.id;
            let entry = self.read_states.entry(channel_id).or_insert_with(|| ReadStateEntry { id: channel_id, ..Default::default() });
            if m.author.id == me {
                entry.last_message_id = Some(message_id);
                entry.mention_count = 0;
            } else if mentions_me {
                entry.mention_count += 1;
            }
        }
        if let Some(list) = self.messages.get_mut(&channel_id).filter(|l| !l.has_newer) {
            // Replace the optimistic copy of a message we sent.
            if let Some(nonce) = m.nonce_str().and_then(|n| n.parse::<u64>().ok()) {
                list.list.remove(&Snowflake(nonce));
            }
            list.list.insert(message_id, m);
            while list.list.len() > MESSAGE_CACHE_SIZE {
                list.list.pop_first();
                list.reached_start = false;
            }
        }
        Notice::MessageCreated { message: copy, notify }
    }

    fn on_message_update(&mut self, u: MessageUpdate) {
        let Some(m) = self.message_mut(u.channel_id, u.id) else { return };
        if let Some(a) = u.author {
            m.author = a;
        }
        if let Some(c) = u.content {
            m.content = c;
        }
        if u.edited_timestamp.is_some() {
            m.edited_timestamp = u.edited_timestamp;
        }
        if let Some(v) = u.mentions {
            m.mentions = v;
        }
        if let Some(v) = u.mention_roles {
            m.mention_roles = v;
        }
        if let Some(v) = u.mention_everyone {
            m.mention_everyone = v;
        }
        if let Some(v) = u.attachments {
            m.attachments = v;
        }
        if let Some(v) = u.embeds {
            m.embeds = v;
        }
        if let Some(v) = u.components {
            m.components = v;
        }
        if let Some(v) = u.flags {
            m.flags = v;
        }
        if let Some(v) = u.pinned {
            m.pinned = v;
        }
    }

    fn on_reaction(&mut self, e: ReactionEvent, added: bool) {
        let is_me = self.me.as_ref().is_some_and(|m| m.id == e.user_id);
        let Some(m) = self.message_mut(e.channel_id, e.message_id) else { return };
        let idx = m.reactions.iter().position(|r| r.emoji.same_as(&e.emoji));
        if added {
            let r = match idx {
                Some(i) => &mut m.reactions[i],
                None => {
                    m.reactions.push(Reaction { emoji: e.emoji, count: 0, me: false });
                    m.reactions.last_mut().unwrap()
                }
            };
            // Our own reaction may already be counted optimistically.
            if !(is_me && r.me) {
                r.count += 1;
            }
            r.me |= is_me;
        } else if let Some(i) = idx {
            let r = &mut m.reactions[i];
            if !(is_me && !r.me) {
                r.count = r.count.saturating_sub(1);
            }
            if is_me {
                r.me = false;
            }
            if r.count == 0 {
                m.reactions.remove(i);
            }
        }
    }

    /// Applies our own reaction change immediately, before the gateway confirms it.
    pub fn optimistic_reaction(&mut self, channel: Snowflake, message: Snowflake, emoji: &Emoji, add: bool) {
        let Some(m) = self.message_mut(channel, message) else { return };
        match m.reactions.iter().position(|r| r.emoji.same_as(emoji)) {
            Some(i) => {
                let r = &mut m.reactions[i];
                if add && !r.me {
                    r.me = true;
                    r.count += 1;
                } else if !add && r.me {
                    r.me = false;
                    r.count = r.count.saturating_sub(1);
                    if r.count == 0 {
                        m.reactions.remove(i);
                    }
                }
            }
            None if add => m.reactions.push(Reaction { emoji: emoji.clone(), count: 1, me: true }),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::value::RawValue;

    fn ev(name: &str, json: &str) -> Event {
        Event::parse(name, RawValue::from_string(json.to_owned()).unwrap()).unwrap()
    }

    fn ready_state() -> State {
        let mut s = State::default();
        s.apply(ev(
            "READY",
            r#"{"session_id":"x","user":{"id":"1","username":"me"},
               "guilds":[{"id":"100","name":"G","owner_id":"9","roles":[{"id":"100","permissions":"3072"}],
                 "channels":[{"id":"200","type":0,"name":"general","position":1},
                             {"id":"201","type":0,"name":"secret","position":0,
                              "permission_overwrites":[{"id":"100","type":0,"allow":"0","deny":"1024"}]},
                             {"id":"202","type":2,"name":"Voice","position":0}],
                 "voice_states":[{"user_id":"5","channel_id":"202","session_id":"s"}]}],
               "private_channels":[{"id":"300","type":1,"recipients":[{"id":"2","username":"friend"}],"last_message_id":"400"}],
               "read_state":[{"id":"300","last_message_id":"399"}]}"#,
        ));
        s
    }

    fn list_member(id: u64, status: &str) -> String {
        format!(r#"{{"member":{{"user":{{"id":"{id}","username":"u{id}"}},"roles":[],"presence":{{"user":{{"id":"{id}"}},"status":"{status}","activities":[{{"type":0,"name":"Chess"}}]}}}}}}"#)
    }

    #[test]
    fn threads_sync_join_and_link_to_their_message() {
        let mut s = ready_state();
        // A message in #general that a thread is then started from (threads share the message's ID).
        s.add_history(Snowflake(200), vec![ev_message(r#"{"id":"500","channel_id":"200","author":{"id":"5","username":"u5"},"content":"hi"}"#)], 50);
        s.apply(ev(
            "THREAD_LIST_SYNC",
            r#"{"guild_id":"100","threads":[
                {"id":"500","type":11,"name":"Plans","parent_id":"200","guild_id":"100","message_count":2,"thread_metadata":{"archived":false}},
                {"id":"501","type":11,"name":"Old","parent_id":"200","thread_metadata":{"archived":true}},
                {"id":"502","type":11,"name":"Joined","parent_id":"200","thread_metadata":{"archived":false}}],
               "members":[{"id":"502","user_id":"1"}]}"#,
        ));
        let names = |v: Vec<&Channel>| v.iter().map(|c| c.label()).collect::<Vec<_>>();
        assert_eq!(names(s.threads(Snowflake(200))).len(), 2, "archived threads aren't active");
        assert_eq!(names(s.joined_threads(Snowflake(200))), ["Joined"]);
        assert_eq!(s.channel(Snowflake(501)).unwrap().guild_id, Some(Snowflake(100)));

        // Our membership arrives separately; later updates without it keep it.
        s.apply(ev("THREAD_MEMBER_UPDATE", r#"{"id":"500","user_id":"1","guild_id":"100"}"#));
        s.apply(ev("THREAD_UPDATE", r#"{"id":"500","type":11,"name":"Plans!","parent_id":"200","guild_id":"100","message_count":2}"#));
        assert_eq!(names(s.joined_threads(Snowflake(200))), ["Plans!", "Joined"]);

        // New messages count up on the thread and on its starter message's chip.
        s.apply(ev("MESSAGE_CREATE", r#"{"id":"600","channel_id":"500","author":{"id":"5","username":"u5"},"content":"x"}"#));
        let chip = s.message(Snowflake(200), Snowflake(500)).unwrap().thread.as_ref().unwrap();
        assert_eq!((chip.label().as_str(), chip.message_count), ("Plans!", Some(3)));

        s.apply(ev("THREAD_MEMBERS_UPDATE", r#"{"id":"500","guild_id":"100","member_count":4,"removed_member_ids":["1"]}"#));
        assert_eq!(names(s.joined_threads(Snowflake(200))), ["Joined"]);
        assert_eq!(s.channel(Snowflake(500)).unwrap().member_count, Some(4));
    }

    fn ev_message(json: &str) -> Message {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn member_list_syncs_and_applies_ops() {
        let mut s = ready_state();
        let sync = format!(
            r#"{{"guild_id":"100","id":"everyone","member_count":3,"online_count":2,
                "groups":[{{"id":"online","count":2}},{{"id":"offline","count":1}}],
                "ops":[{{"op":"SYNC","range":[0,99],"items":[{{"group":{{"id":"online","count":2}}}},{},{},
                    {{"group":{{"id":"offline","count":1}}}},{}]}}]}}"#,
            list_member(5, "online"),
            list_member(6, "dnd"),
            list_member(7, "offline"),
        );
        s.apply(ev("GUILD_MEMBER_LIST_UPDATE", &sync));
        let list = s.member_list(Snowflake(100)).unwrap();
        assert_eq!(list.rows.len(), 5);
        assert_eq!(list.rows[0], Some(ListRow::Group("online".into())));
        assert_eq!(list.rows[2], Some(ListRow::Member(Snowflake(6))));
        assert_eq!(s.presence(Snowflake(6)), PresenceStatus::Dnd);
        assert_eq!(s.activity(Snowflake(6)), Some("Playing Chess"));
        assert_eq!(s.display_name(Snowflake(7), Some(Snowflake(100))), "u7");

        // 6 goes offline: deleted from the online group, inserted into the offline one.
        let update = format!(
            r#"{{"guild_id":"100","id":"everyone","member_count":3,"online_count":1,
                "groups":[{{"id":"online","count":1}},{{"id":"offline","count":2}}],
                "ops":[{{"op":"DELETE","index":2}},{{"op":"INSERT","index":3,"item":{}}}]}}"#,
            list_member(6, "offline"),
        );
        s.apply(ev("GUILD_MEMBER_LIST_UPDATE", &update));
        let list = s.member_list(Snowflake(100)).unwrap();
        let rows: Vec<_> = list.rows.iter().map(|r| r.clone().unwrap()).collect();
        assert_eq!(
            rows,
            [
                ListRow::Group("online".into()),
                ListRow::Member(Snowflake(5)),
                ListRow::Group("offline".into()),
                ListRow::Member(Snowflake(6)),
                ListRow::Member(Snowflake(7)),
            ]
        );
        assert_eq!(list.group_count("offline"), 2);
        assert_eq!(s.presence(Snowflake(6)), PresenceStatus::Offline);

        // Another list's updates are ignored until it syncs.
        let other = r#"{"guild_id":"100","id":"123","member_count":1,"groups":[{"id":"online","count":1}],
            "ops":[{"op":"INVALIDATE","range":[0,99]}]}"#;
        s.apply(ev("GUILD_MEMBER_LIST_UPDATE", other));
        assert_eq!(s.member_list(Snowflake(100)).unwrap().id, "everyone");
    }

    #[test]
    fn own_status_comes_from_sessions() {
        let mut s = ready_state();
        assert_eq!(s.presence(Snowflake(1)), PresenceStatus::Offline);
        s.apply(ev(
            "SESSIONS_REPLACE",
            r#"[{"session_id":"abc","status":"online"},
                {"session_id":"all","status":"idle","activities":[{"type":4,"name":"Custom Status","state":"brb","emoji":{"name":"☕"}}]}]"#,
        ));
        assert_eq!(s.presence(Snowflake(1)), PresenceStatus::Idle);
        assert_eq!(s.activity(Snowflake(1)), Some("☕ brb"));
        // Kept to send back with status changes; the emoji survives the round trip.
        let sent = serde_json::to_value(s.own_custom_status()).unwrap();
        assert_eq!(sent[0]["state"], "brb");
        assert_eq!(sent[0]["emoji"]["name"], "☕");
        s.set_own_status(PresenceStatus::Dnd);
        assert_eq!(s.own_status(), PresenceStatus::Dnd);
    }

    #[test]
    fn ready_builds_tree_with_permissions_and_voice() {
        let s = ready_state();
        let tree = s.channel_tree(Snowflake(100));
        let names: Vec<String> = tree[0].1.iter().map(|c| c.label()).collect();
        // Hidden channel omitted; text before voice.
        assert_eq!(names, ["general", "Voice"]);
        assert_eq!(s.voice_members(s.channel(Snowflake(202)).unwrap()).len(), 1);
        assert!(s.is_unread(Snowflake(300)));
        assert_eq!(s.private_channels()[0].label(), "friend");
    }

    #[test]
    fn message_create_replaces_pending_and_counts_dm_mentions() {
        let mut s = ready_state();
        s.add_history(Snowflake(300), vec![], 50);
        s.add_pending(Message { id: Snowflake(5000), channel_id: Snowflake(300), pending: Some(PendingState::Sending), ..Default::default() });
        s.apply(ev("MESSAGE_CREATE", r#"{"id":"6000","channel_id":"300","author":{"id":"1","username":"me"},"content":"hi","nonce":"5000"}"#));
        let list = &s.messages(Snowflake(300)).unwrap().list;
        assert_eq!(list.keys().copied().collect::<Vec<_>>(), [Snowflake(6000)]);

        s.apply(ev("MESSAGE_CREATE", r#"{"id":"6001","channel_id":"300","author":{"id":"2","username":"friend"},"content":"yo"}"#));
        assert_eq!(s.mention_count(Snowflake(300)), 1);
        assert!(s.is_unread(Snowflake(300)));
    }

    #[test]
    fn voice_state_leave_removes_member() {
        let mut s = ready_state();
        let n = s.apply(ev("VOICE_STATE_UPDATE", r#"{"guild_id":"100","user_id":"1","channel_id":"202","session_id":"a"}"#));
        assert!(matches!(n[..], [Notice::OwnVoiceState]));
        assert!(s.my_voice_state().is_some());
        s.apply(ev("VOICE_STATE_UPDATE", r#"{"guild_id":"100","user_id":"1","channel_id":null,"session_id":"a"}"#));
        assert!(s.my_voice_state().is_none());
    }

    #[test]
    fn searches_people_by_any_name() {
        let mut s = ready_state();
        s.apply(ev(
            "GUILD_MEMBERS_CHUNK",
            r#"{"guild_id":"100","members":[
                {"user":{"id":"10","username":"alice","global_name":"Ally"},"roles":[]},
                {"user":{"id":"11","username":"bob"},"nick":"Alfred","roles":[]},
                {"user":{"id":"12","username":"malice"},"roles":[]}]}"#,
        ));
        let g = s.channel(Snowflake(200)).unwrap().clone();
        let names: Vec<&str> = s.search_people(&g, "al", 10).iter().map(|u| u.username.as_str()).collect();
        // Prefix matches (alice by username, bob by nickname) before the substring match.
        assert_eq!(names, ["alice", "bob", "malice"]);
        let dm = s.channel(Snowflake(300)).unwrap().clone();
        assert_eq!(s.search_people(&dm, "fr", 10).len(), 1);
        // Channel 201 hides itself from @everyone, so nobody without a role can be mentioned there.
        let secret = s.channel(Snowflake(201)).unwrap().clone();
        assert!(s.search_people(&secret, "al", 10).is_empty());
    }

    #[test]
    fn calls_ring_and_stop() {
        let mut s = ready_state();
        let n = s.apply(ev("CALL_CREATE", r#"{"channel_id":"300","ringing":["1"],"voice_states":[{"user_id":"2","channel_id":"300","session_id":"x"}]}"#));
        assert!(matches!(n[..], [Notice::Ring(Snowflake(300))]));
        assert_eq!(s.ringing_me(), [Snowflake(300)]);
        assert_eq!(s.voice_members(s.channel(Snowflake(300)).unwrap()).len(), 1);
        let n = s.apply(ev("CALL_UPDATE", r#"{"channel_id":"300","ringing":[]}"#));
        assert!(matches!(n[..], [Notice::RingStopped(Snowflake(300))]));
        s.apply(ev("CALL_DELETE", r#"{"channel_id":"300"}"#));
        assert!(s.call(Snowflake(300)).is_none());
        assert!(s.voice_members(s.channel(Snowflake(300)).unwrap()).is_empty());
    }

    #[test]
    fn reactions_count_once_with_optimistic_updates() {
        let mut s = ready_state();
        s.add_history(Snowflake(200), vec![Message { id: Snowflake(10), channel_id: Snowflake(200), ..Default::default() }], 50);
        let thumbs = Emoji { name: Some("👍".into()), ..Default::default() };
        s.optimistic_reaction(Snowflake(200), Snowflake(10), &thumbs, true);
        s.apply(ev("MESSAGE_REACTION_ADD", r#"{"user_id":"1","channel_id":"200","message_id":"10","emoji":{"name":"👍"}}"#));
        let r = &s.message(Snowflake(200), Snowflake(10)).unwrap().reactions[0];
        assert_eq!((r.count, r.me), (1, true));
    }
}
