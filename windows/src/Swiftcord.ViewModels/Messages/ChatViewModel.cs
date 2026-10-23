using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Models;
using Swiftcord.Core.Permissions;
using Swiftcord.Core.Rest;
using Swiftcord.ViewModels.Formatting;

namespace Swiftcord.ViewModels.Messages;

/// <summary>One open channel: its message history, composer and typing indicator.</summary>
public sealed partial class ChatViewModel : ObservableObject, IMessageContext, IMessageActions, IDisposable
{
    public const int PageSize = 50;
    private static readonly TimeSpan TypingThrottle = TimeSpan.FromSeconds(8);
    private static readonly TimeSpan TypingRefreshDelay = TimeSpan.FromSeconds(10.5);
    private static readonly TimeSpan HighlightDuration = TimeSpan.FromSeconds(2);

    private readonly DiscordClient _client;
    private readonly IUiDispatcher _ui;
    private readonly TimeProvider _time;
    private readonly Action<Snowflake, Snowflake?>? _openChannel;
    private readonly Dictionary<Snowflake, MessageItemViewModel> _byId = [];
    private readonly Dictionary<string, MessageItemViewModel> _pendingByNonce = [];
    private ITimer? _typingTimer;
    private DateTimeOffset _lastTypingSent = DateTimeOffset.MinValue;
    private bool _disposed;

    /// <param name="openChannel">Opens another channel, optionally at a message.</param>
    public ChatViewModel(DiscordClient client, IUiDispatcher ui, Channel channel, Action<Snowflake, Snowflake?>? openChannel = null,
        TimeProvider? time = null)
    {
        _client = client;
        _ui = ui;
        _time = time ?? TimeProvider.System;
        _openChannel = openChannel;
        Channel = channel;
        UpdatePermissions();

        var state = client.State;
        state.MessageCreated += OnMessageCreated;
        state.MessageUpdated += OnMessageUpdated;
        state.MessageDeleted += OnMessageDeleted;
        state.ReactionsChanged += OnReactionsChanged;
        state.TypingStarted += OnTypingStarted;
        state.PermissionsChanged += OnPermissionsChanged;
    }

    public Channel Channel { get; }
    public Snowflake ChannelId => Channel.Id;
    public ObservableCollection<MessageItemViewModel> Messages { get; } = [];
    public ObservableCollection<PendingAttachment> Attachments { get; } = [];

    public string Title => Channel.IsPrivate ? Channel.DisplayName : Channel.Name ?? "";
    public string? Topic => Channel.Topic;
    public bool HasTopic => !string.IsNullOrWhiteSpace(Topic);
    public bool IsDirectMessage => Channel.IsPrivate;
    public string Placeholder => Channel.IsPrivate ? $"Message @{Channel.DisplayName}" : $"Message #{Channel.Name}";

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(SendCommand))]
    private string _draft = "";

    [ObservableProperty]
    private bool _isLoading;

    [ObservableProperty]
    private bool _isLoadingOlder;

    [ObservableProperty]
    private bool _hasMoreHistory = true;

    /// <summary>
    /// Showing older messages reached by a jump, not connected to the latest ones. New messages aren't
    /// added while this is set; scrolling down pages forward, and <see cref="JumpToPresentCommand"/> returns.
    /// </summary>
    [ObservableProperty]
    private bool _isViewingHistory;

    [ObservableProperty]
    private bool _isLoadingNewer;

    /// <summary>The message just jumped to, for the view to scroll to.</summary>
    [ObservableProperty]
    private MessageItemViewModel? _highlighted;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsReplying))]
    private MessageItemViewModel? _replyingTo;

    [ObservableProperty]
    private bool _mentionReplyAuthor = true;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsEditing))]
    private MessageItemViewModel? _editing;

    [ObservableProperty]
    private string _typingText = "";

    [ObservableProperty]
    private string? _error;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(ComposerPlaceholder))]
    private bool _canSend;

    [ObservableProperty]
    private bool _canAttach;

    [ObservableProperty]
    private bool _canAddReactions;

    public bool IsReplying => ReplyingTo is not null;
    public bool IsEditing => Editing is not null;
    public string ComposerPlaceholder => CanSend ? Placeholder : "You do not have permission to send messages in this channel.";

    /// <summary>Whether the window is focused with this channel showing; new messages are acknowledged while it is.</summary>
    public bool IsActive
    {
        get;
        set
        {
            if (field == value) return;
            field = value;
            if (value) _ = MarkReadAsync();
        }
    }

    #region Loading

    /// <param name="jumpTo">A message to show (and highlight) instead of the latest ones.</param>
    public async Task InitializeAsync(Snowflake? jumpTo = null)
    {
        IsLoading = true;
        Error = null;
        try
        {
            var messages = await _client.GetLatestMessagesAsync(ChannelId);
            ResetMessages(messages);
            HasMoreHistory = true;
            IsViewingHistory = false;
            if (jumpTo is { } id)
                await JumpCoreAsync(id);
            if (!IsViewingHistory)
                await MarkReadAsync();
        }
        catch (DiscordApiException ex) when (ex.Code is 50001 or 50013)
        {
            Error = "You don't have access to this channel.";
            HasMoreHistory = false;
        }
        catch (Exception)
        {
            Error = "Couldn't load messages. Check your connection and try again.";
        }
        finally
        {
            IsLoading = false;
        }
    }

    [RelayCommand]
    private async Task LoadOlderAsync()
    {
        if (IsLoadingOlder || !HasMoreHistory || Messages.Count == 0)
            return;
        IsLoadingOlder = true;
        try
        {
            var oldest = Messages.FirstOrDefault(m => !m.IsPending)?.Id ?? Messages[0].Id;
            var older = await _client.LoadOlderMessagesAsync(ChannelId, oldest, cache: !IsViewingHistory);
            // Only an empty page proves we've reached the start; a short page can happen mid-history.
            HasMoreHistory = older.Count > 0;
            var options = CreateOptions();
            var items = older.Where(m => !_byId.ContainsKey(m.Id)).Select(m => new MessageItemViewModel(m, options)).ToList();
            for (var i = items.Count - 1; i >= 0; i--)
            {
                _byId[items[i].Id] = items[i];
                Messages.Insert(0, items[i]);
            }
            for (var i = 0; i <= items.Count && i < Messages.Count; i++)
                Messages[i].UpdateGrouping(i > 0 ? Messages[i - 1] : null);
        }
        catch (Exception)
        {
            Error = "Couldn't load older messages.";
        }
        finally
        {
            IsLoadingOlder = false;
        }
    }

    [RelayCommand]
    private async Task LoadNewerAsync()
    {
        if (!IsViewingHistory || IsLoadingNewer || Messages.Count == 0)
            return;
        IsLoadingNewer = true;
        try
        {
            var newest = Messages.Last(m => !m.IsPending && !m.IsFailed).Id;
            var newer = await _client.LoadNewerMessagesAsync(ChannelId, newest);
            var options = CreateOptions();
            foreach (var message in newer.Where(m => !_byId.ContainsKey(m.Id)))
                Insert(new MessageItemViewModel(message, options));
            // A short page means we've caught up; from here new messages can be appended again.
            if (newer.Count < PageSize)
            {
                IsViewingHistory = false;
                await MarkReadAsync();
            }
        }
        catch (Exception)
        {
            Error = "Couldn't load newer messages.";
        }
        finally
        {
            IsLoadingNewer = false;
        }
    }

    [RelayCommand]
    private Task JumpToPresentAsync() => InitializeAsync();

    /// <summary>Shows and highlights a message in this channel, loading the history around it if needed.</summary>
    public async Task JumpToMessageAsync(Snowflake messageId)
    {
        if (_byId.TryGetValue(messageId, out var loaded))
        {
            Highlight(loaded);
            return;
        }
        IsLoading = true;
        Error = null;
        try
        {
            await JumpCoreAsync(messageId);
        }
        catch (Exception)
        {
            Error = "Couldn't load that message.";
        }
        finally
        {
            IsLoading = false;
        }
    }

    private async Task JumpCoreAsync(Snowflake messageId)
    {
        if (!_byId.ContainsKey(messageId))
        {
            var page = await _client.GetMessagesAroundAsync(ChannelId, messageId);
            if (page.Count == 0)
            {
                Error = "Couldn't find that message. It may have been deleted.";
                return;
            }
            ResetMessages(page);
            HasMoreHistory = true;
            // Unless the page reaches the newest message, it isn't connected to the present.
            IsViewingHistory = Channel.LastMessageId is not { } last || page[^1].Id < last;
        }
        if (_byId.TryGetValue(messageId, out var item))
            Highlight(item);
        else
            Error = "Couldn't find that message. It may have been deleted.";
    }

    private async void Highlight(MessageItemViewModel item)
    {
        if (Highlighted is { } previous) previous.IsHighlighted = false;
        item.IsHighlighted = true;
        Highlighted = item;
        await Task.Delay(HighlightDuration, _time);
        item.IsHighlighted = false;
    }

    Task IMessageActions.JumpToMessageAsync(Snowflake messageId) => JumpToMessageAsync(messageId);

    private void ResetMessages(IEnumerable<Message> messages)
    {
        // Keep locally-pending messages; they haven't reached the cache yet.
        var pending = Messages.Where(m => m.IsPending || m.IsFailed).ToList();
        Messages.Clear();
        _byId.Clear();
        var options = CreateOptions();
        MessageItemViewModel? previous = null;
        foreach (var message in messages)
        {
            var item = new MessageItemViewModel(message, options);
            item.UpdateGrouping(previous);
            _byId[item.Id] = item;
            Messages.Add(item);
            previous = item;
        }
        foreach (var item in pending)
            Insert(item);
    }

    #endregion

    #region Composer

    private bool CanSendDraft() => CanSend && (!string.IsNullOrWhiteSpace(Draft) || Attachments.Count > 0);

    [RelayCommand(CanExecute = nameof(CanSendDraft))]
    private async Task SendAsync()
    {
        // A sent message belongs after the latest ones, so return to the present first.
        if (IsViewingHistory && Editing is null)
            await InitializeAsync();
        var text = Draft.Trim();
        if (Editing is { } editing)
        {
            Draft = "";
            Editing = null;
            if (text.Length == 0)
            {
                await DeleteAsync(editing);
                return;
            }
            if (text != editing.Content)
            {
                try
                {
                    await _client.Rest.EditMessageAsync(ChannelId, editing.Id, new MessageEditRequest { Content = text });
                }
                catch (Exception)
                {
                    Error = "Couldn't edit the message.";
                }
            }
            return;
        }

        if (text.Length == 0 && Attachments.Count == 0)
            return;

        var files = Attachments.ToList();
        var replyTo = ReplyingTo;
        Draft = "";
        ReplyingTo = null;
        Attachments.Clear();
        SendCommand.NotifyCanExecuteChanged();

        var pending = CreatePendingMessage(text, replyTo, files);
        Insert(pending);
        await DeliverAsync(pending, replyTo?.Id);
    }

    private MessageItemViewModel CreatePendingMessage(string text, MessageItemViewModel? replyTo, IReadOnlyList<PendingAttachment> files)
    {
        var me = _client.State.CurrentUser ?? new User();
        var now = _time.GetUtcNow();
        var nonce = DiscordClient.NewNonce();
        var message = new Message
        {
            // A provisional ID that sorts after everything already received.
            Id = new Snowflake(Math.Max(Snowflake.FromTime(now).Value, (Messages.LastOrDefault()?.Id.Value ?? 0) + 1)),
            ChannelId = ChannelId,
            GuildId = Channel.GuildId,
            Author = me,
            Content = text,
            Timestamp = now,
            Nonce = nonce,
            Type = replyTo is null ? MessageType.Default : MessageType.Reply,
            ReferencedMessage = replyTo?.Message,
            Attachments = files.Select(f => new Attachment { Filename = f.FileName, Size = f.Size, Url = "about:blank" }).ToList(),
        };
        return new MessageItemViewModel(message, CreateOptions()) { Nonce = nonce, PendingFiles = files, IsPending = true };
    }

    private async Task DeliverAsync(MessageItemViewModel pending, Snowflake? replyTo)
    {
        var nonce = pending.Nonce!;
        _pendingByNonce[nonce] = pending;
        pending.IsFailed = false;
        pending.IsPending = true;

        var uploads = new List<FileUpload>();
        try
        {
            foreach (var file in pending.PendingFiles)
            {
                var name = file.IsSpoiler && !file.FileName.StartsWith("SPOILER_", StringComparison.Ordinal)
                    ? "SPOILER_" + file.FileName
                    : file.FileName;
                uploads.Add(new FileUpload(name, await file.Open(), file.ContentType));
            }

            var sent = await _client.SendMessageAsync(ChannelId, pending.Content, replyTo, uploads,
                MentionReplyAuthor, nonce);
            ConfirmPending(nonce, sent);
        }
        catch (Exception ex)
        {
            pending.IsPending = false;
            pending.IsFailed = true;
            Error = ex is DiscordApiException api ? $"Message failed to send: {api.ApiMessage}" : "Message failed to send.";
        }
        finally
        {
            foreach (var upload in uploads)
                await upload.Content.DisposeAsync();
        }
    }

    /// <summary>Swaps a pending message for the real one, from whichever of the REST response or gateway echo comes first.</summary>
    private void ConfirmPending(string nonce, Message message)
    {
        if (!_pendingByNonce.Remove(nonce, out var pending))
            return;
        Remove(pending);
        if (!_byId.ContainsKey(message.Id))
            Insert(new MessageItemViewModel(message, CreateOptions()));
    }

    [RelayCommand]
    private void CancelReplyOrEdit()
    {
        if (Editing is not null)
            Draft = "";
        ReplyingTo = null;
        Editing = null;
    }

    /// <summary>Starts editing the user's most recent message (Up arrow in an empty composer).</summary>
    public bool EditLastOwnMessage()
    {
        if (!string.IsNullOrEmpty(Draft))
            return false;
        var last = Messages.LastOrDefault(m => m.CanEdit && !m.IsPending && !m.IsFailed);
        if (last is null)
            return false;
        Edit(last);
        return true;
    }

    public void AddAttachment(PendingAttachment attachment)
    {
        if (!CanAttach)
            return;
        attachment.RemoveCommand = new RelayCommand(() =>
        {
            Attachments.Remove(attachment);
            SendCommand.NotifyCanExecuteChanged();
        });
        Attachments.Add(attachment);
        SendCommand.NotifyCanExecuteChanged();
    }

    partial void OnDraftChanged(string value)
    {
        if (string.IsNullOrEmpty(value) || Editing is not null || !CanSend)
            return;
        var now = _time.GetUtcNow();
        if (now - _lastTypingSent < TypingThrottle)
            return;
        _lastTypingSent = now;
        _ = SendTypingAsync();
    }

    private async Task SendTypingAsync()
    {
        try { await _client.Rest.TriggerTypingAsync(ChannelId); }
        catch (Exception) { /* Typing indicators are best-effort. */ }
    }

    #endregion

    #region IMessageActions

    public void Reply(MessageItemViewModel message)
    {
        Editing = null;
        ReplyingTo = message;
        // Discord doesn't ping yourself, and defaults to not pinging bots' owners twice.
        MentionReplyAuthor = !message.IsOwn;
    }

    public void Edit(MessageItemViewModel message)
    {
        if (!message.CanEdit)
            return;
        ReplyingTo = null;
        Editing = message;
        Draft = message.Content;
    }

    public async Task DeleteAsync(MessageItemViewModel message)
    {
        if (message.IsFailed)
        {
            Remove(message);
            return;
        }
        try
        {
            await _client.Rest.DeleteMessageAsync(ChannelId, message.Id);
        }
        catch (Exception)
        {
            Error = "Couldn't delete the message.";
        }
    }

    public async Task ToggleReactionAsync(MessageItemViewModel message, ReactionViewModel reaction)
    {
        var adding = !reaction.Me;
        if (adding && !CanAddReactions)
            return;

        // Optimistic: the gateway event will replace these with the authoritative counts.
        reaction.Me = adding;
        reaction.Count += adding ? 1 : -1;
        if (reaction.Count <= 0)
            message.Reactions.Remove(reaction);
        try
        {
            if (adding) await _client.Rest.AddReactionAsync(ChannelId, message.Id, reaction.Emoji);
            else await _client.Rest.RemoveReactionAsync(ChannelId, message.Id, reaction.Emoji);
        }
        catch (Exception)
        {
            Error = "Couldn't update the reaction.";
            message.SetReactions(_client.State.GetMessage(ChannelId, message.Id)?.Reactions ?? message.Message.Reactions);
        }
    }

    public Task AddReactionAsync(MessageItemViewModel message, string emoji)
    {
        var existing = message.Reactions.FirstOrDefault(r => r.Emoji.Id is null && r.Emoji.Name == emoji);
        if (existing is not null)
            return existing.Me ? Task.CompletedTask : ToggleReactionAsync(message, existing);

        var reaction = new ReactionViewModel(new Reaction { Emoji = new Emoji { Name = emoji } },
            r => ToggleReactionAsync(message, r));
        message.Reactions.Add(reaction);
        return ToggleReactionAsync(message, reaction);
    }

    public Task RetryAsync(MessageItemViewModel message) =>
        message.IsFailed ? DeliverAsync(message, message.Message.ReferencedMessage?.Id) : Task.CompletedTask;

    #endregion

    #region IMessageContext

    public string UserName(Snowflake userId) => _client.State.GetDisplayName(userId, Channel.GuildId);

    public string ChannelName(Snowflake channelId) => _client.State.GetChannel(channelId)?.Name ?? "unknown-channel";

    public (string Name, int Color) Role(Snowflake roleId)
    {
        var role = Channel.GuildId is { } g ? _client.State.GetGuild(g)?.Roles.FirstOrDefault(r => r.Id == roleId) : null;
        return role is null ? ("deleted-role", 0) : (role.Name, role.Color);
    }

    public Uri EmojiUri(Snowflake emojiId, bool animated, int size) => new(Cdn.Emoji(emojiId, animated, size));

    public string FormatTimestamp(DateTimeOffset time, char style) => Formatters.MarkdownTimestamp(time, style, _time.GetUtcNow());

    public void OpenChannel(Snowflake channelId) => _openChannel?.Invoke(channelId, null);

    public void OpenLink(Snowflake channelId, Snowflake? messageId)
    {
        if (channelId == ChannelId && messageId is { } id)
            _ = JumpToMessageAsync(id);
        else if (channelId != ChannelId)
            _openChannel?.Invoke(channelId, messageId);
    }

    #endregion

    #region State events (background thread)

    private void OnMessageCreated(Message message)
    {
        if (message.ChannelId != ChannelId) return;
        _ui.Post(() =>
        {
            if (_disposed) return;
            if (message.Nonce is { } nonce && _pendingByNonce.ContainsKey(nonce))
                ConfirmPending(nonce, message);
            else if (!_byId.ContainsKey(message.Id) && !IsViewingHistory)
                Insert(new MessageItemViewModel(message, CreateOptions()));
            RefreshTyping();
            if (IsActive && message.Author.Id != _client.State.CurrentUser?.Id)
                _ = MarkReadAsync();
        });
    }

    private void OnMessageUpdated(Message message)
    {
        if (message.ChannelId != ChannelId) return;
        _ui.Post(() =>
        {
            if (_disposed || !_byId.TryGetValue(message.Id, out var existing)) return;
            var index = Messages.IndexOf(existing);
            var updated = new MessageItemViewModel(message, CreateOptions());
            _byId[message.Id] = updated;
            Messages[index] = updated;
            updated.UpdateGrouping(index > 0 ? Messages[index - 1] : null);
        });
    }

    private void OnMessageDeleted(Snowflake channelId, Snowflake messageId)
    {
        if (channelId != ChannelId) return;
        _ui.Post(() =>
        {
            if (_disposed || !_byId.TryGetValue(messageId, out var existing)) return;
            if (ReferenceEquals(Editing, existing)) CancelReplyOrEdit();
            if (ReferenceEquals(ReplyingTo, existing)) ReplyingTo = null;
            Remove(existing);
        });
    }

    private void OnReactionsChanged(Message message)
    {
        if (message.ChannelId != ChannelId) return;
        var reactions = message.Reactions.Select(r => new Reaction { Count = r.Count, Me = r.Me, Emoji = r.Emoji }).ToList();
        _ui.Post(() =>
        {
            if (!_disposed && _byId.TryGetValue(message.Id, out var item))
                item.SetReactions(reactions);
        });
    }

    private void OnTypingStarted(Snowflake channelId, Snowflake userId)
    {
        if (channelId != ChannelId) return;
        _ui.Post(() =>
        {
            if (_disposed) return;
            RefreshTyping();
            _typingTimer?.Dispose();
            _typingTimer = _time.CreateTimer(_ => _ui.Post(RefreshTyping), null, TypingRefreshDelay, Timeout.InfiniteTimeSpan);
        });
    }

    private void OnPermissionsChanged(Snowflake guildId)
    {
        if (guildId != Channel.GuildId) return;
        _ui.Post(UpdatePermissions);
    }

    #endregion

    #region Helpers

    private void RefreshTyping()
    {
        if (_disposed) return;
        var names = _client.State.GetTypingUsers(ChannelId).Select(UserName).ToList();
        TypingText = Formatters.TypingText(names);
    }

    private void UpdatePermissions()
    {
        var permissions = _client.State.GetPermissions(ChannelId);
        CanSend = permissions.HasFlag(Permission.SendMessages)
            || (Channel.IsThread && permissions.HasFlag(Permission.SendMessagesInThreads));
        CanAttach = CanSend && permissions.HasFlag(Permission.AttachFiles);
        CanAddReactions = permissions.HasFlag(Permission.AddReactions);
        SendCommand.NotifyCanExecuteChanged();
    }

    private async Task MarkReadAsync()
    {
        try { await _client.MarkChannelReadAsync(ChannelId); }
        catch (Exception) { /* Read state syncs again on the next message. */ }
    }

    private MessageItemOptions CreateOptions()
    {
        var state = _client.State;
        var me = state.CurrentUser;
        var guildId = Channel.GuildId;
        var myRoles = me is not null && guildId is { } g ? state.GetMember(g, me.Id)?.Roles ?? [] : [];
        return new MessageItemOptions
        {
            Context = this,
            Actions = this,
            GuildId = guildId,
            CurrentUserId = me?.Id ?? default,
            CanManageMessages = state.GetPermissions(ChannelId).HasFlag(Permission.ManageMessages),
            DisplayName = id => state.GetDisplayName(id, guildId),
            RoleColor = id => guildId is { } gid ? state.GetRoleColor(gid, id) : null,
            MentionsMe = m => me is not null && (m.MentionEveryone
                || m.Mentions.Any(u => u.Id == me.Id)
                || m.MentionRoles.Any(myRoles.Contains)),
            Now = _time.GetUtcNow(),
        };
    }

    /// <summary>Inserts in ID order (usually at the end) and fixes grouping around it.</summary>
    private void Insert(MessageItemViewModel item)
    {
        var index = Messages.Count;
        while (index > 0 && Messages[index - 1].Id > item.Id)
            index--;
        Messages.Insert(index, item);
        if (!item.IsPending && !item.IsFailed)
            _byId[item.Id] = item;
        item.UpdateGrouping(index > 0 ? Messages[index - 1] : null);
        if (index + 1 < Messages.Count)
            Messages[index + 1].UpdateGrouping(item);
    }

    private void Remove(MessageItemViewModel item)
    {
        var index = Messages.IndexOf(item);
        if (index < 0) return;
        Messages.RemoveAt(index);
        if (_byId.TryGetValue(item.Id, out var current) && ReferenceEquals(current, item))
            _byId.Remove(item.Id);
        if (index < Messages.Count)
            Messages[index].UpdateGrouping(index > 0 ? Messages[index - 1] : null);
    }

    #endregion

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        var state = _client.State;
        state.MessageCreated -= OnMessageCreated;
        state.MessageUpdated -= OnMessageUpdated;
        state.MessageDeleted -= OnMessageDeleted;
        state.ReactionsChanged -= OnReactionsChanged;
        state.TypingStarted -= OnTypingStarted;
        state.PermissionsChanged -= OnPermissionsChanged;
        _typingTimer?.Dispose();
    }
}
