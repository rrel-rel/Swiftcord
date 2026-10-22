using Swiftcord.Core.Models;

namespace Swiftcord.Core;

/// <summary>Builds Discord CDN URLs for avatars, icons, emoji and stickers.</summary>
public static class Cdn
{
    public const string BaseUrl = "https://cdn.discordapp.com";
    public const string MediaUrl = "https://media.discordapp.net";

    private static bool IsAnimated(string hash) => hash.StartsWith("a_", StringComparison.Ordinal);

    /// <param name="size">A power of two between 16 and 4096.</param>
    /// <param name="animate">Use the animated variant when the hash is animated.</param>
    public static string Avatar(User user, int size = 128, bool animate = false) =>
        user.Avatar is { } hash
            ? $"{BaseUrl}/avatars/{user.Id}/{hash}.{Ext(hash, animate)}?size={size}"
            : DefaultAvatar(user);

    /// <summary>Guild-specific avatar, falling back to the user's global avatar.</summary>
    public static string MemberAvatar(Snowflake guildId, User user, GuildMember? member, int size = 128, bool animate = false) =>
        member?.Avatar is { } hash
            ? $"{BaseUrl}/guilds/{guildId}/users/{user.Id}/avatars/{hash}.{Ext(hash, animate)}?size={size}"
            : Avatar(user, size, animate);

    public static string DefaultAvatar(User user)
    {
        // Migrated (pomelo) accounts index by ID; legacy accounts by discriminator.
        var index = user.Discriminator is "0" or ""
            ? (int)((user.Id.Value >> 22) % 6)
            : int.Parse(user.Discriminator) % 5;
        return $"{BaseUrl}/embed/avatars/{index}.png";
    }

    public static string? GuildIcon(Guild guild, int size = 128, bool animate = false) =>
        guild.Icon is { } hash ? $"{BaseUrl}/icons/{guild.Id}/{hash}.{Ext(hash, animate)}?size={size}" : null;

    public static string? GuildBanner(Guild guild, int size = 480) =>
        guild.Banner is { } hash ? $"{BaseUrl}/banners/{guild.Id}/{hash}.{Ext(hash, false)}?size={size}" : null;

    public static string? ChannelIcon(Channel channel, int size = 128) =>
        channel.Icon is { } hash ? $"{BaseUrl}/channel-icons/{channel.Id}/{hash}.png?size={size}" : null;

    public static string Emoji(Snowflake id, bool animated, int size = 48) =>
        $"{BaseUrl}/emojis/{id}.{(animated ? "gif" : "png")}?size={size}";

    public static string? Sticker(StickerItem sticker) => sticker.FormatType switch
    {
        // GIF stickers are only served from the media proxy.
        StickerFormat.Gif => $"{MediaUrl}/stickers/{sticker.Id}.gif",
        StickerFormat.Lottie => $"{BaseUrl}/stickers/{sticker.Id}.json",
        _ => $"{BaseUrl}/stickers/{sticker.Id}.png",
    };

    // PNG rather than WebP: WebP decoding isn't guaranteed on every Windows edition.
    private static string Ext(string hash, bool animate) => animate && IsAnimated(hash) ? "gif" : "png";
}
