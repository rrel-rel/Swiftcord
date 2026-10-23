using System.Text;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;
using static Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.Core.Tests.State;

public sealed class GuildOrderTests
{
    private static string Ready(string settings) => $$"""
        {
          "v": 9, "session_id": "s", "user": {"id":"1","username":"me"},
          "guilds": [
            {"id":"10","name":"A","joined_at":"2020-01-01T00:00:00+00:00"},
            {"id":"20","name":"B","joined_at":"2020-01-01T00:00:00+00:00"},
            {"id":"30","name":"C","joined_at":"2020-01-01T00:00:00+00:00"},
            {"id":"40","name":"New","joined_at":"2024-06-01T00:00:00+00:00"},
            {"id":"50","name":"Newer","joined_at":"2024-07-01T00:00:00+00:00"}
          ]
          {{settings}}
        }
        """;

    [Fact]
    public void Servers_follow_the_saved_folders_with_newly_joined_servers_on_top()
    {
        var state = new DiscordState();
        state.HandleDispatch("READY", ParseJson(Ready("""
            ,"user_settings": {"guild_folders": [
              {"guild_ids":["30"],"id":null,"name":null,"color":null},
              {"guild_ids":["20","10","999"],"id":12345,"name":"Friends","color":16711680}
            ]}
            """)));

        Assert.Equal([50UL, 40, 30, 20, 10], state.Guilds.Select(g => g.Id.Value));

        var folders = state.GuildFolders!;
        Assert.Equal(4, folders.Count);
        Assert.All(folders.Take(3), f => Assert.Null(f.Id));
        var friends = folders[3];
        Assert.Equal((12345L, "Friends", 16711680), (friends.Id, friends.Name, friends.Color));
        Assert.Equal([20UL, 10], friends.GuildIds.Select(id => id.Value)); // Servers the user has left are dropped.
    }

    [Fact]
    public void Without_saved_settings_servers_keep_the_ready_order()
    {
        var state = new DiscordState();
        state.HandleDispatch("READY", ParseJson(Ready("")));

        Assert.Null(state.GuildFolders);
        Assert.Equal([10UL, 20, 30, 40, 50], state.Guilds.Select(g => g.Id.Value));
    }

    [Fact]
    public void Reads_folders_from_protobuf_settings_and_their_updates()
    {
        var state = new DiscordState();
        var changes = 0;
        state.GuildFoldersChanged += () => changes++;
        var initial = Settings(Folder([40, 50, 30, 20, 10]));
        state.HandleDispatch("READY", ParseJson(Ready($$""","user_settings_proto":"{{initial}}" """)));
        Assert.Equal([40UL, 50, 30, 20, 10], state.Guilds.Select(g => g.Id.Value));

        var updated = Settings(Folder([10]), Folder([20, 30], id: 7, name: "Games", color: 0x00ff00), Folder([40, 50]));
        state.HandleDispatch("USER_SETTINGS_PROTO_UPDATE", ParseJson($$"""
            {"settings":{"type":1,"proto":"{{updated}}"},"partial":false}
            """));

        Assert.Equal(1, changes);
        Assert.Equal([10UL, 20, 30, 40, 50], state.Guilds.Select(g => g.Id.Value));
        var games = state.GuildFolders![1];
        Assert.Equal((7L, "Games", 0x00ff00), (games.Id, games.Name, games.Color));
    }

    [Fact]
    public void A_settings_update_without_folders_leaves_the_order_alone()
    {
        var state = new DiscordState();
        state.HandleDispatch("READY", ParseJson(Ready($$""","user_settings_proto":"{{Settings(Folder([50, 40, 30, 20, 10]))}}" """)));
        var changes = 0;
        state.GuildFoldersChanged += () => changes++;

        // Field 13 (appearance) only, as a partial update changing the theme would send.
        var appearance = Convert.ToBase64String([.. Tag(13, 2), 2, .. Tag(1, 0), 2]);
        state.HandleDispatch("USER_SETTINGS_PROTO_UPDATE", ParseJson($$"""
            {"settings":{"type":1,"proto":"{{appearance}}"},"partial":true}
            """));

        Assert.Equal(0, changes);
        Assert.Equal([50UL, 40, 30, 20, 10], state.Guilds.Select(g => g.Id.Value));
    }

    [Fact]
    public void Malformed_protobuf_is_ignored() =>
        Assert.Null(UserSettingsReader.ReadGuildFolders(Convert.ToBase64String([0x72, 0x7f, 0x01])));

    #region Protobuf encoding

    /// <summary>PreloadedUserSettings with guild_folders (14) set, plus an unrelated field to skip.</summary>
    private static string Settings(params byte[][] folders)
    {
        var guildFolders = folders.SelectMany(f => LengthDelimited(1, f)).ToArray();
        byte[] status = [.. Tag(11, 2), 2, .. Tag(1, 0), 1];
        return Convert.ToBase64String([.. status, .. LengthDelimited(14, guildFolders)]);
    }

    private static byte[] Folder(ulong[] guildIds, long? id = null, string? name = null, int? color = null)
    {
        var bytes = new List<byte>();
        bytes.AddRange(LengthDelimited(1, guildIds.SelectMany(BitConverter.GetBytes).ToArray()));
        if (id is { } i) bytes.AddRange(LengthDelimited(2, [.. Tag(1, 0), .. Varint((ulong)i)]));
        if (name is not null) bytes.AddRange(LengthDelimited(3, LengthDelimited(1, Encoding.UTF8.GetBytes(name))));
        if (color is { } c) bytes.AddRange(LengthDelimited(4, [.. Tag(1, 0), .. Varint((ulong)c)]));
        return [.. bytes];
    }

    private static byte[] LengthDelimited(int field, byte[] value) => [.. Tag(field, 2), .. Varint((ulong)value.Length), .. value];

    private static byte[] Tag(int field, int wireType) => Varint((ulong)(field << 3 | wireType));

    private static byte[] Varint(ulong value)
    {
        var bytes = new List<byte>();
        do
        {
            var b = (byte)(value & 0x7F);
            value >>= 7;
            bytes.Add(value != 0 ? (byte)(b | 0x80) : b);
        } while (value != 0);
        return [.. bytes];
    }

    #endregion
}
