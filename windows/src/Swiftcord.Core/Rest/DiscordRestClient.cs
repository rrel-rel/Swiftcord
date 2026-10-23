using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using Swiftcord.Core.Json;
using Swiftcord.Core.Models;

namespace Swiftcord.Core.Rest;

public sealed class DiscordRestClient : IDisposable
{
    private const int MaxRateLimitRetries = 3;

    private readonly HttpClient _http;
    private readonly RateLimiter _rateLimiter;
    private readonly ILogger _logger;

    public DiscordRestClient(DiscordClientOptions options, HttpMessageHandler? handler = null,
        TimeProvider? time = null, ILogger<DiscordRestClient>? logger = null)
    {
        _rateLimiter = new RateLimiter(time ?? TimeProvider.System);
        _logger = logger ?? NullLogger<DiscordRestClient>.Instance;
        _http = handler is null ? new HttpClient() : new HttpClient(handler, disposeHandler: false);
        _http.BaseAddress = options.ApiBaseUrl;
        _http.DefaultRequestHeaders.TryAddWithoutValidation("Authorization", options.AuthorizationHeader);
        _http.DefaultRequestHeaders.UserAgent.ParseAdd(options.UserAgent);
        _http.DefaultRequestHeaders.AcceptLanguage.ParseAdd(options.Locale);
        _http.DefaultRequestHeaders.TryAddWithoutValidation("X-Discord-Locale", options.Locale);
    }

    #region Users

    public Task<User> GetCurrentUserAsync(CancellationToken ct = default) =>
        SendAsync<User>(HttpMethod.Get, "users/@me", ct: ct);

    public Task<User> GetUserAsync(Snowflake userId, CancellationToken ct = default) =>
        SendAsync<User>(HttpMethod.Get, $"users/{userId}", ct: ct);

    /// <summary>A user's full profile; with <paramref name="guildId"/>, their server profile and membership too. User accounts only.</summary>
    public Task<UserProfile> GetUserProfileAsync(Snowflake userId, Snowflake? guildId = null, CancellationToken ct = default) =>
        SendAsync<UserProfile>(HttpMethod.Get,
            $"users/{userId}/profile?with_mutual_guilds=true{(guildId is { } g ? $"&guild_id={g}" : "")}", ct: ct);

    /// <summary>The user's protobuf-encoded PreloadedUserSettings, base64; read with <see cref="UserSettingsReader"/>. User accounts only.</summary>
    public async Task<string> GetUserSettingsProtoAsync(CancellationToken ct = default) =>
        (await SendAsync<UserSettingsProtoResponse>(HttpMethod.Get, "users/@me/settings-proto/1", ct: ct).ConfigureAwait(false)).Settings;

    /// <summary>Invalidates a user token server-side, as signing out of the official client does.</summary>
    public Task LogoutAsync(CancellationToken ct = default) =>
        SendAsync(HttpMethod.Post, "auth/logout", new LogoutRequest(), ct);

    /// <summary>Opens (or returns the existing) DM channel with a user.</summary>
    public Task<Channel> CreateDmAsync(Snowflake recipientId, CancellationToken ct = default) =>
        SendAsync<Channel>(HttpMethod.Post, "users/@me/channels", new CreateDmRequest { Recipients = [recipientId] }, ct);

    #endregion

    #region Guilds & channels

    public Task<Guild> GetGuildAsync(Snowflake guildId, CancellationToken ct = default) =>
        SendAsync<Guild>(HttpMethod.Get, $"guilds/{guildId}", ct: ct);

    public Task<List<Channel>> GetGuildChannelsAsync(Snowflake guildId, CancellationToken ct = default) =>
        SendAsync<List<Channel>>(HttpMethod.Get, $"guilds/{guildId}/channels", ct: ct);

    public Task<GuildMember> GetGuildMemberAsync(Snowflake guildId, Snowflake userId, CancellationToken ct = default) =>
        SendAsync<GuildMember>(HttpMethod.Get, $"guilds/{guildId}/members/{userId}", ct: ct);

    public Task<Channel> GetChannelAsync(Snowflake channelId, CancellationToken ct = default) =>
        SendAsync<Channel>(HttpMethod.Get, $"channels/{channelId}", ct: ct);

    public Task TriggerTypingAsync(Snowflake channelId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Post, $"channels/{channelId}/typing", ct: ct);

    #endregion

    #region Messages

    /// <param name="limit">1–100.</param>
    /// <param name="before">Return messages older than this ID.</param>
    /// <param name="after">Return messages newer than this ID.</param>
    /// <param name="around">Return messages centered on this ID.</param>
    public Task<List<Message>> GetMessagesAsync(Snowflake channelId, int limit = 50, Snowflake? before = null,
        Snowflake? after = null, Snowflake? around = null, CancellationToken ct = default)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(limit, 1);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(limit, 100);
        var query = $"?limit={limit}";
        if (before is { } b) query += $"&before={b}";
        if (after is { } a) query += $"&after={a}";
        if (around is { } r) query += $"&around={r}";
        return SendListAsync(HttpMethod.Get, $"channels/{channelId}/messages{query}", MinimalMessage, ct);
    }

    public Task<List<Message>> GetPinnedMessagesAsync(Snowflake channelId, CancellationToken ct = default) =>
        SendListAsync(HttpMethod.Get, $"channels/{channelId}/pins", MinimalMessage, ct);

    /// <summary>
    /// The fields every message has, for when the full model can't be read. Keeping the message
    /// (rather than dropping it) preserves its text and keeps history pagination moving.
    /// </summary>
    private static Message? MinimalMessage(JsonElement element)
    {
        if (!element.TryGetProperty("id", out var id) || !Snowflake.TryParse(id.GetString(), out var messageId))
            return null;
        var message = new Message { Id = messageId, Timestamp = messageId.CreatedAt };
        if (element.TryGetProperty("channel_id", out var channel) && Snowflake.TryParse(channel.GetString(), out var channelId))
            message.ChannelId = channelId;
        if (element.TryGetProperty("content", out var content) && content.ValueKind == JsonValueKind.String)
            message.Content = content.GetString()!;
        if (element.TryGetProperty("author", out var author))
        {
            try { message.Author = DiscordJson.Deserialize<User>(author); }
            catch (JsonException) { /* Keep the placeholder author. */ }
        }
        return message;
    }

    public Task<Message> SendMessageAsync(Snowflake channelId, MessageCreateRequest message,
        IReadOnlyList<FileUpload>? files = null, CancellationToken ct = default)
    {
        if (files is not { Count: > 0 })
            return SendAsync<Message>(HttpMethod.Post, $"channels/{channelId}/messages", message, ct);

        message.Attachments = files.Select((f, i) => new AttachmentRequest
        {
            Id = i,
            Filename = f.FileName,
            Description = f.Description,
        }).ToList();

        return SendAsync<Message>(HttpMethod.Post, $"channels/{channelId}/messages", ct: ct, contentFactory: () =>
        {
            // Rebuilt per attempt so rate-limit retries don't reuse a consumed request body.
            var form = new MultipartFormDataContent();
            form.Add(new ByteArrayContent(DiscordJson.SerializeToUtf8Bytes(message))
            {
                Headers = { ContentType = new MediaTypeHeaderValue("application/json") },
            }, "payload_json");
            for (var i = 0; i < files.Count; i++)
            {
                var file = files[i];
                if (file.Content.CanSeek) file.Content.Position = 0;
                var part = new StreamContent(file.Content);
                part.Headers.ContentType = new MediaTypeHeaderValue(file.ContentType ?? "application/octet-stream");
                form.Add(part, $"files[{i}]", file.FileName);
            }
            return form;
        });
    }

    public Task<Message> EditMessageAsync(Snowflake channelId, Snowflake messageId, MessageEditRequest edit,
        CancellationToken ct = default) =>
        SendAsync<Message>(HttpMethod.Patch, $"channels/{channelId}/messages/{messageId}", edit, ct);

    public Task DeleteMessageAsync(Snowflake channelId, Snowflake messageId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Delete, $"channels/{channelId}/messages/{messageId}", ct: ct);

    public Task AddReactionAsync(Snowflake channelId, Snowflake messageId, Emoji emoji, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Put, $"channels/{channelId}/messages/{messageId}/reactions/{Uri.EscapeDataString(emoji.ReactionKey)}/@me", ct: ct);

    public Task RemoveReactionAsync(Snowflake channelId, Snowflake messageId, Emoji emoji, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Delete, $"channels/{channelId}/messages/{messageId}/reactions/{Uri.EscapeDataString(emoji.ReactionKey)}/@me", ct: ct);

    public Task PinMessageAsync(Snowflake channelId, Snowflake messageId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Put, $"channels/{channelId}/pins/{messageId}", ct: ct);

    public Task UnpinMessageAsync(Snowflake channelId, Snowflake messageId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Delete, $"channels/{channelId}/pins/{messageId}", ct: ct);

    /// <summary>Marks a channel as read up to a message (user accounts only).</summary>
    public Task AckMessageAsync(Snowflake channelId, Snowflake messageId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Post, $"channels/{channelId}/messages/{messageId}/ack", new AckRequest(), ct);

    #endregion

    #region Transport

    public async Task<T> SendAsync<T>(HttpMethod method, string path, object? body = null, CancellationToken ct = default,
        Func<HttpContent>? contentFactory = null)
    {
        using var response = await SendCoreAsync(method, path, body, contentFactory, ct).ConfigureAwait(false);
        await using var stream = await response.Content.ReadAsStreamAsync(ct).ConfigureAwait(false);
        return await JsonSerializer.DeserializeAsync<T>(stream, DiscordJson.Options, ct).ConfigureAwait(false)
            ?? throw new JsonException($"Empty response body for {method} {path}.");
    }

    /// <summary>
    /// Fetches a JSON array, reading each element on its own so one unexpected shape can't make a
    /// whole page unreadable. Elements that fail use <paramref name="fallback"/> (or are skipped) and are logged.
    /// </summary>
    public async Task<List<T>> SendListAsync<T>(HttpMethod method, string path, Func<JsonElement, T?>? fallback = null,
        CancellationToken ct = default) where T : class
    {
        using var response = await SendCoreAsync(method, path, null, null, ct).ConfigureAwait(false);
        await using var stream = await response.Content.ReadAsStreamAsync(ct).ConfigureAwait(false);
        using var doc = await JsonDocument.ParseAsync(stream, cancellationToken: ct).ConfigureAwait(false);

        var items = new List<T>(doc.RootElement.GetArrayLength());
        foreach (var element in doc.RootElement.EnumerateArray())
        {
            try
            {
                items.Add(DiscordJson.Deserialize<T>(element));
            }
            // Converters can also throw FormatException/OverflowException for malformed values.
            catch (Exception ex) when (ex is JsonException or FormatException or OverflowException or InvalidOperationException)
            {
                var id = element.TryGetProperty("id", out var idElement) ? idElement.ToString() : "?";
                _logger.LogWarning(ex, "Couldn't read {Type} {Id} from {Path}; using a simplified copy", typeof(T).Name, id, path);
                if (fallback?.Invoke(element) is { } simplified)
                    items.Add(simplified);
            }
        }
        return items;
    }

    public async Task SendAsync(HttpMethod method, string path, object? body = null, CancellationToken ct = default)
    {
        using var response = await SendCoreAsync(method, path, body, null, ct).ConfigureAwait(false);
    }

    private async Task<HttpResponseMessage> SendCoreAsync(HttpMethod method, string path, object? body,
        Func<HttpContent>? contentFactory, CancellationToken ct)
    {
        var routeKey = RateLimiter.GetRouteKey(method, path);

        for (var attempt = 0; ; attempt++)
        {
            var bucket = await _rateLimiter.AcquireAsync(routeKey, ct).ConfigureAwait(false);
            HttpResponseMessage response;
            try
            {
                using var request = new HttpRequestMessage(method, path);
                request.Content = contentFactory?.Invoke() ?? (body is null ? null : JsonContent(body));
                response = await _http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, ct).ConfigureAwait(false);
                _rateLimiter.Update(bucket, response.Headers);

                if (response.StatusCode == HttpStatusCode.TooManyRequests)
                {
                    var (retryAfter, global) = await ReadRateLimitBodyAsync(response, ct).ConfigureAwait(false);
                    var wait = _rateLimiter.OnRateLimited(bucket, response.Headers, retryAfter, global);
                    response.Dispose();
                    _logger.LogWarning("Rate limited on {Route} (global: {Global}); retrying in {Wait}", routeKey, global, wait);
                    if (attempt >= MaxRateLimitRetries)
                        throw new DiscordApiException(HttpStatusCode.TooManyRequests, 0, "Rate limited", null);
                    continue;
                }
            }
            finally
            {
                RateLimiter.Release(bucket);
            }

            if (!response.IsSuccessStatusCode)
            {
                var text = await response.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
                response.Dispose();
                throw DiscordApiException.FromBody(response.StatusCode, text);
            }
            return response;
        }
    }

    private static ByteArrayContent JsonContent(object body)
    {
        var content = new ByteArrayContent(JsonSerializer.SerializeToUtf8Bytes(body, body.GetType(), DiscordJson.Options));
        content.Headers.ContentType = new MediaTypeHeaderValue("application/json");
        return content;
    }

    private static async Task<(double? RetryAfter, bool Global)> ReadRateLimitBodyAsync(HttpResponseMessage response, CancellationToken ct)
    {
        try
        {
            var text = await response.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
            using var doc = JsonDocument.Parse(text);
            var root = doc.RootElement;
            double? retryAfter = root.TryGetProperty("retry_after", out var r) && r.ValueKind == JsonValueKind.Number ? r.GetDouble() : null;
            var global = root.TryGetProperty("global", out var g) && g.ValueKind == JsonValueKind.True;
            return (retryAfter, global);
        }
        catch (JsonException)
        {
            return (null, false);
        }
    }

    #endregion

    public void Dispose() => _http.Dispose();
}
