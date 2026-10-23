using Swiftcord.Core;
using Swiftcord.Core.Models;
using Swiftcord.ViewModels.Members;
using static Swiftcord.ViewModels.Tests.TestSession;

namespace Swiftcord.ViewModels.Tests;

public sealed class MemberListViewModelTests : IAsyncLifetime
{
    private readonly TestSession _session = new TestSession().WithReady();
    private MemberListViewModel _list = null!;

    public Task InitializeAsync()
    {
        _list = new MemberListViewModel(_session.Client, ImmediateUiDispatcher.Instance, _session.Client.State.GetChannel(new Snowflake(General))!);
        return Task.CompletedTask;
    }

    public async Task DisposeAsync()
    {
        _list.Dispose();
        await _session.DisposeAsync();
    }

    private static string Member(ulong id, string name, string status = "online", string activities = "[]") =>
        "{\"member\":{\"user\":{\"id\":\"" + id + "\",\"username\":\"" + name + "\"},\"roles\":[],"
        + "\"presence\":{\"status\":\"" + status + "\",\"activities\":" + activities + "}}}";

    private const string BusyStatus = """[{"type":4,"name":"Custom Status","state":"busy","emoji":{"name":"🔥"}}]""";

    private void Update(string groups, string ops, string id = "everyone") =>
        _session.Dispatch("GUILD_MEMBER_LIST_UPDATE", $$$"""
            {"guild_id":"{{{GuildId}}}","id":"{{{id}}}","member_count":3,"online_count":2,"groups":{{{groups}}},"ops":{{{ops}}}}
            """);

    private IEnumerable<string> Shape() => _list.Rows.Select(r => r switch
    {
        MemberGroupRow g => g.Title,
        MemberRow m => m.Name,
        _ => "…",
    });

    [Fact]
    public void Sync_fills_rows_and_later_ops_edit_them_by_index()
    {
        const string groups = """[{"id":"online","count":2},{"id":"offline","count":1}]""";
        Update(groups, "[{\"op\":\"SYNC\",\"range\":[0,99],\"items\":["
            + string.Join(",", """{"group":{"id":"online","count":2}}""", Member(2, "ana"), Member(3, "bo", activities: BusyStatus),
                """{"group":{"id":"offline","count":1}}""", Member(4, "cy", "offline"))
            + "]}]");

        Assert.False(_list.IsLoading);
        Assert.Equal(["ONLINE — 2", "ana", "bo", "OFFLINE — 1", "cy"], Shape());
        Assert.Equal("🔥 busy", ((MemberRow)_list.Rows[2]).ActivityText);
        Assert.True(((MemberRow)_list.Rows[4]).IsOffline);
        Assert.Equal("2 online · 3 members", _list.Summary);

        Update(groups, """[{"op":"DELETE","index":2},{"op":"INSERT","index":1,"item":""" + Member(5, "al")
            + """},{"op":"UPDATE","index":2,"item":""" + Member(2, "ana", "idle") + "}]");
        Assert.Equal(["ONLINE — 2", "al", "ana", "OFFLINE — 1", "cy"], Shape());
        Assert.Equal(PresenceStatus.Idle, ((MemberRow)_list.Rows[2]).Status);
    }

    [Fact]
    public void Ranges_beyond_the_loaded_rows_are_padded_with_placeholders()
    {
        Update("""[{"id":"online","count":150}]""", """[{"op":"SYNC","range":[100,199],"items":[""" + Member(9, "far") + "]}]");

        Assert.Equal(101, _list.Rows.Count);
        Assert.IsType<MemberPlaceholderRow>(_list.Rows[0]);
        Assert.Equal("far", ((MemberRow)_list.Rows[100]).Name);
    }

    [Fact]
    public void Updates_for_other_lists_are_ignored_once_this_one_has_arrived()
    {
        Update("""[{"id":"online","count":1}]""", """[{"op":"SYNC","range":[0,99],"items":[""" + Member(2, "mine") + "]}]");
        Update("""[{"id":"online","count":1}]""", """[{"op":"SYNC","range":[0,99],"items":[""" + Member(3, "other") + "]}]", id: "12345");

        Assert.Equal(["mine"], Shape());
    }

    [Theory]
    [InlineData("""[{"type":0,"name":"Chess"}]""", "Playing Chess")]
    [InlineData("""[{"type":2,"name":"Spotify"}]""", "Listening to Spotify")]
    [InlineData("""[{"type":4,"name":"Custom Status","emoji":{"id":"1","name":"blob"}},{"type":3,"name":"YouTube"}]""", "Watching YouTube")]
    [InlineData("[]", null)]
    public void Activity_text_prefers_a_custom_status(string activities, string? expected)
    {
        var parsed = Swiftcord.Core.Json.DiscordJson.Deserialize<List<Activity>>(System.Text.Json.JsonDocument.Parse(activities).RootElement);
        Assert.Equal(expected, MemberListViewModel.ActivityText(parsed));
    }
}
