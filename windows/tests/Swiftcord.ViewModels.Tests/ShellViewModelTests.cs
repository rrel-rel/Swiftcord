using Swiftcord.ViewModels.Navigation;
using static Swiftcord.ViewModels.Tests.TestSession;

namespace Swiftcord.ViewModels.Tests;

public sealed class ShellViewModelTests : IAsyncLifetime
{
    private readonly TestSession _session = new();
    private ShellViewModel _shell = null!;

    public Task InitializeAsync()
    {
        _shell = new ShellViewModel(_session.Client, ImmediateUiDispatcher.Instance);
        _session.WithReady();
        return Task.CompletedTask;
    }

    public async Task DisposeAsync()
    {
        _shell.Dispose();
        await _session.DisposeAsync();
    }

    [Fact]
    public void Ready_fills_the_rail_and_starts_on_the_dm_list()
    {
        Assert.True(_shell.IsReady);
        Assert.Equal("Me", _shell.CurrentUserName);
        Assert.Equal("Swift Community", Assert.Single(_shell.Guilds).Name);
        Assert.True(_shell.IsHomeSelected);
        Assert.Equal("Direct Messages", _shell.Sidebar.Title);
        Assert.Equal("Friend", Assert.Single(_shell.Sidebar.Items).Name);
        Assert.Null(_shell.Chat);
    }

    [Fact]
    public void The_rail_follows_the_saved_server_order_when_it_changes()
    {
        _session.Dispatch("GUILD_CREATE", """{"id":"20","name":"Games","joined_at":"2024-01-01T00:00:00+00:00"}""");
        var community = _shell.Guilds.Single(g => g.Id == GuildId);

        _session.Dispatch("USER_SETTINGS_UPDATE", $$"""
            {"guild_folders":[{"guild_ids":["20"],"id":null},{"guild_ids":["{{GuildId}}"],"id":null}]}
            """);

        Assert.Equal([20UL, GuildId], _shell.Guilds.Select(g => g.Id.Value));
        Assert.Same(community, _shell.Guilds[1]); // Items are moved, not recreated.
    }

    [Fact]
    public void Folders_collapse_into_one_rail_item_and_expand_in_place()
    {
        _session.Dispatch("GUILD_CREATE", """{"id":"20","name":"Games"}""");
        _session.Dispatch("GUILD_CREATE", """{"id":"30","name":"Music"}""");
        _session.Dispatch("USER_SETTINGS_UPDATE", $$"""
            {"guild_folders":[
              {"guild_ids":["20","30"],"id":7,"name":null,"color":null},
              {"guild_ids":["{{GuildId}}"],"id":null}
            ]}
            """);

        var folder = Assert.IsType<GuildFolderViewModel>(_shell.RailItems[0]);
        Assert.Equal(2, _shell.RailItems.Count);
        Assert.Equal("Games, Music", folder.Name);
        Assert.Equal(GuildFolderViewModel.DefaultColor, folder.Color);

        _shell.ToggleFolderCommand.Execute(folder);
        Assert.Equal(["folder", "20", "30", $"{GuildId}"],
            _shell.RailItems.Select(i => i is GuildItemViewModel g ? g.Id.ToString() : "folder"));
        var music = (GuildItemViewModel)_shell.RailItems[2];
        Assert.True(music.IsLastInFolder);
        Assert.Equal(folder.Color, music.FolderColor);

        // Expanded state survives a re-layout, and a collapsed folder shows its servers' state.
        _session.Dispatch("USER_SETTINGS_UPDATE", $$"""
            {"guild_folders":[{"guild_ids":["20","30"],"id":7},{"guild_ids":["{{GuildId}}"],"id":null}]}
            """);
        folder = Assert.IsType<GuildFolderViewModel>(_shell.RailItems[0]);
        Assert.True(folder.IsExpanded);
        _shell.SelectGuildCommand.Execute(_shell.Guilds.Single(g => g.Id.Value == 20));
        Assert.False(folder.ShowsSelection);
        _shell.ToggleFolderCommand.Execute(folder);
        Assert.True(folder.ShowsSelection);
        Assert.Equal(-1, music.FolderColor);
        Assert.Equal(2, _shell.RailItems.Count);
    }

    [Fact]
    public void Selecting_a_guild_opens_its_first_readable_channel_and_remembers_the_last_one()
    {
        var guild = _shell.Guilds[0];
        Assert.True(guild.IsUnread);

        _shell.SelectGuildCommand.Execute(guild);
        Assert.True(guild.IsSelected);
        Assert.Equal(["general", "random"], _shell.Sidebar.Items.Select(i => i.Name));
        Assert.Equal(General, _shell.Chat!.ChannelId.Value);
        Assert.True(_shell.Sidebar.Find(General)!.IsSelected);

        _shell.ActivateChannelCommand.Execute(_shell.Sidebar.Find(RandomChannel)!);
        Assert.Equal(RandomChannel, _shell.Chat!.ChannelId.Value);

        _shell.SelectHomeCommand.Execute(null);
        Assert.False(guild.IsSelected);
        Assert.Null(_shell.Chat);

        _shell.SelectGuildCommand.Execute(guild);
        Assert.Equal(RandomChannel, _shell.Chat!.ChannelId.Value);
    }

    [Fact]
    public void Channel_mentions_switch_guilds()
    {
        _shell.OpenChannel(General);
        Assert.False(_shell.IsHomeSelected);
        Assert.Equal(GuildId, _shell.SelectedGuild!.Id.Value);
        Assert.Equal(General, _shell.Chat!.ChannelId.Value);

        _shell.OpenChannel(Dm);
        Assert.True(_shell.IsHomeSelected);
        Assert.Equal(Dm, _shell.Chat!.ChannelId.Value);
    }

    [Fact]
    public void Dm_messages_badge_home_and_bump_the_conversation()
    {
        _session.Dispatch("CHANNEL_CREATE", """{"id":"201","type":1,"recipients":[{"id":"3","username":"new"}],"last_message_id":"70"}""");
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend));

        Assert.Equal(1, _shell.HomeMentionCount);
        Assert.Equal(Dm, _shell.Sidebar.Items[0].Id.Value);
        Assert.Equal(1, _shell.Sidebar.Items[0].MentionCount);
    }

    [Fact]
    public void Leaving_the_selected_guild_returns_home()
    {
        _shell.SelectGuildCommand.Execute(_shell.Guilds[0]);
        _session.Dispatch("GUILD_DELETE", $$"""{"id":"{{GuildId}}"}""");

        Assert.Empty(_shell.Guilds);
        Assert.True(_shell.IsHomeSelected);
    }

    [Fact]
    public void Reconnect_ready_keeps_the_selected_guild_and_channel()
    {
        _shell.SelectGuildCommand.Execute(_shell.Guilds[0]);
        _shell.ActivateChannelCommand.Execute(_shell.Sidebar.Find(RandomChannel)!);

        _session.WithReady();

        Assert.Equal(GuildId, _shell.SelectedGuild!.Id.Value);
        Assert.True(_shell.SelectedGuild.IsSelected);
        Assert.Equal(RandomChannel, _shell.Chat!.ChannelId.Value);
    }

    [Fact]
    public void Collapsing_a_category_hides_its_read_channels()
    {
        _session.Dispatch("CHANNEL_CREATE", $$"""{"id":"300","type":4,"name":"Voice","guild_id":"{{GuildId}}","position":5}""");
        _session.Dispatch("CHANNEL_CREATE", $$"""{"id":"301","type":2,"name":"Lounge","guild_id":"{{GuildId}}","parent_id":"300"}""");
        _shell.SelectGuildCommand.Execute(_shell.Guilds[0]);
        Assert.Equal(["general", "random", "VOICE", "Lounge"], _shell.Sidebar.Items.Select(i => i.Name));

        _shell.ActivateChannelCommand.Execute(_shell.Sidebar.Items[2]);
        Assert.Equal(["general", "random", "VOICE"], _shell.Sidebar.Items.Select(i => i.Name));
        Assert.True(_shell.Sidebar.Items[2].IsCollapsed);
    }
}
