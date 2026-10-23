using Microsoft.Extensions.Time.Testing;
using Swiftcord.Core.Models;
using Swiftcord.Core.State;
using static Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.Core.Tests.State;

public sealed class NotificationRulesTests
{
    private const ulong Me = 1, Other = 2, GuildId = 10, MyRole = 11, OtherRole = 12;
    private const ulong Category = 100, General = 101, Memes = 102, Thread = 103, Dm = 200;

    private readonly FakeTimeProvider _time = new(DateTimeOffset.Parse("2024-05-01T12:00:00Z"));
    private readonly DiscordState _state;

    public NotificationRulesTests()
    {
        _state = new DiscordState(time: _time);
    }

    private void Ready(string guildSettings, int defaultLevel = 1) =>
        _state.HandleDispatch("READY", ParseJson($$"""
        {
          "v": 9, "session_id": "s", "user": {"id":"{{Me}}","username":"me"},
          "guilds": [{
            "id":"{{GuildId}}","name":"G","default_message_notifications":{{defaultLevel}},
            "channels":[
              {"id":"{{Category}}","type":4,"name":"Chat"},
              {"id":"{{General}}","type":0,"name":"general","parent_id":"{{Category}}"},
              {"id":"{{Memes}}","type":0,"name":"memes"}
            ],
            "threads":[{"id":"{{Thread}}","type":11,"name":"t","parent_id":"{{General}}"}]
          }],
          "merged_members": [[{"user_id":"{{Me}}","roles":["{{MyRole}}"]}]],
          "private_channels": [{"id":"{{Dm}}","type":1,"recipients":[{"id":"{{Other}}","username":"o"}]}],
          "user_guild_settings": {"version":1,"partial":false,"entries":[{{guildSettings}}]}
        }
        """));

    private static Message Message(ulong channel, string extra = "", ulong author = Other) =>
        Swiftcord.Core.Json.DiscordJson.Deserialize<Message>(ParseJson($$"""
            {"id":"500","channel_id":"{{channel}}","author":{"id":"{{author}}","username":"u"},"content":"hi",
             "timestamp":"2024-05-01T12:00:00+00:00","type":0{{extra}}}
            """));

    private const string MentionsMe = ""","mentions":[{"id":"1","username":"me"}]""";

    [Fact]
    public void Server_default_of_only_mentions_applies_when_the_user_has_not_chosen()
    {
        Ready("");

        Assert.False(_state.ShouldNotify(Message(Memes)));
        Assert.True(_state.ShouldNotify(Message(Memes, MentionsMe)));
        Assert.True(_state.ShouldNotify(Message(Memes, ""","mention_everyone":true""")));
        Assert.True(_state.ShouldNotify(Message(Memes, $$""","mention_roles":["{{MyRole}}"]""")));
        Assert.False(_state.ShouldNotify(Message(Memes, $$""","mention_roles":["{{OtherRole}}"]""")));
    }

    [Fact]
    public void Dms_notify_and_own_or_silent_messages_never_do()
    {
        Ready("");

        Assert.True(_state.ShouldNotify(Message(Dm)));
        Assert.False(_state.ShouldNotify(Message(Dm, author: Me)));
        Assert.False(_state.ShouldNotify(Message(Dm, ""","flags":4096""")));
    }

    [Fact]
    public void Channel_overrides_beat_category_overrides_which_beat_the_server_setting()
    {
        Ready($$"""
            {"guild_id":"{{GuildId}}","message_notifications":2,"channel_overrides":[
              {"channel_id":"{{Category}}","message_notifications":0},
              {"channel_id":"{{Thread}}","message_notifications":1}
            ]}
            """, defaultLevel: 0);

        Assert.False(_state.ShouldNotify(Message(Memes)));   // Server: nothing.
        Assert.True(_state.ShouldNotify(Message(General)));  // Category: all messages.
        Assert.False(_state.ShouldNotify(Message(Thread)));  // Thread: only mentions.
        Assert.True(_state.ShouldNotify(Message(Thread, MentionsMe)));
    }

    [Fact]
    public void Suppressed_everyone_and_role_mentions_do_not_notify()
    {
        Ready($$"""
            {"guild_id":"{{GuildId}}","message_notifications":1,"suppress_everyone":true,"suppress_roles":true}
            """);

        Assert.False(_state.ShouldNotify(Message(Memes, ""","mention_everyone":true""")));
        Assert.False(_state.ShouldNotify(Message(Memes, $$""","mention_roles":["{{MyRole}}"]""")));
        Assert.True(_state.ShouldNotify(Message(Memes, MentionsMe)));
    }

    [Fact]
    public void Muted_servers_and_channels_only_let_direct_mentions_through_until_the_mute_ends()
    {
        Ready($$"""
            {"guild_id":"{{GuildId}}","message_notifications":0,"muted":true,"mute_config":{"end_time":"2024-05-01T13:00:00+00:00"} },
            {"guild_id":null,"channel_overrides":[{"channel_id":"{{Dm}}","muted":true,"mute_config":null}]}
            """);

        Assert.False(_state.ShouldNotify(Message(Memes)));
        Assert.True(_state.ShouldNotify(Message(Memes, MentionsMe)));
        Assert.False(_state.ShouldNotify(Message(Dm)));

        _time.Advance(TimeSpan.FromHours(2));
        Assert.True(_state.ShouldNotify(Message(Memes)));
        Assert.False(_state.ShouldNotify(Message(Dm))); // Muted with no end time.
    }

    [Fact]
    public void Settings_updates_replace_the_servers_settings()
    {
        Ready("", defaultLevel: 0);
        Assert.True(_state.ShouldNotify(Message(Memes)));

        _state.HandleDispatch("USER_GUILD_SETTINGS_UPDATE", ParseJson($$"""
            {"guild_id":"{{GuildId}}","message_notifications":2}
            """));

        Assert.False(_state.ShouldNotify(Message(Memes)));
    }
}
