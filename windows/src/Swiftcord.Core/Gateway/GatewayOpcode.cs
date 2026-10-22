namespace Swiftcord.Core.Gateway;

public enum GatewayOpcode
{
    Dispatch = 0,
    Heartbeat = 1,
    Identify = 2,
    PresenceUpdate = 3,
    VoiceStateUpdate = 4,
    Resume = 6,
    Reconnect = 7,
    RequestGuildMembers = 8,
    InvalidSession = 9,
    Hello = 10,
    HeartbeatAck = 11,
    /// <summary>User-account only: subscribe to typing, activity and member-list updates for a guild.</summary>
    LazyGuildRequest = 14,
}

/// <summary>Bot gateway intents. Ignored for user sessions.</summary>
[Flags]
public enum GatewayIntents
{
    None = 0,
    Guilds = 1 << 0,
    GuildMembers = 1 << 1,
    GuildModeration = 1 << 2,
    GuildExpressions = 1 << 3,
    GuildVoiceStates = 1 << 7,
    GuildPresences = 1 << 8,
    GuildMessages = 1 << 9,
    GuildMessageReactions = 1 << 10,
    GuildMessageTyping = 1 << 11,
    DirectMessages = 1 << 12,
    DirectMessageReactions = 1 << 13,
    DirectMessageTyping = 1 << 14,
    MessageContent = 1 << 15,

    /// <summary>Everything a chat client needs without privileged presence/member intents.</summary>
    ChatClient = Guilds | GuildMessages | GuildMessageReactions | GuildMessageTyping | GuildExpressions
        | GuildVoiceStates | DirectMessages | DirectMessageReactions | DirectMessageTyping | MessageContent,
}

public enum GatewayConnectionState
{
    Disconnected,
    Connecting,
    Identifying,
    Resuming,
    Connected,
    /// <summary>Waiting before a reconnect attempt.</summary>
    Reconnecting,
}

/// <summary>Close codes after which reconnecting cannot succeed.</summary>
public static class GatewayCloseCodes
{
    public const int UnknownError = 4000;
    public const int AuthenticationFailed = 4004;
    public const int InvalidSequence = 4007;
    public const int SessionTimedOut = 4009;

    public static bool IsFatal(int code) => code is 4004 or 4010 or 4011 or 4012 or 4013 or 4014;

    /// <summary>The session can't be resumed, but a fresh IDENTIFY may succeed.</summary>
    public static bool RequiresReidentify(int code) => code is InvalidSequence or SessionTimedOut;
}
