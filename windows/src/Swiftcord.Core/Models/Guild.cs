using System.Text.Json.Serialization;
using Swiftcord.Core.Json;

namespace Swiftcord.Core.Models;

public sealed class Guild
{
    public Snowflake Id { get; set; }
    public string Name { get; set; } = "";
    public string? Icon { get; set; }
    public string? Banner { get; set; }
    public string? Splash { get; set; }
    public string? Description { get; set; }
    public Snowflake OwnerId { get; set; }
    public bool Unavailable { get; set; }
    public int MemberCount { get; set; }
    public int PremiumTier { get; set; }
    public Snowflake? SystemChannelId { get; set; }
    public Snowflake? RulesChannelId { get; set; }
    public List<string> Features { get; set; } = [];
    public List<Role> Roles { get; set; } = [];
    public List<Emoji> Emojis { get; set; } = [];
    public List<Sticker> Stickers { get; set; } = [];
    public List<Channel> Channels { get; set; } = [];
    public List<Channel> Threads { get; set; } = [];
    public List<GuildMember> Members { get; set; } = [];

    /// <summary>Initials shown in place of a missing icon, e.g. "Swift Community" → "SC".</summary>
    [JsonIgnore]
    public string Acronym => string.Concat(Name.Split(' ', StringSplitOptions.RemoveEmptyEntries).Select(w => w[0]));
}

public sealed class Role
{
    public Snowflake Id { get; set; }
    public string Name { get; set; } = "";
    public int Color { get; set; }
    public bool Hoist { get; set; }
    public string? Icon { get; set; }
    public string? UnicodeEmoji { get; set; }
    public int Position { get; set; }
    [JsonConverter(typeof(StringULongConverter))]
    public ulong Permissions { get; set; }
    public bool Managed { get; set; }
    public bool Mentionable { get; set; }
}

public sealed class GuildMember
{
    public User? User { get; set; }
    /// <summary>Present in READY's merged_members, where the user object is omitted.</summary>
    public Snowflake? UserId { get; set; }
    public string? Nick { get; set; }
    public string? Avatar { get; set; }
    public List<Snowflake> Roles { get; set; } = [];
    public DateTimeOffset? JoinedAt { get; set; }
    public DateTimeOffset? PremiumSince { get; set; }
    public DateTimeOffset? CommunicationDisabledUntil { get; set; }
    public bool Deaf { get; set; }
    public bool Mute { get; set; }
    public bool Pending { get; set; }

    [JsonIgnore]
    public Snowflake? EffectiveUserId => User?.Id ?? UserId;
}

public sealed class Emoji
{
    /// <summary>Null for unicode emoji.</summary>
    public Snowflake? Id { get; set; }
    public string? Name { get; set; }
    public bool Animated { get; set; }
    public bool Available { get; set; } = true;
    public List<Snowflake> Roles { get; set; } = [];

    /// <summary>The form used in REST reaction routes: "name:id" for custom emoji, raw unicode otherwise.</summary>
    [JsonIgnore]
    public string ReactionKey => Id is { } id ? $"{Name}:{id}" : Name ?? "";
}

public enum StickerFormat
{
    Png = 1,
    Apng = 2,
    Lottie = 3,
    Gif = 4,
}

public sealed class Sticker
{
    public Snowflake Id { get; set; }
    public string Name { get; set; } = "";
    public string? Description { get; set; }
    public StickerFormat FormatType { get; set; }
    public Snowflake? GuildId { get; set; }
}

public sealed class StickerItem
{
    public Snowflake Id { get; set; }
    public string Name { get; set; } = "";
    public StickerFormat FormatType { get; set; }
}
