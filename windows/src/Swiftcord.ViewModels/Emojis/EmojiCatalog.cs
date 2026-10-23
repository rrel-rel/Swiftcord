using System.Text.Json;

namespace Swiftcord.ViewModels.Emojis;

/// <summary>A unicode emoji: its fully-qualified text, Discord-style shortcode and search terms.</summary>
public sealed record UnicodeEmoji(string Text, string Name, string Terms, int Group, IReadOnlyList<string>? Skins);

/// <summary>The bundled unicode emoji table (build/make_emoji.py), loaded on first use.</summary>
public static class EmojiCatalog
{
    private static readonly Lazy<(IReadOnlyList<string> Groups, IReadOnlyList<UnicodeEmoji> Emoji)> Data = new(Load);

    public static IReadOnlyList<string> Groups => Data.Value.Groups;
    public static IReadOnlyList<UnicodeEmoji> All => Data.Value.Emoji;

    /// <summary>The emoji for a shortcode, e.g. "joy" → 😂.</summary>
    public static UnicodeEmoji? ByName(string name) =>
        All.FirstOrDefault(e => e.Name == name) ?? All.FirstOrDefault(e => e.Terms.Split(' ').Contains(name));

    /// <summary>Matches on shortcode first (exact, prefix, then anywhere), then on the other names and tags.</summary>
    public static IEnumerable<UnicodeEmoji> Search(string query)
    {
        query = query.Trim().Trim(':').ToLowerInvariant();
        if (query.Length == 0) return [];
        return All.Select(e => (Emoji: e, Rank: e.Name == query ? 0
                : e.Name.StartsWith(query, StringComparison.Ordinal) ? 1
                : e.Name.Contains(query, StringComparison.Ordinal) ? 2
                : e.Terms.Contains(query, StringComparison.Ordinal) ? 3 : -1))
            .Where(x => x.Rank >= 0)
            .OrderBy(x => x.Rank)
            .Select(x => x.Emoji);
    }

    private static (IReadOnlyList<string>, IReadOnlyList<UnicodeEmoji>) Load()
    {
        using var stream = typeof(EmojiCatalog).Assembly.GetManifestResourceStream("Swiftcord.Emoji.json")
            ?? throw new InvalidOperationException("The emoji table isn't embedded.");
        using var doc = JsonDocument.Parse(stream);
        var groups = doc.RootElement.GetProperty("groups").EnumerateArray().Select(g => g.GetString()!).ToList();
        var emoji = doc.RootElement.GetProperty("emoji").EnumerateArray().Select(row => new UnicodeEmoji(
            row[0].GetString()!,
            row[1].GetString()!,
            row[2].GetString()!,
            row[3].GetInt32(),
            row.GetArrayLength() > 4 ? row[4].EnumerateArray().Select(s => s.GetString()!).ToList() : null)).ToList();
        return (groups, emoji);
    }
}
