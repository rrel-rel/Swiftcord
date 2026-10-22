using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using Swiftcord.Core.Gateway;

namespace Swiftcord.Core.Tests.Support;

/// <summary>An in-memory gateway connection the test plays the server side of.</summary>
internal sealed class FakeGatewayTransport : IGatewayTransport
{
    private readonly Channel<GatewayFrame> _incoming = Channel.CreateUnbounded<GatewayFrame>();
    private readonly List<JsonObject> _sent = [];

    public Uri? ConnectedUri { get; private set; }
    public int? ClosedWithCode { get; private set; }
    public bool Disposed { get; private set; }

    public IReadOnlyList<JsonObject> Sent
    {
        get { lock (_sent) return _sent.ToList(); }
    }

    public IReadOnlyList<JsonObject> SentWithOp(GatewayOpcode op) =>
        Sent.Where(p => (int)p["op"]! == (int)op).ToList();

    public Task ConnectAsync(Uri uri, CancellationToken ct)
    {
        ConnectedUri = uri;
        return Task.CompletedTask;
    }

    public Task SendTextAsync(ReadOnlyMemory<byte> utf8, CancellationToken ct)
    {
        var payload = JsonNode.Parse(utf8.Span)!.AsObject();
        lock (_sent) _sent.Add(payload);
        return Task.CompletedTask;
    }

    public async Task<GatewayFrame> ReceiveAsync(CancellationToken ct) =>
        await _incoming.Reader.ReadAsync(ct);

    public Task CloseAsync(int code, string reason, CancellationToken ct)
    {
        ClosedWithCode ??= code;
        _incoming.Writer.TryWrite(GatewayFrame.Closed(code, reason));
        return Task.CompletedTask;
    }

    public ValueTask DisposeAsync()
    {
        Disposed = true;
        return ValueTask.CompletedTask;
    }

    public void ServerSend(GatewayOpcode op, object? d = null, int? s = null, string? t = null)
    {
        var json = JsonSerializer.Serialize(new { op = (int)op, d, s, t });
        _incoming.Writer.TryWrite(new GatewayFrame(Encoding.UTF8.GetBytes(json), IsBinary: false));
    }

    public void ServerHello(int intervalMs = 41250) =>
        ServerSend(GatewayOpcode.Hello, new { heartbeat_interval = intervalMs });

    public void ServerDispatch(string name, object d, int seq) =>
        ServerSend(GatewayOpcode.Dispatch, d, seq, name);

    public void ServerClose(int code, string reason = "") =>
        _incoming.Writer.TryWrite(GatewayFrame.Closed(code, reason));
}
