using System.Text;
using Swiftcord.Core.Permissions;

namespace Swiftcord.Core.Models;

/// <summary>
/// GUILD_MEMBER_LIST_UPDATE: changes to the sidebar member list of the channels a user session has subscribed
/// to (op 14). The list is a flat sequence of group headers and members; ops address it by index.
/// </summary>
public sealed class MemberListUpdate
{
    public Snowflake GuildId { get; set; }
    /// <summary>Which list: "everyone", or a hash of the view permissions (see <see cref="MemberListId"/>).</summary>
    public string Id { get; set; } = "";
    public int MemberCount { get; set; }
    public int OnlineCount { get; set; }
    public List<MemberListGroup> Groups { get; set; } = [];
    public List<MemberListOp> Ops { get; set; } = [];
}

public sealed class MemberListGroup
{
    /// <summary>"online", "offline", or the ID of a hoisted role.</summary>
    public string Id { get; set; } = "";
    public int Count { get; set; }
}

public sealed class MemberListOp
{
    /// <summary>SYNC, INSERT, UPDATE, DELETE or INVALIDATE.</summary>
    public string Op { get; set; } = "";
    public int? Index { get; set; }
    public List<int>? Range { get; set; }
    public List<MemberListItem>? Items { get; set; }
    public MemberListItem? Item { get; set; }
}

public sealed class MemberListItem
{
    public MemberListGroup? Group { get; set; }
    public MemberListMember? Member { get; set; }
}

public sealed class MemberListMember
{
    public User User { get; set; } = new();
    public string? Nick { get; set; }
    public string? Avatar { get; set; }
    public List<Snowflake> Roles { get; set; } = [];
    public DateTimeOffset? JoinedAt { get; set; }
    public MemberListPresence? Presence { get; set; }

    public GuildMember ToGuildMember() => new() { User = User, Nick = Nick, Avatar = Avatar, Roles = Roles, JoinedAt = JoinedAt };
}

public sealed class MemberListPresence
{
    public string? Status { get; set; }
    public List<Activity> Activities { get; set; } = [];
}

/// <summary>Computes the member list ID Discord uses for a channel.</summary>
public static class MemberListId
{
    public const string Everyone = "everyone";

    /// <summary>
    /// "everyone" when no overwrite touches View Channel; otherwise the unsigned MurmurHash3 (x86, 32-bit,
    /// seed 0) of the sorted "allow:{id}" / "deny:{id}" entries joined with commas.
    /// </summary>
    public static string For(Channel channel)
    {
        const ulong view = (ulong)Permission.ViewChannel;
        var parts = channel.PermissionOverwrites
            .Select(o => (o.Allow & view) != 0 ? $"allow:{o.Id}" : (o.Deny & view) != 0 ? $"deny:{o.Id}" : null)
            .OfType<string>()
            .Order(StringComparer.Ordinal)
            .ToList();
        return parts.Count == 0 ? Everyone : Murmur3(Encoding.UTF8.GetBytes(string.Join(",", parts))).ToString();
    }

    internal static uint Murmur3(ReadOnlySpan<byte> data, uint seed = 0)
    {
        const uint c1 = 0xcc9e2d51, c2 = 0x1b873593;
        var h = seed;
        var blocks = data.Length / 4;
        for (var i = 0; i < blocks; i++)
        {
            var k = BitConverter.ToUInt32(data.Slice(i * 4, 4));
            k *= c1;
            k = (k << 15) | (k >> 17);
            k *= c2;
            h ^= k;
            h = (h << 13) | (h >> 19);
            h = h * 5 + 0xe6546b64;
        }
        uint tail = 0;
        var rest = data[(blocks * 4)..];
        switch (rest.Length)
        {
            case 3: tail ^= (uint)rest[2] << 16; goto case 2;
            case 2: tail ^= (uint)rest[1] << 8; goto case 1;
            case 1:
                tail ^= rest[0];
                tail *= c1;
                tail = (tail << 15) | (tail >> 17);
                tail *= c2;
                h ^= tail;
                break;
        }
        h ^= (uint)data.Length;
        h ^= h >> 16;
        h *= 0x85ebca6b;
        h ^= h >> 13;
        h *= 0xc2b2ae35;
        h ^= h >> 16;
        return h;
    }
}
