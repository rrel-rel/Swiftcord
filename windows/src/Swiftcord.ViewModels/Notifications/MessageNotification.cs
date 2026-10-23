using System.Text;
using Swiftcord.Core;
using Swiftcord.Core.Markdown;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;
using Swiftcord.ViewModels.Formatting;

namespace Swiftcord.ViewModels.Notifications;

/// <summary>A desktop notification for a new message; clicking it should open <see cref="ChannelId"/>.</summary>
public sealed record MessageNotification(
    string Title,
    string Body,
    Uri? AvatarUrl,
    Snowflake ChannelId,
    Snowflake? GuildId,
    Snowflake MessageId)
{
    /// <summary>Notifications for the same channel replace each other's group in the notification centre.</summary>
    public string Group => ChannelId.ToString();

    public static MessageNotification Create(Message message, DiscordState state, bool showPreview)
    {
        var channel = state.GetChannel(message.ChannelId);
        var guild = channel?.GuildId is { } guildId ? state.GetGuild(guildId) : null;
        var author = state.GetDisplayName(message.Author.Id, guild?.Id);

        var title = channel switch
        {
            null or { Type: ChannelType.DirectMessage } => author,
            { IsPrivate: true } => $"{author} ({channel.DisplayName})",
            _ => $"{author} (#{channel.Name}, {guild?.Name})",
        };
        var body = showPreview ? Preview(message, state, guild?.Id, author) : "New message";
        return new MessageNotification(title, body, new Uri(Cdn.Avatar(message.Author, 128)),
            message.ChannelId, guild?.Id, message.Id);
    }

    private static string Preview(Message message, DiscordState state, Snowflake? guildId, string author)
    {
        if (Formatters.SystemMessage(message, author, id => state.GetDisplayName(id, guildId)) is { } system)
            return system;

        var text = PlainText(MarkdownParser.Parse(message.Content), state, guildId).Trim();
        if (text.Length > 0)
            return text;
        if (message.StickerItems.Count > 0)
            return $"Sent a sticker: {message.StickerItems[0].Name}";
        return message.Attachments.Count switch
        {
            0 when message.Embeds.Count > 0 => message.Embeds[0].Title ?? message.Embeds[0].Description ?? "Sent an embed",
            0 => "New message",
            1 => $"Sent an attachment: {message.Attachments[0].Filename}",
            var n => $"Sent {n} attachments",
        };
    }

    /// <summary>Flattens markdown to readable text, resolving mentions and hiding spoilers.</summary>
    public static string PlainText(IReadOnlyList<MarkdownNode> nodes, DiscordState state, Snowflake? guildId)
    {
        var builder = new StringBuilder();
        Append(nodes);
        return builder.ToString();

        void Append(IEnumerable<MarkdownNode> children)
        {
            foreach (var node in children)
            {
                switch (node)
                {
                    case TextNode t: builder.Append(t.Text); break;
                    case SpoilerNode: builder.Append("▒▒▒▒"); break;
                    case LinkNode { Children.Count: > 0 } link: Append(link.Children); break;
                    case LinkNode link: builder.Append(link.Url); break;
                    case ContainerNode c: Append(c.Children); break;
                    case ListNode list:
                        foreach (var item in list.Items)
                        {
                            Append(item);
                            builder.Append(' ');
                        }
                        break;
                    case InlineCodeNode code: builder.Append(code.Code); break;
                    case CodeBlockNode code: builder.Append(code.Code); break;
                    case UserMentionNode m: builder.Append('@').Append(state.GetDisplayName(m.UserId, guildId)); break;
                    case RoleMentionNode r:
                        var role = guildId is { } g ? state.GetGuild(g)?.Roles.Find(x => x.Id == r.RoleId) : null;
                        builder.Append('@').Append(role?.Name ?? "deleted-role");
                        break;
                    case ChannelMentionNode c: builder.Append('#').Append(state.GetChannel(c.ChannelId)?.Name ?? "unknown"); break;
                    case EveryoneMentionNode e: builder.Append(e.Here ? "@here" : "@everyone"); break;
                    case CustomEmojiNode e: builder.Append(':').Append(e.Name).Append(':'); break;
                    case TimestampNode t: builder.Append(t.Time.ToLocalTime().ToString("g")); break;
                }
            }
        }
    }
}

/// <summary>Shows desktop notifications; implemented with Windows app notifications.</summary>
public interface INotificationSink
{
    void Show(MessageNotification notification);

    /// <summary>Removes notifications for a channel once it has been read.</summary>
    void Clear(Snowflake channelId);
}
