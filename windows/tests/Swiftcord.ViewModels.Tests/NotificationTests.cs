using Swiftcord.Core;
using Swiftcord.ViewModels.Navigation;
using Swiftcord.ViewModels.Notifications;
using Swiftcord.ViewModels.Settings;
using static Swiftcord.ViewModels.Tests.TestSession;

namespace Swiftcord.ViewModels.Tests;

public sealed class NotificationTests : IAsyncLifetime
{
    private readonly TestSession _session = new();
    private readonly FakeNotifications _notifications = new();
    private readonly SettingsViewModel _settings = new(new MemorySettingsStore());
    private ShellViewModel _shell = null!;

    public Task InitializeAsync()
    {
        _shell = new ShellViewModel(_session.Client, ImmediateUiDispatcher.Instance)
        {
            Notifications = _notifications,
            Settings = _settings,
        };
        _session.WithReady();
        return Task.CompletedTask;
    }

    public async Task DisposeAsync()
    {
        _shell.Dispose();
        await _session.DisposeAsync();
    }

    [Fact]
    public void A_dm_raises_a_notification_with_the_resolved_message_text()
    {
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend, content: "**hey** <@1> see <#100>",
            extra: ""","author":{"id":"2","username":"friend","global_name":"Friend"}"""));

        var shown = Assert.Single(_notifications.Shown);
        Assert.Equal("Friend", shown.Title);
        Assert.Equal("hey @Me see #general", shown.Body);
        Assert.Equal(Dm, shown.ChannelId.Value);
    }

    [Fact]
    public void Server_messages_name_the_channel_and_server()
    {
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, General, Friend, content: "<@1> hi",
            extra: $$""","guild_id":"{{GuildId}}","mentions":[{"id":"1","username":"me"}]"""));

        Assert.Equal("u2 (#general, Swift Community)", Assert.Single(_notifications.Shown).Title);
    }

    [Fact]
    public void The_open_conversation_does_not_notify_while_the_window_is_focused()
    {
        _shell.OpenChannel(Dm);
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend));
        Assert.Empty(_notifications.Shown);

        _shell.SetWindowActive(false);
        _session.Dispatch("MESSAGE_CREATE", MessageJson(81, Dm, Friend));
        Assert.Single(_notifications.Shown);
    }

    [Fact]
    public void Opening_a_channel_clears_its_notifications()
    {
        _shell.OpenChannel(Dm);
        Assert.Equal([Dm], _notifications.Cleared.Select(c => c.Value));
    }

    [Fact]
    public void Settings_can_turn_notifications_or_previews_off()
    {
        _settings.NotificationPreviews = false;
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend, content: "secret"));
        Assert.Equal("New message", Assert.Single(_notifications.Shown).Body);

        _settings.NotificationsEnabled = false;
        _session.Dispatch("MESSAGE_CREATE", MessageJson(81, Dm, Friend));
        Assert.Single(_notifications.Shown);
    }

    [Fact]
    public void Forwards_preview_the_original_message()
    {
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend, content: "", extra: """
            ,"message_reference":{"type":1,"channel_id":"5","message_id":"6"},
            "message_snapshots":[{"message":{"content":"hello from elsewhere","timestamp":"2024-04-30T09:00:00+00:00"}}]
            """));

        Assert.Equal("Forwarded: hello from elsewhere", Assert.Single(_notifications.Shown).Body);
    }

    [Fact]
    public void Attachments_and_spoilers_are_described_rather_than_shown()
    {
        _session.Dispatch("MESSAGE_CREATE", MessageJson(80, Dm, Friend, content: "",
            extra: ""","attachments":[{"id":"1","filename":"cat.png","size":1,"url":"u","proxy_url":"p"}]"""));
        _session.Dispatch("MESSAGE_CREATE", MessageJson(81, Dm, Friend, content: "the end is ||a twist||"));

        Assert.Equal(["Sent an attachment: cat.png", "the end is ▒▒▒▒"], _notifications.Shown.Select(n => n.Body));
    }

    [Fact]
    public async Task A_link_followed_before_ready_opens_once_the_cache_loads()
    {
        await using var session = new TestSession();
        using var shell = new ShellViewModel(session.Client, ImmediateUiDispatcher.Instance);

        shell.Navigate(new DeepLink(new Snowflake(GuildId), new Snowflake(RandomChannel)));
        Assert.Null(shell.Chat);

        session.WithReady();
        Assert.Equal(RandomChannel, shell.Chat!.ChannelId.Value);
        Assert.Equal(GuildId, shell.SelectedGuild!.Id.Value);
    }

    [Fact]
    public async Task A_message_link_opens_its_channel_at_that_message()
    {
        _session.Respond = (request, _) => Json(request.RequestUri!.Query.Contains("around=5")
            ? $"[{MessageJson(5, RandomChannel, Friend, "target")}]"
            : "[]");

        _shell.Navigate(new DeepLink(new Snowflake(GuildId), new Snowflake(RandomChannel), new Snowflake(5)));

        await Swiftcord.Core.Tests.Support.TestUtil.EventuallyAsync(() => _shell.Chat?.Highlighted is not null);
        Assert.Equal(RandomChannel, _shell.Chat!.ChannelId.Value);
        Assert.Equal("target", _shell.Chat.Highlighted!.Content);
    }

    [Theory]
    [InlineData("swiftcord://channels/@me/200", null, 200UL, null)]
    [InlineData("swiftcord://channels/10/100/123", 10UL, 100UL, 123UL)]
    [InlineData("https://discord.com/channels/10/100", 10UL, 100UL, null)]
    [InlineData("https://canary.discord.com/channels/@me/200/5", null, 200UL, 5UL)]
    public void Parses_channel_links(string url, ulong? guild, ulong channel, ulong? message)
    {
        Assert.True(DeepLink.TryParse(url, out var link));
        Assert.Equal(guild, link.GuildId?.Value);
        Assert.Equal(channel, link.ChannelId.Value);
        Assert.Equal(message, link.MessageId?.Value);
        Assert.True(DeepLink.TryParse(link.ToUri(), out var roundTripped));
        Assert.Equal(link, roundTripped);
    }

    [Theory]
    [InlineData("swiftcord://channels/10")]
    [InlineData("swiftcord://guilds/10/100")]
    [InlineData("https://example.com/channels/10/100")]
    [InlineData("http://discord.com/channels/10/100")]
    [InlineData("swiftcord://channels/x/100")]
    [InlineData("not a url")]
    public void Rejects_other_links(string url) => Assert.False(DeepLink.TryParse(url, out _));

    private sealed class FakeNotifications : INotificationSink
    {
        public List<MessageNotification> Shown { get; } = [];
        public List<Snowflake> Cleared { get; } = [];
        public void Show(MessageNotification notification) => Shown.Add(notification);
        public void Clear(Snowflake channelId) => Cleared.Add(channelId);
    }
}

internal sealed class MemorySettingsStore(AppSettings? initial = null) : ISettingsStore
{
    public AppSettings? Saved { get; private set; } = initial;
    public AppSettings Load() => Saved?.Clone() ?? new AppSettings();
    public void Save(AppSettings settings) => Saved = settings.Clone();
}
