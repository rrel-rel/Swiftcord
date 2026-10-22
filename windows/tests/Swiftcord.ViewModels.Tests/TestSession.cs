using System.Net;
using System.Text.Json;
using Swiftcord.Core;
using Swiftcord.Core.Tests.Support;

namespace Swiftcord.ViewModels.Tests;

/// <summary>
/// A DiscordClient whose cache is seeded with a READY payload and whose REST calls are answered
/// by <see cref="Respond"/>. The gateway is never connected.
/// </summary>
internal sealed class TestSession : IAsyncDisposable
{
    public const ulong Me = 1, Friend = 2, GuildId = 10, General = 100, RandomChannel = 101, Dm = 200;
    public const ulong ViewSendReact = (1 << 10) | (1 << 11) | (1 << 6) | (1 << 16) | (1 << 15);

    public TestSession()
    {
        Http = new FakeHttpHandler((request, body) => Respond(request, body));
        Client = new DiscordClient(new DiscordClientOptions { Token = "t" }, httpHandler: Http);
    }

    public FakeHttpHandler Http { get; }
    public DiscordClient Client { get; }

    /// <summary>Answers REST requests; defaults to an empty message history.</summary>
    public Func<HttpRequestMessage, string?, HttpResponseMessage> Respond { get; set; } =
        (_, _) => FakeHttpHandler.Json("[]");

    public IEnumerable<string> RequestLines =>
        Http.Requests.Select(r => $"{r.Request.Method} {r.Request.RequestUri!.PathAndQuery}");

    public TestSession WithReady()
    {
        Dispatch("READY", $$"""
        {
          "v": 9, "session_id": "s",
          "user": {"id":"{{Me}}","username":"me","global_name":"Me"},
          "guilds": [{
            "id":"{{GuildId}}","name":"Swift Community","owner_id":"999",
            "roles":[{"id":"{{GuildId}}","name":"@everyone","permissions":"{{ViewSendReact}}","position":0}],
            "channels":[
              {"id":"{{RandomChannel}}","type":0,"name":"random","position":1},
              {"id":"{{General}}","type":0,"name":"general","position":0,"last_message_id":"50"}
            ]
          }],
          "merged_members": [[{"user_id":"{{Me}}","roles":[]}]],
          "private_channels": [{"id":"{{Dm}}","type":1,"recipients":[{"id":"{{Friend}}","username":"friend","global_name":"Friend"}],"last_message_id":"60"}],
          "read_state": [{"id":"{{General}}","last_message_id":"40","mention_count":0}]
        }
        """);
        return this;
    }

    public void Dispatch(string name, string json)
    {
        using var doc = JsonDocument.Parse(json);
        Client.State.HandleDispatch(name, doc.RootElement);
    }

    public static string MessageJson(ulong id, ulong channel, ulong author, string content = "hi",
        string timestamp = "2024-05-01T12:00:00+00:00", string extra = "") => $$"""
        {"id":"{{id}}","channel_id":"{{channel}}","author":{"id":"{{author}}","username":"u{{author}}"},
         "content":"{{content}}","timestamp":"{{timestamp}}","type":0{{extra}}}
        """;

    public static HttpResponseMessage Json(string json, HttpStatusCode status = HttpStatusCode.OK) =>
        FakeHttpHandler.Json(json, status);

    public ValueTask DisposeAsync() => Client.DisposeAsync();
}
