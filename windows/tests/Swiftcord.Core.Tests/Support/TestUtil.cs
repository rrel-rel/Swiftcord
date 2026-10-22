using System.Net;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Time.Testing;

namespace Swiftcord.Core.Tests.Support;

internal static class TestUtil
{
    /// <summary>
    /// Polls until <paramref name="condition"/> holds, advancing fake time by <paramref name="step"/>
    /// on every poll so delays inside the code under test elapse.
    /// </summary>
    public static async Task EventuallyAsync(Func<bool> condition, FakeTimeProvider? time = null, TimeSpan? step = null,
        string? because = null)
    {
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(5);
        while (!condition())
        {
            if (DateTime.UtcNow > deadline)
                Assert.Fail($"Condition not met within 5s{(because is null ? "" : $": {because}")}");
            time?.Advance(step ?? TimeSpan.FromMilliseconds(100));
            await Task.Delay(5);
        }
    }

    public static JsonElement ParseJson(string json) => JsonDocument.Parse(json).RootElement.Clone();

    public static JsonElement ParseJson(object value) => JsonSerializer.SerializeToElement(value);
}

/// <summary>An HttpMessageHandler that answers from a test-supplied function and records requests.</summary>
internal sealed class FakeHttpHandler(Func<HttpRequestMessage, string?, HttpResponseMessage> respond) : HttpMessageHandler
{
    public List<(HttpRequestMessage Request, string? Body)> Requests { get; } = [];

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
    {
        var body = request.Content is null ? null : await request.Content.ReadAsStringAsync(ct);
        lock (Requests) Requests.Add((request, body));
        return respond(request, body);
    }

    public static HttpResponseMessage Json(string json, HttpStatusCode status = HttpStatusCode.OK) =>
        new(status) { Content = new StringContent(json, Encoding.UTF8, "application/json") };
}
