using Microsoft.Extensions.Time.Testing;
using Swiftcord.Core.Gateway;
using Swiftcord.Core.Tests.Support;

namespace Swiftcord.Core.Tests.Gateway;

public sealed class GatewayClientTests : IAsyncLifetime
{
    private readonly FakeTimeProvider _time = new();
    private readonly List<FakeGatewayTransport> _transports = [];
    private readonly GatewayClient _gateway;

    public GatewayClientTests()
    {
        var options = new DiscordClientOptions { Token = "user-token" };
        _gateway = new GatewayClient(options, () =>
        {
            var transport = new FakeGatewayTransport();
            lock (_transports) _transports.Add(transport);
            return transport;
        }, _time)
        { Compress = false };
    }

    private FakeGatewayTransport Transport(int index)
    {
        lock (_transports) return _transports[index];
    }

    private int TransportCount
    {
        get { lock (_transports) return _transports.Count; }
    }

    public Task InitializeAsync() => Task.CompletedTask;

    public async Task DisposeAsync() => await _gateway.DisposeAsync();

    private async Task<FakeGatewayTransport> ConnectAndReadyAsync()
    {
        await _gateway.StartAsync();
        await TestUtil.EventuallyAsync(() => TransportCount == 1);
        var transport = Transport(0);
        transport.ServerHello();
        await TestUtil.EventuallyAsync(() => transport.SentWithOp(GatewayOpcode.Identify).Count == 1);
        transport.ServerDispatch("READY", new
        {
            session_id = "session-1",
            resume_gateway_url = "wss://resume.discord.gg",
            user = new { id = "1", username = "me" },
        }, seq: 1);
        await _gateway.WaitForReadyAsync().WaitAsync(TimeSpan.FromSeconds(5));
        return transport;
    }

    [Fact]
    public async Task Identifies_as_a_user_session_after_hello()
    {
        var transport = await ConnectAndReadyAsync();

        var identify = transport.SentWithOp(GatewayOpcode.Identify).Single()["d"]!;
        Assert.Equal("user-token", (string?)identify["token"]);
        Assert.Equal("Windows", (string?)identify["properties"]!["os"]);
        Assert.NotNull(identify["capabilities"]);
        Assert.Null(identify["intents"]);
        Assert.Equal(GatewayConnectionState.Connected, _gateway.State);
        Assert.Equal("session-1", _gateway.SessionId);
        Assert.Equal("/?v=10&encoding=json", transport.ConnectedUri!.PathAndQuery);
    }

    [Fact]
    public async Task Bot_tokens_identify_with_intents()
    {
        var bot = new GatewayClient(new DiscordClientOptions { Token = "bot", TokenType = TokenType.Bot },
            () => { var t = new FakeGatewayTransport(); lock (_transports) _transports.Add(t); return t; }, _time)
        { Compress = false, Intents = GatewayIntents.Guilds };
        await bot.StartAsync();
        await TestUtil.EventuallyAsync(() => TransportCount == 1);
        Transport(0).ServerHello();
        await TestUtil.EventuallyAsync(() => Transport(0).SentWithOp(GatewayOpcode.Identify).Count == 1);

        var identify = Transport(0).SentWithOp(GatewayOpcode.Identify).Single()["d"]!;
        Assert.Equal(1, (int?)identify["intents"]);
        Assert.Null(identify["capabilities"]);
        await bot.DisposeAsync();
    }

    [Fact]
    public async Task Heartbeats_with_the_last_sequence_and_answers_server_heartbeat_requests()
    {
        var transport = await ConnectAndReadyAsync();
        transport.ServerDispatch("MESSAGE_CREATE", new { id = "5" }, seq: 7);

        // The first heartbeat fires after a jittered fraction of the interval.
        await TestUtil.EventuallyAsync(() => transport.SentWithOp(GatewayOpcode.Heartbeat).Count >= 1, _time, TimeSpan.FromSeconds(5));
        Assert.Equal(7, (long?)transport.SentWithOp(GatewayOpcode.Heartbeat)[0]["d"]);

        var before = transport.SentWithOp(GatewayOpcode.Heartbeat).Count;
        transport.ServerSend(GatewayOpcode.Heartbeat);
        await TestUtil.EventuallyAsync(() => transport.SentWithOp(GatewayOpcode.Heartbeat).Count == before + 1);
    }

    [Fact]
    public async Task First_heartbeat_sends_null_sequence_before_any_dispatch()
    {
        await _gateway.StartAsync();
        await TestUtil.EventuallyAsync(() => TransportCount == 1);
        var transport = Transport(0);
        transport.ServerHello();
        await TestUtil.EventuallyAsync(() => transport.SentWithOp(GatewayOpcode.Heartbeat).Count == 1, _time, TimeSpan.FromSeconds(5));

        var heartbeat = transport.SentWithOp(GatewayOpcode.Heartbeat)[0];
        Assert.True(heartbeat.ContainsKey("d"));
        Assert.Null(heartbeat["d"]);
    }

    [Fact]
    public async Task Resumes_on_the_resume_url_after_a_reconnect_request()
    {
        var first = await ConnectAndReadyAsync();
        first.ServerDispatch("MESSAGE_CREATE", new { id = "5" }, seq: 42);
        first.ServerSend(GatewayOpcode.Reconnect);

        await TestUtil.EventuallyAsync(() => TransportCount == 2, _time);
        var second = Transport(1);
        Assert.Equal("resume.discord.gg", second.ConnectedUri!.Host);
        // Closing with a non-1000 code keeps the session resumable.
        Assert.Equal(GatewayCloseCodes.UnknownError, first.ClosedWithCode);

        second.ServerHello();
        await TestUtil.EventuallyAsync(() => second.SentWithOp(GatewayOpcode.Resume).Count == 1);
        var resume = second.SentWithOp(GatewayOpcode.Resume).Single()["d"]!;
        Assert.Equal("session-1", (string?)resume["session_id"]);
        Assert.Equal(42, (long?)resume["seq"]);
        Assert.Empty(second.SentWithOp(GatewayOpcode.Identify));

        second.ServerDispatch("RESUMED", new { }, seq: 43);
        await TestUtil.EventuallyAsync(() => _gateway.State == GatewayConnectionState.Connected);
    }

    [Fact]
    public async Task Non_resumable_invalid_session_identifies_again_on_the_default_url()
    {
        var first = await ConnectAndReadyAsync();
        first.ServerSend(GatewayOpcode.InvalidSession, false);

        await TestUtil.EventuallyAsync(() => TransportCount == 2, _time, TimeSpan.FromSeconds(1));
        var second = Transport(1);
        Assert.Equal("gateway.discord.gg", second.ConnectedUri!.Host);
        second.ServerHello();
        await TestUtil.EventuallyAsync(() => second.SentWithOp(GatewayOpcode.Identify).Count == 1);
        Assert.Empty(second.SentWithOp(GatewayOpcode.Resume));
    }

    [Fact]
    public async Task Session_timeout_close_code_forces_reidentify()
    {
        var first = await ConnectAndReadyAsync();
        first.ServerClose(GatewayCloseCodes.SessionTimedOut);

        await TestUtil.EventuallyAsync(() => TransportCount == 2, _time, TimeSpan.FromSeconds(1));
        Transport(1).ServerHello();
        await TestUtil.EventuallyAsync(() => Transport(1).SentWithOp(GatewayOpcode.Identify).Count == 1);
    }

    [Fact]
    public async Task Authentication_failure_is_fatal_and_does_not_reconnect()
    {
        int? fatalCode = null;
        _gateway.FatalClose += (code, _) => fatalCode = code;
        await _gateway.StartAsync();
        await TestUtil.EventuallyAsync(() => TransportCount == 1);
        Transport(0).ServerHello();
        Transport(0).ServerClose(GatewayCloseCodes.AuthenticationFailed, "Authentication failed.");

        await TestUtil.EventuallyAsync(() => fatalCode is not null);
        Assert.Equal(4004, fatalCode);
        await Assert.ThrowsAsync<InvalidOperationException>(() => _gateway.WaitForReadyAsync());

        _time.Advance(TimeSpan.FromMinutes(5));
        await Task.Delay(50);
        Assert.Equal(1, TransportCount);
        Assert.Equal(GatewayConnectionState.Disconnected, _gateway.State);
    }

    [Fact]
    public async Task Missing_heartbeat_ack_reconnects_and_resumes()
    {
        var first = await ConnectAndReadyAsync();
        // Never ACK: the second heartbeat tick detects the zombie connection.
        await TestUtil.EventuallyAsync(() => TransportCount == 2, _time, TimeSpan.FromSeconds(5),
            because: "a zombied connection should be replaced");
        Assert.Equal(GatewayCloseCodes.UnknownError, first.ClosedWithCode);

        Transport(1).ServerHello();
        await TestUtil.EventuallyAsync(() => Transport(1).SentWithOp(GatewayOpcode.Resume).Count == 1);
    }

    [Fact]
    public async Task Dispatch_handler_exceptions_do_not_drop_the_connection()
    {
        var received = new List<string>();
        _gateway.Dispatch += (name, _) =>
        {
            if (name == "BOOM") throw new InvalidOperationException();
            received.Add(name);
        };
        var transport = await ConnectAndReadyAsync();
        transport.ServerDispatch("BOOM", new { }, seq: 2);
        transport.ServerDispatch("AFTER", new { }, seq: 3);

        await TestUtil.EventuallyAsync(() => received.Contains("AFTER"));
        Assert.Equal(1, TransportCount);
    }

    [Fact]
    public async Task Stop_closes_with_1000_to_end_the_session()
    {
        var transport = await ConnectAndReadyAsync();
        await _gateway.StopAsync();

        Assert.Equal(1000, transport.ClosedWithCode);
        Assert.Null(_gateway.SessionId);
        Assert.Equal(GatewayConnectionState.Disconnected, _gateway.State);
    }
}
