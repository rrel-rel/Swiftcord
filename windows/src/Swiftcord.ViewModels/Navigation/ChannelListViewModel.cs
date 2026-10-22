using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using Swiftcord.Core;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;

namespace Swiftcord.ViewModels.Navigation;

public enum ChannelItemKind
{
    Category,
    Text,
    Announcement,
    Forum,
    Voice,
    Stage,
    DirectMessage,
    GroupDirectMessage,
}

/// <summary>A row in the channel sidebar: a category header, a guild channel, or a DM.</summary>
public sealed partial class ChannelItemViewModel : ObservableObject
{
    public ChannelItemViewModel(Channel channel, DiscordState state)
    {
        Id = channel.Id;
        Kind = KindOf(channel);
        IsNsfw = channel.Nsfw;
        Update(channel, state);
    }

    public Snowflake Id { get; }
    public ChannelItemKind Kind { get; }
    public bool IsNsfw { get; }

    public bool IsCategory => Kind == ChannelItemKind.Category;
    public bool IsPrivate => Kind is ChannelItemKind.DirectMessage or ChannelItemKind.GroupDirectMessage;
    public bool IsVoice => Kind is ChannelItemKind.Voice or ChannelItemKind.Stage;
    /// <summary>Whether selecting the row opens a message view.</summary>
    public bool CanOpen => Kind is not (ChannelItemKind.Category or ChannelItemKind.Forum or ChannelItemKind.Stage);

    /// <summary>A Segoe Fluent Icons glyph for guild channels, or "#" for text channels.</summary>
    public string Glyph => Kind switch
    {
        ChannelItemKind.Text => "#",
        ChannelItemKind.Announcement => "\uE789",
        ChannelItemKind.Forum => "\uE8F2",
        ChannelItemKind.Voice => "\uE767",
        ChannelItemKind.Stage => "\uE720",
        _ => "",
    };

    public bool UsesSymbolFont => Kind is not ChannelItemKind.Text;

    [ObservableProperty]
    private string _name = "";

    /// <summary>For DMs: the recipient's custom status or the group size.</summary>
    [ObservableProperty]
    private string? _subtitle;

    [ObservableProperty]
    private Uri? _avatarUri;

    [ObservableProperty]
    private PresenceStatus _presence;

    [ObservableProperty]
    private bool _isSelected;

    [ObservableProperty]
    private bool _isCollapsed;

    [ObservableProperty]
    private bool _isUnread;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasMentions))]
    private int _mentionCount;

    public bool HasMentions => MentionCount > 0;
    public bool HasSubtitle => !string.IsNullOrEmpty(Subtitle);

    /// <summary>The user ID for one-to-one DMs, used for presence.</summary>
    public Snowflake? RecipientId { get; private set; }

    public void Update(Channel channel, DiscordState state)
    {
        Name = Kind == ChannelItemKind.Category ? (channel.Name ?? "").ToUpperInvariant() : channel.DisplayName;
        if (Kind == ChannelItemKind.DirectMessage && channel.Recipients.FirstOrDefault() is { } recipient)
        {
            RecipientId = recipient.Id;
            AvatarUri = new Uri(Cdn.Avatar(recipient, 64));
            Presence = state.GetPresence(recipient.Id);
        }
        else if (Kind == ChannelItemKind.GroupDirectMessage)
        {
            AvatarUri = Cdn.ChannelIcon(channel, 64) is { } icon ? new Uri(icon)
                : channel.Recipients.FirstOrDefault() is { } first ? new Uri(Cdn.Avatar(first, 64)) : null;
            Subtitle = $"{channel.Recipients.Count + 1} Members";
        }
        RefreshReadState(state);
    }

    public void RefreshReadState(DiscordState state)
    {
        IsUnread = !IsCategory && state.IsUnread(Id);
        MentionCount = state.GetMentionCount(Id);
    }

    private static ChannelItemKind KindOf(Channel channel) => channel.Type switch
    {
        ChannelType.GuildCategory => ChannelItemKind.Category,
        ChannelType.GuildAnnouncement => ChannelItemKind.Announcement,
        ChannelType.GuildForum or ChannelType.GuildMedia => ChannelItemKind.Forum,
        ChannelType.GuildVoice => ChannelItemKind.Voice,
        ChannelType.GuildStageVoice => ChannelItemKind.Stage,
        ChannelType.DirectMessage => ChannelItemKind.DirectMessage,
        ChannelType.GroupDirectMessage => ChannelItemKind.GroupDirectMessage,
        _ => ChannelItemKind.Text,
    };
}

/// <summary>The channel sidebar for the selected guild, or the DM list for Home.</summary>
public sealed partial class ChannelListViewModel(DiscordState state) : ObservableObject
{
    private readonly HashSet<Snowflake> _collapsed = [];

    public ObservableCollection<ChannelItemViewModel> Items { get; } = [];

    [ObservableProperty]
    private string _title = "Direct Messages";

    [ObservableProperty]
    private Snowflake? _guildId;

    [ObservableProperty]
    private Snowflake? _selectedChannelId;

    public bool IsDirectMessages => GuildId is null;

    public void ShowDirectMessages()
    {
        GuildId = null;
        Title = "Direct Messages";
        Rebuild();
    }

    public void ShowGuild(Snowflake guildId)
    {
        GuildId = guildId;
        Title = state.GetGuild(guildId)?.Name ?? "";
        Rebuild();
    }

    /// <summary>Rebuilds the list from the cache, e.g. after channel or permission changes.</summary>
    public void Rebuild()
    {
        Items.Clear();
        if (GuildId is not { } guildId)
        {
            foreach (var channel in state.PrivateChannels)
                Items.Add(CreateItem(channel));
            return;
        }

        foreach (var group in state.GetChannelTree(guildId))
        {
            var collapsed = false;
            if (group.Category is { } category)
            {
                collapsed = _collapsed.Contains(category.Id);
                var header = CreateItem(category);
                header.IsCollapsed = collapsed;
                Items.Add(header);
            }
            foreach (var channel in group.Channels)
            {
                var item = CreateItem(channel);
                // Collapsed categories still show the open channel and anything unread, as Discord does.
                if (!collapsed || item.IsSelected || item.IsUnread)
                    Items.Add(item);
            }
        }
    }

    public void ToggleCategory(ChannelItemViewModel category)
    {
        if (!category.IsCategory) return;
        if (!_collapsed.Remove(category.Id))
            _collapsed.Add(category.Id);
        Rebuild();
    }

    public ChannelItemViewModel? Find(Snowflake channelId) => Items.FirstOrDefault(i => i.Id == channelId);

    public void RefreshReadState(Snowflake channelId) => Find(channelId)?.RefreshReadState(state);

    public void RefreshPresence(Snowflake userId, PresenceStatus status)
    {
        foreach (var item in Items.Where(i => i.RecipientId == userId))
            item.Presence = status;
    }

    /// <summary>Moves a DM to the top when it receives a message.</summary>
    public void BumpDirectMessage(Snowflake channelId)
    {
        if (!IsDirectMessages) return;
        if (Find(channelId) is { } item)
        {
            var index = Items.IndexOf(item);
            if (index > 0) Items.Move(index, 0);
        }
        else if (state.GetChannel(channelId) is { IsPrivate: true } channel)
        {
            Items.Insert(0, CreateItem(channel));
        }
    }

    partial void OnSelectedChannelIdChanged(Snowflake? value)
    {
        foreach (var item in Items)
            item.IsSelected = item.Id == value;
    }

    partial void OnGuildIdChanged(Snowflake? value) => OnPropertyChanged(nameof(IsDirectMessages));

    private ChannelItemViewModel CreateItem(Channel channel) =>
        new(channel, state) { IsSelected = channel.Id == SelectedChannelId };
}
