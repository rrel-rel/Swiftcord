using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Markdown;
using Swiftcord.Core.Models;
using Swiftcord.ViewModels.Formatting;

namespace Swiftcord.ViewModels.Messages;

/// <summary>Commands a message row can trigger; supplied by the owning chat view model.</summary>
public interface IMessageActions
{
    void Reply(MessageItemViewModel message);
    void Edit(MessageItemViewModel message);
    Task DeleteAsync(MessageItemViewModel message);
    Task ToggleReactionAsync(MessageItemViewModel message, ReactionViewModel reaction);
    Task AddReactionAsync(MessageItemViewModel message, string emoji);
    Task RetryAsync(MessageItemViewModel message);
    Task JumpToMessageAsync(Snowflake messageId);
    Task TogglePinAsync(MessageItemViewModel message);
}

public sealed partial class MessageItemViewModel : ObservableObject
{
    /// <summary>Consecutive messages from one author within this window are drawn as a group.</summary>
    public static readonly TimeSpan GroupingWindow = TimeSpan.FromMinutes(7);

    public MessageItemViewModel(Message message, MessageItemOptions options)
    {
        Message = message;
        Context = options.Context;
        var guildId = message.GuildId ?? options.GuildId;

        AuthorName = options.DisplayName(message.Author.Id);
        AuthorColor = options.RoleColor(message.Author.Id) ?? 0;
        AvatarUri = new Uri(guildId is { } g
            ? Cdn.MemberAvatar(g, message.Author, message.Member, 80)
            : Cdn.Avatar(message.Author, 80));
        IsBot = message.Author.Bot || message.WebhookId is not null;
        TimestampText = Formatters.MessageTimestamp(message.Timestamp, options.Now);
        ShortTime = message.Timestamp.ToLocalTime().ToString("t");
        FullTimestamp = message.Timestamp.ToLocalTime().ToString("F");
        IsEdited = message.EditedTimestamp is not null;
        IsOwn = message.Author.Id == options.CurrentUserId;
        CanDelete = IsOwn || options.CanManageMessages;
        CanEdit = IsOwn && message.IsUserContent && message.Forwarded is null;
        MentionsMe = options.MentionsMe(message);

        // A forward shows the original's content under this message's author.
        var body = message.Forwarded ?? message;
        if (message.Forwarded is { } original)
        {
            IsForwarded = true;
            var source = message.MessageReference!;
            var channelName = source.ChannelId is { } c && source.ChannelId != message.ChannelId ? options.Context.ChannelName(c) : null;
            var time = original.Timestamp != default ? Formatters.MessageTimestamp(original.Timestamp, options.Now) : null;
            ForwardedFrom = string.Join(" • ", new[] { channelName is null ? null : "#" + channelName, time }.OfType<string>());
            OpenForwardSourceCommand = new RelayCommand(() =>
            {
                if (source is { ChannelId: { } channel, MessageId: { } id })
                    options.Context.OpenLink(channel, id);
            }, () => source.MessageId is not null);
        }
        else
        {
            OpenForwardSourceCommand = new RelayCommand(() => { }, () => false);
        }

        Nodes = MarkdownParser.Parse(body.Content);
        IsEmojiOnly = !IsForwarded && MarkdownParser.IsEmojiOnly(Nodes);
        SystemText = message.IsUserContent ? null : Formatters.SystemMessage(message, AuthorName, options.DisplayName);

        Attachments = body.Attachments.Select(a => new AttachmentViewModel(a)).ToList();
        Embeds = body.Flags.HasFlag(MessageFlags.SuppressEmbeds)
            ? []
            : body.Embeds.Select(e => new EmbedViewModel(e, options.Context)).Where(e => !e.IsEmpty).ToList();
        Stickers = body.StickerItems.Select(s => new StickerViewModel(s)).ToList();

        if (message.ReferencedMessage is { } reply)
        {
            ReplyAuthorName = options.DisplayName(reply.Author.Id);
            ReplyAuthorColor = options.RoleColor(reply.Author.Id) ?? 0;
            ReplyAvatarUri = new Uri(Cdn.Avatar(reply.Author, 32));
            ReplySnippet = reply.Content.Length > 0
                ? reply.Content.ReplaceLineEndings(" ")
                : reply.Attachments.Count > 0 ? "Click to see attachment" : "Click to see message";
        }
        else if (message.Type == MessageType.Reply)
        {
            ReplySnippet = "Original message was deleted";
        }

        _actions = options.Actions;
        ReplyCommand = new RelayCommand(() => _actions.Reply(this));
        JumpToReplyCommand = new AsyncRelayCommand(
            () => message.MessageReference?.MessageId is { } id ? _actions.JumpToMessageAsync(id) : Task.CompletedTask,
            () => message.ReferencedMessage is not null);
        EditCommand = new RelayCommand(() => _actions.Edit(this), () => CanEdit);
        DeleteCommand = new AsyncRelayCommand(() => _actions.DeleteAsync(this), () => CanDelete);
        CanPin = options.CanPin && message.IsUserContent;
        _isPinned = message.Pinned;
        TogglePinCommand = new AsyncRelayCommand(() => _actions.TogglePinAsync(this), () => CanPin);
        RetryCommand = new AsyncRelayCommand(() => _actions.RetryAsync(this));
        AddReactionCommand = new AsyncRelayCommand<string>(emoji => _actions.AddReactionAsync(this, emoji!));
        SetReactions(message.Reactions);
    }

    private readonly IMessageActions _actions;

    public Message Message { get; }
    public Snowflake Id => Message.Id;
    /// <summary>For "Copy Message ID"; bound as a string because x:Bind mishandles struct arguments.</summary>
    public string IdText => Message.Id.ToString();
    public Snowflake AuthorId => Message.Author.Id;
    public DateTimeOffset Timestamp => Message.Timestamp;
    /// <summary>The text shown (and copied): the original's for a forward.</summary>
    public string Content => (Message.Forwarded ?? Message).Content;
    public IMessageContext Context { get; }

    public string AuthorName { get; }
    /// <summary>RGB role colour, or 0 for the theme's default text colour.</summary>
    public int AuthorColor { get; }
    public Uri AvatarUri { get; }
    public bool IsBot { get; }
    public string TimestampText { get; }
    /// <summary>Shown in the gutter on hover for continuation messages.</summary>
    public string ShortTime { get; }
    public string FullTimestamp { get; }
    public bool IsEdited { get; }
    public bool IsOwn { get; }
    public bool CanDelete { get; }
    public bool CanEdit { get; }
    /// <summary>Highlighted like Discord does for messages that ping the user.</summary>
    public bool MentionsMe { get; }

    public IReadOnlyList<MarkdownNode> Nodes { get; }
    public bool HasContent => Nodes.Count > 0 && !IsSystem;
    public bool IsEmojiOnly { get; }

    public string? SystemText { get; }
    public bool IsSystem => SystemText is not null;

    public IReadOnlyList<AttachmentViewModel> Attachments { get; }
    public IReadOnlyList<EmbedViewModel> Embeds { get; }
    public IReadOnlyList<StickerViewModel> Stickers { get; }
    public ObservableCollection<ReactionViewModel> Reactions { get; } = [];
    public bool HasAttachments => Attachments.Count > 0;

    /// <summary>A one-line stand-in for attachments in compact lists such as pins.</summary>
    public string? AttachmentSummary => Attachments.Count switch
    {
        0 => null,
        1 => Attachments[0].FileName,
        var n => $"{n} attachments",
    };
    public bool HasEmbeds => Embeds.Count > 0;
    public bool HasStickers => Stickers.Count > 0;

    /// <summary>A forwarded message: the content below is the original's, shown with a side bar.</summary>
    public bool IsForwarded { get; }
    /// <summary>Where and when the original was posted, e.g. "#general • Yesterday at 3:12 PM".</summary>
    public string? ForwardedFrom { get; }
    public IRelayCommand OpenForwardSourceCommand { get; }

    public bool IsReply => ReplySnippet is not null;
    public string? ReplyAuthorName { get; }
    public int ReplyAuthorColor { get; }
    public Uri? ReplyAvatarUri { get; }
    public string? ReplySnippet { get; }

    public IRelayCommand ReplyCommand { get; }
    /// <summary>Scrolls to (loading if needed) the message this one replies to.</summary>
    public IAsyncRelayCommand JumpToReplyCommand { get; }

    /// <summary>Briefly set after jumping to this message, so it stands out.</summary>
    [ObservableProperty]
    private bool _isHighlighted;
    public IRelayCommand EditCommand { get; }
    public IAsyncRelayCommand DeleteCommand { get; }
    public IAsyncRelayCommand TogglePinCommand { get; }
    public bool CanPin { get; }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(PinMenuText))]
    private bool _isPinned;

    public string PinMenuText => IsPinned ? "Unpin Message" : "Pin Message";
    public IAsyncRelayCommand RetryCommand { get; }
    public IAsyncRelayCommand<string> AddReactionCommand { get; }

    /// <summary>Starts a new visual group (shows avatar and name) rather than continuing the previous one.</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsContinuation))]
    private bool _isGroupStart = true;

    public bool IsContinuation => !IsGroupStart && !IsSystem;

    [ObservableProperty]
    private bool _showDateSeparator;

    [ObservableProperty]
    private string _dateSeparatorText = "";

    /// <summary>Sent locally and awaiting the server's confirmation.</summary>
    [ObservableProperty]
    private bool _isPending;

    [ObservableProperty]
    private bool _isFailed;

    public bool HasReactions => Reactions.Count > 0;

    /// <summary>Set for locally-sent messages so the gateway echo can replace them.</summary>
    internal string? Nonce { get; init; }

    /// <summary>Files to resend if a failed message is retried.</summary>
    internal IReadOnlyList<PendingAttachment> PendingFiles { get; init; } = [];

    internal void SetReactions(IEnumerable<Reaction> reactions)
    {
        Reactions.Clear();
        foreach (var reaction in reactions)
            Reactions.Add(new ReactionViewModel(reaction, r => _actions.ToggleReactionAsync(this, r)));
        OnPropertyChanged(nameof(HasReactions));
    }

    /// <summary>Recomputes grouping and the date divider relative to the message above.</summary>
    internal void UpdateGrouping(MessageItemViewModel? previous)
    {
        var date = Timestamp.ToLocalTime().Date;
        ShowDateSeparator = previous is null || previous.Timestamp.ToLocalTime().Date != date;
        DateSeparatorText = Timestamp.ToLocalTime().ToString("D");

        IsGroupStart = previous is null
            || ShowDateSeparator
            || IsReply
            || IsSystem
            || previous.IsSystem
            || previous.AuthorId != AuthorId
            || previous.AuthorName != AuthorName // Webhooks share an ID but post under different names.
            || Timestamp - previous.Timestamp > GroupingWindow;
    }
}

/// <summary>Per-channel inputs needed to build message rows.</summary>
public sealed class MessageItemOptions
{
    public required IMessageContext Context { get; init; }
    public required IMessageActions Actions { get; init; }
    public required Func<Snowflake, string> DisplayName { get; init; }
    public required Func<Snowflake, int?> RoleColor { get; init; }
    public required Func<Message, bool> MentionsMe { get; init; }
    public Snowflake? GuildId { get; init; }
    public Snowflake CurrentUserId { get; init; }
    public bool CanManageMessages { get; init; }
    public bool CanPin { get; init; }
    public DateTimeOffset Now { get; init; } = DateTimeOffset.Now;
}
