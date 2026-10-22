using System.Text.Json.Serialization;
using Swiftcord.Core.Json;

namespace Swiftcord.Core.Models;

public enum ChannelType
{
    GuildText = 0,
    DirectMessage = 1,
    GuildVoice = 2,
    GroupDirectMessage = 3,
    GuildCategory = 4,
    GuildAnnouncement = 5,
    AnnouncementThread = 10,
    PublicThread = 11,
    PrivateThread = 12,
    GuildStageVoice = 13,
    GuildDirectory = 14,
    GuildForum = 15,
    GuildMedia = 16,
}

public enum OverwriteType
{
    Role = 0,
    Member = 1,
}

public sealed class PermissionOverwrite
{
    public Snowflake Id { get; set; }
    public OverwriteType Type { get; set; }
    [JsonConverter(typeof(StringULongConverter))]
    public ulong Allow { get; set; }
    [JsonConverter(typeof(StringULongConverter))]
    public ulong Deny { get; set; }
}

public sealed class Channel
{
    public Snowflake Id { get; set; }
    public ChannelType Type { get; set; }
    public Snowflake? GuildId { get; set; }
    public string? Name { get; set; }
    public string? Topic { get; set; }
    public int Position { get; set; }
    public Snowflake? ParentId { get; set; }
    public bool Nsfw { get; set; }
    public Snowflake? LastMessageId { get; set; }
    public int RateLimitPerUser { get; set; }
    public int? Bitrate { get; set; }
    public int? UserLimit { get; set; }
    public string? Icon { get; set; }
    public Snowflake? OwnerId { get; set; }
    public List<PermissionOverwrite> PermissionOverwrites { get; set; } = [];
    public List<User> Recipients { get; set; } = [];
    /// <summary>Sent instead of <see cref="Recipients"/> when user objects are deduplicated in READY.</summary>
    public List<Snowflake>? RecipientIds { get; set; }

    [JsonIgnore]
    public bool IsPrivate => Type is ChannelType.DirectMessage or ChannelType.GroupDirectMessage;

    [JsonIgnore]
    public bool IsThread => Type is ChannelType.AnnouncementThread or ChannelType.PublicThread or ChannelType.PrivateThread;

    [JsonIgnore]
    public bool IsVoice => Type is ChannelType.GuildVoice or ChannelType.GuildStageVoice;

    /// <summary>Channels that have a message history the user can open.</summary>
    [JsonIgnore]
    public bool IsTextBased => Type is ChannelType.GuildText or ChannelType.DirectMessage
        or ChannelType.GroupDirectMessage or ChannelType.GuildAnnouncement or ChannelType.GuildVoice
        or ChannelType.AnnouncementThread or ChannelType.PublicThread or ChannelType.PrivateThread;

    /// <summary>Human-readable label: the channel name, or the recipients' names for DMs.</summary>
    [JsonIgnore]
    public string DisplayName => !string.IsNullOrEmpty(Name)
        ? Name
        : Recipients.Count > 0 ? string.Join(", ", Recipients.Select(r => r.DisplayName)) : "Unnamed";
}
