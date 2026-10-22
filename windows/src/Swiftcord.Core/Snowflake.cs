using System.Globalization;
using System.Text.Json.Serialization;
using Swiftcord.Core.Json;

namespace Swiftcord.Core;

/// <summary>A Discord snowflake ID. Serialized as a string on the wire.</summary>
[JsonConverter(typeof(SnowflakeConverter))]
public readonly record struct Snowflake(ulong Value) : IComparable<Snowflake>
{
    /// <summary>Milliseconds since the Unix epoch at 2015-01-01T00:00:00Z.</summary>
    public const long DiscordEpoch = 1420070400000;

    public DateTimeOffset CreatedAt =>
        DateTimeOffset.FromUnixTimeMilliseconds((long)(Value >> 22) + DiscordEpoch);

    /// <summary>The smallest snowflake that could have been created at <paramref name="time"/>.</summary>
    public static Snowflake FromTime(DateTimeOffset time) =>
        new((ulong)(time.ToUnixTimeMilliseconds() - DiscordEpoch) << 22);

    public static Snowflake Parse(string s) => new(ulong.Parse(s, CultureInfo.InvariantCulture));

    public static bool TryParse(string? s, out Snowflake id)
    {
        if (ulong.TryParse(s, NumberStyles.None, CultureInfo.InvariantCulture, out var value))
        {
            id = new Snowflake(value);
            return true;
        }
        id = default;
        return false;
    }

    public int CompareTo(Snowflake other) => Value.CompareTo(other.Value);

    public static bool operator <(Snowflake a, Snowflake b) => a.Value < b.Value;
    public static bool operator >(Snowflake a, Snowflake b) => a.Value > b.Value;
    public static bool operator <=(Snowflake a, Snowflake b) => a.Value <= b.Value;
    public static bool operator >=(Snowflake a, Snowflake b) => a.Value >= b.Value;

    public static implicit operator Snowflake(ulong value) => new(value);

    public override string ToString() => Value.ToString(CultureInfo.InvariantCulture);
}
