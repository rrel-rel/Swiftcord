namespace Swiftcord.Core.Models;

/// <summary>
/// A server folder from the user's settings. Servers that aren't in a real folder still appear as a
/// folder of one with no <see cref="Id"/>, which is how Discord records the order of the whole list.
/// </summary>
public sealed class GuildFolder
{
    /// <summary>Null for a lone server rather than a real folder.</summary>
    public long? Id { get; set; }
    public string? Name { get; set; }
    public int? Color { get; set; }
    public List<Snowflake> GuildIds { get; set; } = [];
}

/// <summary>
/// Reads the parts of Discord's protobuf-encoded user settings (PreloadedUserSettings) that the app uses.
/// Only the wire format is needed, so this avoids a protobuf dependency; unknown fields are skipped.
/// </summary>
public static class UserSettingsReader
{
    private const int GuildFoldersField = 14;

    /// <summary>
    /// Returns the server folders in display order, or null when the settings don't include them
    /// (e.g. a partial update that changed something else).
    /// </summary>
    public static List<GuildFolder>? ReadGuildFolders(string base64)
    {
        byte[] bytes;
        try { bytes = Convert.FromBase64String(base64); }
        catch (FormatException) { return null; }

        try
        {
            List<GuildFolder>? folders = null;
            foreach (var (field, value) in Fields(bytes))
            {
                if (field == GuildFoldersField)
                    folders = ReadGuildFolders(value);
            }
            return folders;
        }
        catch (FormatException)
        {
            return null;
        }
    }

    // message GuildFolders { repeated GuildFolder folders = 1; repeated fixed64 guild_positions = 2; }
    private static List<GuildFolder> ReadGuildFolders(ReadOnlyMemory<byte> data)
    {
        var folders = new List<GuildFolder>();
        foreach (var (field, value) in Fields(data))
        {
            if (field == 1)
                folders.Add(ReadGuildFolder(value));
        }
        return folders;
    }

    // message GuildFolder { repeated fixed64 guild_ids = 1; Int64Value id = 2; StringValue name = 3; UInt64Value color = 4; }
    private static GuildFolder ReadGuildFolder(ReadOnlyMemory<byte> data)
    {
        var folder = new GuildFolder();
        foreach (var (field, value) in Fields(data))
        {
            switch (field)
            {
                case 1 when value.Length % 8 == 0: // Packed fixed64.
                    for (var i = 0; i < value.Length; i += 8)
                        folder.GuildIds.Add(new Snowflake(BitConverter.ToUInt64(value.Span.Slice(i, 8))));
                    break;
                case 2: folder.Id = (long)WrappedVarint(value); break;
                case 3: folder.Name = WrappedString(value); break;
                case 4: folder.Color = (int)WrappedVarint(value); break;
            }
        }
        return folder;
    }

    /// <summary>The varint in field 1 of a google.protobuf wrapper message.</summary>
    private static ulong WrappedVarint(ReadOnlyMemory<byte> data)
    {
        foreach (var (field, value) in Fields(data))
        {
            if (field == 1)
                return BitConverter.ToUInt64(value.Span);
        }
        return 0;
    }

    private static string? WrappedString(ReadOnlyMemory<byte> data)
    {
        foreach (var (field, value) in Fields(data))
        {
            if (field == 1)
                return System.Text.Encoding.UTF8.GetString(value.Span);
        }
        return "";
    }

    /// <summary>
    /// Enumerates (field number, value) pairs. Varint and fixed values are returned as 8 little-endian bytes;
    /// length-delimited values as their raw bytes. Fixed32 fields are skipped as no field read here uses them.
    /// </summary>
    private static IEnumerable<(int Field, ReadOnlyMemory<byte> Value)> Fields(ReadOnlyMemory<byte> data)
    {
        var pos = 0;
        while (pos < data.Length)
        {
            var key = ReadVarint(data.Span, ref pos);
            var field = (int)(key >> 3);
            switch (key & 7)
            {
                case 0:
                    yield return (field, BitConverter.GetBytes(ReadVarint(data.Span, ref pos)));
                    break;
                case 1:
                    Require(data, pos, 8);
                    yield return (field, data.Slice(pos, 8));
                    pos += 8;
                    break;
                case 2:
                    var length = checked((int)ReadVarint(data.Span, ref pos));
                    Require(data, pos, length);
                    yield return (field, data.Slice(pos, length));
                    pos += length;
                    break;
                case 5:
                    Require(data, pos, 4);
                    pos += 4;
                    break;
                default:
                    throw new FormatException($"Unsupported protobuf wire type {key & 7}.");
            }
        }
    }

    private static void Require(ReadOnlyMemory<byte> data, int pos, int length)
    {
        if (length < 0 || pos + length > data.Length)
            throw new FormatException("Truncated protobuf message.");
    }

    private static ulong ReadVarint(ReadOnlySpan<byte> data, ref int pos)
    {
        ulong result = 0;
        for (var shift = 0; shift < 64; shift += 7)
        {
            if (pos >= data.Length)
                throw new FormatException("Truncated protobuf varint.");
            var b = data[pos++];
            result |= (ulong)(b & 0x7F) << shift;
            if ((b & 0x80) == 0)
                return result;
        }
        throw new FormatException("Malformed protobuf varint.");
    }
}
