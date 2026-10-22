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
    public DiscordClient(DiscordClientOptions options, ILoggerFactory? loggerFactory = null,
        Func<IGatewayTransport>? transportFactory = null, HttpMessageHandler? httpHandler = null)
    {
        Options = options;
        Rest = new DiscordRestClient(options, httpHandler, logger: loggerFactory?.CreateLogger<DiscordRestClient>());
        Gateway = new GatewayClient(options, transportFactory, logger: loggerFactory?.CreateLogger<GatewayClient>());
        State = new DiscordState(options.MessageCacheSize);
        Gateway.Dispatch += State.HandleDispatch;
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

    /// <summary>Loads the page of messages before <paramref name="beforeId"/> and adds it to the cache.</summary>
    public async Task<IReadOnlyList<Message>> LoadOlderMessagesAsync(Snowflake channelId, Snowflake beforeId, CancellationToken ct = default)
    {
        var messages = await Rest.GetMessagesAsync(channelId, limit: 50, before: beforeId, ct: ct).ConfigureAwait(false);
        State.AddMessages(channelId, messages);
        messages.Reverse(); // REST returns newest first.
        return messages;
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

    public async ValueTask DisposeAsync()
    {
        Gateway.Dispatch -= State.HandleDispatch;
        await Gateway.DisposeAsync().ConfigureAwait(false);
        Rest.Dispose();
    }
}
