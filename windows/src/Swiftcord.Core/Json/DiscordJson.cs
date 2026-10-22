using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace Swiftcord.Core.Json;

public static class DiscordJson
{
    /// <summary>Serializer options matching Discord's snake_case wire format.</summary>
    public static JsonSerializerOptions Options { get; } = CreateOptions();

    private static JsonSerializerOptions CreateOptions()
    {
        var options = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
            NumberHandling = JsonNumberHandling.AllowReadingFromString,
            // Discord adds fields constantly; never fail on unknown members.
            UnmappedMemberHandling = JsonUnmappedMemberHandling.Skip,
        };
        options.MakeReadOnly(populateMissingResolver: true);
        return options;
    }

    public static T Deserialize<T>(JsonElement element) =>
        element.Deserialize<T>(Options) ?? throw new JsonException($"Null {typeof(T).Name} payload.");

    public static T Deserialize<T>(ReadOnlySpan<byte> utf8) =>
        JsonSerializer.Deserialize<T>(utf8, Options) ?? throw new JsonException($"Null {typeof(T).Name} payload.");

    public static byte[] SerializeToUtf8Bytes<T>(T value) => JsonSerializer.SerializeToUtf8Bytes(value, Options);

    /// <summary>
    /// Applies a partial update (e.g. a MESSAGE_UPDATE that omits unchanged fields) on top of an
    /// existing object and returns the merged result. Fields present in <paramref name="patch"/> win.
    /// </summary>
    public static T Merge<T>(T existing, JsonElement patch)
    {
        var target = JsonSerializer.SerializeToNode(existing, Options) as JsonObject
            ?? throw new JsonException($"{typeof(T).Name} did not serialize to an object.");
        foreach (var property in patch.EnumerateObject())
            target[property.Name] = JsonNode.Parse(property.Value.GetRawText());
        return target.Deserialize<T>(Options) ?? throw new JsonException($"Null {typeof(T).Name} after merge.");
    }
}
