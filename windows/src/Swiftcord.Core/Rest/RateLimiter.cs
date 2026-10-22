using System.Collections.Concurrent;
using System.Globalization;
using System.Net.Http.Headers;
using System.Text.RegularExpressions;

namespace Swiftcord.Core.Rest;

/// <summary>
/// Client-side rate limiting keyed by route. Requests on the same route are serialized and
/// delayed when the bucket is exhausted; a global limit pauses everything.
/// </summary>
internal sealed partial class RateLimiter(TimeProvider time)
{
    private readonly ConcurrentDictionary<string, Bucket> _buckets = new();
    private long _globalResetTicks;

    internal sealed class Bucket
    {
        public readonly SemaphoreSlim Gate = new(1, 1);
        public int Remaining = 1;
        public DateTimeOffset ResetAt;
    }

    public async Task<Bucket> AcquireAsync(string routeKey, CancellationToken ct)
    {
        var bucket = _buckets.GetOrAdd(routeKey, _ => new Bucket());
        await bucket.Gate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            await DelayUntilAsync(new DateTimeOffset(Interlocked.Read(ref _globalResetTicks), TimeSpan.Zero), ct).ConfigureAwait(false);
            if (bucket.Remaining <= 0)
                await DelayUntilAsync(bucket.ResetAt, ct).ConfigureAwait(false);
            return bucket;
        }
        catch
        {
            bucket.Gate.Release();
            throw;
        }
    }

    public static void Release(Bucket bucket) => bucket.Gate.Release();

    public void Update(Bucket bucket, HttpResponseHeaders headers)
    {
        if (TryGetHeader(headers, "X-RateLimit-Remaining", out var remaining))
            bucket.Remaining = (int)remaining;
        else
            bucket.Remaining = 1;

        if (TryGetHeader(headers, "X-RateLimit-Reset-After", out var resetAfter))
            bucket.ResetAt = time.GetUtcNow() + TimeSpan.FromSeconds(resetAfter);
    }

    /// <summary>Records a 429 and returns how long to wait before retrying.</summary>
    public TimeSpan OnRateLimited(Bucket bucket, HttpResponseHeaders headers, double? bodyRetryAfter, bool global)
    {
        var retryAfter = TimeSpan.FromSeconds(
            bodyRetryAfter ?? (TryGetHeader(headers, "Retry-After", out var h) ? h : 1));
        var resetAt = time.GetUtcNow() + retryAfter;

        if (global || headers.Contains("X-RateLimit-Global"))
            Interlocked.Exchange(ref _globalResetTicks, resetAt.UtcTicks);
        else
        {
            bucket.Remaining = 0;
            bucket.ResetAt = resetAt;
        }
        return retryAfter;
    }

    private async Task DelayUntilAsync(DateTimeOffset until, CancellationToken ct)
    {
        var delay = until - time.GetUtcNow();
        if (delay > TimeSpan.Zero)
            await Task.Delay(delay, time, ct).ConfigureAwait(false);
    }

    private static bool TryGetHeader(HttpResponseHeaders headers, string name, out double value)
    {
        value = 0;
        return headers.TryGetValues(name, out var values)
            && double.TryParse(values.FirstOrDefault(), NumberStyles.Float, CultureInfo.InvariantCulture, out value);
    }

    /// <summary>
    /// Reduces a request path to its rate-limit route: the top-level resource ID (channel, guild,
    /// webhook) is kept as a "major parameter", other IDs and reaction emoji are collapsed.
    /// </summary>
    public static string GetRouteKey(HttpMethod method, string path)
    {
        var trimmed = path.Split('?', 2)[0].Trim('/');
        trimmed = ReactionEmojiRegex().Replace(trimmed, "reactions/{emoji}");
        trimmed = MinorIdRegex().Replace(trimmed, m =>
            m.Groups["major"].Success ? m.Value : $"{m.Groups["prefix"].Value}{{id}}");
        return $"{method.Method} {trimmed}";
    }

    [GeneratedRegex(@"reactions/[^/]+")]
    private static partial Regex ReactionEmojiRegex();

    // Matches "<segment>/<id>". If the segment is a major resource at the start of the path, it's kept.
    [GeneratedRegex(@"(?<major>^(?:channels|guilds|webhooks)/\d+)|(?<prefix>(?:^|/)[^/]+/)\d{15,21}")]
    private static partial Regex MinorIdRegex();
}
