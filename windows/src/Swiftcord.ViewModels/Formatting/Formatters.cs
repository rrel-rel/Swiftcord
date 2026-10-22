using System.Globalization;
using Swiftcord.Core;
using Swiftcord.Core.Models;

namespace Swiftcord.ViewModels.Formatting;

public static class Formatters
{
    /// <summary>Message header timestamps: "Today at 3:04 PM", "Yesterday at …", or a short date and time.</summary>
    public static string MessageTimestamp(DateTimeOffset timestamp, DateTimeOffset now, CultureInfo? culture = null)
    {
        culture ??= CultureInfo.CurrentCulture;
        var local = timestamp.ToLocalTime();
        var today = now.ToLocalTime().Date;
        var time = local.ToString("t", culture);
        if (local.Date == today) return $"Today at {time}";
        if (local.Date == today.AddDays(-1)) return $"Yesterday at {time}";
        return local.ToString("d", culture) + " " + time;
    }

    /// <summary>Formats a &lt;t:…:style&gt; markdown timestamp the way Discord does.</summary>
    public static string MarkdownTimestamp(DateTimeOffset time, char style, DateTimeOffset now, CultureInfo? culture = null)
    {
        culture ??= CultureInfo.CurrentCulture;
        var local = time.ToLocalTime();
        return style switch
        {
            't' => local.ToString("t", culture),
            'T' => local.ToString("T", culture),
            'd' => local.ToString("d", culture),
            'D' => local.ToString("D", culture),
            's' => local.ToString("d", culture) + " " + local.ToString("t", culture),
            'S' => local.ToString("d", culture) + " " + local.ToString("T", culture),
            'F' => local.ToString("dddd, ", culture) + local.ToString("D", culture) + " " + local.ToString("t", culture),
            'R' => Relative(time, now),
            _ => local.ToString("D", culture) + " " + local.ToString("t", culture),
        };
    }

    public static string Relative(DateTimeOffset time, DateTimeOffset now)
    {
        var delta = time - now;
        var future = delta > TimeSpan.Zero;
        var span = delta.Duration();

        string amount;
        if (span < TimeSpan.FromSeconds(45)) amount = "a few seconds";
        else if (span < TimeSpan.FromMinutes(45)) amount = Plural((int)Math.Round(span.TotalMinutes), "minute");
        else if (span < TimeSpan.FromHours(22)) amount = Plural((int)Math.Round(span.TotalHours), "hour");
        else if (span < TimeSpan.FromDays(26)) amount = Plural((int)Math.Round(span.TotalDays), "day");
        else if (span < TimeSpan.FromDays(320)) amount = Plural((int)Math.Round(span.TotalDays / 30.4), "month");
        else amount = Plural((int)Math.Round(span.TotalDays / 365.25), "year");

        return future ? $"in {amount}" : $"{amount} ago";
    }

    /// <summary>"Alice is typing…", "Alice and Bob are typing…", up to three names.</summary>
    public static string TypingText(IReadOnlyList<string> names) => names.Count switch
    {
        0 => "",
        1 => $"{names[0]} is typing…",
        2 => $"{names[0]} and {names[1]} are typing…",
        3 => $"{names[0]}, {names[1]}, and {names[2]} are typing…",
        _ => "Several people are typing…",
    };

    public static string FileSize(long bytes)
    {
        string[] units = ["B", "KB", "MB", "GB"];
        double size = bytes;
        var unit = 0;
        while (size >= 1024 && unit < units.Length - 1)
        {
            size /= 1024;
            unit++;
        }
        return unit == 0 ? $"{bytes} B" : $"{size.ToString("0.##", CultureInfo.CurrentCulture)} {units[unit]}";
    }

    /// <summary>Text for non-chat messages such as joins and pins, or null for regular messages.</summary>
    public static string? SystemMessage(Message message, string authorName, Func<Snowflake, string> userName) => message.Type switch
    {
        MessageType.RecipientAdd => $"{authorName} added {MentionedName(message, userName)} to the group.",
        MessageType.RecipientRemove when message.Mentions.FirstOrDefault()?.Id == message.Author.Id => $"{authorName} left the group.",
        MessageType.RecipientRemove => $"{authorName} removed {MentionedName(message, userName)} from the group.",
        MessageType.Call => $"{authorName} started a call.",
        MessageType.ChannelNameChange => $"{authorName} changed the channel name: {message.Content}",
        MessageType.ChannelIconChange => $"{authorName} changed the channel icon.",
        MessageType.ChannelPinnedMessage => $"{authorName} pinned a message to this channel.",
        MessageType.UserJoin => JoinGreeting(message, authorName),
        MessageType.GuildBoost => $"{authorName} just boosted the server!",
        MessageType.GuildBoostTier1 => $"{authorName} just boosted the server! The server has achieved Level 1!",
        MessageType.GuildBoostTier2 => $"{authorName} just boosted the server! The server has achieved Level 2!",
        MessageType.GuildBoostTier3 => $"{authorName} just boosted the server! The server has achieved Level 3!",
        MessageType.ChannelFollowAdd => $"{authorName} added {message.Content} to this channel.",
        MessageType.ThreadCreated => $"{authorName} started a thread: {message.Content}",
        MessageType.AutoModerationAction => "AutoMod blocked a message.",
        _ => null,
    };

    private static string MentionedName(Message message, Func<Snowflake, string> userName) =>
        message.Mentions.FirstOrDefault() is { } user ? userName(user.Id) : "someone";

    // Discord picks a greeting deterministically from the message's timestamp.
    private static readonly string[] JoinGreetings =
    [
        "{0} joined the party.",
        "{0} is here.",
        "Welcome, {0}. We hope you brought pizza.",
        "A wild {0} appeared.",
        "{0} just landed.",
        "{0} just slid into the server.",
        "{0} just showed up!",
        "Welcome {0}. Say hi!",
        "{0} hopped into the server.",
        "Everyone welcome {0}!",
        "Glad you're here, {0}.",
        "Good to see you, {0}.",
        "Yay you made it, {0}!",
    ];

    private static string JoinGreeting(Message message, string authorName) =>
        string.Format(CultureInfo.CurrentCulture,
            JoinGreetings[message.Timestamp.ToUnixTimeMilliseconds() % JoinGreetings.Length], authorName);

    private static string Plural(int n, string unit) => n == 1 ? $"1 {unit}" : $"{n} {unit}s";
}
