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

#[derive(Default)]
pub struct State {
    pub me: Option<User>,
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

    fn permissions_for(&self, channel: &Channel) -> u64 {
        let Some(guild_id) = channel.guild_id else { return perm::ALL };
        let (Some(guild), Some(me)) = (self.guilds.get(&guild_id), &self.me) else { return 0 };
        // Threads inherit permissions from their parent channel.
        let channel = match channel.parent_id {
            Some(p) if channel.is_thread() => self.channels.get(&p).unwrap_or(channel),
            _ => channel,
        };
        let roles = self.member(guild_id, me.id).map(|m| m.roles.as_slice()).unwrap_or(&[]);
        permissions::compute(guild, channel, me.id, roles)
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

    pub fn add_channel(&mut self, channel: Channel) {
        for r in &channel.recipients {
            self.users.insert(r.id, r.clone());
        }
        self.channels.insert(channel.id, channel);
        self.revision += 1;
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
                    self.presences.insert(p.user_id(), PresenceStatus::parse(&p.status));
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
                self.channels.insert(c.id, *c);
                self.revision += 1;
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
            Event::PresenceUpdate(p) => {
                self.presences.insert(p.user.id, PresenceStatus::parse(&p.status));
            }
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
        *self = State { revision: self.revision + 1, ..State::default() };
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

    fn cache_author(&mut self, m: &Message) {
        if m.author.id.is_none() {
            return;
        }
        self.users.insert(m.author.id, m.author.clone());
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

        if let Some(c) = self.channels.get_mut(&channel_id) {
            c.last_message_id = Some(message_id);
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
