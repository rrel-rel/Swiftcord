using System.Net;
using System.Text.Json;

namespace Swiftcord.Core.Rest;

/// <summary>A non-success response from the Discord REST API.</summary>
public sealed class DiscordApiException(HttpStatusCode status, int code, string message, JsonElement? errors)
    : Exception($"Discord API error {(int)status} ({code}): {message}")
{
    public HttpStatusCode Status { get; } = status;
    /// <summary>Discord's JSON error code, e.g. 50001 (Missing Access). 0 when the body had none.</summary>
    public int Code { get; } = code;
    public string ApiMessage { get; } = message;
    /// <summary>Field-level validation errors, when present.</summary>
    public JsonElement? Errors { get; } = errors;

    internal static DiscordApiException FromBody(HttpStatusCode status, string body)
    {
        try
        {
            using var doc = JsonDocument.Parse(body);
            var root = doc.RootElement;
            var code = root.TryGetProperty("code", out var c) && c.ValueKind == JsonValueKind.Number ? c.GetInt32() : 0;
            var message = root.TryGetProperty("message", out var m) ? m.GetString() ?? "" : "";
            JsonElement? errors = root.TryGetProperty("errors", out var e) ? e.Clone() : null;
            return new DiscordApiException(status, code, message, errors);
        }
        catch (JsonException)
        {
            return new DiscordApiException(status, 0, string.IsNullOrWhiteSpace(body) ? status.ToString() : body, null);
        }
    }
}
