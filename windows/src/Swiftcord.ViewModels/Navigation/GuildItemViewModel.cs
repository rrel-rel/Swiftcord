using CommunityToolkit.Mvvm.ComponentModel;
using Swiftcord.Core;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;

namespace Swiftcord.ViewModels.Navigation;

/// <summary>A server icon in the left rail.</summary>
public sealed partial class GuildItemViewModel(Guild guild) : ObservableObject
{
    public Snowflake Id { get; } = guild.Id;

    [ObservableProperty]
    private string _name = guild.Name;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasIcon))]
    private Uri? _iconUri = IconFor(guild);

    [ObservableProperty]
    private string _acronym = guild.Acronym;

    [ObservableProperty]
    private bool _isUnavailable = guild.Unavailable;

    [ObservableProperty]
    private bool _isSelected;

    [ObservableProperty]
    private bool _isUnread;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasMentions))]
    private int _mentionCount;

    public bool HasIcon => IconUri is not null;
    public bool HasMentions => MentionCount > 0;

    public void Update(Guild guild)
    {
        Name = guild.Name;
        IconUri = IconFor(guild);
        Acronym = guild.Acronym;
        IsUnavailable = guild.Unavailable;
    }

    public void RefreshReadState(DiscordState state)
    {
        IsUnread = state.IsGuildUnread(Id);
        MentionCount = state.GetGuildMentionCount(Id);
    }

    private static Uri? IconFor(Guild guild) => Cdn.GuildIcon(guild, 96) is { } url ? new Uri(url) : null;
}
