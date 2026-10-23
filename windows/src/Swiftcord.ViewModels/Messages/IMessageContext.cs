using Swiftcord.Core;

namespace Swiftcord.ViewModels.Messages;

/// <summary>
/// What the markdown renderer needs to turn mentions, emoji and timestamps into display text.
/// Implemented by the chat view model for the channel being shown.
/// </summary>
public interface IMessageContext
{
    string UserName(Snowflake userId);

    string ChannelName(Snowflake channelId);

    /// <summary>The role's name and colour (0 for the default colour).</summary>
    (string Name, int Color) Role(Snowflake roleId);

    Uri EmojiUri(Snowflake emojiId, bool animated, int size);

    string FormatTimestamp(DateTimeOffset time, char style);

    /// <summary>Opens a channel the user clicked a mention of.</summary>
    void OpenChannel(Snowflake channelId);

    /// <summary>Follows a link to a Discord channel or message (jumping within this channel when it's here).</summary>
    void OpenLink(Snowflake channelId, Snowflake? messageId);
}
