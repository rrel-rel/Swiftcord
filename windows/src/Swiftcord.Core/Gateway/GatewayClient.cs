using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using Swiftcord.Core.Json;
using Swiftcord.Core.Models;

namespace Swiftcord.Core.Gateway;

/// <summary>
/// Maintains a Discord gateway connection: heartbeating, zombie detection, and automatic
/// resume or re-identify after disconnects.
/// </summary>
public sealed class GatewayClient : IAsyncDisposable
{
    private static readonly TimeSpan MaxBackoff = TimeSpan.FromSeconds(60);

    private readonly DiscordClientOptions _options;
    private readonly Func<IGatewayTransport> _transportFactory;
    private readonly TimeProvider _time;
    private readonly ILogger _logger;
    private readonly SemaphoreSlim _sendLock = new(1, 1);

    private CancellationTokenSource? _stopCts;
    private Task? _runTask;
    private IGatewayTransport? _transport;
    private TaskCompletionSource _ready = NewReadySource();

    private string? _sessionId;
    private string? _resumeUrl;
    private long _sequence = -1;
    private volatile bool _ackReceived;
    private long _heartbeatSentTimestamp;

    public GatewayClient(DiscordClientOptions options, Func<IGatewayTransport>? transportFactory = null,
        TimeProvider? time = null, ILogger<GatewayClient>? logger = null)
    {
        _options = options;
        _transportFactory = transportFactory ?? (() => new WebSocketGatewayTransport(options.UserAgent));
        _time = time ?? TimeProvider.System;
        _logger = logger ?? NullLogger<GatewayClient>.Instance;
    }

    /// <summary>Intents sent when identifying with a bot token.</summary>
    public GatewayIntents Intents { get; init; } = GatewayIntents.ChatClient;

    /// <summary>Use zlib-stream transport compression. Disabled in tests for readable frames.</summary>
    public bool Compress { get; init; } = true;

    public GatewayConnectionState State { get; private set; }

    /// <summary>Round-trip time of the last acknowledged heartbeat.</summary>
    public TimeSpan? Latency { get; private set; }

    public string? SessionId => _sessionId;

    /// <summary>
    /// Raised on the gateway thread for every DISPATCH event, with the event name and its "d" payload.
    /// The element is only valid for the duration of the call; Clone() it to keep it.
    /// </summary>
    public event Action<string, JsonElement>? Dispatch;

    public event Action<GatewayConnectionState>? StateChanged;

    /// <summary>Raised when the gateway closes with a code that prevents reconnecting (e.g. an invalid token).</summary>
    public event Action<int, string?>? FatalClose;

    public Task StartAsync()
    {
        if (_runTask is not null)
            throw new InvalidOperationException("The gateway is already running.");
        _stopCts = new CancellationTokenSource();
        _runTask = Task.Run(() => RunAsync(_stopCts.Token));
        return Task.CompletedTask;
    }

    /// <summary>Completes when the current connection has received READY or RESUMED.</summary>
    public Task WaitForReadyAsync(CancellationToken ct = default) => _ready.Task.WaitAsync(ct);

    public async Task StopAsync()
    {
        if (_stopCts is null || _runTask is null)
            return;

        // Closing with 1000 tells Discord to end the session rather than keep it resumable.
        if (_transport is { } transport)
        {
            try { await transport.CloseAsync(1000, "Client disconnect", CancellationToken.None).ConfigureAwait(false); }
            catch (Exception ex) { _logger.LogDebug(ex, "Error while closing gateway"); }
        }

        await _stopCts.CancelAsync().ConfigureAwait(false);
        try { await _runTask.ConfigureAwait(false); }
        catch (OperationCanceledException) { }

        _stopCts.Dispose();
        _stopCts = null;
        _runTask = null;
        ClearSession();
    }

    public async Task SendAsync<T>(GatewayOpcode op, T data, CancellationToken ct = default)
    {
        var transport = _transport ?? throw new InvalidOperationException("The gateway is not connected.");
        var payload = DiscordJson.SerializeToUtf8Bytes(new OutgoingPayload<T>((int)op, data));
        await _sendLock.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            await transport.SendTextAsync(payload, ct).ConfigureAwait(false);
        }
        finally
        {
            _sendLock.Release();
        }
    }

    /// <summary>
    /// Subscribes to typing, activities, threads and member-list ranges for a guild (user sessions only).
    /// Discord only delivers TYPING_START and member updates for large guilds after this.
    /// </summary>
    public Task SubscribeGuildAsync(Snowflake guildId, IReadOnlyDictionary<Snowflake, (int Start, int End)[]>? channelRanges = null,
        CancellationToken ct = default)
    {
        var channels = new JsonObject();
        foreach (var (channelId, ranges) in channelRanges ?? new Dictionary<Snowflake, (int, int)[]>())
            channels[channelId.ToString()] = new JsonArray(ranges.Select(r => (JsonNode)new JsonArray(r.Start, r.End)).ToArray());

        return SendAsync(GatewayOpcode.LazyGuildRequest, new JsonObject
        {
            ["guild_id"] = guildId.ToString(),
            ["typing"] = true,
            ["threads"] = true,
            ["activities"] = true,
            ["channels"] = channels,
        }, ct);
    }

    public Task UpdatePresenceAsync(PresenceStatus status, CancellationToken ct = default) =>
        SendAsync(GatewayOpcode.PresenceUpdate, new JsonObject
        {
            ["status"] = status.ToWire(),
            ["since"] = 0,
            ["activities"] = new JsonArray(),
            ["afk"] = false,
        }, ct);

    private async Task RunAsync(CancellationToken ct)
    {
        var attempt = 0;
        while (!ct.IsCancellationRequested)
        {
            SessionOutcome outcome;
            try
            {
                outcome = await RunSessionAsync(ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Gateway connection failed");
                outcome = new SessionOutcome();
            }

            if (outcome.Fatal)
            {
                _logger.LogError("Gateway closed with fatal code {Code}: {Reason}", outcome.CloseCode, outcome.CloseReason);
                SetState(GatewayConnectionState.Disconnected);
                _ready.TrySetException(new InvalidOperationException($"Gateway closed with code {outcome.CloseCode}: {outcome.CloseReason}"));
                FatalClose?.Invoke(outcome.CloseCode ?? 0, outcome.CloseReason);
                return;
            }

            if (outcome.Reidentify)
                ClearSession();
            if (outcome.WasReady)
                attempt = 0;
            if (_ready.Task.IsCompleted)
                _ready = NewReadySource();

            var delay = outcome.Delay ?? Backoff(attempt++);
            _logger.LogInformation("Gateway disconnected (code {Code}); reconnecting in {Delay}", outcome.CloseCode, delay);
            SetState(GatewayConnectionState.Reconnecting);
            try
            {
                await Task.Delay(delay, _time, ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                break;
            }
        }
        SetState(GatewayConnectionState.Disconnected);
    }

    private async Task<SessionOutcome> RunSessionAsync(CancellationToken stopToken)
    {
        using var sessionCts = CancellationTokenSource.CreateLinkedTokenSource(stopToken);
        var ct = sessionCts.Token;
        await using var transport = _transportFactory();
        using var zlib = new ZlibStreamDecompressor();

        var resuming = _sessionId is not null;
        var baseUrl = resuming && _resumeUrl is not null ? new Uri(_resumeUrl) : _options.GatewayUrl;
        SetState(GatewayConnectionState.Connecting);
        await transport.ConnectAsync(BuildUri(baseUrl), ct).ConfigureAwait(false);
        _transport = transport;

        Task? heartbeat = null;
        var wasReady = false;
        try
        {
            while (true)
            {
                var frame = await transport.ReceiveAsync(ct).ConfigureAwait(false);
                if (frame.IsClose)
                    return OutcomeForClose(frame.CloseCode!.Value, frame.CloseReason, wasReady);

                var data = frame.Data;
                if (frame.IsBinary && !zlib.TryDecompress(frame.Data.Span, out data))
                    continue;

                using var doc = JsonDocument.Parse(data);
                var root = doc.RootElement;
                var op = (GatewayOpcode)root.GetProperty("op").GetInt32();
                root.TryGetProperty("d", out var d);

                switch (op)
                {
                    case GatewayOpcode.Hello:
                        var interval = TimeSpan.FromMilliseconds(d.GetProperty("heartbeat_interval").GetDouble());
                        _ackReceived = true;
                        heartbeat = HeartbeatLoopAsync(transport, interval, sessionCts);
                        if (resuming)
                        {
                            SetState(GatewayConnectionState.Resuming);
                            await SendResumeAsync(ct).ConfigureAwait(false);
                        }
                        else
                        {
                            SetState(GatewayConnectionState.Identifying);
                            await SendIdentifyAsync(ct).ConfigureAwait(false);
                        }
                        break;

                    case GatewayOpcode.HeartbeatAck:
                        _ackReceived = true;
                        Latency = _time.GetElapsedTime(Interlocked.Read(ref _heartbeatSentTimestamp));
                        break;

                    case GatewayOpcode.Heartbeat:
                        await SendHeartbeatAsync(ct).ConfigureAwait(false);
                        break;

                    case GatewayOpcode.Reconnect:
                        _logger.LogInformation("Gateway requested reconnect");
                        await transport.CloseAsync(GatewayCloseCodes.UnknownError, "Reconnect requested", CancellationToken.None).ConfigureAwait(false);
                        return new SessionOutcome { WasReady = wasReady, Delay = TimeSpan.Zero };

                    case GatewayOpcode.InvalidSession:
                        var resumable = d.ValueKind == JsonValueKind.True;
                        _logger.LogInformation("Invalid session (resumable: {Resumable})", resumable);
                        await transport.CloseAsync(GatewayCloseCodes.UnknownError, "Invalid session", CancellationToken.None).ConfigureAwait(false);
                        // Discord asks clients to wait 1–5 seconds before identifying again.
                        return new SessionOutcome
                        {
                            Reidentify = !resumable,
                            WasReady = wasReady,
                            Delay = TimeSpan.FromMilliseconds(Random.Shared.Next(1000, 5000)),
                        };

                    case GatewayOpcode.Dispatch:
                        if (root.TryGetProperty("s", out var s) && s.ValueKind == JsonValueKind.Number)
                            Interlocked.Exchange(ref _sequence, s.GetInt64());
                        var name = root.GetProperty("t").GetString()!;
                        if (name == "READY")
                        {
                            _sessionId = d.GetProperty("session_id").GetString();
                            _resumeUrl = d.TryGetProperty("resume_gateway_url", out var r) ? r.GetString() : null;
                        }
                        if (name is "READY" or "RESUMED")
                        {
                            wasReady = true;
                            SetState(GatewayConnectionState.Connected);
                        }
                        OnDispatch(name, d);
                        if (wasReady)
                            _ready.TrySetResult();
                        break;
                }
            }
        }
        catch (OperationCanceledException) when (!stopToken.IsCancellationRequested)
        {
            // The heartbeat loop cancelled the session after a missed ACK.
            return new SessionOutcome { WasReady = wasReady };
        }
        finally
        {
            _transport = null;
            await sessionCts.CancelAsync().ConfigureAwait(false);
            if (heartbeat is not null)
            {
                try { await heartbeat.ConfigureAwait(false); }
                catch (OperationCanceledException) { }
            }
        }
    }

    private void OnDispatch(string name, JsonElement data)
    {
        try
        {
            Dispatch?.Invoke(name, data);
        }
        catch (Exception ex)
        {
            // A handler bug must not take the connection down.
            _logger.LogError(ex, "Error handling gateway event {Event}", name);
        }
    }

    private async Task HeartbeatLoopAsync(IGatewayTransport transport, TimeSpan interval, CancellationTokenSource sessionCts)
    {
        var ct = sessionCts.Token;
        // The first heartbeat is jittered so that reconnecting clients don't stampede.
        await Task.Delay(interval * Random.Shared.NextDouble(), _time, ct).ConfigureAwait(false);
        while (!ct.IsCancellationRequested)
        {
            if (!_ackReceived)
            {
                _logger.LogWarning("Heartbeat not acknowledged; reconnecting");
                await transport.CloseAsync(GatewayCloseCodes.UnknownError, "Heartbeat timeout", CancellationToken.None).ConfigureAwait(false);
                await sessionCts.CancelAsync().ConfigureAwait(false);
                return;
            }
            _ackReceived = false;
            await SendHeartbeatAsync(ct).ConfigureAwait(false);
            await Task.Delay(interval, _time, ct).ConfigureAwait(false);
        }
    }

    private Task SendHeartbeatAsync(CancellationToken ct)
    {
        var seq = Interlocked.Read(ref _sequence);
        Interlocked.Exchange(ref _heartbeatSentTimestamp, _time.GetTimestamp());
        return SendAsync<long?>(GatewayOpcode.Heartbeat, seq >= 0 ? seq : null, ct);
    }

    private Task SendIdentifyAsync(CancellationToken ct)
    {
        var identify = new JsonObject
        {
            ["token"] = _options.Token,
            ["properties"] = new JsonObject
            {
                ["os"] = "Windows",
                ["browser"] = "Swiftcord",
                ["device"] = "Swiftcord",
                ["system_locale"] = _options.Locale,
            },
        };

        if (_options.TokenType == TokenType.Bot)
        {
            identify["intents"] = (int)Intents;
        }
        else
        {
            identify["capabilities"] = _options.GatewayCapabilities;
            identify["compress"] = false;
            identify["presence"] = new JsonObject
            {
                ["status"] = "online",
                ["since"] = 0,
                ["activities"] = new JsonArray(),
                ["afk"] = false,
            };
            identify["client_state"] = new JsonObject { ["guild_versions"] = new JsonObject() };
        }

        return SendAsync(GatewayOpcode.Identify, identify, ct);
    }

    private Task SendResumeAsync(CancellationToken ct) =>
        SendAsync(GatewayOpcode.Resume, new JsonObject
        {
            ["token"] = _options.Token,
            ["session_id"] = _sessionId,
            ["seq"] = Interlocked.Read(ref _sequence),
        }, ct);

    private SessionOutcome OutcomeForClose(int code, string? reason, bool wasReady) => new()
    {
        CloseCode = code,
        CloseReason = reason,
        WasReady = wasReady,
        Fatal = GatewayCloseCodes.IsFatal(code),
        Reidentify = GatewayCloseCodes.RequiresReidentify(code),
    };

    private Uri BuildUri(Uri baseUrl)
    {
        var query = Compress ? "?v=10&encoding=json&compress=zlib-stream" : "?v=10&encoding=json";
        return new Uri(baseUrl.GetLeftPart(UriPartial.Authority) + "/" + query);
    }

    private void ClearSession()
    {
        _sessionId = null;
        _resumeUrl = null;
        Interlocked.Exchange(ref _sequence, -1);
    }

    private void SetState(GatewayConnectionState state)
    {
        if (State == state) return;
        State = state;
        StateChanged?.Invoke(state);
    }

    /// <summary>Exponential backoff with jitter: ~1s, 2s, 4s … capped at 60s.</summary>
    private static TimeSpan Backoff(int attempt)
    {
        var seconds = Math.Min(MaxBackoff.TotalSeconds, Math.Pow(2, Math.Min(attempt, 6)));
        return TimeSpan.FromSeconds(seconds * (0.75 + Random.Shared.NextDouble() * 0.5));
    }

    private static TaskCompletionSource NewReadySource() => new(TaskCreationOptions.RunContinuationsAsynchronously);

    public async ValueTask DisposeAsync()
    {
        await StopAsync().ConfigureAwait(false);
        _sendLock.Dispose();
    }

    // "d" must be sent even when null (e.g. the first heartbeat), overriding the global ignore-null policy.
    private sealed record OutgoingPayload<T>(int Op, [property: JsonIgnore(Condition = JsonIgnoreCondition.Never)] T D);

    private readonly record struct SessionOutcome
    {
        public bool Fatal { get; init; }
        public bool Reidentify { get; init; }
        public bool WasReady { get; init; }
        public TimeSpan? Delay { get; init; }
        public int? CloseCode { get; init; }
        public string? CloseReason { get; init; }
    }
}
