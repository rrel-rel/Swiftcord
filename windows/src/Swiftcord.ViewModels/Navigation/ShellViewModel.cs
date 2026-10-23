using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Gateway;
using Swiftcord.Core.Models;
using Swiftcord.Core.Permissions;
using Swiftcord.ViewModels.Messages;
using Swiftcord.ViewModels.Notifications;
using Swiftcord.ViewModels.Settings;

namespace Swiftcord.ViewModels.Navigation;

/// <summary>
/// The logged-in window: server rail, channel sidebar, open chat and user panel. Translates
/// state-cache events (raised on the gateway thread) into UI-thread updates.
/// </summary>
public sealed partial class ShellViewModel : ObservableObject, IDisposable
{
    /// <summary>Key for remembering the last DM opened from Home.</summary>
    private static readonly Snowflake HomeKey = default;

    private readonly DiscordClient _client;
    private readonly IUiDispatcher _ui;
    private readonly TimeProvider _time;
    private readonly Dictionary<Snowflake, Snowflake> _lastChannelByGuild = [];
    private readonly HashSet<Snowflake> _subscribedGuilds = [];
    private bool _windowActive = true;
    private bool _disposed;
    private DeepLink? _pendingLink;
    /// <summary>A message to show once its channel opens (switching servers first goes through SelectGuild).</summary>
    private (Snowflake Channel, Snowflake Message)? _pendingJump;
    private readonly HashSet<long> _expandedFolders = [];

    public ShellViewModel(DiscordClient client, IUiDispatcher ui, TimeProvider? time = null)
    {
        _client = client;
        _ui = ui;
        _time = time ?? TimeProvider.System;
        Sidebar = new ChannelListViewModel(client.State);

        var state = client.State;
        state.Ready += OnReady;
        state.GuildAvailable += OnGuildAvailable;
        state.GuildFoldersChanged += OnGuildFoldersChanged;
        state.GuildUpdated += OnGuildUpdated;
        state.GuildRemoved += OnGuildRemoved;
        state.ChannelCreated += OnChannelChanged;
        state.ChannelUpdated += OnChannelChanged;
        state.ChannelDeleted += OnChannelDeleted;
        state.PermissionsChanged += OnPermissionsChanged;
        state.ReadStateChanged += OnReadStateChanged;
        state.MessageCreated += OnMessageCreated;
        state.PresenceChanged += OnPresenceChanged;
        state.CurrentUserUpdated += OnCurrentUserUpdated;
        client.Gateway.StateChanged += OnGatewayStateChanged;
        client.Gateway.FatalClose += OnFatalClose;

        // The cache may already be populated if READY arrived before the shell was created.
        if (state.CurrentUser is not null)
            LoadFromState();
    }

    /// <summary>Where desktop notifications go; none in tests or when unsupported.</summary>
    public INotificationSink? Notifications { get; init; }

    /// <summary>App preferences, shown in the settings overlay.</summary>
    public SettingsViewModel? Settings { get; init; }

    /// <summary>Every server, in the user's saved order.</summary>
    public ObservableCollection<GuildItemViewModel> Guilds { get; } = [];

    /// <summary>What the rail shows: lone servers and folders, with an expanded folder followed by its servers.</summary>
    public ObservableCollection<object> RailItems { get; } = [];
    public ChannelListViewModel Sidebar { get; }

    [ObservableProperty]
    private GuildItemViewModel? _selectedGuild;

    [ObservableProperty]
    private bool _isHomeSelected = true;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasChat))]
    private ChatViewModel? _chat;

    [ObservableProperty]
    private bool _isReady;

    [ObservableProperty]
    private bool _isSettingsOpen;

    [ObservableProperty]
    private bool _isConnected;

    [ObservableProperty]
    private string _connectionStatus = "Connecting…";

    [ObservableProperty]
    private string _currentUserName = "";

    [ObservableProperty]
    private string _currentUserTag = "";

    [ObservableProperty]
    private Uri? _currentUserAvatar;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HomeHasMentions))]
    private int _homeMentionCount;

    public bool HasChat => Chat is not null;
    public bool HomeHasMentions => HomeMentionCount > 0;

    /// <summary>Raised when the token is rejected and the user must sign in again.</summary>
    public event Action? SessionExpired;

    /// <summary>Raised when the user chooses to log out.</summary>
    public event Action? LogoutRequested;

    /// <summary>Tell the shell when the window gains or loses focus, so reads are only acknowledged while visible.</summary>
    public void SetWindowActive(bool active)
    {
        _windowActive = active;
        if (Chat is { } chat) chat.IsActive = active;
    }

    #region Commands

    [RelayCommand]
    private void SelectHome()
    {
        if (SelectedGuild is { } previous) previous.IsSelected = false;
        SelectedGuild = null;
        IsHomeSelected = true;
        Sidebar.ShowDirectMessages();

        if (_lastChannelByGuild.TryGetValue(HomeKey, out var last) && _client.State.GetChannel(last) is not null)
            OpenChannel(last);
        else
            CloseChat();
    }

    [RelayCommand]
    private void SelectGuild(GuildItemViewModel guild)
    {
        if (guild.IsUnavailable) return;
        if (SelectedGuild is { } previous) previous.IsSelected = false;
        guild.IsSelected = true;
        SelectedGuild = guild;
        IsHomeSelected = false;
        Sidebar.ShowGuild(guild.Id);
        _ = SubscribeAsync(guild.Id);

        var target = _lastChannelByGuild.TryGetValue(guild.Id, out var last) && Sidebar.Find(last) is { CanOpen: true }
            ? last
            : DefaultChannel(guild.Id);
        if (target is { } id) OpenChannel(id);
        else CloseChat();
    }

    [RelayCommand]
    private void ToggleFolder(GuildFolderViewModel folder)
    {
        var index = RailItems.IndexOf(folder);
        if (index < 0) return;
        folder.IsExpanded = !folder.IsExpanded;
        if (folder.IsExpanded)
        {
            _expandedFolders.Add(folder.Id);
            for (var i = 0; i < folder.Guilds.Count; i++)
            {
                PlaceInFolder(folder.Guilds[i], folder, i);
                RailItems.Insert(index + 1 + i, folder.Guilds[i]);
            }
        }
        else
        {
            _expandedFolders.Remove(folder.Id);
            foreach (var guild in folder.Guilds)
            {
                RailItems.Remove(guild);
                PlaceInFolder(guild, null, 0);
            }
        }
    }

    [RelayCommand]
    private void ActivateChannel(ChannelItemViewModel item)
    {
        if (item.IsCategory)
            Sidebar.ToggleCategory(item);
        else if (item.CanOpen)
            OpenChannel(item.Id);
    }

    [RelayCommand]
    private void Logout() => LogoutRequested?.Invoke();

    [RelayCommand]
    private void OpenSettings() => IsSettingsOpen = true;

    [RelayCommand]
    private void CloseSettings() => IsSettingsOpen = false;

    /// <summary>
    /// Opens the channel a link or notification points at. Before READY the link is kept and followed
    /// once the cache is loaded (e.g. when a notification click launched the app).
    /// </summary>
    public void Navigate(DeepLink link)
    {
        IsSettingsOpen = false;
        if (!IsReady)
        {
            _pendingLink = link;
            return;
        }
        OpenChannel(link.ChannelId, link.MessageId);
    }

    /// <summary>Opens a channel by ID, switching guilds if needed (e.g. from a #channel mention).</summary>
    public void OpenChannel(Snowflake channelId) => OpenChannel(channelId, null);

    /// <summary>Opens a channel, scrolled to <paramref name="messageId"/> when given.</summary>
    public void OpenChannel(Snowflake channelId, Snowflake? messageId)
    {
        if (messageId is { } target) _pendingJump = (channelId, target);
        var channel = _client.State.GetChannel(channelId);
        if (channel is null || !channel.IsTextBased) return;

        if (channel.GuildId is { } guildId && SelectedGuild?.Id != guildId)
        {
            if (Guilds.FirstOrDefault(g => g.Id == guildId) is not { } guild) return;
            _lastChannelByGuild[guildId] = channelId;
            SelectGuild(guild);
            return;
        }
        if (channel.IsPrivate && !IsHomeSelected)
        {
            _lastChannelByGuild[HomeKey] = channelId;
            SelectHome();
            return;
        }

        Notifications?.Clear(channelId);
        Snowflake? jumpTo = _pendingJump is { } jump && jump.Channel == channelId ? jump.Message : null;
        _pendingJump = null;
        if (Chat?.ChannelId == channelId)
        {
            if (jumpTo is { } id) _ = Chat.JumpToMessageAsync(id);
            return;
        }
        _lastChannelByGuild[channel.GuildId ?? HomeKey] = channelId;
        Sidebar.SelectedChannelId = channelId;

        Chat?.Dispose();
        var chat = new ChatViewModel(_client, _ui, channel, OpenChannel, _time) { IsActive = _windowActive };
        Chat = chat;
        _ = chat.InitializeAsync(jumpTo);
    }

    #endregion

    #region State events (background thread)

    private void OnReady() => _ui.Post(LoadFromState);

    private void LoadFromState()
    {
        if (_disposed) return;
        var state = _client.State;
        IsReady = true;
        UpdateCurrentUser(state.CurrentUser);

        var selectedId = SelectedGuild?.Id;
        Guilds.Clear();
        foreach (var guild in state.Guilds)
            Guilds.Add(CreateGuildItem(guild));
        RebuildRail();
        RefreshHomeBadge();

        // After a reconnect the cache is rebuilt from scratch, so reopen what was showing.
        var reopen = Chat?.ChannelId;
        Chat?.Dispose();
        Chat = null;
        _subscribedGuilds.Clear();

        if (selectedId is { } id && Guilds.FirstOrDefault(g => g.Id == id) is { } selected)
        {
            if (reopen is { } r) _lastChannelByGuild[id] = r;
            SelectGuild(selected);
        }
        else
        {
            if (reopen is { } r) _lastChannelByGuild[HomeKey] = r;
            SelectHome();
        }

        if (_pendingLink is { } link)
        {
            _pendingLink = null;
            Navigate(link);
        }
    }

    private void OnGuildAvailable(Guild guild) => _ui.Post(() =>
    {
        if (_disposed) return;
        if (Guilds.FirstOrDefault(g => g.Id == guild.Id) is { } existing)
            existing.Update(guild);
        else
            Guilds.Add(CreateGuildItem(guild));
        SortGuilds();
        RebuildRail();
    });

    private void OnGuildFoldersChanged() => _ui.Post(() =>
    {
        if (_disposed) return;
        SortGuilds();
        RebuildRail();
    });

    /// <summary>Moves the server items into the user's saved order, keeping the existing items (and selection).</summary>
    private void SortGuilds()
    {
        var order = _client.State.Guilds.Select((g, i) => (g.Id, i)).ToDictionary(x => x.Id, x => x.i);
        var sorted = Guilds.OrderBy(g => order.GetValueOrDefault(g.Id, int.MaxValue)).ToList();
        for (var i = 0; i < sorted.Count; i++)
        {
            var from = Guilds.IndexOf(sorted[i]);
            if (from != i)
                Guilds.Move(from, i);
        }
    }

    private void OnGuildUpdated(Guild guild) => _ui.Post(() =>
    {
        if (_disposed) return;
        Guilds.FirstOrDefault(g => g.Id == guild.Id)?.Update(guild);
        if (Sidebar.GuildId == guild.Id) Sidebar.Title = guild.Name;
    });

    private void OnGuildRemoved(Snowflake guildId, bool unavailable) => _ui.Post(() =>
    {
        if (_disposed || Guilds.FirstOrDefault(g => g.Id == guildId) is not { } item) return;
        if (unavailable)
        {
            item.IsUnavailable = true;
            return;
        }
        Guilds.Remove(item);
        RebuildRail();
        _lastChannelByGuild.Remove(guildId);
        if (SelectedGuild?.Id == guildId) SelectHome();
    });

    private void OnChannelChanged(Channel channel) => _ui.Post(() =>
    {
        if (_disposed) return;
        if (Sidebar.GuildId == channel.GuildId) Sidebar.Rebuild();
    });

    private void OnChannelDeleted(Channel channel) => _ui.Post(() =>
    {
        if (_disposed) return;
        if (Sidebar.GuildId == channel.GuildId) Sidebar.Rebuild();
        if (Chat?.ChannelId == channel.Id) CloseChat();
    });

    private void OnPermissionsChanged(Snowflake guildId) => _ui.Post(() =>
    {
        if (_disposed || Sidebar.GuildId != guildId) return;
        Sidebar.Rebuild();
        // Losing access to the open channel closes it.
        if (Chat is { } chat && !_client.State.GetPermissions(chat.ChannelId).HasFlag(Permission.ViewChannel))
            CloseChat();
    });

    private void OnReadStateChanged(Snowflake channelId) => _ui.Post(() =>
    {
        if (_disposed) return;
        Sidebar.RefreshReadState(channelId);
        var channel = _client.State.GetChannel(channelId);
        if (channel?.GuildId is { } guildId)
            Guilds.FirstOrDefault(g => g.Id == guildId)?.RefreshReadState(_client.State);
        else
            RefreshHomeBadge();
    });

    private void OnMessageCreated(Message message)
    {
        // Decide on the gateway thread, against the cache as of this message.
        var notify = Notifications is not null && Settings?.Current.NotificationsEnabled != false && _client.State.ShouldNotify(message);
        if (message.GuildId is null || notify)
            _ui.Post(() =>
            {
                if (_disposed) return;
                if (message.GuildId is null)
                    Sidebar.BumpDirectMessage(message.ChannelId);
                // Don't notify for the conversation the user is looking at.
                if (notify && !(_windowActive && Chat?.ChannelId == message.ChannelId))
                    Notifications!.Show(MessageNotification.Create(message, _client.State, Settings?.Current.NotificationPreviews ?? true));
            });
    }

    private void OnPresenceChanged(Snowflake userId, PresenceStatus status) => _ui.Post(() =>
    {
        if (!_disposed) Sidebar.RefreshPresence(userId, status);
    });

    private void OnCurrentUserUpdated(User user) => _ui.Post(() => UpdateCurrentUser(user));

    private void OnGatewayStateChanged(GatewayConnectionState state) => _ui.Post(() =>
    {
        IsConnected = state == GatewayConnectionState.Connected;
        ConnectionStatus = state switch
        {
            GatewayConnectionState.Connected => "Connected",
            GatewayConnectionState.Reconnecting => "Reconnecting…",
            GatewayConnectionState.Resuming => "Resuming…",
            GatewayConnectionState.Disconnected => "Disconnected",
            _ => "Connecting…",
        };
    });

    private void OnFatalClose(int code, string? reason) => _ui.Post(() =>
    {
        if (code == GatewayCloseCodes.AuthenticationFailed)
            SessionExpired?.Invoke();
        else
            ConnectionStatus = $"Disconnected: {reason ?? code.ToString()}";
    });

    #endregion

    #region Helpers

    /// <summary>Lays out <see cref="RailItems"/> from the saved folders, keeping the existing item view models.</summary>
    private void RebuildRail()
    {
        foreach (var old in RailItems.OfType<GuildFolderViewModel>())
            old.Dispose();
        RailItems.Clear();

        var byId = Guilds.ToDictionary(g => g.Id);
        var placed = new HashSet<Snowflake>();
        foreach (var folder in _client.State.GuildFolders ?? [])
        {
            var guilds = folder.GuildIds.Select(id => byId.GetValueOrDefault(id)).OfType<GuildItemViewModel>()
                .Where(g => placed.Add(g.Id)).ToList();
            if (guilds.Count == 0) continue;

            if (folder.Id is not { } folderId)
            {
                // Lone servers are stored as folders without an ID.
                foreach (var guild in guilds)
                {
                    PlaceInFolder(guild, null, 0);
                    RailItems.Add(guild);
                }
                continue;
            }

            var item = new GuildFolderViewModel(folderId, folder.Name, folder.Color, guilds, _expandedFolders.Contains(folderId));
            RailItems.Add(item);
            for (var i = 0; i < guilds.Count; i++)
            {
                PlaceInFolder(guilds[i], item.IsExpanded ? item : null, i);
                if (item.IsExpanded) RailItems.Add(guilds[i]);
            }
        }

        // Servers the saved order doesn't cover (or all of them, before the settings arrive).
        foreach (var guild in Guilds.Where(g => !placed.Contains(g.Id)))
        {
            PlaceInFolder(guild, null, 0);
            RailItems.Add(guild);
        }
    }

    private static void PlaceInFolder(GuildItemViewModel guild, GuildFolderViewModel? folder, int index)
    {
        guild.FolderColor = folder?.Color ?? -1;
        guild.IsLastInFolder = folder is not null && index == folder.Guilds.Count - 1;
    }

    private GuildItemViewModel CreateGuildItem(Guild guild)
    {
        var item = new GuildItemViewModel(guild) { IsSelected = guild.Id == SelectedGuild?.Id };
        item.RefreshReadState(_client.State);
        return item;
    }

    private void UpdateCurrentUser(User? user)
    {
        if (user is null) return;
        CurrentUserName = user.DisplayName;
        CurrentUserTag = user.Tag;
        CurrentUserAvatar = new Uri(Cdn.Avatar(user, 64));
    }

    private void RefreshHomeBadge() =>
        HomeMentionCount = _client.State.PrivateChannels.Sum(c => _client.State.GetMentionCount(c.Id));

    /// <summary>The first channel in sidebar order the user can read.</summary>
    private Snowflake? DefaultChannel(Snowflake guildId) =>
        _client.State.GetChannelTree(guildId)
            .SelectMany(g => g.Channels)
            .Where(c => c.Type is ChannelType.GuildText or ChannelType.GuildAnnouncement)
            .FirstOrDefault(c => _client.State.GetPermissions(c.Id).HasFlag(Permission.ReadMessageHistory))
            ?.Id;

    private async Task SubscribeAsync(Snowflake guildId)
    {
        // User sessions only receive typing and member updates for guilds they've subscribed to.
        if (_client.Options.TokenType != TokenType.User || !_subscribedGuilds.Add(guildId))
            return;
        try { await _client.Gateway.SubscribeGuildAsync(guildId); }
        catch (Exception) { _subscribedGuilds.Remove(guildId); }
    }

    private void CloseChat()
    {
        Chat?.Dispose();
        Chat = null;
        Sidebar.SelectedChannelId = null;
    }

    #endregion

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        var state = _client.State;
        state.Ready -= OnReady;
        state.GuildAvailable -= OnGuildAvailable;
        state.GuildFoldersChanged -= OnGuildFoldersChanged;
        state.GuildUpdated -= OnGuildUpdated;
        state.GuildRemoved -= OnGuildRemoved;
        state.ChannelCreated -= OnChannelChanged;
        state.ChannelUpdated -= OnChannelChanged;
        state.ChannelDeleted -= OnChannelDeleted;
        state.PermissionsChanged -= OnPermissionsChanged;
        state.ReadStateChanged -= OnReadStateChanged;
        state.MessageCreated -= OnMessageCreated;
        state.PresenceChanged -= OnPresenceChanged;
        state.CurrentUserUpdated -= OnCurrentUserUpdated;
        _client.Gateway.StateChanged -= OnGatewayStateChanged;
        _client.Gateway.FatalClose -= OnFatalClose;
        foreach (var folder in RailItems.OfType<GuildFolderViewModel>())
            folder.Dispose();
        Chat?.Dispose();
    }
}
