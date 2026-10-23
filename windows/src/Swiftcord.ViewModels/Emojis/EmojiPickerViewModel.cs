using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using Swiftcord.Core;
using Swiftcord.Core.State;
using DiscordEmoji = Swiftcord.Core.Models.Emoji;

namespace Swiftcord.ViewModels.Emojis;

/// <summary>One emoji in the picker: unicode (<see cref="Text"/>) or custom (<see cref="ImageUri"/>).</summary>
public sealed record EmojiItem(string Name, string? Text, Uri? ImageUri, DiscordEmoji Emoji, bool IsLocked)
{
    public bool IsCustom => ImageUri is not null;

    /// <summary>What to type into a message to send this emoji.</summary>
    public string MessageText => Emoji.Id is { } id ? $"<{(Emoji.Animated ? "a" : "")}:{Name}:{id}>" : Text ?? "";

    public string ToolTip => $":{Name}:{(IsLocked ? " (needs Nitro here)" : "")}";
}

/// <summary>
/// A titled block of emoji: a server's custom emoji, or a unicode category. It is itself the list of items,
/// which is the shape a grouped, virtualized grid binds to.
/// </summary>
public sealed class EmojiSection(string title, IEnumerable<EmojiItem> items) : List<EmojiItem>(items)
{
    public string Title { get; } = title;
}

/// <summary>
/// The reaction / composer emoji picker. Shows recently used, this server's emoji, other servers' emoji
/// (locked without Nitro) and the unicode categories; typing filters everything into one list.
/// </summary>
public sealed partial class EmojiPickerViewModel : ObservableObject
{
    private const int RecentLimit = 24;
    /// <summary>Recently picked emoji, shared by every picker for the session.</summary>
    private static readonly List<EmojiItem> Recent = [];

    private readonly IReadOnlyList<EmojiItem> _all;

    public EmojiPickerViewModel(DiscordState state, Snowflake? guildId)
    {
        var nitro = state.CurrentUser?.PremiumType is > 0;
        var sections = new List<EmojiSection>();
        if (Recent.Count > 0)
            sections.Add(new EmojiSection("Recently used", [.. Recent]));

        // This server first, then the others in rail order.
        var guilds = state.Guilds.OrderBy(g => g.Id == guildId ? 0 : 1).ToList();
        foreach (var guild in guilds)
        {
            var items = guild.Emojis.Where(e => e.Available && e.Id is not null && e.Name is not null)
                .Select(e => new EmojiItem(e.Name!, null, new Uri(Cdn.Emoji(e.Id!.Value, e.Animated, 48)), e,
                    IsLocked: !nitro && guild.Id != guildId))
                .ToList();
            if (items.Count > 0)
                sections.Add(new EmojiSection(guild.Name, items));
        }

        var unicode = EmojiCatalog.All.Select(e => new EmojiItem(e.Name, e.Text, null, new DiscordEmoji { Name = e.Text }, false)).ToList();
        foreach (var group in unicode.Zip(EmojiCatalog.All).GroupBy(x => x.Second.Group).OrderBy(g => g.Key))
            sections.Add(new EmojiSection(EmojiCatalog.Groups[group.Key], group.Select(x => x.First).ToList()));

        Sections = sections;
        _all = sections.Where(s => s.Title != "Recently used").SelectMany(s => s).ToList();
    }

    public IReadOnlyList<EmojiSection> Sections { get; }

    /// <summary>Search results while <see cref="Query"/> isn't empty.</summary>
    public ObservableCollection<EmojiItem> Results { get; } = [];

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsSearching))]
    private string _query = "";

    public bool IsSearching => Query.Trim().Length > 0;
    public bool HasNoResults => Results.Count == 0;

    /// <summary>Raised when an emoji is picked (not for locked ones).</summary>
    public event Action<EmojiItem>? Picked;

    partial void OnQueryChanged(string value)
    {
        Results.Clear();
        var query = value.Trim().Trim(':').ToLowerInvariant();
        if (query.Length == 0)
        {
            OnPropertyChanged(nameof(HasNoResults));
            return;
        }
        var custom = _all.Where(e => e.IsCustom && e.Name.Contains(query, StringComparison.OrdinalIgnoreCase))
            .OrderBy(e => e.Name.StartsWith(query, StringComparison.OrdinalIgnoreCase) ? 0 : 1);
        var unicode = EmojiCatalog.Search(query).Select(e => new EmojiItem(e.Name, e.Text, null, new DiscordEmoji { Name = e.Text }, false));
        foreach (var item in custom.OrderBy(e => e.IsLocked).Concat(unicode).Take(200))
            Results.Add(item);
        OnPropertyChanged(nameof(HasNoResults));
    }

    /// <summary>The first search result, for Enter in the search box.</summary>
    public EmojiItem? FirstResult => Results.FirstOrDefault(r => !r.IsLocked);

    public bool Pick(EmojiItem item)
    {
        if (item.IsLocked) return false;
        Recent.RemoveAll(r => r.Name == item.Name && r.Emoji.Id == item.Emoji.Id);
        Recent.Insert(0, item);
        if (Recent.Count > RecentLimit) Recent.RemoveAt(Recent.Count - 1);
        Picked?.Invoke(item);
        return true;
    }
}
