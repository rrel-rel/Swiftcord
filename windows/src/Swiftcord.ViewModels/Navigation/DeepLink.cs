using Swiftcord.Core;

namespace Swiftcord.ViewModels.Navigation;

/// <summary>
/// A link to a channel (and optionally a message): <c>swiftcord://channels/{guild or @me}/{channel}[/{message}]</c>,
/// or the equivalent https://discord.com/channels/… URL.
/// </summary>
public sealed record DeepLink(Snowflake? GuildId, Snowflake ChannelId, Snowflake? MessageId = null)
{
    public const string Scheme = "swiftcord";

    private static readonly string[] DiscordHosts = ["discord.com", "ptb.discord.com", "canary.discord.com", "discordapp.com"];

    public static bool TryParse(string? text, out DeepLink link)
    {
        link = null!;
        return Uri.TryCreate(text?.Trim(), UriKind.Absolute, out var uri) && TryParse(uri, out link);
    }

    public static bool TryParse(Uri uri, out DeepLink link)
    {
        link = null!;
        // swiftcord://channels/a/b puts "channels" in the host; https://discord.com/channels/a/b in the path.
        string[] parts;
        if (uri.Scheme.Equals(Scheme, StringComparison.OrdinalIgnoreCase))
            parts = [uri.Host, .. Segments(uri)];
        else if (uri.Scheme == Uri.UriSchemeHttps && DiscordHosts.Contains(uri.Host, StringComparer.OrdinalIgnoreCase))
            parts = Segments(uri);
        else
            return false;

        if (parts.Length is < 3 or > 4 || !parts[0].Equals("channels", StringComparison.OrdinalIgnoreCase))
            return false;

        Snowflake? guild = null;
        if (parts[1] != "@me")
        {
            if (!Snowflake.TryParse(parts[1], out var g)) return false;
            guild = g;
        }
        if (!Snowflake.TryParse(parts[2], out var channel))
            return false;
        Snowflake? message = null;
        if (parts.Length == 4)
        {
            if (!Snowflake.TryParse(parts[3], out var m)) return false;
            message = m;
        }
        link = new DeepLink(guild, channel, message);
        return true;
    }

    private static string[] Segments(Uri uri) =>
        uri.AbsolutePath.Split('/', StringSplitOptions.RemoveEmptyEntries).Select(Uri.UnescapeDataString).ToArray();

    public Uri ToUri() => new($"{Scheme}://channels/{(GuildId is { } g ? g.ToString() : "@me")}/{ChannelId}{(MessageId is { } m ? "/" + m : "")}");
}
