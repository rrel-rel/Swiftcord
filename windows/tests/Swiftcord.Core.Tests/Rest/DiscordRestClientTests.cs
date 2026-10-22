using System.Net;
using Microsoft.Extensions.Time.Testing;
using Swiftcord.Core.Models;
using Swiftcord.Core.Rest;
using Swiftcord.Core.Tests.Support;

namespace Swiftcord.Core.Tests.Rest;

public sealed class DiscordRestClientTests
{
    private static DiscordRestClient Client(FakeHttpHandler handler, TokenType type = TokenType.User, TimeProvider? time = null) =>
        new(new DiscordClientOptions { Token = "secret", TokenType = type }, handler, time);

    private const string MessageJson = """
        {"id":"200","channel_id":"100","author":{"id":"1","username":"me","discriminator":"0"},
         "content":"hi","timestamp":"2024-01-01T00:00:00+00:00","type":0,"nonce":12345}
        """;

    [Theory]
    [InlineData(TokenType.User, "secret")]
    [InlineData(TokenType.Bot, "Bot secret")]
    public async Task Sends_the_authorization_header_for_the_token_type(TokenType type, string expected)
    {
        var handler = new FakeHttpHandler((_, _) => FakeHttpHandler.Json("""{"id":"1","username":"me"}"""));
        using var client = Client(handler, type);

        var user = await client.GetCurrentUserAsync();

        Assert.Equal("me", user.Username);
        var request = handler.Requests.Single().Request;
        Assert.Equal(expected, request.Headers.GetValues("Authorization").Single());
        Assert.Equal("https://discord.com/api/v10/users/@me", request.RequestUri!.ToString());
    }

    [Fact]
    public async Task Builds_message_history_queries()
    {
        var handler = new FakeHttpHandler((_, _) => FakeHttpHandler.Json($"[{MessageJson}]"));
        using var client = Client(handler);

        var messages = await client.GetMessagesAsync(100, limit: 25, before: 500);

        Assert.Equal("12345", messages.Single().Nonce);
        Assert.Equal("/api/v10/channels/100/messages?limit=25&before=500", handler.Requests.Single().Request.RequestUri!.PathAndQuery);
    }

    [Fact]
    public async Task Sends_json_messages_in_snake_case_without_nulls()
    {
        var handler = new FakeHttpHandler((_, _) => FakeHttpHandler.Json(MessageJson));
        using var client = Client(handler);

        await client.SendMessageAsync(100, new MessageCreateRequest
        {
            Content = "hi",
            MessageReference = new MessageReference { MessageId = 50 },
        });

        var body = handler.Requests.Single().Body!;
        Assert.Contains("\"message_reference\":{\"type\":0,\"message_id\":\"50\"}", body);
        Assert.DoesNotContain("null", body);
    }

    [Fact]
    public async Task Uploads_attachments_as_multipart_with_payload_json()
    {
        var handler = new FakeHttpHandler((_, _) => FakeHttpHandler.Json(MessageJson));
        using var client = Client(handler);

        await client.SendMessageAsync(100, new MessageCreateRequest { Content = "pic" },
            [new FileUpload("cat.png", new MemoryStream([1, 2, 3]), "image/png")]);

        var (request, body) = handler.Requests.Single();
        Assert.Equal("multipart/form-data", request.Content!.Headers.ContentType!.MediaType);
        Assert.Contains("name=payload_json", body);
        Assert.Contains("\"attachments\":[{\"id\":0,\"filename\":\"cat.png\"}]", body);
        Assert.Contains("name=\"files[0]\"; filename=cat.png", body);
    }

    [Fact]
    public async Task Escapes_unicode_and_custom_emoji_in_reaction_routes()
    {
        var handler = new FakeHttpHandler((_, _) => new HttpResponseMessage(HttpStatusCode.NoContent));
        using var client = Client(handler);

        await client.AddReactionAsync(1, 2, new Emoji { Name = "👍" });
        await client.AddReactionAsync(1, 2, new Emoji { Name = "blob", Id = 99 });

        Assert.EndsWith("/reactions/%F0%9F%91%8D/@me", handler.Requests[0].Request.RequestUri!.AbsoluteUri);
        Assert.EndsWith("/reactions/blob%3A99/@me", handler.Requests[1].Request.RequestUri!.AbsoluteUri);
    }

    [Fact]
    public async Task Surfaces_discord_error_codes()
    {
        var handler = new FakeHttpHandler((_, _) =>
            FakeHttpHandler.Json("""{"code":50001,"message":"Missing Access"}""", HttpStatusCode.Forbidden));
        using var client = Client(handler);

        var ex = await Assert.ThrowsAsync<DiscordApiException>(() => client.GetChannelAsync(1));

        Assert.Equal(HttpStatusCode.Forbidden, ex.Status);
        Assert.Equal(50001, ex.Code);
        Assert.Equal("Missing Access", ex.ApiMessage);
    }

    [Fact]
    public async Task Retries_after_a_429_once_retry_after_elapses()
    {
        var time = new FakeTimeProvider();
        var calls = 0;
        var handler = new FakeHttpHandler((_, _) => ++calls == 1
            ? FakeHttpHandler.Json("""{"message":"You are being rate limited.","retry_after":2.5,"global":false}""", HttpStatusCode.TooManyRequests)
            : FakeHttpHandler.Json("""{"id":"1","username":"me"}"""));
        using var client = Client(handler, time: time);

        var task = client.GetCurrentUserAsync();
        await TestUtil.EventuallyAsync(() => calls == 1);
        time.Advance(TimeSpan.FromSeconds(2));
        await Task.Delay(50);
        Assert.False(task.IsCompleted, "should still be waiting out retry_after");

        time.Advance(TimeSpan.FromSeconds(1));
        var user = await task.WaitAsync(TimeSpan.FromSeconds(5));
        Assert.Equal("me", user.Username);
        Assert.Equal(2, calls);
    }

    [Fact]
    public async Task Waits_for_an_exhausted_bucket_to_reset_before_the_next_request()
    {
        var time = new FakeTimeProvider();
        var calls = 0;
        var handler = new FakeHttpHandler((_, _) =>
        {
            calls++;
            var response = FakeHttpHandler.Json("""{"id":"1","username":"me"}""");
            response.Headers.Add("X-RateLimit-Remaining", "0");
            response.Headers.Add("X-RateLimit-Reset-After", "1.000");
            return response;
        });
        using var client = Client(handler, time: time);

        await client.GetCurrentUserAsync();
        var second = client.GetCurrentUserAsync();
        await Task.Delay(50);
        Assert.Equal(1, calls);

        time.Advance(TimeSpan.FromSeconds(1.1));
        await second.WaitAsync(TimeSpan.FromSeconds(5));
        Assert.Equal(2, calls);
    }

    [Theory]
    [InlineData("GET", "channels/123456789012345678/messages/223456789012345678", "GET channels/123456789012345678/messages/{id}")]
    [InlineData("GET", "channels/123456789012345678/messages?limit=50", "GET channels/123456789012345678/messages")]
    [InlineData("PUT", "channels/123456789012345678/messages/223456789012345678/reactions/%F0%9F%91%8D/@me",
        "PUT channels/123456789012345678/messages/{id}/reactions/{emoji}/@me")]
    [InlineData("GET", "users/123456789012345678", "GET users/{id}")]
    [InlineData("GET", "guilds/123456789012345678/members/223456789012345678", "GET guilds/123456789012345678/members/{id}")]
    public void Route_keys_keep_major_parameters_only(string method, string path, string expected) =>
        Assert.Equal(expected, RateLimiter.GetRouteKey(new HttpMethod(method), path));
}
