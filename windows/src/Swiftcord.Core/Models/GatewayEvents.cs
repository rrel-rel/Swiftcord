using System.Text.Json;

namespace Swiftcord.Core.Models;

// Payloads of gateway dispatch events that don't map directly onto a resource model.

public sealed class ReadyEvent
{
    public int V { get; set; }
    public User User { get; set; } = new();
    public string SessionId { get; set; } = "";
    public string? ResumeGatewayUrl { get; set; }
    public List<Guild> Guilds { get; set; } = [];
    public List<Channel> PrivateChannels { get; set; } = [];
    /// <summary>Users referenced by <see cref="Channel.RecipientIds"/> when deduplication is enabled.</summary>
    public List<User> Users { get; set; } = [];
    /// <summary>The current user's member object for each guild, in the same order as <see cref="Guilds"/>.</summary>
    public List<List<GuildMember>> MergedMembers { get; set; } = [];
    /// <summary>Either an array of entries or a versioned { entries: [...] } object depending on capabilities.</summary>
    public JsonElement? ReadState { get; set; }
    public JsonElement? UserGuildSettings { get; set; }
}

public sealed class ReadStateEntry
{
    /// <summary>The channel ID.</summary>
    public Snowflake Id { get; set; }
    public Snowflake? LastMessageId { get; set; }
    public int MentionCount { get; set; }
    public DateTimeOffset? LastPinTimestamp { get; set; }
}

public sealed class MessageDeleteEvent
{
    public Snowflake Id { get; set; }
    public Snowflake ChannelId { get; set; }
    public Snowflake? GuildId { get; set; }
}

public sealed class MessageDeleteBulkEvent
{
    public List<Snowflake> Ids { get; set; } = [];
    public Snowflake ChannelId { get; set; }
    public Snowflake? GuildId { get; set; }
}

public sealed class MessageAckEvent
{
    public Snowflake ChannelId { get; set; }
    public Snowflake MessageId { get; set; }
    public int? MentionCount { get; set; }
}

public sealed class ReactionEvent
{
    public Snowflake UserId { get; set; }
    public Snowflake ChannelId { get; set; }
    public Snowflake MessageId { get; set; }
    public Snowflake? GuildId { get; set; }
    public GuildMember? Member { get; set; }
    public Emoji Emoji { get; set; } = new();
    public bool Burst { get; set; }
}

public sealed class ReactionRemoveAllEvent
{
    public Snowflake ChannelId { get; set; }
    public Snowflake MessageId { get; set; }
    public Snowflake? GuildId { get; set; }
    /// <summary>Set for MESSAGE_REACTION_REMOVE_EMOJI; null for REMOVE_ALL.</summary>
    public Emoji? Emoji { get; set; }
}

public sealed class TypingStartEvent
{
    public Snowflake ChannelId { get; set; }
    public Snowflake? GuildId { get; set; }
    public Snowflake UserId { get; set; }
    /// <summary>Unix time in seconds.</summary>
    public long Timestamp { get; set; }
    public GuildMember? Member { get; set; }
}

public sealed class UnavailableGuild
{
    public Snowflake Id { get; set; }
    public bool Unavailable { get; set; }
}

public sealed class GuildRoleEvent
{
    public Snowflake GuildId { get; set; }
    public Role? Role { get; set; }
    public Snowflake? RoleId { get; set; }
}

public sealed class GuildMemberUpdateEvent
{
    public Snowflake GuildId { get; set; }
    public User User { get; set; } = new();
    public string? Nick { get; set; }
    public string? Avatar { get; set; }
    public List<Snowflake> Roles { get; set; } = [];
    public DateTimeOffset? JoinedAt { get; set; }
    public DateTimeOffset? CommunicationDisabledUntil { get; set; }
}

public sealed class GuildEmojisUpdateEvent
{
    public Snowflake GuildId { get; set; }
    public List<Emoji> Emojis { get; set; } = [];
}

public sealed class PresenceUpdateEvent
{
    public PartialUser User { get; set; } = new();
    public Snowflake? GuildId { get; set; }
    public string? Status { get; set; }
    public List<Activity> Activities { get; set; } = [];
}

public sealed class PartialUser
{
    public Snowflake Id { get; set; }
}

public sealed class Activity
{
    public string Name { get; set; } = "";
    public int Type { get; set; }
    public string? State { get; set; }
    public string? Details { get; set; }
    public string? Url { get; set; }
    public Emoji? Emoji { get; set; }
}
