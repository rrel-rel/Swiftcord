using System.Text.Json.Serialization;
using Swiftcord.Core.Json;

namespace Swiftcord.Core.Models;

public enum MessageType
{
    Default = 0,
    RecipientAdd = 1,
    RecipientRemove = 2,
    Call = 3,
    ChannelNameChange = 4,
    ChannelIconChange = 5,
    ChannelPinnedMessage = 6,
    UserJoin = 7,
    GuildBoost = 8,
    GuildBoostTier1 = 9,
    GuildBoostTier2 = 10,
    GuildBoostTier3 = 11,
    ChannelFollowAdd = 12,
    ThreadCreated = 18,
    Reply = 19,
    ChatInputCommand = 20,
    ThreadStarterMessage = 21,
    ContextMenuCommand = 23,
    AutoModerationAction = 24,
}

[Flags]
public enum MessageFlags
{
    None = 0,
    Crossposted = 1 << 0,
    IsCrosspost = 1 << 1,
    SuppressEmbeds = 1 << 2,
    Urgent = 1 << 4,
    HasThread = 1 << 5,
    Ephemeral = 1 << 6,
    Loading = 1 << 7,
    SuppressNotifications = 1 << 12,
    IsVoiceMessage = 1 << 13,
}

public sealed class Message
{
    public Snowflake Id { get; set; }
    public Snowflake ChannelId { get; set; }
    public Snowflake? GuildId { get; set; }
    public User Author { get; set; } = new();
    public GuildMember? Member { get; set; }
    public string Content { get; set; } = "";
    public DateTimeOffset Timestamp { get; set; }
    public DateTimeOffset? EditedTimestamp { get; set; }
    public bool Tts { get; set; }
    public bool MentionEveryone { get; set; }
    public List<User> Mentions { get; set; } = [];
    public List<Snowflake> MentionRoles { get; set; } = [];
    public List<Attachment> Attachments { get; set; } = [];
    public List<Embed> Embeds { get; set; } = [];
    public List<Reaction> Reactions { get; set; } = [];
    public List<StickerItem> StickerItems { get; set; } = [];
    [JsonConverter(typeof(NonceConverter))]
    public string? Nonce { get; set; }
    public bool Pinned { get; set; }
    public Snowflake? WebhookId { get; set; }
    public MessageType Type { get; set; }
    public MessageFlags Flags { get; set; }
    public MessageReference? MessageReference { get; set; }
    public Message? ReferencedMessage { get; set; }
    /// <summary>For a forwarded message, a copy of the original (without author or ID).</summary>
    public List<MessageSnapshot> MessageSnapshots { get; set; } = [];
    public Channel? Thread { get; set; }

    /// <summary>The copy of the original message when this one is a forward.</summary>
    [JsonIgnore]
    public Message? Forwarded => MessageReference?.Type == MessageReference.ForwardType ? MessageSnapshots.FirstOrDefault()?.Message : null;

    /// <summary>Whether this message renders as a normal chat bubble rather than a system notice.</summary>
    [JsonIgnore]
    public bool IsUserContent => Type is MessageType.Default or MessageType.Reply
        or MessageType.ChatInputCommand or MessageType.ContextMenuCommand or MessageType.ThreadStarterMessage;
}

public sealed class MessageSnapshot
{
    public Message? Message { get; set; }
}

public sealed class MessageReference
{
    public const int DefaultType = 0;
    /// <summary>The reference points at a forwarded message, whose content is in <see cref="Message.MessageSnapshots"/>.</summary>
    public const int ForwardType = 1;

    public int Type { get; set; }
    public Snowflake? MessageId { get; set; }
    public Snowflake? ChannelId { get; set; }
    public Snowflake? GuildId { get; set; }
    public bool? FailIfNotExists { get; set; }
}

public sealed class Attachment
{
    public Snowflake Id { get; set; }
    public string Filename { get; set; } = "";
    public string? Title { get; set; }
    public string? Description { get; set; }
    public string? ContentType { get; set; }
    public long Size { get; set; }
    public string Url { get; set; } = "";
    public string ProxyUrl { get; set; } = "";
    public int? Height { get; set; }
    public int? Width { get; set; }
    public bool Ephemeral { get; set; }
    public double? DurationSecs { get; set; }
    public string? Waveform { get; set; }
    public int Flags { get; set; }

    [JsonIgnore]
    public bool IsImage => ContentType?.StartsWith("image/", StringComparison.Ordinal) == true;

    [JsonIgnore]
    public bool IsVideo => ContentType?.StartsWith("video/", StringComparison.Ordinal) == true;

    [JsonIgnore]
    public bool IsAudio => ContentType?.StartsWith("audio/", StringComparison.Ordinal) == true;

    /// <summary>Spoilered attachments are marked by a SPOILER_ filename prefix (or flag bit 3).</summary>
    [JsonIgnore]
    public bool IsSpoiler => Filename.StartsWith("SPOILER_", StringComparison.Ordinal) || (Flags & (1 << 3)) != 0;
}

public sealed class Reaction
{
    public int Count { get; set; }
    public bool Me { get; set; }
    public bool MeBurst { get; set; }
    public Emoji Emoji { get; set; } = new();
    /// <summary>Hex colours (e.g. "#7fa5d2") for super reactions.</summary>
    public List<string> BurstColors { get; set; } = [];
}

public sealed class Embed
{
    public string? Title { get; set; }
    public string? Type { get; set; }
    public string? Description { get; set; }
    public string? Url { get; set; }
    public DateTimeOffset? Timestamp { get; set; }
    public int? Color { get; set; }
    public EmbedFooter? Footer { get; set; }
    public EmbedMedia? Image { get; set; }
    public EmbedMedia? Thumbnail { get; set; }
    public EmbedMedia? Video { get; set; }
    public EmbedProvider? Provider { get; set; }
    public EmbedAuthor? Author { get; set; }
    public List<EmbedField> Fields { get; set; } = [];
}

public sealed class EmbedFooter
{
    public string Text { get; set; } = "";
    public string? IconUrl { get; set; }
    public string? ProxyIconUrl { get; set; }
}

public sealed class EmbedMedia
{
    public string? Url { get; set; }
    public string? ProxyUrl { get; set; }
    public int? Height { get; set; }
    public int? Width { get; set; }
}

public sealed class EmbedProvider
{
    public string? Name { get; set; }
    public string? Url { get; set; }
}

public sealed class EmbedAuthor
{
    public string Name { get; set; } = "";
    public string? Url { get; set; }
    public string? IconUrl { get; set; }
    public string? ProxyIconUrl { get; set; }
}

public sealed class EmbedField
{
    public string Name { get; set; } = "";
    public string Value { get; set; } = "";
    public bool Inline { get; set; }
}
