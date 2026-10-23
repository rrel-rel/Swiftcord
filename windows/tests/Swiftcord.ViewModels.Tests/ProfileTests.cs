using Swiftcord.Core;
using Swiftcord.ViewModels.Profiles;
using static Swiftcord.ViewModels.Tests.TestSession;

namespace Swiftcord.ViewModels.Tests;

public sealed class ProfileTests : IAsyncLifetime
{
    private readonly TestSession _session = new TestSession().WithReady();

    public Task InitializeAsync() => Task.CompletedTask;

    public async Task DisposeAsync() => await _session.DisposeAsync();

    [Fact]
    public async Task Shows_cached_details_then_fills_in_the_profile()
    {
        _session.Dispatch("GUILD_ROLE_CREATE", $$"""
            {"guild_id":"{{GuildId}}","role":{"id":"11","name":"Mods","color":16711680,"position":2} }
            """);
        _session.Dispatch("GUILD_MEMBER_UPDATE", $$"""
            {"guild_id":"{{GuildId}}","user":{"id":"{{Friend}}","username":"friend","global_name":"Friend"},"roles":["11"],"nick":"Pal",
             "joined_at":"2023-02-03T00:00:00+00:00"}
            """);
        _session.Respond = (_, _) => Json($$"""
            {"user":{"id":"{{Friend}}","username":"friend","global_name":"Friend","banner":"abc"},
             "user_profile":{"bio":"hello **world**","pronouns":"they/them","accent_color":255},
             "guild_member_profile":{"bio":"","pronouns":"","banner":null},
             "guild_member":{"roles":["11"],"joined_at":"2023-02-03T00:00:00+00:00"},
             "mutual_guilds":[{"id":"{{GuildId}}"},{"id":"77"}]}
            """);

        var profile = new ProfileViewModel(_session.Client, new Snowflake(Friend), new Snowflake(GuildId));
        Assert.Equal("Pal", profile.DisplayName);
        Assert.Equal("friend", profile.Username);
        Assert.Equal(["Mods"], profile.Roles.Select(r => r.Name));
        Assert.True(profile.HasServerJoined);
        Assert.True(profile.CanMessage);

        await profile.LoadAsync();

        Assert.Contains($"GET /api/v10/users/{Friend}/profile?with_mutual_guilds=true&guild_id={GuildId}", _session.RequestLines);
        Assert.Equal("they/them", profile.Pronouns); // Empty server pronouns fall back to the global ones.
        Assert.True(profile.HasBio);
        Assert.Equal($"https://cdn.discordapp.com/banners/{Friend}/abc.png?size=600", profile.BannerUri!.AbsoluteUri);
        Assert.Equal(255, profile.BannerColor);
        Assert.Equal("2 mutual servers", profile.MutualServers);
    }

    [Fact]
    public async Task A_failed_profile_request_keeps_the_cached_details()
    {
        _session.Respond = (_, _) => Json("""{"message":"Unknown User","code":10013}""", System.Net.HttpStatusCode.NotFound);
        var profile = new ProfileViewModel(_session.Client, new Snowflake(Friend), null);

        await profile.LoadAsync();

        Assert.Equal("Friend", profile.DisplayName);
        Assert.Null(profile.Error);
    }

    [Fact]
    public async Task Message_opens_the_existing_dm_or_creates_one()
    {
        Snowflake? opened = null;
        var existing = new ProfileViewModel(_session.Client, new Snowflake(Friend), null, openChannel: id => opened = id);
        await existing.SendMessageCommand.ExecuteAsync(null);
        Assert.Equal(Dm, opened!.Value.Value);
        Assert.DoesNotContain(_session.RequestLines, l => l.StartsWith("POST /api/v10/users/@me/channels"));

        _session.Respond = (_, _) => Json("""{"id":"300","type":1,"recipients":[{"id":"5","username":"stranger"}]}""");
        var stranger = new ProfileViewModel(_session.Client, new Snowflake(5), null, openChannel: id => opened = id);
        await stranger.SendMessageCommand.ExecuteAsync(null);
        Assert.Contains("POST /api/v10/users/@me/channels", _session.RequestLines);
        Assert.Equal(300UL, opened!.Value.Value);
        Assert.NotNull(_session.Client.State.GetChannel(new Snowflake(300)));
    }

    [Fact]
    public void You_cannot_message_yourself()
    {
        var profile = new ProfileViewModel(_session.Client, new Snowflake(Me), null);
        Assert.False(profile.CanMessage);
        Assert.Equal("Me", profile.DisplayName);
    }
}
