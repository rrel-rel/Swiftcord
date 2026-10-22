namespace Swiftcord.Core;

public enum TokenType
{
    /// <summary>A user account token, sent as-is in the Authorization header.</summary>
    User,
    /// <summary>A bot token, sent with a "Bot " prefix. Useful for development and testing.</summary>
    Bot,
}

public sealed class DiscordClientOptions
{
    public required string Token { get; init; }
    public TokenType TokenType { get; init; } = TokenType.User;
    public Uri ApiBaseUrl { get; init; } = new("https://discord.com/api/v10/");
    public Uri GatewayUrl { get; init; } = new("wss://gateway.discord.gg/");
    public string UserAgent { get; init; } = $"Swiftcord-Windows/{typeof(DiscordClientOptions).Assembly.GetName().Version?.ToString(3)}";
    /// <summary>BCP-47 locale sent with requests, e.g. "en-US".</summary>
    public string Locale { get; init; } = "en-US";
    /// <summary>Gateway capability flags for user sessions. 0 requests the legacy, non-deduplicated READY format.</summary>
    public int GatewayCapabilities { get; init; }
    /// <summary>Maximum number of messages kept in memory per channel.</summary>
    public int MessageCacheSize { get; init; } = 250;

    internal string AuthorizationHeader => TokenType == TokenType.Bot ? $"Bot {Token}" : Token;
}
