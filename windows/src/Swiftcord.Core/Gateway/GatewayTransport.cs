using System.Buffers;
using System.Net.WebSockets;

namespace Swiftcord.Core.Gateway;

/// <summary>A single received WebSocket message, or the close that ended the connection.</summary>
public readonly record struct GatewayFrame(ReadOnlyMemory<byte> Data, bool IsBinary, int? CloseCode = null, string? CloseReason = null)
{
    public bool IsClose => CloseCode is not null;

    public static GatewayFrame Closed(int code, string? reason = null) => new(default, false, code, reason);
}

/// <summary>Abstraction over the WebSocket so the gateway state machine can be tested without a network.</summary>
public interface IGatewayTransport : IAsyncDisposable
{
    Task ConnectAsync(Uri uri, CancellationToken ct);

    Task SendTextAsync(ReadOnlyMemory<byte> utf8, CancellationToken ct);

    /// <summary>Receives a complete message. The returned data is valid until the next call.</summary>
    Task<GatewayFrame> ReceiveAsync(CancellationToken ct);

    Task CloseAsync(int code, string reason, CancellationToken ct);
}

public sealed class WebSocketGatewayTransport : IGatewayTransport
{
    private readonly ClientWebSocket _socket = new();
    private readonly ArrayBufferWriter<byte> _buffer = new(64 * 1024);

    public WebSocketGatewayTransport(string? userAgent = null)
    {
        if (userAgent is not null)
            _socket.Options.SetRequestHeader("User-Agent", userAgent);
        _socket.Options.KeepAliveInterval = TimeSpan.Zero; // Discord's heartbeat handles liveness.
    }

    public Task ConnectAsync(Uri uri, CancellationToken ct) => _socket.ConnectAsync(uri, ct);

    public Task SendTextAsync(ReadOnlyMemory<byte> utf8, CancellationToken ct) =>
        _socket.SendAsync(utf8, WebSocketMessageType.Text, endOfMessage: true, ct).AsTask();

    public async Task<GatewayFrame> ReceiveAsync(CancellationToken ct)
    {
        _buffer.ResetWrittenCount();
        while (true)
        {
            ValueWebSocketReceiveResult result;
            try
            {
                result = await _socket.ReceiveAsync(_buffer.GetMemory(16 * 1024), ct).ConfigureAwait(false);
            }
            catch (WebSocketException) when (_socket.CloseStatus is { } status)
            {
                return GatewayFrame.Closed((int)status, _socket.CloseStatusDescription);
            }

            if (result.MessageType == WebSocketMessageType.Close)
                return GatewayFrame.Closed((int)(_socket.CloseStatus ?? WebSocketCloseStatus.Empty), _socket.CloseStatusDescription);

            _buffer.Advance(result.Count);
            if (result.EndOfMessage)
                return new GatewayFrame(_buffer.WrittenMemory, result.MessageType == WebSocketMessageType.Binary);
        }
    }

    public async Task CloseAsync(int code, string reason, CancellationToken ct)
    {
        if (_socket.State is WebSocketState.Open or WebSocketState.CloseReceived)
        {
            try
            {
                await _socket.CloseOutputAsync((WebSocketCloseStatus)code, reason, ct).ConfigureAwait(false);
            }
            catch (WebSocketException)
            {
                // The connection is already gone.
            }
        }
    }

    public ValueTask DisposeAsync()
    {
        _socket.Dispose();
        return ValueTask.CompletedTask;
    }
}
