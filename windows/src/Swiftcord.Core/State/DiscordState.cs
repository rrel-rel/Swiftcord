using System.Text.Json;
using Swiftcord.Core.Json;
using Swiftcord.Core.Models;
using Swiftcord.Core.Permissions;

namespace Swiftcord.Core.State;

/// <summary>
/// In-memory cache of everything the client knows, kept current from gateway events.
/// Mutations happen on the gateway thread under a lock; events are raised after the lock is
/// released, so UI handlers must marshal to their own thread.
/// </summary>
public sealed class DiscordState(int messageCacheSize = 250, TimeProvider? time = null)
{
    private static readonly TimeSpan TypingDuration = TimeSpan.FromSeconds(10);

    private readonly Lock _lock = new();
    private readonly TimeProvider _time = time ?? TimeProvider.System;

    private readonly List<Snowflake> _guildOrder = [];
    private List<GuildFolder>? _guildFolders;
    private readonly Dictionary<Snowflake, Guild> _guilds = [];
    private readonly Dictionary<Snowflake, Channel> _channels = [];
    private readonly Dictionary<Snowflake, User> _users = [];
    private readonly Dictionary<Snowflake, Dictionary<Snowflake, GuildMember>> _members = [];
    private readonly Dictionary<Snowflake, SortedList<Snowflake, Message>> _messages = [];
    private readonly Dictionary<Snowflake, ReadStateEntry> _readStates = [];
    private readonly Dictionary<Snowflake, PresenceStatus> _presences = [];
    /// <summary>Notification settings by server ID; DM settings are under the default ID.</summary>
    private readonly Dictionary<Snowflake, UserGuildSettings> _guildSettings = [];
    private readonly Dictionary<Snowflake, Dictionary<Snowflake, DateTimeOffset>> _typing = [];

    public User? CurrentUser { get; private set; }

    #region Events

    public event Action? Ready;
    public event Action<Guild>? GuildAvailable;
    /// <summary>The user's saved server order or folders changed.</summary>
    public event Action? GuildFoldersChanged;
    public event Action<Guild>? GuildUpdated;
    /// <summary>The guild was left, or became unavailable when the flag is true.</summary>
    public event Action<Snowflake, bool>? GuildRemoved;
    public event Action<Channel>? ChannelCreated;
    public event Action<Channel>? ChannelUpdated;
    public event Action<Channel>? ChannelDeleted;
    public event Action<Message>? MessageCreated;
    public event Action<Message>? MessageUpdated;
    /// <summary>Channel ID and message ID.</summary>
    public event Action<Snowflake, Snowflake>? MessageDeleted;
    public event Action<Message>? ReactionsChanged;
    /// <summary>Channel ID and user ID.</summary>
    public event Action<Snowflake, Snowflake>? TypingStarted;
    /// <summary>The unread or mention state of a channel changed.</summary>
    public event Action<Snowflake>? ReadStateChanged;
    public event Action<Snowflake, PresenceStatus>? PresenceChanged;
    public event Action<User>? CurrentUserUpdated;
    /// <summary>The current user's roles or the guild's roles changed, so channel visibility may differ.</summary>
    public event Action<Snowflake>? PermissionsChanged;

    #endregion

    #region Queries

    /// <summary>Servers in the user's saved order (see <see cref="GuildFolders"/>).</summary>
    public IReadOnlyList<Guild> Guilds
    {
        get { lock (_lock) return OrderedGuildIdsLocked().Select(id => _guilds[id]).ToList(); }
    }

    /// <summary>
    /// The user's server folders in display order, covering every server; lone servers are folders without an ID.
    /// Null until the settings have been received.
    /// </summary>
    public IReadOnlyList<GuildFolder>? GuildFolders
    {
        get
        {
            lock (_lock)
            {
                if (_guildFolders is null) return null;
                var placed = _guildFolders.SelectMany(f => f.GuildIds).ToHashSet();
                return UnplacedGuildIdsLocked(placed).Select(id => new GuildFolder { GuildIds = [id] })
                    .Concat(_guildFolders
                        .Select(f => new GuildFolder { Id = f.Id, Name = f.Name, Color = f.Color, GuildIds = f.GuildIds.Where(_guilds.ContainsKey).ToList() })
                        .Where(f => f.GuildIds.Count > 0))
                    .ToList();
            }
        }
    }

    /// <summary>Replaces the saved server order, e.g. from settings fetched over REST.</summary>
    public void SetGuildFolders(List<GuildFolder> folders)
    {
        lock (_lock) _guildFolders = folders;
        GuildFoldersChanged?.Invoke();
    }

    private IEnumerable<Snowflake> OrderedGuildIdsLocked()
    {
        if (_guildFolders is null)
            return _guildOrder.Where(_guilds.ContainsKey);
        var placed = _guildFolders.SelectMany(f => f.GuildIds).Where(_guilds.ContainsKey).Distinct().ToList();
        return UnplacedGuildIdsLocked(placed.ToHashSet()).Concat(placed);
    }

    /// <summary>Servers missing from the saved order (joined since it was saved) go first, newest first, as in Discord.</summary>
    private IEnumerable<Snowflake> UnplacedGuildIdsLocked(HashSet<Snowflake> placed) =>
        _guildOrder.Where(id => _guilds.ContainsKey(id) && !placed.Contains(id))
            .OrderByDescending(id => _guilds[id].JoinedAt ?? id.CreatedAt);

    public Guild? GetGuild(Snowflake id)
    {
        lock (_lock) return _guilds.GetValueOrDefault(id);
    }

    public Channel? GetChannel(Snowflake id)
    {
        lock (_lock) return _channels.GetValueOrDefault(id);
    }

    public User? GetUser(Snowflake id)
    {
        lock (_lock) return _users.GetValueOrDefault(id);
    }

    public GuildMember? GetMember(Snowflake guildId, Snowflake userId)
    {
        lock (_lock) return _members.GetValueOrDefault(guildId)?.GetValueOrDefault(userId);
    }

    public PresenceStatus GetPresence(Snowflake userId)
    {
        lock (_lock) return _presences.GetValueOrDefault(userId);
    }

    /// <summary>DMs and group DMs, most recently active first.</summary>
    public IReadOnlyList<Channel> PrivateChannels
    {
        get
        {
            lock (_lock)
                return _channels.Values.Where(c => c.IsPrivate)
                    .OrderByDescending(c => c.LastMessageId ?? c.Id)
                    .ToList();
        }
    }

    /// <summary>Cached messages for a channel, oldest first.</summary>
    public IReadOnlyList<Message> GetMessages(Snowflake channelId)
    {
        lock (_lock) return _messages.TryGetValue(channelId, out var list) ? list.Values.ToList() : [];
    }

    public Message? GetMessage(Snowflake channelId, Snowflake messageId)
    {
        lock (_lock) return _messages.GetValueOrDefault(channelId)?.GetValueOrDefault(messageId);
    }

    public bool IsUnread(Snowflake channelId)
    {
        lock (_lock) return IsUnreadLocked(channelId);
    }

    /// <summary>
    /// Whether a new message should raise a desktop notification under the user's Discord notification
    /// settings: per-channel (then category, then server) level and mutes, @everyone and role suppression,
    /// and @silent messages. A mute still lets direct @mentions through, as in Discord.
    /// </summary>
    public bool ShouldNotify(Message message)
    {
        lock (_lock)
        {
            if (CurrentUser is not { } me || message.Author.Id == me.Id || message.Flags.HasFlag(MessageFlags.SuppressNotifications))
                return false;
            if (!_channels.TryGetValue(message.ChannelId, out var channel))
                return false;

            var now = _time.GetUtcNow();
            var settings = _guildSettings.GetValueOrDefault(channel.GuildId ?? default);
            var overrides = ChannelAndParentsLocked(channel)
                .Select(c => settings?.ChannelOverrides.Find(o => o.ChannelId == c.Id))
                .OfType<ChannelNotificationOverride>()
                .ToList();
            var directMention = message.Mentions.Any(u => u.Id == me.Id);
            var muted = overrides.Any(o => o.IsMuted(now)) || settings?.IsMuted(now) == true;
            if (muted)
                return directMention;
            if (channel.IsPrivate)
                return true;

            var level = overrides.Select(o => o.MessageNotifications).FirstOrDefault(l => l != MessageNotificationLevel.Inherit, MessageNotificationLevel.Inherit);
            if (level == MessageNotificationLevel.Inherit)
                level = settings?.MessageNotifications ?? MessageNotificationLevel.Inherit;
            if (level == MessageNotificationLevel.Inherit && channel.GuildId is { } guildId)
                level = _guilds.GetValueOrDefault(guildId)?.DefaultMessageNotifications ?? MessageNotificationLevel.AllMessages;

            return level switch
            {
                MessageNotificationLevel.AllMessages => true,
                MessageNotificationLevel.OnlyMentions => directMention
                    || (message.MentionEveryone && settings?.SuppressEveryone != true)
                    || (settings?.SuppressRoles != true && channel.GuildId is { } g
                        && _members.GetValueOrDefault(g)?.GetValueOrDefault(me.Id)?.Roles is { } roles
                        && message.MentionRoles.Any(roles.Contains)),
                _ => false,
            };
        }
    }

    /// <summary>A channel followed by its parents: a thread's channel, then the category.</summary>
    private IEnumerable<Channel> ChannelAndParentsLocked(Channel channel)
    {
        for (var current = channel; current is not null;
             current = current.ParentId is { } parent ? _channels.GetValueOrDefault(parent) : null)
            yield return current;
    }

    public int GetMentionCount(Snowflake channelId)
    {
        lock (_lock) return _readStates.GetValueOrDefault(channelId)?.MentionCount ?? 0;
    }

    public bool IsGuildUnread(Snowflake guildId)
    {
        lock (_lock)
            return _channels.Values.Any(c => c.GuildId == guildId && c.IsTextBased && !c.IsThread
                && IsUnreadLocked(c.Id) && CanViewLocked(c));
    }

    public int GetGuildMentionCount(Snowflake guildId)
    {
        lock (_lock)
            return _channels.Values.Where(c => c.GuildId == guildId)
                .Sum(c => _readStates.GetValueOrDefault(c.Id)?.MentionCount ?? 0);
    }

    /// <summary>The current user's permissions in a channel; everything for DMs.</summary>
    public Permission GetPermissions(Snowflake channelId)
    {
        lock (_lock)
            return _channels.TryGetValue(channelId, out var channel) ? GetPermissionsLocked(channel) : Permission.None;
    }

    /// <summary>
    /// The guild's channel sidebar: uncategorized channels first, then each category with its children.
    /// Only channels the user can view are included, and empty categories are omitted.
    /// </summary>
    public IReadOnlyList<ChannelGroup> GetChannelTree(Snowflake guildId)
    {
        lock (_lock)
        {
            var channels = _channels.Values
                .Where(c => c.GuildId == guildId && !c.IsThread && CanViewLocked(c))
                .ToList();

            var categories = channels.Where(c => c.Type == ChannelType.GuildCategory)
                .OrderBy(c => c.Position).ThenBy(c => c.Id)
                .ToList();
            var categoryIds = categories.Select(c => c.Id).ToHashSet();

            // Within a group, text-like channels come before voice channels, then by position.
            List<Channel> Children(Snowflake? parentId) => channels
                .Where(c => c.Type != ChannelType.GuildCategory
                    && (parentId is null ? c.ParentId is not { } p || !categoryIds.Contains(p) : c.ParentId == parentId))
                .OrderBy(c => c.IsVoice ? 1 : 0).ThenBy(c => c.Position).ThenBy(c => c.Id)
                .ToList();

            var groups = new List<ChannelGroup>();
            var uncategorized = Children(null);
            if (uncategorized.Count > 0)
                groups.Add(new ChannelGroup(null, uncategorized));
            foreach (var category in categories)
            {
                var children = Children(category.Id);
                if (children.Count > 0)
                    groups.Add(new ChannelGroup(category, children));
            }
            return groups;
        }
    }

    /// <summary>Users currently typing in a channel, excluding the current user.</summary>
    public IReadOnlyList<Snowflake> GetTypingUsers(Snowflake channelId)
    {
        lock (_lock)
        {
            if (!_typing.TryGetValue(channelId, out var typing))
                return [];
            var now = _time.GetUtcNow();
            foreach (var expired in typing.Where(kv => kv.Value <= now).Select(kv => kv.Key).ToList())
                typing.Remove(expired);
            return typing.Keys.ToList();
        }
    }

    /// <summary>Display name for a user in context: guild nickname, then global name, then username.</summary>
    public string GetDisplayName(Snowflake userId, Snowflake? guildId = null)
    {
        lock (_lock)
        {
            if (guildId is { } g && _members.GetValueOrDefault(g)?.GetValueOrDefault(userId)?.Nick is { Length: > 0 } nick)
                return nick;
            return _users.GetValueOrDefault(userId)?.DisplayName ?? "Unknown User";
        }
    }

    /// <summary>The colour of the member's highest coloured role, or null for the default colour.</summary>
    public int? GetRoleColor(Snowflake guildId, Snowflake userId)
    {
        lock (_lock)
        {
            if (!_guilds.TryGetValue(guildId, out var guild)
                || _members.GetValueOrDefault(guildId)?.GetValueOrDefault(userId) is not { } member)
                return null;
            return guild.Roles.Where(r => r.Color != 0 && member.Roles.Contains(r.Id))
                .OrderByDescending(r => r.Position)
                .Select(r => (int?)r.Color)
                .FirstOrDefault();
        }
    }

    #endregion

    #region Mutations from REST

    /// <summary>Adds messages fetched over REST (e.g. history pagination) to the cache.</summary>
    public void AddMessages(Snowflake channelId, IEnumerable<Message> messages)
    {
        lock (_lock)
        {
            var list = GetMessageListLocked(channelId);
            foreach (var message in messages)
            {
                CacheAuthorLocked(message);
                list[message.Id] = message;
            }
        }
    }

    /// <summary>Records that the current user has read a channel up to a message.</summary>
    public void MarkRead(Snowflake channelId, Snowflake messageId)
    {
        lock (_lock)
        {
            var entry = GetReadStateLocked(channelId);
            if (entry.LastMessageId is { } last && last >= messageId && entry.MentionCount == 0)
                return;
            entry.LastMessageId = entry.LastMessageId is { } l && l > messageId ? l : messageId;
            entry.MentionCount = 0;
        }
        ReadStateChanged?.Invoke(channelId);
    }

    public void AddChannel(Channel channel)
    {
        lock (_lock)
        {
            _channels[channel.Id] = channel;
            foreach (var recipient in channel.Recipients)
                _users[recipient.Id] = recipient;
        }
        ChannelCreated?.Invoke(channel);
    }

    #endregion

    #region Gateway dispatch

    public void HandleDispatch(string eventName, JsonElement data)
    {
        switch (eventName)
        {
            case "READY": OnReady(DiscordJson.Deserialize<ReadyEvent>(data)); break;
            case "READY_SUPPLEMENTAL": OnReadySupplemental(data); break;
            case "GUILD_CREATE": OnGuildCreate(DiscordJson.Deserialize<Guild>(data)); break;
            case "GUILD_UPDATE": OnGuildUpdate(data); break;
            case "GUILD_DELETE": OnGuildDelete(DiscordJson.Deserialize<UnavailableGuild>(data)); break;
            case "GUILD_ROLE_CREATE":
            case "GUILD_ROLE_UPDATE":
            case "GUILD_ROLE_DELETE": OnGuildRole(eventName, DiscordJson.Deserialize<GuildRoleEvent>(data)); break;
            case "GUILD_EMOJIS_UPDATE": OnGuildEmojis(DiscordJson.Deserialize<GuildEmojisUpdateEvent>(data)); break;
            case "GUILD_MEMBER_UPDATE": OnGuildMemberUpdate(DiscordJson.Deserialize<GuildMemberUpdateEvent>(data)); break;
            case "CHANNEL_CREATE":
            case "THREAD_CREATE": AddChannel(DiscordJson.Deserialize<Channel>(data)); break;
            case "CHANNEL_UPDATE":
            case "THREAD_UPDATE": OnChannelUpdate(DiscordJson.Deserialize<Channel>(data)); break;
            case "CHANNEL_DELETE":
            case "THREAD_DELETE": OnChannelDelete(DiscordJson.Deserialize<Channel>(data)); break;
            case "MESSAGE_CREATE": OnMessageCreate(DiscordJson.Deserialize<Message>(data)); break;
            case "MESSAGE_UPDATE": OnMessageUpdate(data); break;
            case "MESSAGE_DELETE": OnMessageDelete(DiscordJson.Deserialize<MessageDeleteEvent>(data)); break;
            case "MESSAGE_DELETE_BULK": OnMessageDeleteBulk(DiscordJson.Deserialize<MessageDeleteBulkEvent>(data)); break;
            case "MESSAGE_ACK": OnMessageAck(DiscordJson.Deserialize<MessageAckEvent>(data)); break;
            case "MESSAGE_REACTION_ADD": OnReaction(DiscordJson.Deserialize<ReactionEvent>(data), added: true); break;
            case "MESSAGE_REACTION_REMOVE": OnReaction(DiscordJson.Deserialize<ReactionEvent>(data), added: false); break;
            case "MESSAGE_REACTION_REMOVE_ALL":
            case "MESSAGE_REACTION_REMOVE_EMOJI": OnReactionRemoveAll(DiscordJson.Deserialize<ReactionRemoveAllEvent>(data)); break;
            case "TYPING_START": OnTypingStart(DiscordJson.Deserialize<TypingStartEvent>(data)); break;
            case "PRESENCE_UPDATE": OnPresenceUpdate(DiscordJson.Deserialize<PresenceUpdateEvent>(data)); break;
            case "USER_UPDATE": OnUserUpdate(DiscordJson.Deserialize<User>(data)); break;
            case "USER_SETTINGS_UPDATE": OnUserSettingsUpdate(data); break;
            case "USER_GUILD_SETTINGS_UPDATE": OnUserGuildSettingsUpdate(DiscordJson.Deserialize<UserGuildSettings>(data)); break;
            case "USER_SETTINGS_PROTO_UPDATE": OnUserSettingsProtoUpdate(data); break;
        }
    }

    private void OnReady(ReadyEvent ready)
    {
        lock (_lock)
        {
            _guildOrder.Clear();
            _guilds.Clear();
            _channels.Clear();
            _members.Clear();
            _messages.Clear();
            _readStates.Clear();
            _typing.Clear();

            CurrentUser = ready.User;
            _users[ready.User.Id] = ready.User;
            foreach (var user in ready.Users)
                _users[user.Id] = user;

            for (var i = 0; i < ready.Guilds.Count; i++)
            {
                var guild = ready.Guilds[i];
                if (i < ready.MergedMembers.Count)
                    guild.Members.AddRange(ready.MergedMembers[i]);
                AddGuildLocked(guild);
            }

            foreach (var channel in ready.PrivateChannels)
            {
                if (channel.Recipients.Count == 0 && channel.RecipientIds is { } ids)
                    channel.Recipients = ids.Select(id => _users.GetValueOrDefault(id)).OfType<User>().ToList();
                foreach (var recipient in channel.Recipients)
                    _users[recipient.Id] = recipient;
                _channels[channel.Id] = channel;
            }

            if (ready.ReadState is { } readState)
                LoadReadStateLocked(readState);
            _guildSettings.Clear();
            if (ready.UserGuildSettings is { } guildSettings)
                LoadUserGuildSettingsLocked(guildSettings);

            _guildFolders = ready.UserSettingsProto is { Length: > 0 } proto ? UserSettingsReader.ReadGuildFolders(proto)
                : ready.UserSettings is { } settings ? ReadLegacyGuildFolders(settings)
                : null;
        }
        Ready?.Invoke();
    }

    private static List<GuildFolder>? ReadLegacyGuildFolders(JsonElement settings) =>
        settings.ValueKind == JsonValueKind.Object && settings.TryGetProperty("guild_folders", out var folders)
            && folders.ValueKind == JsonValueKind.Array
            ? DiscordJson.Deserialize<List<GuildFolder>>(folders)
            : null;

    private void OnUserSettingsUpdate(JsonElement data)
    {
        if (ReadLegacyGuildFolders(data) is { } folders)
            SetGuildFolders(folders);
    }

    private void OnUserSettingsProtoUpdate(JsonElement data)
    {
        // { settings: { type: 1, proto: "<base64>" }, partial: bool }. Type 1 is PreloadedUserSettings.
        if (data.TryGetProperty("settings", out var settings)
            && settings.TryGetProperty("type", out var type) && type.ValueKind == JsonValueKind.Number && type.GetInt32() == 1
            && settings.TryGetProperty("proto", out var proto) && proto.GetString() is { } base64
            && UserSettingsReader.ReadGuildFolders(base64) is { } folders)
        {
            SetGuildFolders(folders);
        }
    }

    private void LoadReadStateLocked(JsonElement readState)
    {
        // Older capability sets send a bare array; newer ones wrap it as { version, partial, entries }.
        var entries = readState.ValueKind == JsonValueKind.Object && readState.TryGetProperty("entries", out var e) ? e : readState;
        if (entries.ValueKind != JsonValueKind.Array)
            return;
        foreach (var element in entries.EnumerateArray())
        {
            // Non-channel read states (e.g. guild events) have a read_state_type and are skipped.
            if (element.TryGetProperty("read_state_type", out var type) && type.ValueKind == JsonValueKind.Number && type.GetInt32() != 0)
                continue;
            var entry = DiscordJson.Deserialize<ReadStateEntry>(element);
            _readStates[entry.Id] = entry;
        }
    }

    private void LoadUserGuildSettingsLocked(JsonElement settings)
    {
        // Like read states: a bare array, or { version, partial, entries } with newer capabilities.
        var entries = settings.ValueKind == JsonValueKind.Object && settings.TryGetProperty("entries", out var e) ? e : settings;
        if (entries.ValueKind != JsonValueKind.Array)
            return;
        foreach (var element in entries.EnumerateArray())
        {
            var entry = DiscordJson.Deserialize<UserGuildSettings>(element);
            _guildSettings[entry.GuildId ?? default] = entry;
        }
    }

    private void OnUserGuildSettingsUpdate(UserGuildSettings settings)
    {
        lock (_lock) _guildSettings[settings.GuildId ?? default] = settings;
    }

    private void OnReadySupplemental(JsonElement data)
    {
        var changed = new List<(Snowflake, PresenceStatus)>();
        lock (_lock)
        {
            if (data.TryGetProperty("merged_presences", out var merged) && merged.TryGetProperty("friends", out var friends))
            {
                foreach (var presence in friends.EnumerateArray())
                {
                    if (presence.TryGetProperty("user_id", out var idElement) && Snowflake.TryParse(idElement.GetString(), out var id))
                    {
                        var status = PresenceStatusExtensions.Parse(presence.TryGetProperty("status", out var s) ? s.GetString() : null);
                        _presences[id] = status;
                        changed.Add((id, status));
                    }
                }
            }
        }
        foreach (var (id, status) in changed)
            PresenceChanged?.Invoke(id, status);
    }

    private void OnGuildCreate(Guild guild)
    {
        lock (_lock) AddGuildLocked(guild);
        GuildAvailable?.Invoke(guild);
    }

    private void AddGuildLocked(Guild guild)
    {
        if (!_guildOrder.Contains(guild.Id))
            _guildOrder.Add(guild.Id);
        _guilds[guild.Id] = guild;
        if (guild.Unavailable)
            return;

        foreach (var channel in guild.Channels.Concat(guild.Threads))
        {
            channel.GuildId = guild.Id;
            _channels[channel.Id] = channel;
        }

        var members = GetMembersLocked(guild.Id);
        foreach (var member in guild.Members)
        {
            if (member.EffectiveUserId is not { } userId)
                continue;
            if (member.User is { } user)
                _users[user.Id] = user;
            members[userId] = member;
        }
    }

    private void OnGuildUpdate(JsonElement data)
    {
        Guild? merged;
        lock (_lock)
        {
            var id = DiscordJson.Deserialize<UnavailableGuild>(data).Id;
            if (!_guilds.TryGetValue(id, out var existing))
                return;
            // GUILD_UPDATE omits channels and members; merge to keep them.
            merged = DiscordJson.Merge(existing, data);
            merged.Channels = existing.Channels;
            merged.Threads = existing.Threads;
            merged.Members = existing.Members;
            _guilds[id] = merged;
        }
        GuildUpdated?.Invoke(merged);
        PermissionsChanged?.Invoke(merged.Id);
    }

    private void OnGuildDelete(UnavailableGuild deleted)
    {
        lock (_lock)
        {
            if (deleted.Unavailable && _guilds.TryGetValue(deleted.Id, out var guild))
            {
                guild.Unavailable = true;
            }
            else
            {
                _guilds.Remove(deleted.Id);
                _guildOrder.Remove(deleted.Id);
                _members.Remove(deleted.Id);
                foreach (var channel in _channels.Values.Where(c => c.GuildId == deleted.Id).ToList())
                {
                    _channels.Remove(channel.Id);
                    _messages.Remove(channel.Id);
                    _readStates.Remove(channel.Id);
                }
            }
        }
        GuildRemoved?.Invoke(deleted.Id, deleted.Unavailable);
    }

    private void OnGuildRole(string eventName, GuildRoleEvent e)
    {
        lock (_lock)
        {
            if (!_guilds.TryGetValue(e.GuildId, out var guild))
                return;
            var roleId = e.Role?.Id ?? e.RoleId;
            guild.Roles.RemoveAll(r => r.Id == roleId);
            if (eventName != "GUILD_ROLE_DELETE" && e.Role is { } role)
                guild.Roles.Add(role);
        }
        PermissionsChanged?.Invoke(e.GuildId);
    }

    private void OnGuildEmojis(GuildEmojisUpdateEvent e)
    {
        Guild? guild;
        lock (_lock)
        {
            if (!_guilds.TryGetValue(e.GuildId, out guild))
                return;
            guild.Emojis = e.Emojis;
        }
        GuildUpdated?.Invoke(guild);
    }

    private void OnGuildMemberUpdate(GuildMemberUpdateEvent e)
    {
        bool isSelf;
        lock (_lock)
        {
            _users[e.User.Id] = e.User;
            var members = GetMembersLocked(e.GuildId);
            var member = members.GetValueOrDefault(e.User.Id) ?? new GuildMember();
            member.User = e.User;
            member.Nick = e.Nick;
            member.Avatar = e.Avatar;
            member.Roles = e.Roles;
            member.JoinedAt = e.JoinedAt ?? member.JoinedAt;
            member.CommunicationDisabledUntil = e.CommunicationDisabledUntil;
            members[e.User.Id] = member;
            isSelf = e.User.Id == CurrentUser?.Id;
        }
        if (isSelf)
            PermissionsChanged?.Invoke(e.GuildId);
    }

    private void OnChannelUpdate(Channel channel)
    {
        lock (_lock)
        {
            if (_channels.TryGetValue(channel.Id, out var existing))
            {
                channel.GuildId ??= existing.GuildId;
                // The update may omit the last message ID; don't regress unread state.
                if (channel.LastMessageId is null || existing.LastMessageId > channel.LastMessageId)
                    channel.LastMessageId = existing.LastMessageId;
            }
            _channels[channel.Id] = channel;
        }
        ChannelUpdated?.Invoke(channel);
    }

    private void OnChannelDelete(Channel channel)
    {
        lock (_lock)
        {
            _channels.Remove(channel.Id);
            _messages.Remove(channel.Id);
            _readStates.Remove(channel.Id);
            _typing.Remove(channel.Id);
        }
        ChannelDeleted?.Invoke(channel);
    }

    private void OnMessageCreate(Message message)
    {
        var readStateChanged = false;
        lock (_lock)
        {
            CacheAuthorLocked(message);
            if (_messages.TryGetValue(message.ChannelId, out var list))
            {
                list[message.Id] = message;
                while (list.Count > messageCacheSize)
                    list.RemoveAt(0);
            }

            if (_channels.TryGetValue(message.ChannelId, out var channel))
                channel.LastMessageId = message.Id;

            // A new message ends that author's typing indicator.
            _typing.GetValueOrDefault(message.ChannelId)?.Remove(message.Author.Id);

            if (CurrentUser is { } me)
            {
                var entry = GetReadStateLocked(message.ChannelId);
                if (message.Author.Id == me.Id)
                {
                    entry.LastMessageId = message.Id;
                    entry.MentionCount = 0;
                }
                else if (MentionsCurrentUserLocked(message, me))
                {
                    entry.MentionCount++;
                }
                readStateChanged = true;
            }
        }
        MessageCreated?.Invoke(message);
        if (readStateChanged)
            ReadStateChanged?.Invoke(message.ChannelId);
    }

    private void OnMessageUpdate(JsonElement data)
    {
        Message? updated;
        lock (_lock)
        {
            var id = data.GetProperty("id");
            var channelId = Snowflake.Parse(data.GetProperty("channel_id").GetString()!);
            var messageId = Snowflake.Parse(id.GetString()!);
            var list = _messages.GetValueOrDefault(channelId);
            if (list?.GetValueOrDefault(messageId) is { } existing)
                updated = DiscordJson.Merge(existing, data);
            else if (data.TryGetProperty("author", out _))
                updated = DiscordJson.Deserialize<Message>(data);
            else
                return; // A partial update for a message we never saw.

            if (list is not null)
                list[messageId] = updated;
        }
        MessageUpdated?.Invoke(updated);
    }

    private void OnMessageDelete(MessageDeleteEvent e)
    {
        lock (_lock) _messages.GetValueOrDefault(e.ChannelId)?.Remove(e.Id);
        MessageDeleted?.Invoke(e.ChannelId, e.Id);
    }

    private void OnMessageDeleteBulk(MessageDeleteBulkEvent e)
    {
        lock (_lock)
        {
            var list = _messages.GetValueOrDefault(e.ChannelId);
            foreach (var id in e.Ids)
                list?.Remove(id);
        }
        foreach (var id in e.Ids)
            MessageDeleted?.Invoke(e.ChannelId, id);
    }

    private void OnMessageAck(MessageAckEvent e)
    {
        lock (_lock)
        {
            var entry = GetReadStateLocked(e.ChannelId);
            entry.LastMessageId = e.MessageId;
            entry.MentionCount = e.MentionCount ?? 0;
        }
        ReadStateChanged?.Invoke(e.ChannelId);
    }

    private void OnReaction(ReactionEvent e, bool added)
    {
        Message? message;
        lock (_lock)
        {
            message = _messages.GetValueOrDefault(e.ChannelId)?.GetValueOrDefault(e.MessageId);
            if (message is null)
                return;

            var isMe = e.UserId == CurrentUser?.Id;
            var reaction = message.Reactions.FirstOrDefault(r => SameEmoji(r.Emoji, e.Emoji));
            if (added)
            {
                if (reaction is null)
                {
                    reaction = new Reaction { Emoji = e.Emoji };
                    message.Reactions.Add(reaction);
                }
                // Our own reaction may already be counted optimistically.
                if (!(isMe && reaction.Me))
                    reaction.Count++;
                reaction.Me |= isMe;
            }
            else if (reaction is not null)
            {
                if (!(isMe && !reaction.Me))
                    reaction.Count--;
                if (isMe)
                    reaction.Me = false;
                if (reaction.Count <= 0)
                    message.Reactions.Remove(reaction);
            }
        }
        ReactionsChanged?.Invoke(message);
    }

    private void OnReactionRemoveAll(ReactionRemoveAllEvent e)
    {
        Message? message;
        lock (_lock)
        {
            message = _messages.GetValueOrDefault(e.ChannelId)?.GetValueOrDefault(e.MessageId);
            if (message is null)
                return;
            if (e.Emoji is { } emoji)
                message.Reactions.RemoveAll(r => SameEmoji(r.Emoji, emoji));
            else
                message.Reactions.Clear();
        }
        ReactionsChanged?.Invoke(message);
    }

    private void OnTypingStart(TypingStartEvent e)
    {
        lock (_lock)
        {
            if (e.UserId == CurrentUser?.Id)
                return;
            if (e is { GuildId: { } guildId, Member: { } member })
            {
                if (member.User is { } user)
                    _users[user.Id] = user;
                GetMembersLocked(guildId)[e.UserId] = member;
            }
            if (!_typing.TryGetValue(e.ChannelId, out var typing))
                _typing[e.ChannelId] = typing = [];
            // Use the local clock rather than the event timestamp so clock skew can't hide indicators.
            typing[e.UserId] = _time.GetUtcNow() + TypingDuration;
        }
        TypingStarted?.Invoke(e.ChannelId, e.UserId);
    }

    private void OnPresenceUpdate(PresenceUpdateEvent e)
    {
        var status = PresenceStatusExtensions.Parse(e.Status);
        lock (_lock) _presences[e.User.Id] = status;
        PresenceChanged?.Invoke(e.User.Id, status);
    }

    private void OnUserUpdate(User user)
    {
        lock (_lock)
        {
            if (CurrentUser?.Id != user.Id)
                return;
            CurrentUser = user;
            _users[user.Id] = user;
        }
        CurrentUserUpdated?.Invoke(user);
    }

    #endregion

    #region Helpers

    private bool IsUnreadLocked(Snowflake channelId)
    {
        if (!_channels.TryGetValue(channelId, out var channel) || channel.LastMessageId is not { } last)
            return false;
        // Without a read state the channel has never been opened; treat as read to avoid a sea of badges.
        return _readStates.TryGetValue(channelId, out var state) && state.LastMessageId is { } read && last > read;
    }

    private bool CanViewLocked(Channel channel)
    {
        if (channel.Type != ChannelType.GuildCategory)
            return GetPermissionsLocked(channel).HasFlag(Permission.ViewChannel);
        // Categories are visible when any of their children are.
        return _channels.Values.Any(c => c.ParentId == channel.Id && GetPermissionsLocked(c).HasFlag(Permission.ViewChannel));
    }

    private Permission GetPermissionsLocked(Channel channel)
    {
        if (channel.GuildId is not { } guildId)
            return Permission.All;
        if (!_guilds.TryGetValue(guildId, out var guild) || CurrentUser is not { } me)
            return Permission.None;

        // Threads inherit permissions from their parent channel.
        if (channel.IsThread && channel.ParentId is { } parentId && _channels.TryGetValue(parentId, out var parent))
            channel = parent;

        var roles = _members.GetValueOrDefault(guildId)?.GetValueOrDefault(me.Id)?.Roles ?? [];
        return PermissionCalculator.Compute(guild, channel, me.Id, roles);
    }

    private bool MentionsCurrentUserLocked(Message message, User me)
    {
        if (message.Mentions.Any(u => u.Id == me.Id))
            return true;
        if (!_channels.TryGetValue(message.ChannelId, out var channel))
            return false;
        // Every message in a DM counts towards its badge.
        if (channel.IsPrivate)
            return true;
        if (message.MentionEveryone)
            return true;
        var myRoles = channel.GuildId is { } g ? _members.GetValueOrDefault(g)?.GetValueOrDefault(me.Id)?.Roles : null;
        return myRoles is not null && message.MentionRoles.Any(myRoles.Contains);
    }

    private void CacheAuthorLocked(Message message)
    {
        if (message.Author.Id.Value == 0)
            return;
        _users[message.Author.Id] = message.Author;
        if (message is { GuildId: { } guildId, Member: { } member })
        {
            member.User ??= message.Author;
            GetMembersLocked(guildId)[message.Author.Id] = member;
        }
    }

    private SortedList<Snowflake, Message> GetMessageListLocked(Snowflake channelId)
    {
        if (!_messages.TryGetValue(channelId, out var list))
            _messages[channelId] = list = [];
        return list;
    }

    private Dictionary<Snowflake, GuildMember> GetMembersLocked(Snowflake guildId)
    {
        if (!_members.TryGetValue(guildId, out var members))
            _members[guildId] = members = [];
        return members;
    }

    private ReadStateEntry GetReadStateLocked(Snowflake channelId)
    {
        if (!_readStates.TryGetValue(channelId, out var entry))
            _readStates[channelId] = entry = new ReadStateEntry { Id = channelId };
        return entry;
    }

    private static bool SameEmoji(Emoji a, Emoji b) =>
        a.Id is { } id ? id == b.Id : b.Id is null && a.Name == b.Name;

    #endregion
}

public sealed record ChannelGroup(Channel? Category, IReadOnlyList<Channel> Channels);
