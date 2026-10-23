using Microsoft.Extensions.Logging;
using Swiftcord.Core.Gateway;
using Swiftcord.Core.Models;
using Swiftcord.Core.Rest;
using Swiftcord.Core.State;

namespace Swiftcord.Core;

/// <summary>
/// The entry point for the app: owns the REST client, gateway connection and state cache, and
/// wires gateway events into the cache.
/// </summary>
public sealed class DiscordClient : IAsyncDisposable
{
    private readonly ILogger? _logger;

    public DiscordClient(DiscordClientOptions options, ILoggerFactory? loggerFactory = null,
        Func<IGatewayTransport>? transportFactory = null, HttpMessageHandler? httpHandler = null)
    {
        Options = options;
        _logger = loggerFactory?.CreateLogger<DiscordClient>();
        Rest = new DiscordRestClient(options, httpHandler, logger: loggerFactory?.CreateLogger<DiscordRestClient>());
        Gateway = new GatewayClient(options, transportFactory, logger: loggerFactory?.CreateLogger<GatewayClient>());
        State = new DiscordState(options.MessageCacheSize);
        Gateway.Dispatch += State.HandleDispatch;
        State.Ready += OnReady;
    }

    public DiscordClientOptions Options { get; }
    public DiscordRestClient Rest { get; }
    public GatewayClient Gateway { get; }
    public DiscordState State { get; }

    /// <summary>Connects to the gateway and waits for READY.</summary>
    public async Task ConnectAsync(CancellationToken ct = default)
    {
        await Gateway.StartAsync().ConfigureAwait(false);
        await Gateway.WaitForReadyAsync(ct).ConfigureAwait(false);
    }

    /// <summary>
    /// Returns cached messages for a channel, fetching the latest page from REST the first time
    /// a channel is opened.
    /// </summary>
    public async Task<IReadOnlyList<Message>> GetLatestMessagesAsync(Snowflake channelId, CancellationToken ct = default)
    {
        var cached = State.GetMessages(channelId);
        if (cached.Count > 0)
            return cached;
        var messages = await Rest.GetMessagesAsync(channelId, limit: 50, ct: ct).ConfigureAwait(false);
        State.AddMessages(channelId, messages);
        return State.GetMessages(channelId);
    }

    /// <summary>
    /// Loads the page of messages before <paramref name="beforeId"/>, oldest first. Pages are cached only when
    /// <paramref name="cache"/> is set: the cache must stay contiguous with the latest message, which history
    /// reached by jumping to an old message isn't.
    /// </summary>
    public async Task<IReadOnlyList<Message>> LoadOlderMessagesAsync(Snowflake channelId, Snowflake beforeId, bool cache = true,
        CancellationToken ct = default)
    {
        var messages = await Rest.GetMessagesAsync(channelId, limit: 50, before: beforeId, ct: ct).ConfigureAwait(false);
        _logger?.LogInformation("Fetched {Count} messages before {Before} in channel {Channel}", messages.Count, beforeId, channelId);
        if (cache)
            State.AddMessages(channelId, messages);
        messages.Reverse(); // REST returns newest first.
        return messages;
    }

    /// <summary>Loads the page of messages after <paramref name="afterId"/>, oldest first, without caching.</summary>
    public async Task<IReadOnlyList<Message>> LoadNewerMessagesAsync(Snowflake channelId, Snowflake afterId, CancellationToken ct = default)
    {
        var messages = await Rest.GetMessagesAsync(channelId, limit: 50, after: afterId, ct: ct).ConfigureAwait(false);
        return messages.OrderBy(m => m.Id).ToList();
    }

    /// <summary>Loads the messages around <paramref name="messageId"/> (including it), oldest first, without caching.</summary>
    public async Task<IReadOnlyList<Message>> GetMessagesAroundAsync(Snowflake channelId, Snowflake messageId, CancellationToken ct = default)
    {
        var messages = await Rest.GetMessagesAsync(channelId, limit: 50, around: messageId, ct: ct).ConfigureAwait(false);
        return messages.OrderBy(m => m.Id).ToList();
    }

    /// <summary>A fresh nonce for matching a sent message to its gateway echo.</summary>
    public static string NewNonce() => Snowflake.FromTime(DateTimeOffset.UtcNow).ToString();

    /// <summary>Sends a message with a nonce so the echo from the gateway can be matched to it.</summary>
    public Task<Message> SendMessageAsync(Snowflake channelId, string content, Snowflake? replyTo = null,
        IReadOnlyList<FileUpload>? files = null, bool mentionRepliedUser = true, string? nonce = null,
        CancellationToken ct = default)
    {
        var request = new MessageCreateRequest
        {
            Content = content,
            Nonce = nonce ?? NewNonce(),
        };
        if (replyTo is { } replyId)
        {
            request.MessageReference = new MessageReference { MessageId = replyId, ChannelId = channelId, FailIfNotExists = false };
            request.AllowedMentions = new AllowedMentions { Parse = ["users", "roles", "everyone"], RepliedUser = mentionRepliedUser };
        }
        return Rest.SendMessageAsync(channelId, request, files, ct);
    }

    /// <summary>Acknowledges the newest message in a channel if it's unread.</summary>
    public async Task MarkChannelReadAsync(Snowflake channelId, CancellationToken ct = default)
    {
        if (State.GetChannel(channelId)?.LastMessageId is not { } last
            || (!State.IsUnread(channelId) && State.GetMentionCount(channelId) == 0))
            return;
        State.MarkRead(channelId, last);
        if (Options.TokenType == TokenType.User)
            await Rest.AckMessageAsync(channelId, last, ct).ConfigureAwait(false);
    }

    private async void OnReady()
    {
        // READY carries the saved server order with most capability sets; fetch it when it didn't.
        if (State.GuildFolders is not null || Options.TokenType != TokenType.User)
            return;
        try
        {
            if (UserSettingsReader.ReadGuildFolders(await Rest.GetUserSettingsProtoAsync().ConfigureAwait(false)) is { } folders)
                State.SetGuildFolders(folders);
        }
        catch (Exception ex)
        {
            _logger?.LogWarning(ex, "Couldn't load the saved server order");
        }
    }

    public async ValueTask DisposeAsync()
    {
        State.Ready -= OnReady;
        Gateway.Dispatch -= State.HandleDispatch;
        await Gateway.DisposeAsync().ConfigureAwait(false);
        Rest.Dispose();
    }
}
