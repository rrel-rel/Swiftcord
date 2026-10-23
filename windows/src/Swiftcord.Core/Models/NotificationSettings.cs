namespace Swiftcord.Core.Models;

public enum MessageNotificationLevel
{
    AllMessages = 0,
    OnlyMentions = 1,
    NoMessages = 2,
    /// <summary>Use the parent's setting (category, then server, then the server default).</summary>
    Inherit = 3,
}

public sealed class MuteConfig
{
    /// <summary>When a timed mute ends; null for "until I turn it back on".</summary>
    public DateTimeOffset? EndTime { get; set; }
}

/// <summary>The user's notification settings for one server (or, with no <see cref="GuildId"/>, for DMs).</summary>
public sealed class UserGuildSettings
{
    public Snowflake? GuildId { get; set; }
    public MessageNotificationLevel MessageNotifications { get; set; } = MessageNotificationLevel.Inherit;
    public bool Muted { get; set; }
    public MuteConfig? MuteConfig { get; set; }
    public bool SuppressEveryone { get; set; }
    public bool SuppressRoles { get; set; }
    public List<ChannelNotificationOverride> ChannelOverrides { get; set; } = [];

    public bool IsMuted(DateTimeOffset now) => Muted && (MuteConfig?.EndTime is not { } end || end > now);
}

public sealed class ChannelNotificationOverride
{
    public Snowflake ChannelId { get; set; }
    public MessageNotificationLevel MessageNotifications { get; set; } = MessageNotificationLevel.Inherit;
    public bool Muted { get; set; }
    public MuteConfig? MuteConfig { get; set; }

    public bool IsMuted(DateTimeOffset now) => Muted && (MuteConfig?.EndTime is not { } end || end > now);
}
