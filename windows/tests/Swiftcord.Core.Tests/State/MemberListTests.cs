using System.Text;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;
using static Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.Core.Tests.State;

public sealed class MemberListTests
{
    [Theory]
    [InlineData("", 0u)]
    [InlineData("hello", 0x248bfa47u)]
    [InlineData("The quick brown fox jumps over the lazy dog", 0x2e4ff723u)]
    public void Murmur3_matches_the_reference_implementation(string input, uint expected) =>
        Assert.Equal(expected, MemberListId.Murmur3(Encoding.UTF8.GetBytes(input)));

    [Fact]
    public void Channels_without_view_overwrites_use_the_everyone_list()
    {
        var channel = new Channel { PermissionOverwrites = [new PermissionOverwrite { Id = new Snowflake(5), Allow = 1 << 11 }] };
        Assert.Equal("everyone", MemberListId.For(channel));
    }

    [Fact]
    public void Restricted_channels_hash_their_sorted_view_overwrites()
    {
        var channel = new Channel
        {
            PermissionOverwrites =
            [
                new PermissionOverwrite { Id = new Snowflake(20), Allow = 1 << 10 },
                new PermissionOverwrite { Id = new Snowflake(10), Deny = 1 << 10 },
            ],
        };
        var expected = MemberListId.Murmur3(Encoding.UTF8.GetBytes("allow:20,deny:10")).ToString();
        Assert.Equal(expected, MemberListId.For(channel));
    }

    [Fact]
    public void Member_list_updates_cache_members_and_presences()
    {
        var state = new DiscordState();
        MemberListUpdate? received = null;
        state.MemberListUpdated += u => received = u;

        state.HandleDispatch("GUILD_MEMBER_LIST_UPDATE", ParseJson("""
            {"guild_id":"10","id":"everyone","member_count":2,"online_count":1,
             "groups":[{"id":"online","count":1},{"id":"offline","count":1}],
             "ops":[{"op":"SYNC","range":[0,99],"items":[
               {"group":{"id":"online","count":1}},
               {"member":{"user":{"id":"2","username":"pal"},"nick":"Pal","roles":["11"],
                          "presence":{"status":"dnd","activities":[{"type":0,"name":"Chess"}]}}},
               {"group":{"id":"offline","count":1}},
               {"member":{"user":{"id":"3","username":"quiet"},"roles":[],"presence":{"status":"offline","activities":[]}}}
             ]}]}
            """));

        Assert.Equal(4, received!.Ops[0].Items!.Count);
        Assert.Equal("Pal", state.GetMember(new Snowflake(10), new Snowflake(2))!.Nick);
        Assert.Equal(PresenceStatus.DoNotDisturb, state.GetPresence(new Snowflake(2)));
        Assert.Equal("quiet", state.GetUser(new Snowflake(3))!.Username);
    }
}
