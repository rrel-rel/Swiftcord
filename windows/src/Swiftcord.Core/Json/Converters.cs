using System.Globalization;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Swiftcord.Core.Json;

/// <summary>Reads snowflakes from either JSON strings or numbers and writes them as strings.</summary>
public sealed class SnowflakeConverter : JsonConverter<Snowflake>
{
    public override Snowflake Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options) =>
        reader.TokenType switch
        {
            JsonTokenType.String => Snowflake.Parse(reader.GetString()!),
            JsonTokenType.Number => new Snowflake(reader.GetUInt64()),
            _ => throw new JsonException($"Unexpected token {reader.TokenType} for snowflake."),
        };

    public override void Write(Utf8JsonWriter writer, Snowflake value, JsonSerializerOptions options) =>
        writer.WriteStringValue(value.ToString());
}

/// <summary>
/// Permission bitfields are sent as decimal strings because they exceed 53 bits.
/// </summary>
public sealed class StringULongConverter : JsonConverter<ulong>
{
    public override ulong Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options) =>
        reader.TokenType switch
        {
            JsonTokenType.String => ulong.Parse(reader.GetString()!, CultureInfo.InvariantCulture),
            JsonTokenType.Number => reader.GetUInt64(),
            _ => throw new JsonException($"Unexpected token {reader.TokenType} for bitfield."),
        };

    public override void Write(Utf8JsonWriter writer, ulong value, JsonSerializerOptions options) =>
        writer.WriteStringValue(value.ToString(CultureInfo.InvariantCulture));
}

/// <summary>Message nonces may be strings or integers; normalize them to strings.</summary>
public sealed class NonceConverter : JsonConverter<string?>
{
    public override bool HandleNull => true;

    public override string? Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options) =>
        reader.TokenType switch
        {
            JsonTokenType.Null => null,
            JsonTokenType.String => reader.GetString(),
            JsonTokenType.Number => reader.GetInt64().ToString(CultureInfo.InvariantCulture),
            _ => throw new JsonException($"Unexpected token {reader.TokenType} for nonce."),
        };

    public override void Write(Utf8JsonWriter writer, string? value, JsonSerializerOptions options)
    {
        if (value is null) writer.WriteNullValue();
        else writer.WriteStringValue(value);
    }
}
