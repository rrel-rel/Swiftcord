using Microsoft.Extensions.Time.Testing;
using Swiftcord.Core.Models;
using Swiftcord.Core.Permissions;
using Swiftcord.Core.State;
using static Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.Core.Tests.State;

public sealed class DiscordStateTests
{
    private const ulong Me = 1, Other = 2, GuildId = 10, ModRole = 11;
    private const ulong General = 100, Mods = 101, Category = 102, Voice = 103, Announcements = 104, Dm = 200;
    private const ulong ViewChannel = 1 << 10, SendMessages = 1 << 11;

    private readonly FakeTimeProvider _time = new();
    private readonly DiscordState _state;

    public DiscordStateTests()
    {
        _state = new DiscordState(messageCacheSize: 3, _time);
        _state.HandleDispatch("READY", ParseJson($$"""
        {
          "v": 9, "session_id": "s",
          "user": {"id":"{{Me}}","username":"me","discriminator":"0"},
          "users": [{"id":"{{Other}}","username":"friend","global_name":"Friend"}],
          "guilds": [{
            "id":"{{GuildId}}","name":"Swift Community","owner_id":"999",
            "roles":[
              {"id":"{{GuildId}}","name":"@everyone","permissions":"{{ViewChannel | SendMessages}}","position":0},
              {"id":"{{ModRole}}","name":"Mod","permissions":"0","position":1,"color":16711680}
            ],
            "channels":[
              {"id":"{{General}}","type":0,"name":"general","position":1,"last_message_id":"500"},
              {"id":"{{Category}}","type":4,"name":"Staff","position":0},
              {"id":"{{Mods}}","type":0,"name":"mods","position":0,"parent_id":"{{Category}}",
               "permission_overwrites":[
                 {"id":"{{GuildId}}","type":0,"allow":"0","deny":"{{ViewChannel}}"},
                 {"id":"{{ModRole}}","type":0,"allow":"{{ViewChannel}}","deny":"0"}
               ]},
              {"id":"{{Voice}}","type":2,"name":"Lounge","position":0},
              {"id":"{{Announcements}}","type":5,"name":"news","position":2}
            ]
          }],
          "merged_members": [[{"user_id":"{{Me}}","roles":[],"nick":"Captain"}]],
          "private_channels": [{"id":"{{Dm}}","type":1,"recipient_ids":["{{Other}}"],"last_message_id":"600"}],
          "read_state": {"version":1,"partial":false,"entries":[
            {"id":"{{General}}","last_message_id":"400","mention_count":0},
            {"id":"{{Dm}}","last_message_id":"600","mention_count":0}
          ]}
        }
        """));
    }

    private static string MessageJson(ulong id, ulong channel, ulong author, string content = "hello", string extra = "") => $$"""
        {"id":"{{id}}","channel_id":"{{channel}}","author":{"id":"{{author}}","username":"u{{author}}"},
         "content":"{{content}}","timestamp":"2024-01-01T00:00:00+00:00","type":0{{extra}}}
        """;

    [Fact]
    public void Ready_populates_guilds_dms_and_resolves_deduplicated_recipients()
    {
        Assert.Equal("me", _state.CurrentUser!.Username);
        Assert.Equal("Swift Community", _state.Guilds.Single().Name);
        Assert.Equal("SC", _state.Guilds.Single().Acronym);
        Assert.Equal(GuildId, _state.GetChannel(General)!.GuildId!.Value.Value);

        var dm = _state.PrivateChannels.Single();
        Assert.Equal("Friend", dm.DisplayName);
        Assert.Equal("Captain", _state.GetDisplayName(Me, GuildId));
        Assert.Equal("Friend", _state.GetDisplayName(Other));
    }

    [Fact]
    public void Channel_tree_hides_channels_denied_by_overwrites_and_orders_text_before_voice()
    {
        var tree = _state.GetChannelTree(GuildId);

        var group = Assert.Single(tree); // The Staff category has no visible children, so it's dropped.
        Assert.Null(group.Category);
        Assert.Equal(["general", "news", "Lounge"], group.Channels.Select(c => c.Name));
    }

    [Fact]
    public void Gaining_a_role_reveals_channels_and_raises_permissions_changed()
    {
        Snowflake? changed = null;
        _state.PermissionsChanged += id => changed = id;

        _state.HandleDispatch("GUILD_MEMBER_UPDATE", ParseJson($$"""
            {"guild_id":"{{GuildId}}","user":{"id":"{{Me}}","username":"me"},"roles":["{{ModRole}}"],"nick":"Captain"}
            """));

        Assert.Equal(GuildId, changed!.Value.Value);
        var staff = _state.GetChannelTree(GuildId).Single(g => g.Category is not null);
        Assert.Equal("Staff", staff.Category!.Name);
        Assert.Equal("mods", staff.Channels.Single().Name);
        Assert.True(_state.GetPermissions(Mods).HasFlag(Permission.ViewChannel));
        Assert.Equal(16711680, _state.GetRoleColor(GuildId, Me));
    }

    [Fact]
    public void Unread_state_follows_last_message_and_read_state()
    {
        Assert.True(_state.IsUnread(General));
        Assert.True(_state.IsGuildUnread(GuildId));
        Assert.False(_state.IsUnread(Dm));

        _state.MarkRead(General, 500);
        Assert.False(_state.IsUnread(General));
        Assert.False(_state.IsGuildUnread(GuildId));
    }

    [Fact]
    public void Mentions_and_dm_messages_increment_mention_counts_but_own_messages_clear_them()
    {
        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(700, General, Other, extra: $$""","mentions":[{"id":"{{Me}}","username":"me"}]""")));
        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(701, General, Other)));
        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(702, Dm, Other)));

        Assert.Equal(1, _state.GetMentionCount(General));
        Assert.Equal(1, _state.GetGuildMentionCount(GuildId));
        Assert.Equal(1, _state.GetMentionCount(Dm));
        Assert.True(_state.IsUnread(Dm));
        Assert.Equal(Dm, _state.PrivateChannels[0].Id.Value);

        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(703, General, Me)));
        Assert.Equal(0, _state.GetMentionCount(General));
        Assert.False(_state.IsUnread(General));
    }

    [Fact]
    public void Message_cache_is_only_kept_for_opened_channels_and_bounded()
    {
        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(1000, General, Other)));
        Assert.Empty(_state.GetMessages(General));

        _state.AddMessages(General, [Models.Message(900), Models.Message(901)]);
        for (ulong id = 1001; id <= 1003; id++)
            _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(id, General, Other)));

        Assert.Equal([1001UL, 1002, 1003], _state.GetMessages(General).Select(m => m.Id.Value));
    }

    [Fact]
    public void Partial_message_updates_merge_into_the_cached_message()
    {
        _state.AddMessages(General, [Models.Message(900, "original")]);
        Message? updated = null;
        _state.MessageUpdated += m => updated = m;

        _state.HandleDispatch("MESSAGE_UPDATE", ParseJson($$"""
            {"id":"900","channel_id":"{{General}}","content":"edited","edited_timestamp":"2024-01-02T00:00:00+00:00"}
            """));

        var message = _state.GetMessage(General, 900)!;
        Assert.Equal("edited", message.Content);
        Assert.NotNull(message.EditedTimestamp);
        Assert.Equal("u2", message.Author.Username);
        Assert.Same(message, updated);
    }

    [Fact]
    public void Reactions_are_counted_and_removed()
    {
        _state.AddMessages(General, [Models.Message(900)]);
        string Reaction(ulong user) => $$"""
            {"user_id":"{{user}}","channel_id":"{{General}}","message_id":"900","emoji":{"id":null,"name":"👍"} }
            """;

        _state.HandleDispatch("MESSAGE_REACTION_ADD", ParseJson(Reaction(Other)));
        _state.HandleDispatch("MESSAGE_REACTION_ADD", ParseJson(Reaction(Me)));
        var reaction = _state.GetMessage(General, 900)!.Reactions.Single();
        Assert.Equal(2, reaction.Count);
        Assert.True(reaction.Me);

        _state.HandleDispatch("MESSAGE_REACTION_REMOVE", ParseJson(Reaction(Me)));
        _state.HandleDispatch("MESSAGE_REACTION_REMOVE", ParseJson(Reaction(Other)));
        Assert.Empty(_state.GetMessage(General, 900)!.Reactions);
    }

    [Fact]
    public void Typing_indicators_expire_and_clear_when_the_user_sends()
    {
        string Typing(ulong user) => $$"""{"channel_id":"{{General}}","user_id":"{{user}}","timestamp":0}""";
        _state.HandleDispatch("TYPING_START", ParseJson(Typing(Other)));
        _state.HandleDispatch("TYPING_START", ParseJson(Typing(Me)));
        Assert.Equal([Other], _state.GetTypingUsers(General).Select(u => u.Value));

        _time.Advance(TimeSpan.FromSeconds(11));
        Assert.Empty(_state.GetTypingUsers(General));

        _state.HandleDispatch("TYPING_START", ParseJson(Typing(Other)));
        _state.HandleDispatch("MESSAGE_CREATE", ParseJson(MessageJson(800, General, Other)));
        Assert.Empty(_state.GetTypingUsers(General));
    }

    [Fact]
    public void Guild_update_keeps_channels_and_leaving_removes_everything()
    {
        _state.HandleDispatch("GUILD_UPDATE", ParseJson($$"""{"id":"{{GuildId}}","name":"Renamed"}"""));
        Assert.Equal("Renamed", _state.GetGuild(GuildId)!.Name);
        Assert.NotEmpty(_state.GetChannelTree(GuildId));

        _state.HandleDispatch("GUILD_DELETE", ParseJson($$"""{"id":"{{GuildId}}"}"""));
        Assert.Empty(_state.Guilds);
        Assert.Null(_state.GetChannel(General));
    }

    [Fact]
    public void Dms_have_all_permissions()
    {
        Assert.Equal(Permission.All, _state.GetPermissions(Dm));
    }

    private static class Models
    {
        public static Message Message(ulong id, string content = "hello") => new()
        {
            Id = id,
            ChannelId = General,
            Author = new User { Id = Other, Username = "u2" },
            Content = content,
        };
    }
}
