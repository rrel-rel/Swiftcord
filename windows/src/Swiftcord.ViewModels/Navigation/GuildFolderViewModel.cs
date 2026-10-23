using System.ComponentModel;
using CommunityToolkit.Mvvm.ComponentModel;

namespace Swiftcord.ViewModels.Navigation;

/// <summary>A server folder in the left rail: collapsed to a 2×2 preview, or expanded above its servers.</summary>
public sealed partial class GuildFolderViewModel : ObservableObject, IDisposable
{
    /// <summary>Discord's colour for folders without one.</summary>
    public const int DefaultColor = 0x5865F2;

    public GuildFolderViewModel(long id, string? name, int? color, IReadOnlyList<GuildItemViewModel> guilds, bool isExpanded)
    {
        Id = id;
        Guilds = guilds;
        Color = color is > 0 ? color.Value : DefaultColor;
        Name = string.IsNullOrWhiteSpace(name) ? string.Join(", ", guilds.Select(g => g.Name)) : name;
        _isExpanded = isExpanded;
        foreach (var guild in guilds)
            guild.PropertyChanged += OnGuildChanged;
        Refresh();
    }

    public long Id { get; }
    public string Name { get; }
    public int Color { get; }
    public IReadOnlyList<GuildItemViewModel> Guilds { get; }

    /// <summary>The first four servers, shown as a 2×2 grid while collapsed.</summary>
    public IReadOnlyList<GuildItemViewModel> Previews => Guilds.Take(4).ToList();

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(ShowsSelection), nameof(ShowsUnread), nameof(ShowsMentions))]
    private bool _isExpanded;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(ShowsSelection))]
    private bool _containsSelected;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(ShowsUnread))]
    private bool _isUnread;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(ShowsMentions))]
    private int _mentionCount;

    // While expanded the servers show their own state, so the folder doesn't repeat it.
    public bool ShowsSelection => !IsExpanded && ContainsSelected;
    public bool ShowsUnread => !IsExpanded && IsUnread;
    public bool ShowsMentions => !IsExpanded && MentionCount > 0;

    private void OnGuildChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(GuildItemViewModel.IsSelected) or nameof(GuildItemViewModel.IsUnread)
            or nameof(GuildItemViewModel.MentionCount))
            Refresh();
    }

    private void Refresh()
    {
        ContainsSelected = Guilds.Any(g => g.IsSelected);
        IsUnread = Guilds.Any(g => g.IsUnread);
        MentionCount = Guilds.Sum(g => g.MentionCount);
    }

    public void Dispose()
    {
        foreach (var guild in Guilds)
            guild.PropertyChanged -= OnGuildChanged;
    }
}
